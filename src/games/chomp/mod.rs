//! Chomp core: a maze chase, pure logic on a millisecond clock. No rendering,
//! no terminal — a caller drives [`Chomp::step`] with an [`Input`] and a `dt`,
//! then reads the maze through the accessors the painter needs.
//!
//! The maze is carved fresh for every level and every arena size — mirrored
//! left-to-right the way the real cabinets were, threaded with loops so there
//! is always a second way out, and pierced by a tunnel that wraps. The ghosts
//! are the game: each has a persona (one hunts you, one cuts you off, one
//! flanks, one loses its nerve up close), they breathe between scatter and
//! chase, and a power pellet turns the whole chase around for a few seconds —
//! which is where the points are.

pub mod paint;

use std::collections::VecDeque;
use std::time::Duration;

use crate::rng::Rng;
use crate::world::layout::Layout;
use crate::world::{Buf, Sparks};

use super::{Game, Input, Kick, Kind, Pop, Turn};

/// The reference arena the tests reason about. A running game carries its
/// own, sized to the frame it was spawned on. Both odd: the carver works on
/// an odd lattice.
pub const COLS: i32 = 27;
pub const ROWS: i32 = 15;

/// Seconds per cell. The player is always a touch faster than the pack —
/// outrunning a straight chase must be possible, being surrounded is what
/// kills. Everything tightens with the level.
const PLAYER_PERIOD: f32 = 0.125;
const GHOST_PERIOD: f32 = 0.140;
const PERIOD_STEP: f32 = 0.006;
const PLAYER_FLOOR: f32 = 0.095;
const GHOST_FLOOR: f32 = 0.108;
/// A frightened ghost waddles; eyes fly home.
const FRIGHT_PERIOD: f32 = 0.20;
const EYES_PERIOD: f32 = 0.055;

/// Seconds of the hunt after a pellet, shrinking per level, and the last
/// stretch of it spent blinking — the classic "it is about to turn back"
/// warning, honoured because it is the decision point of the whole game.
const FRIGHT_SECS: f32 = 6.0;
const FRIGHT_STEP: f32 = 0.45;
const FRIGHT_MIN: f32 = 2.5;
pub const FRIGHT_BLINK: f32 = 1.6;

/// The breath of the pack: a few seconds falling back to the corners, a long
/// stretch hunting. The scatter is what makes a maze survivable — and it
/// shortens as the levels climb.
const SCATTER_SECS: f32 = 6.0;
const SCATTER_STEP: f32 = 0.5;
const SCATTER_MIN: f32 = 2.0;
const CHASE_SECS: f32 = 16.0;

/// Ghosts leave home one at a time, on a clock, so the opening of a level is
/// a mounting problem rather than an instant one.
const RELEASE_EVERY: f32 = 2.2;

/// What things pay. Dots are the wage, pellets are the tool, ghosts are the
/// score — each one taken in a single hunt pays double the one before.
const DOT_POINTS: u32 = 2;
const PELLET_POINTS: u32 = 25;
const GHOST_LADDER: [u32; 4] = [100, 200, 400, 800];

const HEAT_DECAY_SECS: f32 = 2.5;
const SHAKE_DECAY_SECS: f32 = 0.30;
const PRAISE_SECS: f32 = 1.4;
const POP_SECS: f32 = 0.8;
const DEATH_SECS: f32 = 0.9;
/// The held breath between a cleared maze and the next one being carved.
const CLEAR_PAUSE: f32 = 1.3;

/// Render frames the machine freezes for. Dots get none — they are the
/// constant, and a constant that stutters is a broken machine. The freezes
/// belong to the pellet, the kill and the death.
const HITSTOP_PELLET: u32 = 2;
const HITSTOP_GHOST: u32 = 4;
const HITSTOP_DEATH: u32 = 10;

/// Render frames the arena stays inverted after a pellet or a clear.
const EAT_FLASH: u32 = 2;

/// Turns banked beyond this are dropped — enough to pre-book a corner.
const QUEUE_CAP: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    fn delta(self) -> (i32, i32) {
        match self {
            Dir::Up => (0, -1),
            Dir::Down => (0, 1),
            Dir::Left => (-1, 0),
            Dir::Right => (1, 0),
        }
    }

    fn reverse(self) -> Dir {
        match self {
            Dir::Up => Dir::Down,
            Dir::Down => Dir::Up,
            Dir::Left => Dir::Right,
            Dir::Right => Dir::Left,
        }
    }
}

const DIRS: [Dir; 4] = [Dir::Up, Dir::Down, Dir::Left, Dir::Right];

/// What a maze cell holds. Walls never change; the rest is lunch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Wall,
    Empty,
    Dot,
    Pellet,
}

/// What a ghost is doing. `fright` and `eyes` live on the ghost rather than
/// here because a ghost keeps its place in the release queue through both.
#[derive(Clone, Debug)]
pub struct Ghost {
    pub at: (i32, i32),
    pub prev: (i32, i32),
    pub dir: Dir,
    accum: f32,
    /// Seconds until this ghost joins the chase. While positive it sits at
    /// home, which is the staggered opening every maze game lives on.
    pub wait: f32,
    /// Seconds of fright left; hunting is legal while positive.
    pub fright: f32,
    /// Eaten: a pair of eyes flying home to be reborn.
    pub eyes: bool,
    /// Which mind it has — index into the persona table.
    pub persona: usize,
}

impl Ghost {
    /// How far this ghost stands between its last cell and this one.
    pub fn glide_from(&self, interval: f32) -> f32 {
        let t = (self.accum / interval).clamp(0.0, 1.0);
        t * (1.5 - 0.5 * t)
    }
}

