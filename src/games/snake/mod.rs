//! Snake core: a pure-logic arena with a millisecond clock. No rendering, no
//! terminal — a caller drives [`Snake::step`] with an [`Input`] and a `dt`,
//! then reads the arena through the accessors the painter needs.
//!
//! Two things make it a game rather than a demo. The snake gets faster with
//! every apple, so the difficulty is always just ahead of the player; and
//! apples eaten in quick succession build a multiplier that a single hesitation
//! drops back to one, which is what makes a good run feel worth protecting.
//!
//! One thing makes it feel like a snake rather than a state machine: the body
//! *glides*. The logic moves in whole cells, but every segment is drawn part of
//! the way between where it was and where it is, on an eased curve. A snake
//! that jumps a cell at a time reads as a cursor; the same snake sliding the
//! same distance reads as an animal.

pub mod paint;

use std::collections::VecDeque;
use std::time::Duration;

use crate::rng::Rng;
use crate::world::layout::Layout;
use crate::world::{Buf, Sparks};

use super::{Game, Input, Kick, Kind, Pop};

/// The arena, taken from the same place the layout takes it so the logic and
/// the cut hole can never disagree.
pub const COLS: i32 = Kind::Snake.field().0 as i32;
pub const ROWS: i32 = Kind::Snake.field().1 as i32;

/// Seconds per cell at each speed tier, and the points an apple pays there.
/// Discrete rather than a smooth curve: a tier the player can feel arriving is
/// worth more than one that creeps, and it gives the score something to say.
const PERIODS: [f32; 6] = [0.150, 0.125, 0.108, 0.095, 0.085, 0.075];
const POINTS: [u32; 6] = [3, 4, 5, 7, 9, 12];
/// Apples per tier. The top tier lands around the fifteenth — about half a
/// minute in, which is roughly how long anyone waits for a build step.
const TIER_SIZE: u32 = 3;

/// The snake at spawn: long enough to read as a body, short enough to leave
/// the whole arena open.
const START_LEN: usize = 5;

/// What a golden apple multiplies its tier's payout by. It grows the snake by
/// the same single cell an ordinary apple does: the bonus is already the risk
/// of crossing the field for it under a clock, and charging extra length on top
/// makes taking it a punishment for succeeding.
const GOLD_PAYOUT: u32 = 5;
const GOLD_GROWTH: usize = 1;
/// Every Nth apple is golden, and it does not wait around.
const GOLD_EVERY: u32 = 5;
/// Short enough that seeing one means dropping the line you were on and going
/// for it. At the widest crossing of the field it is not always reachable, and
/// that is the point — it has to be a decision rather than a collection.
const GOLD_LIFE: f32 = 3.5;

/// Eat again inside this window and the multiplier climbs; miss it and the run
/// starts over at one. It does not shrink with the tier: the faster the snake,
/// the easier chaining gets, which is the reward for surviving the climb.
const STREAK_WINDOW: f32 = 2.5;
const MAX_MULT: u32 = 8;

const HEAT_DECAY_SECS: f32 = 2.5;
const SHAKE_DECAY_SECS: f32 = 0.30;
const PRAISE_SECS: f32 = 1.4;

/// How long the body takes to burn away after a crash. The shell holds the
/// frame for at least this long, so the dissolve is always seen in full.
const DEATH_SECS: f32 = 0.9;

/// Render frames the machine freezes for on each event. A tap, a hit, and the
/// 150 ms an impact wants before the eye stops reading it as one.
const HITSTOP_APPLE: u32 = 3;
const HITSTOP_GOLD: u32 = 6;
const HITSTOP_DEATH: u32 = 10;

/// How long a `+N` marker stays in the air.
const POP_SECS: f32 = 0.8;

/// Render frames the arena stays inverted after an apple. Short and local: the
/// field fires, the rest of the machine does not move.
const EAT_FLASH: u32 = 3;

/// Turns banked beyond this are dropped. Three is enough to bank a double
/// corner between moves without the snake driving itself.
const QUEUE_CAP: usize = 3;

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

    fn opposite(self, other: Dir) -> bool {
        let (ax, ay) = self.delta();
        let (bx, by) = other.delta();
        ax == -bx && ay == -by
    }
}

