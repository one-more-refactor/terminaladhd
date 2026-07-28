//! SNAKE — a bordered 26×12 field of 3×3-pixel cells, a segmented body that
//! glides between cells rather than jumping them, diamond apples, and every
//! fifth one golden, on a clock.
//!
//! Two things make it a game rather than a demo. The snake gets faster with
//! every apple, so the difficulty is always just ahead of the player; and
//! apples eaten in quick succession build a multiplier that a single
//! hesitation drops back to one, which is what makes a good run feel worth
//! protecting.
//!
//! The whole game is pure state on a millisecond clock: `step` advances it,
//! `draw` only reads it.

use std::collections::VecDeque;
use std::time::Duration;

use crate::rng::Rng;
use crate::screen::{self, Screen};

use super::{Game, Input, Kind, Turn};

/// The field, in cells. Fixed, like everything on the canvas: the border sits
/// under the topbar rule and the grid fills it exactly — 26 × 3 + 2 = 80 wide,
/// 12 × 3 + 2 = 38 of the canvas's remaining rows.
pub const COLS: i32 = 26;
pub const ROWS: i32 = 12;

/// Top of the field border; the grid starts one pixel in.
const FIELD_Y: i32 = 10;
const FIELD_H: u32 = screen::H as u32 - FIELD_Y as u32;
const CELL: i32 = 3;
const GRID_X: i32 = 1;
const GRID_Y: i32 = FIELD_Y + 1;

/// Seconds per cell at each speed tier, and the points an apple pays there.
/// Discrete rather than a smooth curve: a tier the player can feel arriving is
/// worth more than one that creeps, and it gives the score something to say.
const PERIODS: [f32; 6] = [0.150, 0.125, 0.108, 0.095, 0.085, 0.075];
const POINTS: [u32; 6] = [3, 4, 5, 7, 9, 12];
/// Apples per tier: the top tier lands around the fifteenth apple, roughly
/// half a minute in, which is about how long anyone waits for a build step.
const TIER_SIZE: u32 = 3;

/// The snake at spawn: long enough to read as a body, short enough to leave
/// the whole field open.
const START_LEN: usize = 5;

/// What a golden apple multiplies its tier's payout by. It grows the snake by
/// the same single cell an ordinary apple does: the bonus is already the risk
/// of crossing the field under a clock, and charging extra length on top makes
/// taking it a punishment for succeeding.
const GOLD_PAYOUT: u32 = 5;
/// Every Nth apple is golden — on the count, not a die roll: a reward you can
/// see coming is one you will change your line to reach.
const GOLD_EVERY: u32 = 5;
/// And it does not wait around. Short enough that seeing one means dropping
/// the line you were on; at the far corner it is not always reachable, and
/// that is the point — a decision, not a collection.
const GOLD_LIFE: f32 = 3.5;

/// Eat again inside this window and the multiplier climbs; miss it and the
/// run starts over at one. It does not shrink with the tier: the faster the
/// snake, the easier chaining gets, which is the reward for surviving the
/// climb.
const STREAK_WINDOW: f32 = 2.5;
const MAX_MULT: u32 = 8;

const HEAT_DECAY_SECS: f32 = 2.5;

/// How long the dead body blinks before the score is handed over. A collision
/// should read as an event, not a cut.
const DEATH_SECS: f32 = 0.7;
/// Two blinks per second-ish: on for a beat, gone for a beat, three times.
const BLINK_SECS: f32 = 0.12;

/// Render frames the machine freezes for on each event: a tap, a hit, and the
/// 150 ms an impact wants before the eye stops reading it as one.
const HITSTOP_APPLE: u32 = 3;
const HITSTOP_GOLD: u32 = 6;
const HITSTOP_DEATH: u32 = 10;

/// How long the field stays inverted after an apple, and how long a `+N`
/// marker rises before it is gone.
const FLASH_SECS: f32 = 0.06;
const POP_SECS: f32 = 0.45;

/// Turns banked beyond this are dropped; three is enough to bank a double
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

/// What is sitting on the field waiting to be eaten.
#[derive(Clone, Copy, Debug)]
pub struct Apple {
    pub at: (i32, i32),
    pub gold: bool,
    /// Seconds of life left for a golden apple; `None` for a plain one, which
    /// waits forever.
    pub ttl: Option<f32>,
}