pub struct Chomp {
    cols: i32,
    rows: i32,
    cells: Vec<Cell>,
    /// Where the pack is born and where eyes fly back to.
    home: (i32, i32),
    /// Where the player starts, low centre.
    spawn: (i32, i32),

    at: (i32, i32),
    prev: (i32, i32),
    dir: Dir,
    queued: VecDeque<Dir>,
    accum: f32,
    /// Set when the way forward is a wall: the muncher waits at the junction
    /// instead of grinding its face against it.
    parked: bool,

    ghosts: Vec<Ghost>,
    release: f32,

    level: u32,
    dots_left: u32,
    dots_eaten: u32,
    ghosts_eaten: u32,
    /// Rung on the ghost ladder within the current hunt.
    hunt_streak: usize,
    /// Scatter/chase clock: counts down, flips the mode when it runs out.
    mode_left: f32,
    scattering: bool,
    /// Positive while the cleared maze holds its breath before the next.
    clear_pause: f32,

    rng: Rng,
    score: u32,
    pub elapsed: Duration,
    heat: f32,
    shake: f32,
    shout: Option<(String, f32)>,
    pops: Vec<Pop>,
    punch: f32,
    hitstop: u32,
    kick: Option<Kick>,
    pub(crate) sparks: Sparks,
    flash: u32,
    over: bool,
    death: f32,
}

impl Chomp {
    pub fn new() -> Self {
        Self::with_rng(Rng::new())
    }

    pub fn with_rng(rng: Rng) -> Self {
        Self::with_field(rng, COLS, ROWS)
    }

    pub fn with_field(mut rng: Rng, cols: i32, rows: i32) -> Self {
        // Odd on both axes, and never too small to hold a maze.
        let cols = (cols.max(13)) | 1;
        let rows = (rows.max(9)) | 1;
        let home = (cols / 2, rows / 2);
        let spawn = (cols / 2, rows - 2);
        let cells = carve(&mut rng, cols, rows, home, spawn);
        let dots_left = cells
            .iter()
            .filter(|&&c| matches!(c, Cell::Dot | Cell::Pellet))
            .count() as u32;
        let ghosts = pack(cols, rows, home);
        Chomp {
            cols,
            rows,
            cells,
            home,
            spawn,
            at: spawn,
            prev: spawn,
            dir: Dir::Left,
            queued: VecDeque::new(),
            accum: 0.0,
            parked: false,
            ghosts,
            release: 0.0,
            level: 1,
            dots_left,
            dots_eaten: 0,
            ghosts_eaten: 0,
            hunt_streak: 0,
            mode_left: SCATTER_SECS,
            scattering: true,
            clear_pause: 0.0,
            rng,
            score: 0,
            elapsed: Duration::ZERO,
            heat: 0.0,
            shake: 0.0,
            shout: None,
            pops: Vec::new(),
            punch: 0.0,
            hitstop: 0,
            kick: None,
            sparks: Sparks::new(),
            flash: 0,
            over: false,
            death: 0.0,
        }
    }

    // ------------------------------------------------------------ accessors

    pub fn cols(&self) -> i32 {
        self.cols
    }

    pub fn rows(&self) -> i32 {
        self.rows
    }

    pub fn cell(&self, x: i32, y: i32) -> Cell {
        if x < 0 || y < 0 || x >= self.cols || y >= self.rows {
            return Cell::Wall;
        }
        self.cells[(y * self.cols + x) as usize]
    }

    pub fn at(&self) -> (i32, i32) {
        self.at
    }

    pub fn dir(&self) -> Dir {
        self.dir
    }

    pub fn ghosts(&self) -> &[Ghost] {
        &self.ghosts
    }

    pub fn level(&self) -> u32 {
        self.level
    }

    pub fn dots_left(&self) -> u32 {
        self.dots_left
    }

    pub fn death(&self) -> f32 {
        self.death
    }

    pub fn flashing(&self) -> bool {
        self.flash > 0
    }

    /// Positive while the cleared maze holds its breath.
    pub fn clearing(&self) -> bool {
        self.clear_pause > 0.0
    }

    /// The player's glide between cells, eased. Frozen once dead or parked.
    pub fn glide(&self) -> f32 {
        if self.over || self.parked {
            return 1.0;
        }
        let t = (self.accum / self.player_interval()).clamp(0.0, 1.0);
        t * (1.5 - 0.5 * t)
    }

    /// The player's drawn position, part-way through the move. A wrap through
    /// the tunnel snaps rather than glides — a lerp across the whole arena
    /// would be a teleport streak.
    pub fn player_pos(&self) -> (f32, f32) {
        let t = self.glide();
        if (self.at.0 - self.prev.0).abs() > 1 || (self.at.1 - self.prev.1).abs() > 1 {
            return (self.at.0 as f32, self.at.1 as f32);
        }
        (
            self.prev.0 as f32 + (self.at.0 - self.prev.0) as f32 * t,
            self.prev.1 as f32 + (self.at.1 - self.prev.1) as f32 * t,
        )
    }

    /// A ghost's drawn position, same rules as the player's.
    pub fn ghost_pos(&self, g: &Ghost) -> (f32, f32) {
        let t = g.glide_from(self.ghost_interval(g));
        if (g.at.0 - g.prev.0).abs() > 1 || (g.at.1 - g.prev.1).abs() > 1 {
            return (g.at.0 as f32, g.at.1 as f32);
        }
        (
            g.prev.0 as f32 + (g.at.0 - g.prev.0) as f32 * t,
            g.prev.1 as f32 + (g.at.1 - g.prev.1) as f32 * t,
        )
    }