/// What is sitting on the arena floor waiting to be eaten.
#[derive(Clone, Copy, Debug)]
pub struct Apple {
    pub at: (i32, i32),
    pub gold: bool,
    /// Seconds of life left for a golden apple; `None` for a plain one, which
    /// waits forever.
    pub ttl: Option<f32>,
}

pub struct Snake {
    /// Head first, tail last.
    body: VecDeque<(i32, i32)>,
    /// Where those cells were before the last move. Render-only: collision,
    /// self-intersection and apple placement only ever look at `body`, and the
    /// glide between the two is the only thing the eye actually sees.
    prev: VecDeque<(i32, i32)>,
    dir: Dir,
    /// Turns taken but not yet stepped. Two deep, so a fast double-tap round a
    /// corner survives — one deep and the second tap is eaten by the first.
    queued: VecDeque<Dir>,
    apple: Apple,
    rng: Rng,
    /// Cells still owed to a swallowed apple; the tail stays put while these
    /// are outstanding, which is what makes the body grow.
    growth: usize,
    accum: f32,
    eaten: u32,
    mult: u32,
    since_eat: f32,
    score: u32,
    elapsed: Duration,
    heat: f32,
    shake: f32,
    shout: Option<(String, f32)>,
    pops: Vec<Pop>,
    /// Impact banked for the shell, drained once a frame.
    punch: f32,
    hitstop: u32,
    kick: Option<Kick>,
    /// Debris. An apple bursts when it is taken and the body comes apart when
    /// the run ends.
    pub(crate) sparks: Sparks,
    /// Frames of inverted playfield left from the last apple.
    flash: u32,
    over: bool,
    /// `0.0..=1.0` once dead — how far the body has burned away.
    death: f32,
}

impl Snake {
    pub fn new() -> Self {
        Self::with_rng(Rng::new())
    }