/// A `+N` rising from where an apple was taken.
struct Pop {
    /// Pixel position of the cell it came off.
    x: i32,
    y: i32,
    points: u32,
    age: f32,
}

pub struct Snake {
    /// Cells occupied by the body, head first.
    body: VecDeque<(i32, i32)>,
    /// Where those cells were before the last move. Render-only: collision,
    /// self-intersection and apple placement only ever look at `body`, and the
    /// glide between the two is the only thing the eye actually sees.
    prev: VecDeque<(i32, i32)>,
    dir: Dir,
    /// Turns taken but not yet stepped. Each entry was validated against its
    /// predecessor at press time, so popping one can never reverse the snake.
    queued: VecDeque<Dir>,
    apple: Apple,
    rng: Rng,
    /// Cells still owed to a swallowed apple; the tail stays put while these
    /// are outstanding, which is what makes the body grow.
    growth: usize,
    /// Progress towards the next move, in seconds.
    accum: f32,
    eaten: u32,
    mult: u32,
    since_eat: f32,
    score: u32,
    ticks: f32,
    heat: f32,
    pops: Vec<Pop>,
    hitstop: u32,
    flash_out: f32,
    /// Seconds of inverted field left from the last apple.
    flash: f32,
    over: bool,
    /// Seconds into the death blink once dead.
    dying: f32,
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
            ticks: 0.0,
            heat: 0.0,
            pops: Vec::new(),
            hitstop: 0,
            flash_out: 0.0,
            flash: 0.0,
            over: false,
            dying: 0.0,
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
    /// `0.0..=1.0`, eased so a move leaves fast and arrives soft. Frozen at
    /// the far end once dead: a snake still coasting into its last cell reads
    /// as if the collision had not landed.
    pub fn glide(&self) -> f32 {
        if self.over {
            return 1.0;
        }
        let t = (self.accum / self.interval()).clamp(0.0, 1.0);
        // Ahead of linear the whole way but never past the cell: a full ease
        // would leave the head visibly drifting into a cell it has already
        // logically reached.
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

    pub fn mult(&self) -> u32 {
        self.mult
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

    /// Bank the turns from this step's presses, in the order they were made.
    /// Steering reads *taps* and never held state: a key still down from three
    /// cells ago must not outvote the one just pressed, and two keys rolled
    /// around a corner have an order that a held-state snapshot throws away.
    /// Reversing into your own neck is refused here rather than being allowed
    /// to kill, since it is always a misfire.
    fn steer(&mut self, input: &Input) {
        for tap in input.taps.iter() {
            let want = match tap {
                Turn::Up => Dir::Up,
                Turn::Down => Dir::Down,
                Turn::Left => Dir::Left,
                Turn::Right => Dir::Right,
            };
            let last = self.queued.back().copied().unwrap_or(self.dir);
            if want == last || want.opposite(last) {
                continue;
            }
            if self.queued.len() >= QUEUE_CAP {
                return;
            }
            self.queued.push_back(want);
        }
    }

    /// Move one cell: consume a queued turn, then walk the head and either
    /// grow into the new cell or drag the tail after it.
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
        // An apple inside the window steps the multiplier; one that missed it
        // opens a fresh chain at one.
        self.mult = if self.since_eat <= STREAK_WINDOW {
            (self.mult + 1).min(MAX_MULT)
        } else {
            1
        };
        self.since_eat = 0.0;
        // Points at the tier the apple was *taken* on: the tier this apple
        // causes is the next apple's business, not a retroactive raise.
        let mut points = POINTS[self.tier() as usize];
        if gold {
            points *= GOLD_PAYOUT;
        }
        self.eaten += 1;
        let gained = points * self.mult;
        self.score += gained;
        let (px, py) = pixel(self.apple.at.0, self.apple.at.1);
        self.pops.push(Pop {
            x: px,
            y: py,
            points: gained,
            age: 0.0,
        });
        self.hitstop = self
            .hitstop
            .max(if gold { HITSTOP_GOLD } else { HITSTOP_APPLE });
        self.growth += 1;
        self.heat = (self.heat + if gold { 0.55 } else { 0.25 }).min(1.0);
        self.flash = FLASH_SECS;
        // An ordinary apple stays local — one arrives every second or two, and
        // a monitor that blows out that often is a monitor nobody looks at.
        // The gold is what the loud channel is being saved for.
        if gold {
            self.flash_out = 0.6;
        }

        let next_gold = (self.eaten + 1).is_multiple_of(GOLD_EVERY);
        self.apple = place(&mut self.rng, &self.body, next_gold);
    }

    fn die(&mut self) {
        if self.over {
            return;
        }
        self.over = true;
        self.dying = 0.0;
        self.hitstop = self.hitstop.max(HITSTOP_DEATH);
        self.queued.clear();
    }

    /// The whole screen with the body drawn `t` of the way through its move.
    /// Nothing here writes back into the state.
    fn paint(&self, s: &mut Screen, t: f32) {
        // Topbar: score at the left with the live multiplier beside it, the
        // gold countdown mid-right, a rule under all of it.
        let score = self.score.to_string();
        s.text(1, 1, &score);
        if self.mult > 1 {
            // The multiplier blinks out its last moments, because at the far
            // side of the field the bar of its window is the only warning.
            let show = self.since_eat < STREAK_WINDOW - 0.4
                || ((self.ticks / 0.1) as u32).is_multiple_of(2);
            if show {
                s.text(1 + screen::text_width(&score) + 3, 1, &format!("X{}", self.mult));
            }
        }
        if let Some(ttl) = self.apple.ttl {
            self.draw_gold_timer(s, ttl);
        }
        s.hline(0, 8, screen::W as u32);

        s.rect(0, FIELD_Y, screen::W as u32, FIELD_H);
        // Blink: a beat on, a beat off, three times over the death.
        let show_body = !self.over || ((self.dying / BLINK_SECS) as u32).is_multiple_of(2);
        if show_body {
            for i in 0..self.body.len() {
                let (cx, cy) = self.segment_at(i, t);
                let (x, y) = (
                    GRID_X + (cx * CELL as f32).round() as i32,
                    GRID_Y + (cy * CELL as f32).round() as i32,
                );
                s.fill_rect(x, y, 3, 3, true);
                // A hollow centre on everything but the head makes the body
                // read as segments instead of a smear.
                if i > 0 {
                    s.set(x + 1, y + 1, false);
                }
            }
        }

        let (ax, ay) = pixel(self.apple.at.0, self.apple.at.1);
        if self.apple.gold {
            // The gold blinks between a solid block and the diamond so it
            // never disappears outright, and the blink doubles in rate over
            // the last quarter of its clock.
            let ttl = self.apple.ttl.unwrap_or(0.0);
            let rate = if ttl <= GOLD_LIFE / 4.0 { 0.1 } else { 0.2 };
            if ((self.ticks / rate) as u32).is_multiple_of(2) {
                s.fill_rect(ax, ay, 3, 3, true);
            } else {
                diamond(s, ax, ay);
            }
        } else {
            diamond(s, ax, ay);
        }

        if self.flash > 0.0 {
            // Only the interior flips: the border and the topbar holding still
            // is what makes the field itself look like it fired.
            s.invert_rect(1, FIELD_Y + 1, screen::W as u32 - 2, FIELD_H - 2);
        }

        // Markers last, so they stay legible through the inverted frames.
        for p in &self.pops {
            let label = format!("+{}", p.points);
            let w = screen::text_width(&label);
            let x = p.x.min(screen::W as i32 - w - 1).max(1);
            // A pixel of rise per beat: fast enough to read as a lift, slow
            // enough to still be under the eye when it goes.
            let y = (p.y - 1 - (p.age / 0.06) as i32).max(FIELD_Y + 1);
            s.text(x, y, &label);
        }
    }

    /// The gold's countdown bar, in the topbar's spare middle. Under a quarter
    /// left it blinks along with the gold itself.
    fn draw_gold_timer(&self, s: &mut Screen, ttl: f32) {
        if ttl <= GOLD_LIFE / 4.0 && !((self.ticks / 0.1) as u32).is_multiple_of(2) {
            return;
        }
        let len = ((ttl / GOLD_LIFE) * 20.0).ceil().clamp(0.0, 20.0) as u32;
        s.hline(46, 3, len);
        s.hline(46, 4, len);
    }
}

/// A free cell, chosen uniformly. Falls back to the centre if the snake has
/// filled the field — at which point the run is won and about to end anyway,
/// and a panic here would be the worst possible way to say so.
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

/// Pixel corner of a grid cell.
fn pixel(col: i32, row: i32) -> (i32, i32) {
    (GRID_X + col * CELL, GRID_Y + row * CELL)
}

/// A 3×3 block with the corners off — the classic diamond apple.
fn diamond(s: &mut Screen, x: i32, y: i32) {
    s.fill_rect(x, y, 3, 3, true);
    for (cx, cy) in [(x, y), (x + 2, y), (x, y + 2), (x + 2, y + 2)] {
        s.set(cx, cy, false);
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
        self.ticks += dts;
        self.heat = (self.heat - dts / HEAT_DECAY_SECS).max(0.0);
        self.flash = (self.flash - dts).max(0.0);
        for p in &mut self.pops {
            p.age += dts;
        }
        self.pops.retain(|p| p.age < POP_SECS);

        // A dead snake still blinks: the shell keeps stepping so the death
        // runs on the same clock as everything else.
        if self.over {
            self.dying += dts;
            return;
        }

        self.since_eat += dts;
        if self.since_eat > STREAK_WINDOW {
            self.mult = 1;
        }

        // A golden apple that times out is replaced by a plain one, so the
        // field is never left without something to chase.
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
        // across the field the moment it comes back.
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
        self.over && self.dying >= DEATH_SECS
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn heat(&self) -> f32 {
        self.heat.clamp(0.0, 1.0)
    }

    fn take_hitstop(&mut self) -> u32 {
        std::mem::take(&mut self.hitstop)
    }

    fn take_flash(&mut self) -> f32 {
        std::mem::take(&mut self.flash_out)
    }

    /// Head for the apple on whichever axis is further off, and refuse any
    /// move that would end the run. Greedy alone walks into its own body
    /// within a handful of apples; greedy plus a one-step safety check
    /// survives long enough to be worth watching.
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
        match safe {
            Some(Dir::Up) => Input::turn(Turn::Up),
            Some(Dir::Down) => Input::turn(Turn::Down),
            Some(Dir::Left) => Input::turn(Turn::Left),
            Some(Dir::Right) => Input::turn(Turn::Right),
            None => Input::default(),
        }
    }

    fn draw(&self, s: &mut Screen) {
        self.paint(s, self.glide());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Run long enough for at least `cells` cells of movement at the opening
    /// pace, plus a little slack for the accumulator.
    fn run(g: &mut Snake, input: &Input, cells: u32) {
        for _ in 0..(cells as f32 * PERIODS[0] / 0.016) as u32 + 8 {
            g.step(input, ms(16));
        }
    }

    #[test]
    fn the_field_tiles_the_canvas_exactly() {
        // 26 columns of 3px plus the border is the full 80; 12 rows plus the
        // border is the full band under the topbar. A spare pixel would read
        // as a gutter and a missing one would clip the last cell.
        assert_eq!(GRID_X + COLS * CELL + 1, screen::W as i32);
        assert_eq!(GRID_Y + ROWS * CELL + 1, screen::H as i32);
        assert_eq!(FIELD_Y as u32 + FIELD_H, screen::H as u32);
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
        assert!(g.len() >= len);
    }

    #[test]
    fn the_body_glides_between_its_cells_rather_than_jumping() {
        let mut g = Snake::with_rng(Rng::from_seed(20));
        g.apple = Apple {
            at: (0, 0),
            gold: false,
            ttl: None,
        };
        // One whole move first: until something has moved there is nothing to
        // interpolate.
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
    fn a_wall_ends_the_run_and_the_blink_holds_the_score_back() {
        let mut g = Snake::with_rng(Rng::from_seed(2));
        while !g.over {
            g.step(&Input::default(), ms(16));
        }
        assert!(!g.is_over(), "the blink has to finish first");
        assert_eq!(g.take_hitstop(), HITSTOP_DEATH);
        // The body disappears for part of the blink.
        let mut hidden = 0;
        for _ in 0..((DEATH_SECS / 0.016) as u32 + 2) {
            g.step(&Input::default(), ms(16));
            let mut s = Screen::new();
            g.draw(&mut s);
            let (hx, hy) = g.head();
            let (px, py) = pixel(hx, hy);
            if !s.get(px, py) {
                hidden += 1;
            }
        }
        assert!(hidden > 0, "the snake never blinked out");
        assert!(g.is_over());
    }

    #[test]
    fn reversing_into_its_own_neck_is_refused() {
        let mut g = Snake::with_rng(Rng::from_seed(3));
        let input = Input::turn(Turn::Left);
        g.step(&input, ms(16));
        assert!(g.queued.is_empty(), "a reversal is never queued");
        run(&mut g, &input, 2);
        assert!(!g.over, "and so it cannot kill");
        assert_eq!(g.dir(), Dir::Right);
    }

    #[test]
    fn a_corner_rolled_in_one_frame_keeps_its_order() {
        // Up and left land in the same poll — the classic fast corner. Held
        // state would collapse them into one; the taps keep both, in order.
        let mut g = Snake::with_rng(Rng::from_seed(12));
        let mut input = Input::turn(Turn::Up);
        input.taps.push(Turn::Left);
        g.step(&input, ms(1));
        assert_eq!(
            g.queued.iter().copied().collect::<Vec<_>>(),
            vec![Dir::Up, Dir::Left],
            "both turns of the corner are banked, in press order"
        );
        run(&mut g, &Input::default(), 4);
        assert_eq!(g.dir(), Dir::Left, "and both were spent");
    }

    #[test]
    fn a_held_key_cannot_outvote_a_tap() {
        // The old bug: steering read held state through a priority chain, so a
        // key still down from three cells ago re-queued its direction right
        // after the turn the player actually made.
        let mut g = Snake::with_rng(Rng::from_seed(13));
        let mut held = Input::turn(Turn::Up);
        held.right = true;
        g.step(&held, ms(1));
        assert_eq!(g.queued.len(), 1, "only the tap steers");
        assert_eq!(g.queued[0], Dir::Up);
    }

    #[test]
    fn eating_grows_scores_and_banks_the_juice() {
        let mut g = Snake::with_rng(Rng::from_seed(5));
        let len = g.len();
        let (hx, hy) = g.head();
        g.apple = Apple {
            at: (hx + 2, hy),
            gold: false,
            ttl: None,
        };
        while g.eaten == 0 {
            g.step(&Input::default(), ms(16));
        }
        assert_eq!(g.take_hitstop(), HITSTOP_APPLE);
        assert_eq!(g.take_hitstop(), 0, "hitstop is a debt, paid once");
        assert_eq!(g.pops.len(), 1);
        assert_eq!(g.pops[0].points, POINTS[0]);
        // Park the replacement out of reach, then let the owed cell grow in.
        g.apple = Apple {
            at: (0, ROWS - 1),
            gold: false,
            ttl: None,
        };
        while g.growth > 0 {
            g.step(&Input::default(), ms(16));
        }
        assert_eq!(g.score, POINTS[0], "the first apple pays flat");
        assert_eq!(g.len(), len + 1);
    }

    #[test]
    fn a_golden_apple_pays_its_multiple_and_expires_into_a_plain_one() {
        let mut g = Snake::with_rng(Rng::from_seed(7));
        let (hx, hy) = g.head();
        g.apple = Apple {
            at: (hx + 1, hy),
            gold: true,
            ttl: Some(GOLD_LIFE),
        };
        run(&mut g, &Input::default(), 2);
        assert_eq!(g.score, POINTS[0] * GOLD_PAYOUT);
        assert!(g.take_flash() > 0.0, "the gold is the loud one");

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
    fn the_pace_tightens_a_tier_at_a_time() {
        let mut g = Snake::with_rng(Rng::from_seed(23));
        let opening = g.interval();
        assert_eq!(g.tier(), 0);
        g.eaten = TIER_SIZE - 1;
        assert_eq!(g.interval(), opening);
        g.eaten = TIER_SIZE;
        assert!(g.interval() < opening, "a tier boundary speeds the snake up");
        // And the ramp has a floor rather than running away.
        g.eaten = 10_000;
        assert_eq!(g.interval(), PERIODS[PERIODS.len() - 1]);
    }

    #[test]
    fn the_multiplier_climbs_on_a_streak_and_lapses_with_the_window() {
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
        // Base points times the multiplier, apple by apple: 3 + 3×2 + 3×3.
        assert_eq!(g.score, 18);
        g.since_eat = STREAK_WINDOW + 0.1;
        g.step(&Input::default(), ms(16));
        assert_eq!(g.mult(), 1, "a lapsed window resets the chain");
        // And the ceiling holds.
        g.mult = MAX_MULT;
        g.since_eat = 0.0;
        let (hx, hy) = g.head();
        g.apple = Apple {
            at: (hx + 1, hy),
            gold: false,
            ttl: None,
        };
        run(&mut g, &Input::default(), 2);
        assert_eq!(g.mult(), MAX_MULT);
    }

    #[test]
    fn heat_rises_on_apples_and_cools_on_its_own() {
        let mut g = Snake::with_rng(Rng::from_seed(9));
        assert_eq!(g.heat(), 0.0, "a fresh game is cold");
        let (hx, hy) = g.head();
        g.apple = Apple {
            at: (hx + 1, hy),
            gold: false,
            ttl: None,
        };
        run(&mut g, &Input::default(), 2);
        let warm = g.heat();
        assert!(warm > 0.0);
        g.apple = Apple {
            at: (0, 0),
            gold: false,
            ttl: None,
        };
        for _ in 0..30 {
            g.step(&Input::default(), ms(16));
        }
        assert!(g.heat() < warm, "heat decays between apples");
    }

    #[test]
    fn the_apple_never_lands_under_the_snake() {
        let mut g = Snake::with_rng(Rng::from_seed(9));
        for _ in 0..200 {
            let a = place(&mut g.rng, &g.body, false);
            assert!(!g.body.contains(&a.at));
            assert!((0..COLS).contains(&a.at.0) && (0..ROWS).contains(&a.at.1));
        }
    }

    #[test]
    fn drawing_never_touches_the_logical_state() {
        let mut g = Snake::with_rng(Rng::from_seed(11));
        run(&mut g, &Input::default(), 2);
        let before = (
            g.body.clone(),
            g.prev.clone(),
            g.apple.at,
            g.score,
            g.accum.to_bits(),
        );
        let mut s = Screen::new();
        for _ in 0..20 {
            s.clear();
            g.draw(&mut s);
        }
        let after = (
            g.body.clone(),
            g.prev.clone(),
            g.apple.at,
            g.score,
            g.accum.to_bits(),
        );
        assert_eq!(before, after, "drawing leaked into the state");
    }

    #[test]
    fn the_eat_flash_inverts_the_field() {
        let mut g = Snake::with_rng(Rng::from_seed(14));
        let (hx, hy) = g.head();
        g.apple = Apple {
            at: (hx + 1, hy),
            gold: false,
            ttl: None,
        };
        while g.eaten == 0 {
            g.step(&Input::default(), ms(16));
        }
        assert!(g.flash > 0.0);
        let ink = |s: &Screen| {
            (FIELD_Y + 1..screen::H as i32 - 1)
                .flat_map(|y| (1..screen::W as i32 - 1).map(move |x| (x, y)))
                .filter(|&(x, y)| s.get(x, y))
                .count()
        };
        let mut lit = Screen::new();
        g.draw(&mut lit);
        g.flash = 0.0;
        let mut plain = Screen::new();
        g.draw(&mut plain);
        assert!(ink(&lit) > ink(&plain) * 2, "the eat flash should invert");
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
        // This is the whole tension of the game. If every apple were inside
        // the window the multiplier would be free; if none were it would be
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

    /// A square field leaves the player equidistant from everything and the
    /// game loses its rhythm of long runs into tight corners; three times as
    /// wide as it is tall is a corridor. Checked at compile time.
    const _: () = {
        assert!(COLS > ROWS);
        assert!(COLS < 3 * ROWS);
    };
}