    pub fn player_interval(&self) -> f32 {
        (PLAYER_PERIOD - PERIOD_STEP * (self.level - 1) as f32).max(PLAYER_FLOOR)
    }

    pub fn ghost_interval(&self, g: &Ghost) -> f32 {
        if g.eyes {
            EYES_PERIOD
        } else if g.fright > 0.0 {
            FRIGHT_PERIOD
        } else {
            (GHOST_PERIOD - PERIOD_STEP * (self.level - 1) as f32).max(GHOST_FLOOR)
        }
    }

    /// Seconds of hunt a pellet buys on this level.
    pub fn fright_secs(&self) -> f32 {
        (FRIGHT_SECS - FRIGHT_STEP * (self.level - 1) as f32).max(FRIGHT_MIN)
    }

    // ------------------------------------------------------------- stepping

    fn steer(&mut self, input: &Input) {
        for tap in input.taps.iter() {
            let want = match tap {
                Turn::Up => Dir::Up,
                Turn::Down => Dir::Down,
                Turn::Left => Dir::Left,
                Turn::Right => Dir::Right,
            };
            let last = self.queued.back().copied().unwrap_or(self.dir);
            if want == last {
                continue;
            }
            if self.queued.len() >= QUEUE_CAP {
                self.queued.pop_front();
            }
            self.queued.push_back(want);
        }
    }

    fn open(&self, x: i32, y: i32) -> bool {
        self.cell(self.wrap_x(x), y) != Cell::Wall
    }

    fn wrap_x(&self, x: i32) -> i32 {
        x.rem_euclid(self.cols)
    }

    fn advance_player(&mut self) {
        self.prev = self.at;
        // A banked turn is taken the moment it is legal; until then the
        // current heading holds. A reverse is always legal — the one mercy
        // taps-steering owes the player.
        if let Some(&want) = self.queued.front() {
            let (dx, dy) = want.delta();
            if want == self.dir.reverse() || self.open(self.at.0 + dx, self.at.1 + dy) {
                self.dir = want;
                self.queued.pop_front();
            }
        }
        let (dx, dy) = self.dir.delta();
        let next = (self.wrap_x(self.at.0 + dx), self.at.1 + dy);
        if !self.open(next.0, next.1) {
            self.parked = true;
            return;
        }
        self.parked = false;
        self.at = next;
        self.munch();
    }

    fn munch(&mut self) {
        let i = (self.at.1 * self.cols + self.at.0) as usize;
        match self.cells[i] {
            Cell::Dot => {
                self.cells[i] = Cell::Empty;
                self.dots_left -= 1;
                self.dots_eaten += 1;
                self.score += DOT_POINTS;
                self.heat = (self.heat + 0.05).min(1.0);
            }
            Cell::Pellet => {
                self.cells[i] = Cell::Empty;
                self.dots_left -= 1;
                self.dots_eaten += 1;
                self.score += PELLET_POINTS;
                self.hunt_streak = 0;
                let secs = self.fright_secs();
                for g in &mut self.ghosts {
                    if !g.eyes && g.wait <= 0.0 {
                        g.fright = secs;
                        // The whole pack turns on its heel — the moment the
                        // chase reverses is the pellet's entire meaning.
                        g.dir = g.dir.reverse();
                    }
                }
                self.hitstop = self.hitstop.max(HITSTOP_PELLET);
                self.kick = Some(Kick::Small);
                self.punch = self.punch.max(0.6);
                self.heat = (self.heat + 0.3).min(1.0);
                self.shake = self.shake.max(1.0);
                self.flash = EAT_FLASH;
                self.shout = Some(("HUNT".into(), 1.0));
                self.pops.push(Pop {
                    col: self.at.0 as f32,
                    row: self.at.1 as f32,
                    points: PELLET_POINTS,
                    life: 1.0,
                });
                let at = (self.at.0 as f32 + 0.5, self.at.1 as f32 + 0.5);
                self.sparks
                    .burst(&mut self.rng, at, 12, 13.0, crate::world::hex(0xFFE100));
            }
            _ => return,
        }
        if self.dots_left == 0 {
            self.clear();
        }
    }

    fn clear(&mut self) {
        self.clear_pause = CLEAR_PAUSE;
        self.kick = Some(Kick::Big);
        self.punch = 1.0;
        self.flash = EAT_FLASH + 2;
        self.shake = self.shake.max(2.0);
        self.shout = Some(("CLEARED".into(), 1.2));
        self.heat = 1.0;
    }

    /// Carve the next maze and rehome everyone. Score, level and the clock
    /// carry over; everything else is new ground.
    fn next_level(&mut self) {
        self.level += 1;
        self.cells = carve(&mut self.rng, self.cols, self.rows, self.home, self.spawn);
        self.dots_left = self
            .cells
            .iter()
            .filter(|&&c| matches!(c, Cell::Dot | Cell::Pellet))
            .count() as u32;
        self.ghosts = pack(self.cols, self.rows, self.home);
        self.release = 0.0;
        self.at = self.spawn;
        self.prev = self.spawn;
        self.dir = Dir::Left;
        self.queued.clear();
        self.accum = 0.0;
        self.parked = false;
        self.mode_left = self.scatter_secs();
        self.scattering = true;
        self.shout = Some((format!("LEVEL {}", self.level), 1.0));
    }

    fn scatter_secs(&self) -> f32 {
        (SCATTER_SECS - SCATTER_STEP * (self.level - 1) as f32).max(SCATTER_MIN)
    }