    pub fn with_rng(mut rng: Rng) -> Self {
        // Facing right out of the middle: the same runway either side, and no
        // wall to meet before the player has taken hold.
        let cy = ROWS / 2;
        let x0 = COLS / 2;
        let body: VecDeque<(i32, i32)> = (0..START_LEN as i32).map(|i| (x0 - i, cy)).collect();
        let apple = place(&mut rng, &body, false);
        Snake {
            prev: body.clone(),
            body,
            dir: Dir::Right,
            queued: VecDeque::new(),
            apple,
            rng,
            growth: 0,
            accum: 0.0,
            eaten: 0,
            mult: 1,
            // Nothing has been eaten yet, so the first apple must not be able
            // to claim a streak it did not earn.
            since_eat: STREAK_WINDOW + 1.0,
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

    /// Which speed tier the run has reached, `0..PERIODS.len()`.
    pub fn tier(&self) -> u32 {
        (self.eaten / TIER_SIZE).min(PERIODS.len() as u32 - 1)
    }

    /// Seconds per cell right now.
    pub fn interval(&self) -> f32 {
        PERIODS[self.tier() as usize]
    }

    /// How far the body stands between its last cells and its current ones,
    /// `0.0..=1.0`, eased so a move leaves fast and arrives soft. Frozen at the
    /// far end once dead: a snake still coasting into its last cell reads as if
    /// the collision had not landed.
    pub fn glide(&self) -> f32 {
        if self.over {
            return 1.0;
        }
        let t = (self.accum / self.interval()).clamp(0.0, 1.0);
        t * (1.5 - 0.5 * t)
    }

    /// Cell position of body segment `i`, part-way through the current move.
    /// Segment `i` came from wherever segment `i` was before the move, so the
    /// whole body slides along its own path; a tail grown by an apple has no
    /// earlier cell and simply stays put.
    pub fn segment_at(&self, i: usize, t: f32) -> (f32, f32) {
        let cur = self.body[i];
        let prev = self
            .prev
            .get(i)
            .or_else(|| self.prev.back())
            .copied()
            .unwrap_or(cur);
        (
            prev.0 as f32 + (cur.0 - prev.0) as f32 * t,
            prev.1 as f32 + (cur.1 - prev.1) as f32 * t,
        )
    }

    /// `1..=9`. Shown, because a number that climbs is the whole reason to keep
    /// taking the risky line to the next apple.
    pub fn mult(&self) -> u32 {
        self.mult
    }

    /// How much of the streak window is left, `0.0..=1.0` — the draining bar
    /// under the multiplier.
    pub fn streak_left(&self) -> f32 {
        if self.mult <= 1 {
            0.0
        } else {
            (1.0 - self.since_eat / STREAK_WINDOW).clamp(0.0, 1.0)
        }
    }

    pub fn body(&self) -> &VecDeque<(i32, i32)> {
        &self.body
    }

    pub fn head(&self) -> (i32, i32) {
        *self.body.front().unwrap_or(&(0, 0))
    }

    pub fn dir(&self) -> Dir {
        self.dir
    }

    pub fn apple(&self) -> Apple {
        self.apple
    }

    pub fn eaten(&self) -> u32 {
        self.eaten
    }

    pub fn len(&self) -> usize {
        self.body.len()
    }

    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }

    /// `0.0..=1.0` once dead, still 0 while alive.
    pub fn death(&self) -> f32 {
        self.death
    }

    /// Whether the field is still lit from the last apple.
    pub fn flashing(&self) -> bool {
        self.flash > 0
    }

    /// Seconds since the last apple. The frame flare reads it, and so does the
    /// streak window.
    pub fn since_eat(&self) -> f32 {
        self.since_eat
    }

    /// Take a turn from the input. Reversing into your own neck is refused
    /// here rather than being allowed to kill, since it is always a misfire.
    fn steer(&mut self, input: &Input) {
        let want = if input.up {
            Some(Dir::Up)
        } else if input.down {
            Some(Dir::Down)
        } else if input.left {
            Some(Dir::Left)
        } else if input.right {
            Some(Dir::Right)
        } else {
            None
        };
        let Some(want) = want else { return };
        let last = self.queued.back().copied().unwrap_or(self.dir);
        if want == last || want.opposite(last) {
            return;
        }
        if self.queued.len() >= QUEUE_CAP {
            return;
        }
        self.queued.push_back(want);
    }

    /// Move one cell: consume a queued turn, then walk the head and either grow
    /// into the new cell or drag the tail after it.
    fn advance(&mut self) {
        // Snapshot for the glide before anything moves.
        self.prev.clone_from(&self.body);
        if let Some(d) = self.queued.pop_front() {
            self.dir = d;
        }
        let (dx, dy) = self.dir.delta();
        let (hx, hy) = self.head();
        let next = (hx + dx, hy + dy);

        if next.0 < 0 || next.1 < 0 || next.0 >= COLS || next.1 >= ROWS {
            self.die();
            return;
        }
        // The tail cell is about to be vacated, so following it is legal —
        // refusing it would kill on a turn that visibly had room.
        let tail = self.body.back().copied();
        let into_self = self
            .body
            .iter()
            .any(|&c| c == next && (self.growth > 0 || Some(c) != tail));
        if into_self {
            self.die();
            return;
        }

        self.body.push_front(next);
        if self.growth > 0 {
            self.growth -= 1;
        } else {
            self.body.pop_back();
        }

        if next == self.apple.at {
            self.eat();
        }
    }

    fn eat(&mut self) {
        let gold = self.apple.gold;
        self.mult = if self.since_eat <= STREAK_WINDOW {
            (self.mult + 1).min(MAX_MULT)
        } else {
            1
        };
        self.since_eat = 0.0;
        self.eaten += 1;

        let mut points = POINTS[self.tier() as usize];
        if gold {
            points *= GOLD_PAYOUT;
        }
        let gained = points * self.mult;
        self.score += gained;
        self.pops.push(Pop {
            col: self.apple.at.0 as f32,
            row: self.apple.at.1 as f32,
            points: gained,
            life: 1.0,
        });
        self.hitstop = self
            .hitstop
            .max(if gold { HITSTOP_GOLD } else { HITSTOP_APPLE });
        // An ordinary apple stays quiet. One arrives every second or two, and a
        // screen that flashes that often is a screen nobody looks at — it has
        // the hitstop, the marker and the frame flare already. The gold is what
        // the noise is being saved for.
        if gold {
            self.kick = Some(Kick::Bonus);
        }
        self.growth += if gold { GOLD_GROWTH } else { 1 };
        self.heat = (self.heat + if gold { 0.55 } else { 0.25 }).min(1.0);
        self.shake = self.shake.max(if gold { 2.0 } else { 1.0 });

        // The apple comes apart where it was taken. Gold rises rather than
        // falls, because it is light being paid out rather than matter.
        let at = (self.apple.at.0 as f32 + 0.5, self.apple.at.1 as f32 + 0.5);
        if gold {
            let hue = crate::world::hex(0xFFE100);
            self.sparks.burst(&mut self.rng, at, 18, 16.0, hue);
            self.sparks.glimmer(&mut self.rng, at, 10, hue);
        } else {
            self.sparks
                .burst(&mut self.rng, at, 10, 11.0, crate::world::hex(0x00FF87));
        }
        self.flash = EAT_FLASH;
        self.punch = self.punch.max(if gold { 0.9 } else { 0.45 });
        if gold {
            self.shout = Some(("GOLDEN".into(), 1.0));
        } else if self.mult >= 3 {
            self.shout = Some((format!("{}X STREAK", self.mult), 1.0));
        }

        // The next apple is golden on the count, not on a die roll: a reward
        // you can see coming is one you will change your line to reach.
        let next_gold = (self.eaten + 1).is_multiple_of(GOLD_EVERY);
        self.apple = place(&mut self.rng, &self.body, next_gold);
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
        // The whole body goes at once. The dissolve that follows is what is
        // left of it burning down; this is the impact.
        let body: Vec<(i32, i32)> = self.body.iter().copied().collect();
        let n = body.len().max(1) as f32;
        for (i, &(x, y)) in body.iter().enumerate() {
            let t = i as f32 / n;
            let hue = crate::world::hex(0x00F0FF).lerp(crate::world::hex(0xFF23C8), t);
            self.sparks
                .burst(&mut self.rng, (x as f32 + 0.5, y as f32 + 0.5), 3, 13.0, hue);
        }
        self.heat = (self.heat + 0.4).min(1.0);
        self.queued.clear();
    }
}

/// A free cell, chosen uniformly. Falls back to any in-bounds cell if the snake
/// has filled the arena — at which point the run is won and about to end
/// anyway, and a panic here would be the worst possible way to say so.
fn place(rng: &mut Rng, body: &VecDeque<(i32, i32)>, gold: bool) -> Apple {
    let free: Vec<(i32, i32)> = (0..ROWS)
        .flat_map(|y| (0..COLS).map(move |x| (x, y)))
        .filter(|c| !body.contains(c))
        .collect();
    let at = if free.is_empty() {
        (COLS / 2, ROWS / 2)
    } else {
        free[rng.range(free.len() as u32) as usize]
    };
    Apple {
        at,
        gold,
        ttl: gold.then_some(GOLD_LIFE),
    }
}

impl Default for Snake {
    fn default() -> Self {
        Self::new()
    }
}

impl Game for Snake {
    fn kind(&self) -> Kind {
        Kind::Snake
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

        // Dead snakes still burn: the shell keeps stepping so the dissolve runs
        // on the same clock as everything else.
        if self.over {
            self.death = (self.death + dts / DEATH_SECS).min(1.0);
            return;
        }

        self.elapsed = self.elapsed.saturating_add(dt);
        self.since_eat += dts;
        if self.since_eat > STREAK_WINDOW {
            self.mult = 1;
        }

        // A golden apple that times out is replaced by a plain one, so the
        // arena is never left without something to chase.
        if let Some(ttl) = &mut self.apple.ttl {
            *ttl -= dts;
            if *ttl <= 0.0 {
                self.apple = place(&mut self.rng, &self.body, false);
            }
        }

        self.steer(input);

        let interval = self.interval();
        self.accum += dts;
        // Bounded catch-up: a stalled terminal must not teleport the snake
        // across the arena the moment it comes back.
        let mut moves = 0;
        while self.accum >= interval && moves < 4 && !self.over {
            self.accum -= interval;
            moves += 1;
            self.advance();
        }
        if self.over {
            self.accum = 0.0;
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
        [("APPLES", self.eaten), ("LENGTH", self.body.len() as u32)]
    }

    fn pops(&self) -> &[Pop] {
        &self.pops
    }

    /// Head for the apple on whichever axis is further off, and refuse any move
    /// that would end the run. Greedy alone walks into its own body within a
    /// handful of apples; greedy plus a one-step safety check survives long
    /// enough to be worth watching.
    fn autopilot(&self) -> Input {
        let (hx, hy) = self.head();
        let (ax, ay) = self.apple.at;
        let (dx, dy) = (ax - hx, ay - hy);
        let mut want = Vec::with_capacity(6);
        let horizontal = if dx > 0 { Dir::Right } else { Dir::Left };
        let vertical = if dy > 0 { Dir::Down } else { Dir::Up };
        if dx.abs() >= dy.abs() {
            want.push(horizontal);
            want.push(vertical);
        } else {
            want.push(vertical);
            want.push(horizontal);
        }
        want.extend([Dir::Up, Dir::Down, Dir::Left, Dir::Right]);

        let safe = want.into_iter().find(|d| {
            let (ox, oy) = d.delta();
            let next = (hx + ox, hy + oy);
            (0..COLS).contains(&next.0)
                && (0..ROWS).contains(&next.1)
                && !self.body.contains(&next)
        });
        let mut input = Input::default();
        match safe {
            Some(Dir::Up) => input.up = true,
            Some(Dir::Down) => input.down = true,
            Some(Dir::Left) => input.left = true,
            Some(Dir::Right) => input.right = true,
            None => {}
        }
        input
    }

    fn shout(&self) -> Option<(&str, f32)> {
        self.shout.as_ref().map(|(s, life)| (s.as_str(), *life))
    }

    fn paint(&self, b: &mut Buf, l: &Layout) {
        paint::paint(b, l, self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Run long enough for at least `cells` cells of movement at the current
    /// pace, plus a little slack for the accumulator.
    fn run(g: &mut Snake, input: &Input, cells: u32) {
        for _ in 0..(cells as f32 * PERIODS[0] / 0.016) as u32 + 8 {
            g.step(input, ms(16));
        }
    }

    /// Keep the snake alive for `secs` by turning it before it reaches a wall,
    /// so a test about time passing is not secretly a test about crashing. The
    /// apple is parked in the middle, which the border lap never crosses.
    fn lap(g: &mut Snake, secs: f32) {
        g.apple = Apple {
            at: (COLS / 2, ROWS / 2),
            gold: false,
            ttl: None,
        };
        for _ in 0..(secs / 0.016) as u32 {
            let (x, y) = g.head();
            let mut input = Input::default();
            match g.dir() {
                Dir::Right if x >= COLS - 3 => input.down = true,
                Dir::Down if y >= ROWS - 3 => input.left = true,
                Dir::Left if x <= 2 => input.up = true,
                Dir::Up if y <= 2 => input.right = true,
                _ => {}
            }
            g.step(&input, ms(16));
            assert!(!g.over, "the lap is supposed to stay alive");
        }
    }

    #[test]
    fn the_arena_matches_the_hole_the_world_cuts() {
        // The logic and the layout read the same source; this fails the moment
        // one of them stops.
        assert_eq!((COLS as usize, ROWS as usize), Kind::Snake.field());
    }

    #[test]
    fn the_body_glides_between_its_cells_rather_than_jumping() {
        let mut g = Snake::with_rng(Rng::from_seed(20));
        // One whole move first: until something has moved there is nothing to
        // interpolate, and the drawn body deliberately trails the logical one
        // by exactly one cell.
        g.step(&Input::default(), ms(160));
        let start = g.segment_at(0, g.glide());
        g.step(&Input::default(), ms(40));
        let mid = g.segment_at(0, g.glide());
        assert!(mid.0 > start.0, "the head has moved: {start:?} -> {mid:?}");
        assert!(mid.0.fract() != 0.0, "and it is between cells: {}", mid.0);
        assert!(mid.0 < g.head().0 as f32, "and has not arrived yet");
    }

    #[test]
    fn a_dead_snake_is_frozen_on_its_last_cells() {
        let mut g = Snake::with_rng(Rng::from_seed(21));
        while !g.over {
            g.step(&Input::default(), ms(16));
        }
        assert_eq!(g.glide(), 1.0, "no coasting past the collision");
    }

    #[test]
    fn an_apple_banks_hitstop_and_a_marker_once() {
        let mut g = Snake::with_rng(Rng::from_seed(22));
        let (hx, hy) = g.head();
        g.apple = Apple {
            at: (hx + 2, hy),
            gold: false,
            ttl: None,
        };
        while g.eaten == 0 {
            g.step(&Input::default(), ms(16));
        }
        assert_eq!(g.pops().len(), 1);
        assert_eq!(g.pops()[0].points, POINTS[0]);
        assert_eq!(g.take_hitstop(), HITSTOP_APPLE);
        assert_eq!(g.take_hitstop(), 0, "hitstop is a debt, paid once");
    }

    #[test]
    fn walks_forward_and_keeps_its_length() {
        let mut g = Snake::with_rng(Rng::from_seed(1));
        let len = g.len();
        let (x0, y0) = g.head();
        run(&mut g, &Input::default(), 3);
        let (x1, y1) = g.head();
        assert!(x1 > x0, "the snake moved right");
        assert_eq!(y0, y1, "no drift off its row");
        // It may have eaten on the way, but it can never shrink.
        assert!(g.len() >= len);
    }

    #[test]
    fn a_wall_ends_the_run() {
        let mut g = Snake::with_rng(Rng::from_seed(2));
        run(&mut g, &Input::default(), COLS as u32 + 4);
        assert!(g.over, "running east off the arena kills");
    }

    #[test]
    fn reversing_into_its_own_neck_is_refused() {
        let mut g = Snake::with_rng(Rng::from_seed(3));
        let input = Input {
            left: true,
            ..Default::default()
        };
        g.step(&input, ms(16));
        assert!(g.queued.is_empty(), "a reversal is never queued");
        run(&mut g, &input, 2);
        assert!(!g.over, "and so it cannot kill");
        assert_eq!(g.dir(), Dir::Right);
    }

    #[test]
    fn two_turns_inside_one_cell_both_land() {
        let mut g = Snake::with_rng(Rng::from_seed(4));
        g.step(
            &Input {
                up: true,
                ..Default::default()
            },
            ms(1),
        );
        g.step(
            &Input {
                left: true,
                ..Default::default()
            },
            ms(1),
        );
        assert_eq!(g.queued.len(), 2, "the second tap is buffered, not eaten");
        run(&mut g, &Input::default(), 2);
        assert_eq!(g.dir(), Dir::Left);
    }

    #[test]
    fn eating_grows_and_scores() {
        let mut g = Snake::with_rng(Rng::from_seed(5));
        let len = g.len();
        // Put the apple directly ahead so the walk cannot miss it, then park
        // the next one out of reach so exactly one is eaten.
        let (hx, hy) = g.head();
        g.apple = Apple {
            at: (hx + 2, hy),
            gold: false,
            ttl: None,
        };
        while g.eaten == 0 {
            g.step(&Input::default(), ms(16));
        }
        // Park the replacement out of reach, then let the owed cell grow in.
        g.apple = Apple {
            at: (0, ROWS - 1),
            gold: false,
            ttl: None,
        };
        while g.growth > 0 {
            g.step(&Input::default(), ms(16));
        }
        assert_eq!(g.eaten, 1);
        assert_eq!(
            g.score, POINTS[0],
            "the first apple pays its tier flat, with no multiplier"
        );
        assert_eq!(g.len(), len + 1);
    }

    #[test]
    fn the_pace_tightens_a_tier_at_a_time() {
        let mut g = Snake::with_rng(Rng::from_seed(23));
        let opening = g.interval();
        assert_eq!(g.tier(), 0);
        // Within a tier the pace holds; crossing one tightens it.
        g.eaten = TIER_SIZE - 1;
        assert_eq!(g.interval(), opening);
        g.eaten = TIER_SIZE;
        assert!(g.interval() < opening, "a tier boundary speeds the snake up");
        // And the ramp has a floor rather than running away.
        g.eaten = 10_000;
        assert_eq!(g.interval(), PERIODS[PERIODS.len() - 1]);
    }

    #[test]
    fn the_multiplier_climbs_on_a_streak_and_drops_on_a_pause() {
        let mut g = Snake::with_rng(Rng::from_seed(6));
        for _ in 0..3 {
            let (hx, hy) = g.head();
            g.apple = Apple {
                at: (hx + 1, hy),
                gold: false,
                ttl: None,
            };
            run(&mut g, &Input::default(), 2);
        }
        assert!(g.mult() > 1, "back-to-back apples raise the multiplier");
        lap(&mut g, STREAK_WINDOW + 0.2);
        assert_eq!(g.mult(), 1, "a pause drops it back to one");
    }

    #[test]
    fn a_golden_apple_expires_into_a_plain_one() {
        let mut g = Snake::with_rng(Rng::from_seed(7));
        g.apple = Apple {
            at: (0, 0),
            gold: true,
            ttl: Some(0.05),
        };
        g.step(&Input::default(), ms(100));
        assert!(!g.apple().gold, "it does not wait around");
        assert!(g.apple().ttl.is_none());
    }

    #[test]
    fn death_burns_before_the_shell_moves_on() {
        let mut g = Snake::with_rng(Rng::from_seed(8));
        while !g.over {
            g.step(&Input::default(), ms(16));
        }
        assert!(!g.is_over(), "the dissolve has to finish first");
        for _ in 0..((DEATH_SECS / 0.016) as u32 + 2) {
            g.step(&Input::default(), ms(16));
        }
        assert!(g.is_over());
    }

    #[test]
    fn the_apple_never_lands_under_the_snake() {
        let mut g = Snake::with_rng(Rng::from_seed(9));
        for _ in 0..200 {
            let a = place(&mut g.rng, &g.body, false);
            assert!(!g.body.contains(&a.at));
        }
    }
}

#[cfg(test)]
mod balance {
    use super::*;

    /// Seconds to cross the field corner to corner at a given tier — the worst
    /// an apple can be placed.
    fn worst(eaten: u32) -> f32 {
        let mut g = Snake::with_rng(Rng::from_seed(1));
        g.eaten = eaten;
        (COLS + ROWS - 2) as f32 * g.interval()
    }

    /// Seconds to the average apple: the mean Manhattan distance between two
    /// uniform points on the grid is a third of each side.
    fn typical(eaten: u32) -> f32 {
        let mut g = Snake::with_rng(Rng::from_seed(1));
        g.eaten = eaten;
        (COLS + ROWS) as f32 / 3.0 * g.interval()
    }

    #[test]
    fn the_ordinary_apple_is_chainable_and_the_far_one_is_not() {
        // This is the whole tension of the game. If every apple were inside the
        // window the multiplier would be free; if none were it would be
        // decoration. The average has to be in and the corner has to be out.
        for eaten in [0, 3, 6, 9, 12, 15, 40] {
            assert!(
                typical(eaten) < STREAK_WINDOW,
                "the average apple is out of reach at {eaten} eaten"
            );
            assert!(
                worst(eaten) > STREAK_WINDOW,
                "even the far corner keeps the chain at {eaten} eaten"
            );
        }
    }

    #[test]
    fn chaining_gets_easier_as_the_snake_gets_faster() {
        // The reward for surviving the climb: the window does not shrink with
        // the tier, so the same field takes less time to cross.
        assert!(typical(40) < typical(0));
        assert!(worst(40) < worst(0));
    }

    #[test]
    fn a_golden_apple_is_a_decision_rather_than_a_collection() {
        // Reachable on average from the moment it appears, and not reachable
        // from the far corner until the snake has earned some speed.
        assert!(typical(0) < GOLD_LIFE, "never worth going for");
        assert!(worst(0) > GOLD_LIFE, "always worth going for");
    }

    /// A square arena leaves the player equidistant from everything and the
    /// game loses its rhythm of long runs into tight corners; three times as
    /// wide as it is tall is a corridor. Checked at compile time, since both
    /// sides are constants.
    const _: () = {
        assert!(COLS > ROWS);
        assert!(COLS < 3 * ROWS);
        // Charging length for the bonus punishes taking it.
        assert!(GOLD_GROWTH == 1);
    };
}