    /// The cell a ghost is steering for, per its persona. This table is the
    /// whole difference between a pack and four copies of one ghost.
    fn target(&self, g: &Ghost) -> (i32, i32) {
        if g.eyes {
            return self.home;
        }
        if self.scattering && g.fright <= 0.0 {
            // Falling back to its own corner. The breather, and why two
            // ghosts rarely arrive from the same direction.
            return match g.persona {
                0 => (self.cols - 2, 1),
                1 => (1, 1),
                2 => (self.cols - 2, self.rows - 2),
                _ => (1, self.rows - 2),
            };
        }
        let (px, py) = self.at;
        let (dx, dy) = self.dir.delta();
        match g.persona {
            // The hunter: straight at you.
            0 => (px, py),
            // The ambusher: four cells ahead of your nose.
            1 => (px + dx * 4, py + dy * 4),
            // The flank: your position mirrored through the hunter, so the
            // two of them close like pincers.
            2 => {
                let anchor = self
                    .ghosts
                    .iter()
                    .find(|o| o.persona == 0)
                    .map(|o| o.at)
                    .unwrap_or(self.home);
                (2 * px - anchor.0, 2 * py - anchor.1)
            }
            // The coward: hunts from range, loses its nerve up close.
            _ => {
                let d = (g.at.0 - px).abs() + (g.at.1 - py).abs();
                if d > 6 {
                    (px, py)
                } else {
                    (1, self.rows - 2)
                }
            }
        }
    }

    fn advance_ghost(&mut self, gi: usize) {
        let g = &self.ghosts[gi];
        let at = g.at;
        let reverse = g.dir.reverse();
        let frightened = g.fright > 0.0 && !g.eyes;
        let target = self.target(g);

        // Every open neighbour except the way it came — a ghost never turns
        // on its heel by choice, which is what makes cornering one possible.
        let mut options: Vec<Dir> = DIRS
            .into_iter()
            .filter(|&d| {
                let (dx, dy) = d.delta();
                d != reverse && self.open(at.0 + dx, at.1 + dy)
            })
            .collect();
        if options.is_empty() {
            options.push(reverse);
        }

        let dir = if frightened {
            // Panic is random turns, not clever flight — and exactly what
            // makes a fleeing ghost catchable.
            options[self.rng.range(options.len() as u32) as usize]
        } else {
            *options
                .iter()
                .min_by_key(|d| {
                    let (dx, dy) = d.delta();
                    let n = (self.wrap_x(at.0 + dx), at.1 + dy);
                    (n.0 - target.0).abs() + (n.1 - target.1).abs()
                })
                .unwrap()
        };

        let (dx, dy) = dir.delta();
        let next = (self.wrap_x(at.0 + dx), at.1 + dy);
        let g = &mut self.ghosts[gi];
        g.prev = at;
        g.dir = dir;
        g.at = next;

        // Eyes crossing home are reborn — back in the pack, back on the
        // release clock for a beat so the rebirth is visible.
        if self.ghosts[gi].eyes && next == self.home {
            let g = &mut self.ghosts[gi];
            g.eyes = false;
            g.fright = 0.0;
            g.wait = 0.6;
            g.prev = self.home;
        }
    }

    /// The one meeting that matters. Checked after every move on either side,
    /// including the pass-through — two things swapping cells have met even
    /// though no cell ever held both.
    fn meetings(&mut self) {
        for gi in 0..self.ghosts.len() {
            let g = &self.ghosts[gi];
            if g.wait > 0.0 || g.eyes {
                continue;
            }
            let touch = g.at == self.at || (g.at == self.prev && g.prev == self.at);
            if !touch {
                continue;
            }
            if g.fright > 0.0 {
                self.eat_ghost(gi);
            } else {
                self.die();
                return;
            }
        }
    }

    fn eat_ghost(&mut self, gi: usize) {
        let rung = self.hunt_streak.min(GHOST_LADDER.len() - 1);
        let points = GHOST_LADDER[rung];
        self.hunt_streak += 1;
        self.ghosts_eaten += 1;
        self.score += points;
        let g = &mut self.ghosts[gi];
        g.eyes = true;
        g.fright = 0.0;
        let at = (g.at.0 as f32 + 0.5, g.at.1 as f32 + 0.5);
        self.pops.push(Pop {
            col: g.at.0 as f32,
            row: g.at.1 as f32,
            points,
            life: 1.0,
        });
        self.hitstop = self.hitstop.max(HITSTOP_GHOST);
        self.kick = Some(Kick::Bonus);
        self.punch = self.punch.max(0.8);
        self.heat = (self.heat + 0.4).min(1.0);
        self.shake = self.shake.max(2.0);
        let hue = persona_hue(self.ghosts[gi].persona);
        self.sparks.burst(&mut self.rng, at, 14, 14.0, hue);
        if self.hunt_streak >= 2 {
            self.shout = Some((format!("{} GHOSTS", self.hunt_streak), 1.0));
        }
    }

    fn die(&mut self) {
        if self.over {
            return;
        }
        self.over = true;
        self.death = 0.0;
        self.shake = 3.0;
        self.punch = 1.0;
        self.hitstop = self.hitstop.max(HITSTOP_DEATH);
        self.kick = Some(Kick::Death);
        self.queued.clear();
        let at = (self.at.0 as f32 + 0.5, self.at.1 as f32 + 0.5);
        self.sparks
            .burst(&mut self.rng, at, 16, 14.0, crate::world::hex(0xFFE100));
        self.heat = (self.heat + 0.4).min(1.0);
    }
}

/// A ghost's body colour, by persona.
pub fn persona_hue(persona: usize) -> crate::world::Rgb {
    crate::world::hex(match persona {
        0 => 0xFF23C8, // the hunter, magenta
        1 => 0x00F0FF, // the ambusher, cyan
        2 => 0x9DEA38, // the flank, acid
        _ => 0xFFA400, // the coward, orange
    })
}

/// The pack at the start of a maze: at home, staggered onto the clock.
fn pack(cols: i32, rows: i32, home: (i32, i32)) -> Vec<Ghost> {
    // Small mazes get three ghosts; full-width ones get the whole pack.
    let n = if (cols + rows) < 40 { 3 } else { 4 };
    (0..n)
        .map(|i| Ghost {
            at: home,
            prev: home,
            dir: Dir::Left,
            accum: 0.0,
            wait: 0.4 + RELEASE_EVERY * i as f32,
            fright: 0.0,
            eyes: false,
            persona: i,
        })
        .collect()
}

impl Default for Chomp {
    fn default() -> Self {
        Self::new()
    }
}

impl Game for Chomp {
    fn kind(&self) -> Kind {
        Kind::Chomp
    }

    fn field(&self) -> (usize, usize) {
        (self.cols as usize, self.rows as usize)
    }

    fn step(&mut self, input: &Input, dt: Duration) {
        let dts = dt.as_secs_f32();
        self.heat = (self.heat - dts / HEAT_DECAY_SECS).max(0.0);
        self.shake = (self.shake - dts / SHAKE_DECAY_SECS).max(0.0);
        if let Some((_, life)) = &mut self.shout {
            *life -= dts / PRAISE_SECS;
            if *life <= 0.0 {
                self.shout = None;
            }
        }
        for p in &mut self.pops {
            p.life -= dts / POP_SECS;
        }
        self.pops.retain(|p| p.life > 0.0);
        self.sparks.step(dts);
        self.flash = self.flash.saturating_sub(1);

        if self.over {
            self.death = (self.death + dts / DEATH_SECS).min(1.0);
            return;
        }

        self.elapsed = self.elapsed.saturating_add(dt);

        // The held breath after a clear: nothing moves, then a new maze.
        if self.clear_pause > 0.0 {
            self.clear_pause -= dts;
            if self.clear_pause <= 0.0 {
                self.next_level();
            }
            return;
        }

        // The pack breathes: scatter, chase, scatter…
        self.mode_left -= dts;
        if self.mode_left <= 0.0 {
            self.scattering = !self.scattering;
            self.mode_left = if self.scattering {
                self.scatter_secs()
            } else {
                CHASE_SECS
            };
            // The classic tell: the whole pack reverses on a mode change, so
            // the player can feel the tide turn without a HUD for it.
            for g in &mut self.ghosts {
                if !g.eyes && g.wait <= 0.0 {
                    g.dir = g.dir.reverse();
                }
            }
        }

        self.steer(input);

        let interval = self.player_interval();
        self.accum += dts;
        let mut moves = 0;
        while self.accum >= interval && moves < 4 && !self.over {
            self.accum -= interval;
            moves += 1;
            self.advance_player();
            self.meetings();
        }
        if self.parked {
            self.accum = self.accum.min(interval);
        }

        for gi in 0..self.ghosts.len() {
            {
                let g = &mut self.ghosts[gi];
                g.fright = (g.fright - dts).max(0.0);
                if g.wait > 0.0 {
                    g.wait -= dts;
                    continue;
                }
            }
            let interval = self.ghost_interval(&self.ghosts[gi]);
            self.ghosts[gi].accum += dts;
            let mut moves = 0;
            while self.ghosts[gi].accum >= interval && moves < 6 && !self.over {
                self.ghosts[gi].accum -= interval;
                moves += 1;
                self.advance_ghost(gi);
                self.meetings();
            }
        }
    }

    fn is_over(&self) -> bool {
        self.over && self.death >= 1.0
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn heat(&self) -> f32 {
        self.heat.clamp(0.0, 1.0)
    }

    fn shake(&self) -> i32 {
        self.shake.round() as i32
    }

    fn take_punch(&mut self) -> f32 {
        std::mem::take(&mut self.punch)
    }

    fn take_hitstop(&mut self) -> u32 {
        std::mem::take(&mut self.hitstop)
    }

    fn take_kick(&mut self) -> Option<Kick> {
        self.kick.take()
    }

    fn tally(&self) -> [(&'static str, u32); 2] {
        [("LEVELS", self.level), ("GHOSTS", self.ghosts_eaten)]
    }

    fn pops(&self) -> &[Pop] {
        &self.pops
    }

    /// Chase the nearest meal along real corridors, giving live ghosts a wide
    /// berth — and turn hunter the moment the pack is blue. A breadth-first
    /// search is cheap at this size and the demo has to look like someone who
    /// knows the maze.
    fn autopilot(&self) -> Input {
        let hunting = self
            .ghosts
            .iter()
            .any(|g| g.fright > FRIGHT_BLINK * 0.5 && !g.eyes && g.wait <= 0.0);
        let danger: Vec<(i32, i32)> = self
            .ghosts
            .iter()
            .filter(|g| g.fright <= 0.0 && !g.eyes && g.wait <= 0.0)
            .map(|g| g.at)
            .collect();
        let is_goal = |c: (i32, i32)| -> bool {
            if hunting {
                self.ghosts
                    .iter()
                    .any(|g| g.fright > 0.0 && !g.eyes && g.at == c)
            } else {
                matches!(self.cell(c.0, c.1), Cell::Dot | Cell::Pellet)
            }
        };
        let risky = |c: (i32, i32)| {
            danger
                .iter()
                .any(|&(gx, gy)| (gx - c.0).abs() + (gy - c.1).abs() <= 2)
        };

        // BFS from the head, remembering the first step of each path.
        let mut seen = vec![false; (self.cols * self.rows) as usize];
        let mut queue: VecDeque<((i32, i32), Option<Dir>)> = VecDeque::new();
        seen[(self.at.1 * self.cols + self.at.0) as usize] = true;
        queue.push_back((self.at, None));
        let mut fallback: Option<Dir> = None;
        while let Some((c, first)) = queue.pop_front() {
            if is_goal(c) && c != self.at {
                if let Some(d) = first {
                    return turn_input(d);
                }
            }
            for d in DIRS {
                let (dx, dy) = d.delta();
                let n = (self.wrap_x(c.0 + dx), c.1 + dy);
                if !self.open(n.0, n.1) || risky(n) {
                    continue;
                }
                let i = (n.1 * self.cols + n.0) as usize;
                if seen[i] {
                    continue;
                }
                seen[i] = true;
                let step = first.or(Some(d));
                if fallback.is_none() {
                    fallback = step;
                }
                queue.push_back((n, step));
            }
        }
        // Everything reachable is risky: flee properly — take the open
        // neighbour that keeps the most distance from the nearest ghost,
        // which turns a cornering into a chase the player's speed can win.
        let _ = fallback;
        let flee = DIRS
            .into_iter()
            .filter(|d| {
                let (dx, dy) = d.delta();
                self.open(self.at.0 + dx, self.at.1 + dy)
            })
            .max_by_key(|d| {
                let (dx, dy) = d.delta();
                let n = (self.wrap_x(self.at.0 + dx), self.at.1 + dy);
                danger
                    .iter()
                    .map(|&(gx, gy)| (gx - n.0).abs() + (gy - n.1).abs())
                    .min()
                    .unwrap_or(0)
            });
        match flee {
            Some(d) => turn_input(d),
            None => Input::default(),
        }
    }

    fn shout(&self) -> Option<(&str, f32)> {
        self.shout.as_ref().map(|(s, life)| (s.as_str(), *life))
    }

    fn paint(&self, b: &mut Buf, l: &Layout) {
        paint::paint(b, l, self);
    }
}

fn turn_input(d: Dir) -> Input {
    Input::turn(match d {
        Dir::Up => Turn::Up,
        Dir::Down => Turn::Down,
        Dir::Left => Turn::Left,
        Dir::Right => Turn::Right,
    })
}

// -------------------------------------------------------------- the carver

/// Carve a maze: corridors on the odd lattice, mirrored left-to-right,
/// threaded with loops, pierced by a wrapping tunnel, then dotted. The centre
/// is opened as the pack's home and the spawn is kept clear underneath.
///
/// Everything reachable is guaranteed by construction and then verified by a
/// flood fill that walls off anything the carver orphaned — a dot that cannot
/// be eaten is a level that cannot be finished.
fn carve(rng: &mut Rng, cols: i32, rows: i32, home: (i32, i32), spawn: (i32, i32)) -> Vec<Cell> {
    let idx = |x: i32, y: i32| (y * cols + x) as usize;
    let mut cells = vec![Cell::Wall; (cols * rows) as usize];
    let mid = cols / 2;

    // Depth-first over the odd lattice of the left half, knocking the wall
    // between each pair of visited nodes. A spanning tree: every corridor
    // reachable, no loops yet.
    let mut stack = vec![(1, 1)];
    cells[idx(1, 1)] = Cell::Empty;
    while let Some(&(x, y)) = stack.last() {
        let mut moves: Vec<(i32, i32)> = [(2, 0), (-2, 0), (0, 2), (0, -2)]
            .into_iter()
            .map(|(dx, dy)| (x + dx, y + dy))
            .filter(|&(nx, ny)| {
                nx >= 1 && nx <= mid && ny >= 1 && ny < rows - 1 && cells[idx(nx, ny)] == Cell::Wall
            })
            .collect();
        if moves.is_empty() {
            stack.pop();
            continue;
        }
        let (nx, ny) = moves.swap_remove(rng.range(moves.len() as u32) as usize);
        cells[idx((x + nx) / 2, (y + ny) / 2)] = Cell::Empty;
        cells[idx(nx, ny)] = Cell::Empty;
        stack.push((nx, ny));
    }

    // Loops: knock a share of the walls that separate two corridors. A maze
    // that is a pure tree is a trap with one exit per room, and being able
    // to run a circle around a ghost is the entire skill of the game.
    for y in 1..rows - 1 {
        for x in 1..=mid {
            if cells[idx(x, y)] != Cell::Wall {
                continue;
            }
            let h = cells[idx(x - 1, y)] != Cell::Wall && cells[idx(x + 1, y)] != Cell::Wall;
            let v = cells[idx(x, y - 1)] != Cell::Wall && cells[idx(x, y + 1)] != Cell::Wall;
            if (h ^ v) && rng.range(100) < 30 {
                cells[idx(x, y)] = Cell::Empty;
            }
        }
    }

    // Mirror the left half onto the right. Symmetry is not decoration: a
    // player reads half the maze and knows the whole, which is what makes a
    // glance at a fresh level enough to start running.
    for y in 0..rows {
        for x in 0..mid {
            cells[idx(cols - 1 - x, y)] = cells[idx(x, y)];
        }
    }
    // Stitch the halves through the spine so the mirror is reachable.
    let mut stitched = 0;
    for y in (1..rows - 1).step_by(2) {
        if cells[idx(mid - 1, y)] != Cell::Wall && (rng.range(100) < 40 || stitched == 0) {
            cells[idx(mid, y)] = Cell::Empty;
            stitched += 1;
        }
    }

    // The tunnel: one row open through both walls, wrapping. The signature
    // move of the genre and the panic button of every good escape.
    let ty = (rows / 2) | 1;
    for x in 0..=2 {
        cells[idx(x, ty)] = Cell::Empty;
        cells[idx(cols - 1 - x, ty)] = Cell::Empty;
    }

    // Home for the pack and a clear spawn for the player.
    for (cx, cy) in [home, spawn] {
        for (dx, dy) in [(0, 0), (-1, 0), (1, 0)] {
            let (x, y) = ((cx + dx).clamp(1, cols - 2), (cy + dy).clamp(1, rows - 2));
            cells[idx(x, y)] = Cell::Empty;
        }
    }

    // Wall off anything the carving orphaned, then knock doors until the
    // maze is one piece. The knocks come in mirrored pairs to keep the
    // symmetry honest.
    loop {
        let mut seen = vec![false; cells.len()];
        let mut queue = VecDeque::from([spawn]);
        seen[idx(spawn.0, spawn.1)] = true;
        while let Some((x, y)) = queue.pop_front() {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = ((x + dx).rem_euclid(cols), y + dy);
                if ny < 0 || ny >= rows || seen[idx(nx, ny)] || cells[idx(nx, ny)] == Cell::Wall {
                    continue;
                }
                seen[idx(nx, ny)] = true;
                queue.push_back((nx, ny));
            }
        }
        let mut door = None;
        'find: for y in 1..rows - 1 {
            for x in 1..cols - 1 {
                if cells[idx(x, y)] != Cell::Wall {
                    continue;
                }
                for (dx, dy) in [(1, 0), (0, 1)] {
                    let a = (x - dx, y - dy);
                    let b = (x + dx, y + dy);
                    if cells[idx(a.0, a.1)] != Cell::Wall
                        && cells[idx(b.0, b.1)] != Cell::Wall
                        && (seen[idx(a.0, a.1)] != seen[idx(b.0, b.1)])
                    {
                        door = Some((x, y));
                        break 'find;
                    }
                }
            }
        }
        match door {
            Some((x, y)) => {
                cells[idx(x, y)] = Cell::Empty;
                cells[idx(cols - 1 - x, y)] = Cell::Empty;
            }
            None => {
                // Connected: anything never reached is sealed for good.
                for (i, c) in cells.iter_mut().enumerate() {
                    if *c != Cell::Wall && !seen[i] {
                        *c = Cell::Wall;
                    }
                }
                break;
            }
        }
    }

    // Dots everywhere the player can walk, except home ground — a meal at
    // the pack's doorstep or under your own feet is not a meal.
    for y in 0..rows {
        for x in 0..cols {
            if cells[idx(x, y)] != Cell::Empty {
                continue;
            }
            let near_home = (x - home.0).abs() <= 1 && (y - home.1).abs() <= 1;
            let near_spawn = (x - spawn.0).abs() <= 1 && y == spawn.1;
            if !near_home && !near_spawn {
                cells[idx(x, y)] = Cell::Dot;
            }
        }
    }

    // Four pellets, one per quarter, as deep into the corners as the maze
    // allows. Their placement is the level's shape: every hunt starts from
    // one of these four rooms.
    for (cx, cy) in [
        (1, 1),
        (cols - 2, 1),
        (1, rows - 2),
        (cols - 2, rows - 2),
    ] {
        let mut best = None;
        let mut best_d = i32::MAX;
        for y in 1..rows - 1 {
            for x in 1..cols - 1 {
                if cells[idx(x, y)] == Cell::Dot {
                    let d = (x - cx).abs() + (y - cy).abs();
                    if d < best_d {
                        best_d = d;
                        best = Some((x, y));
                    }
                }
            }
        }
        if let Some((x, y)) = best {
            cells[idx(x, y)] = Cell::Pellet;
        }
    }

    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn game(seed: u64) -> Chomp {
        Chomp::with_rng(Rng::from_seed(seed))
    }

    #[test]
    fn the_maze_is_mirrored_and_walled() {
        for seed in 0..20 {
            let g = game(seed);
            for y in 0..g.rows {
                // The outer ring is wall except the tunnel row.
                let tunnel = g.cell(0, y) != Cell::Wall;
                assert_eq!(tunnel, g.cell(g.cols - 1, y) != Cell::Wall);
                for x in 0..g.cols {
                    let a = g.cell(x, y) == Cell::Wall;
                    let b = g.cell(g.cols - 1 - x, y) == Cell::Wall;
                    assert_eq!(a, b, "asymmetry at {x},{y} seed {seed}");
                }
            }
            assert!(g.cell(0, 0) == Cell::Wall);
        }
    }

    #[test]
    fn there_is_a_tunnel_and_it_wraps() {
        let g = game(3);
        let ty = (0..g.rows)
            .find(|&y| g.cell(0, y) != Cell::Wall)
            .expect("no tunnel row");
        assert_ne!(g.cell(g.cols - 1, ty), Cell::Wall);
        assert_eq!(g.wrap_x(-1), g.cols - 1);
    }

    #[test]
    fn every_dot_is_reachable() {
        for seed in 0..20 {
            let g = game(seed);
            let mut seen = vec![false; (g.cols * g.rows) as usize];
            let mut queue = VecDeque::from([g.spawn]);
            seen[(g.spawn.1 * g.cols + g.spawn.0) as usize] = true;
            while let Some((x, y)) = queue.pop_front() {
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (g.wrap_x(x + dx), y + dy);
                    if ny < 0 || ny >= g.rows {
                        continue;
                    }
                    let i = (ny * g.cols + nx) as usize;
                    if seen[i] || g.cell(nx, ny) == Cell::Wall {
                        continue;
                    }
                    seen[i] = true;
                    queue.push_back((nx, ny));
                }
            }
            for y in 0..g.rows {
                for x in 0..g.cols {
                    if matches!(g.cell(x, y), Cell::Dot | Cell::Pellet) {
                        assert!(
                            seen[(y * g.cols + x) as usize],
                            "unreachable dot at {x},{y} seed {seed}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn there_are_four_pellets_on_a_full_maze() {
        let g = game(7);
        let pellets = g.cells.iter().filter(|&&c| c == Cell::Pellet).count();
        assert_eq!(pellets, 4);
    }

    #[test]
    fn the_field_follows_the_frame_and_stays_odd() {
        for (w, h) in [(60, 24), (120, 34), (270, 60)] {
            let (cols, rows) = Kind::Chomp.field(w, h);
            assert_eq!(cols % 2, 1, "{w}x{h} gave even cols");
            assert_eq!(rows % 2, 1);
            let g = Chomp::with_field(Rng::from_seed(1), cols as i32, rows as i32);
            assert_eq!(g.field(), (cols, rows));
        }
    }

    #[test]
    fn eating_every_dot_clears_the_level_and_carves_a_new_maze() {
        let mut g = game(5);
        // Cheat the maze empty except one dot next to the player.
        for c in g.cells.iter_mut() {
            if matches!(*c, Cell::Dot | Cell::Pellet) {
                *c = Cell::Empty;
            }
        }
        let (dx, dy) = Dir::Left.delta();
        let at = (g.at.0 + dx, g.at.1 + dy);
        // The spawn row is carved clear, so the cell to the left is open.
        assert_ne!(g.cell(at.0, at.1), Cell::Wall);
        g.cells[(at.1 * g.cols + at.0) as usize] = Cell::Dot;
        g.dots_left = 1;
        for _ in 0..30 {
            g.step(&Input::default(), ms(16));
        }
        assert!(g.clearing() || g.level() == 2, "clear did not begin");
        for _ in 0..120 {
            g.step(&Input::default(), ms(16));
        }
        assert_eq!(g.level(), 2);
        assert!(g.dots_left() > 40, "the new maze is not dotted");
        assert!(!g.is_over());
    }

    #[test]
    fn a_pellet_turns_the_pack_and_a_ghost_pays_the_ladder() {
        let mut g = game(9);
        // Release the pack and put the hunter on the player's doorstep.
        for gh in &mut g.ghosts {
            gh.wait = 0.0;
        }
        let i = (g.at.1 * g.cols + g.at.0) as usize;
        g.cells[i] = Cell::Pellet;
        g.dots_left += 1;
        g.at.0 -= 0; // stay put; munch checks the cell under the head
        g.munch();
        assert!(g.ghosts.iter().all(|gh| gh.fright > 0.0));
        let before = g.score;
        g.ghosts[0].at = g.at;
        g.meetings();
        assert!(g.ghosts[0].eyes, "the ghost was not eaten");
        assert_eq!(g.score - before, GHOST_LADDER[0]);
        g.ghosts[1].at = g.at;
        g.ghosts[1].prev = g.at;
        g.meetings();
        assert_eq!(g.score - before, GHOST_LADDER[0] + GHOST_LADDER[1]);
        assert!(!g.over);
    }

    #[test]
    fn a_live_ghost_ends_the_run() {
        let mut g = game(11);
        g.ghosts[0].wait = 0.0;
        g.ghosts[0].at = g.at;
        g.meetings();
        assert!(g.over);
        for _ in 0..80 {
            g.step(&Input::default(), ms(16));
        }
        assert!(g.is_over());
    }

    #[test]
    fn eyes_fly_home_and_are_reborn() {
        let mut g = game(13);
        g.ghosts[0].wait = 0.0;
        g.ghosts[0].fright = 5.0;
        g.ghosts[0].at = g.at;
        g.meetings();
        assert!(g.ghosts[0].eyes);
        // Step long enough for the eyes to cross any arena this size.
        for _ in 0..1200 {
            g.step(&Input::default(), ms(16));
            if !g.ghosts[0].eyes {
                break;
            }
        }
        assert!(!g.ghosts[0].eyes, "the eyes never made it home");
    }

    #[test]
    fn the_autopilot_survives_and_eats() {
        for seed in [1u64, 8, 21] {
            let mut g = game(seed);
            for _ in 0..1800 {
                let input = g.autopilot();
                g.step(&input, ms(16));
                if g.over {
                    break;
                }
            }
            assert!(
                g.dots_eaten > 12,
                "seed {seed}: the demo only ate {} dots",
                g.dots_eaten
            );
        }
    }

    #[test]
    fn a_reverse_is_always_legal() {
        let mut g = game(17);
        let before = g.dir;
        let want = before.reverse();
        g.step(&turn_input(want), ms(200));
        assert_eq!(g.dir, want, "the reverse was refused");
    }

    #[test]
    fn parking_at_a_wall_does_not_bank_movement() {
        let mut g = game(19);
        // Drive into the nearest wall and sit there for a while.
        for _ in 0..600 {
            g.step(&Input::default(), ms(16));
        }
        let at = g.at;
        g.step(&Input::default(), ms(16));
        // Parked: no drift, and the accumulator is not building a teleport.
        if g.parked {
            assert_eq!(g.at, at);
            assert!(g.accum <= g.player_interval() + 0.001);
        }
    }

    #[test]
    fn the_hunt_shrinks_but_never_vanishes() {
        let mut g = game(23);
        g.level = 30;
        assert!(g.fright_secs() >= FRIGHT_MIN);
        assert!(g.player_interval() >= PLAYER_FLOOR);
        assert!(g.scatter_secs() >= SCATTER_MIN);
    }
}
