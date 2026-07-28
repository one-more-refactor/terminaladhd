//! Breakout core: a wall of bricks, one paddle, one ball, three lives — pure
//! logic on a millisecond clock, the way the other games are. No terminal, no
//! rendering; a caller drives [`Breakout::step`] and reads the court through
//! the accessors the painter needs.
//!
//! The 1976 rules, kept because they are load-bearing: the ball speeds up on
//! a count of paddle hits and again the first time it reaches the deep rows,
//! the paddle *shrinks* the first time the ball touches the ceiling, and the
//! bounce angle comes from where the ball lands on the paddle — the paddle is
//! the only aim mechanism there is, which is the entire skill of the game.

pub mod paint;

use std::time::Duration;

use crate::rng::Rng;
use crate::world::layout::Layout;
use crate::world::{Buf, Sparks};

use super::{Game, Input, Kick, Kind, Pop};

/// The court the tests and the autopilot reason about when no live game is at
/// hand. A running game carries its own, sized to the frame it spawned on.
pub const COLS: i32 = 26;
pub const ROWS: i32 = 16;

/// The wall: this many rows of bricks, each brick two cells wide, starting
/// this many rows under the ceiling. Deep enough to be a wall, shallow enough
/// that the first serve reaches it inside a second.
pub const WALL_ROWS: usize = 6;
const WALL_TOP: i32 = 1;

/// Points by wall row, top first — the classic ladder: the deep rows pay
/// most, and they are also the ones that speed the ball up when first hit.
const ROW_POINTS: [u32; WALL_ROWS] = [7, 7, 5, 5, 3, 1];

/// Clearing the whole wall pays a bonus on top of its bricks.
const CLEAR_BONUS: u32 = 50;

/// Balls per run. Three is the cabinet number, and a run at three lasts about
/// as long as a snake run — which is what the rotation wants.
const BALLS: u32 = 3;

/// Paddle width in cells, and what the first ceiling touch shrinks it to.
const PADDLE_W: f32 = 4.0;
const PADDLE_SHRUNK: f32 = 3.0;
/// Cells per second the paddle moves while a direction is held.
const PADDLE_SPEED: f32 = 26.0;

/// Ball speed in cells per second: the serve, the two hit-count steps, the
/// deep-row step, and the ceiling every later wall adds onto.
const SPEED_LADDER: [f32; 4] = [9.0, 11.5, 14.0, 17.0];
const SPEED_HITS: [u32; 2] = [4, 12];
const WALL_SPEED: f32 = 1.5;
const SPEED_CAP: f32 = 21.0;

/// The serve: how long the ball sits on the paddle before it launches
/// itself. Long enough to find the paddle, short enough to never be a wait.
const SERVE_SECS: f32 = 0.8;

/// Steepest and shallowest the paddle can send the ball, as the horizontal
/// share of a unit velocity. A ball that leaves the paddle flatter than this
/// crosses forever and never comes down; steeper and the english is gone.
const ENGLISH_MAX: f32 = 0.80;

/// How long the lost ball's court holds before the next serve, and how long
/// the paddle blinks out at the end of the run.
const LOSS_SECS: f32 = 0.55;
const DEATH_SECS: f32 = 0.7;

const HEAT_DECAY_SECS: f32 = 2.5;
const SHAKE_DECAY_SECS: f32 = 0.30;
const PRAISE_SECS: f32 = 1.4;
const POP_SECS: f32 = 0.8;

/// Render frames the machine freezes for: a brick is felt only through the
/// paddle (a court that stutters on every brick is unplayable), so the stops
/// are the ball coming back off the paddle, a lost ball, and the wall going.
const HITSTOP_PADDLE: u32 = 2;
const HITSTOP_LOSS: u32 = 8;
const HITSTOP_CLEAR: u32 = 10;

/// One brick of the wall.
#[derive(Clone, Copy, Debug)]
pub struct Brick {
    /// Cell of its left half; every brick is two cells wide, one tall.
    pub at: (i32, i32),
    /// Wall row it belongs to, 0 at the top — its colour and its points.
    pub row: usize,
}

/// What the ball is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Glued to the paddle, counting down to the launch.
    Serve(f32),
    Flying,
    /// Lost below the paddle; the court holds before the next serve.
    Lost(f32),
}

pub struct Breakout {
    cols: i32,
    rows: i32,
    bricks: Vec<Brick>,
    /// Ball centre in cell coordinates, and cells per second.
    ball: (f32, f32),
    vel: (f32, f32),
    /// Paddle centre x in cells; y is always the last row.
    paddle: f32,
    paddle_w: f32,
    /// Whether the ceiling has already taken its toll this run.
    shrunk: bool,
    phase: Phase,
    balls_left: u32,
    /// Paddle hits this wall — the speed ladder counts them.
    hits: u32,
    /// Highest wall row index the ball has broken into (lowest number).
    deepest: usize,
    walls: u32,
    broken: u32,
    score: u32,
    rng: Rng,
    elapsed: f32,
    heat: f32,
    shake: f32,
    shout: Option<(String, f32)>,
    pops: Vec<Pop>,
    punch: f32,
    hitstop: u32,
    kick: Option<Kick>,
    pub(crate) sparks: Sparks,
    over: bool,
    death: f32,
}

impl Breakout {
    pub fn new() -> Self {
        Self::with_rng(Rng::new())
    }

    pub fn with_rng(rng: Rng) -> Self {
        Self::with_field(rng, COLS, ROWS)
    }

    pub fn with_field(rng: Rng, cols: i32, rows: i32) -> Self {
        let (cols, rows) = (cols.max(12), rows.max(12));
        let mut g = Breakout {
            cols,
            rows,
            bricks: Vec::new(),
            ball: (0.0, 0.0),
            vel: (0.0, 0.0),
            paddle: cols as f32 * 0.5,
            paddle_w: PADDLE_W,
            shrunk: false,
            phase: Phase::Serve(SERVE_SECS),
            balls_left: BALLS,
            hits: 0,
            deepest: WALL_ROWS,
            walls: 0,
            broken: 0,
            score: 0,
            rng,
            elapsed: 0.0,
            heat: 0.0,
            shake: 0.0,
            shout: None,
            pops: Vec::new(),
            punch: 0.0,
            hitstop: 0,
            kick: None,
            sparks: Sparks::new(),
            over: false,
            death: 0.0,
        };
        g.build_wall();
        g
    }

    /// A fresh wall of bricks, two cells to a brick, a cell of air at either
    /// side wall so the ball can always thread the edge lane.
    fn build_wall(&mut self) {
        self.bricks.clear();
        let per_row = ((self.cols - 2) / 2) as usize;
        for row in 0..WALL_ROWS {
            for i in 0..per_row {
                self.bricks.push(Brick {
                    at: (1 + i as i32 * 2, WALL_TOP + row as i32),
                    row,
                });
            }
        }
        self.hits = 0;
        self.deepest = WALL_ROWS;
    }

    /// The speed the ladder currently says, plus what cleared walls add.
    fn speed(&self) -> f32 {
        let mut step = 0;
        if self.hits >= SPEED_HITS[0] {
            step = 1;
        }
        if self.hits >= SPEED_HITS[1] {
            step = 2;
        }
        // Breaking into the deep half of the wall is the third step, wherever
        // the hit count stands — the classic rule, and the moment a run stops
        // being a warm-up.
        if self.deepest < WALL_ROWS / 2 {
            step = 3;
        }
        (SPEED_LADDER[step] + self.walls as f32 * WALL_SPEED).min(SPEED_CAP)
    }

    /// Which rung of the speed ladder the run is on, `0..=3` — the readout.
    pub fn tier(&self) -> usize {
        let s = self.speed();
        SPEED_LADDER.iter().filter(|&&r| s >= r).count().saturating_sub(1)
    }

    pub fn bricks(&self) -> &[Brick] {
        &self.bricks
    }

    pub fn ball(&self) -> (f32, f32) {
        self.ball
    }

    /// Whether the ball is live — a serving or lost ball is drawn differently.
    pub fn flying(&self) -> bool {
        self.phase == Phase::Flying
    }

    pub fn serving(&self) -> bool {
        matches!(self.phase, Phase::Serve(_))
    }

    pub fn paddle(&self) -> (f32, f32) {
        (self.paddle, self.paddle_w)
    }

    pub fn paddle_row(&self) -> i32 {
        self.rows - 1
    }

    pub fn balls_left(&self) -> u32 {
        self.balls_left
    }

    pub fn walls(&self) -> u32 {
        self.walls
    }

    /// `0.0..=1.0` once the run is over — how far the paddle has burned out.
    pub fn death(&self) -> f32 {
        self.death
    }

    /// Unit direction of flight, for the painter's wake; zero on the serve.
    pub fn vel_dir(&self) -> (f32, f32) {
        let len = (self.vel.0 * self.vel.0 + self.vel.1 * self.vel.1).sqrt();
        if len <= 0.0 {
            (0.0, 0.0)
        } else {
            (self.vel.0 / len, self.vel.1 / len)
        }
    }

    fn serve(&mut self) {
        self.phase = Phase::Serve(SERVE_SECS);
        self.vel = (0.0, 0.0);
    }

    fn launch(&mut self) {
        // Upward, at a modest random slant — never straight up, which would
        // bounce between the walls forever without asking anything of anyone.
        let side = if self.rng.range(2) == 0 { -1.0 } else { 1.0 };
        let dx = side * (0.25 + 0.35 * (self.rng.range(1000) as f32 / 1000.0));
        let dy = -(1.0f32 - dx * dx).sqrt();
        let s = self.speed();
        self.vel = (dx * s, dy * s);
        self.phase = Phase::Flying;
    }

    fn lose_ball(&mut self) {
        self.hitstop = self.hitstop.max(HITSTOP_LOSS);
        self.punch = self.punch.max(0.6);
        self.shake = self.shake.max(2.0);
        self.balls_left -= 1;
        if self.balls_left == 0 {
            self.over = true;
            self.death = 0.0;
            self.punch = 1.0;
            self.kick = Some(Kick::Death);
        } else {
            self.phase = Phase::Lost(LOSS_SECS);
            self.shout = Some((format!("BALL {}", BALLS - self.balls_left + 1), 1.0));
        }
    }

    /// One brick gone. The wall pays, the court reacts, and an empty wall
    /// rebuilds itself a step faster.
    fn break_brick(&mut self, i: usize) {
        let b = self.bricks.swap_remove(i);
        self.broken += 1;
        let points = ROW_POINTS[b.row];
        self.score += points;
        self.deepest = self.deepest.min(b.row);
        self.heat = (self.heat + 0.10).min(1.0);
        self.punch = self.punch.max(0.25);
        let hue = paint::row_color(b.row);
        self.sparks.burst(
            &mut self.rng,
            (b.at.0 as f32 + 1.0, b.at.1 as f32 + 0.5),
            6,
            9.0,
            hue,
        );
        // Only the top rows are worth a marker — seven points is an event,
        // one point is a metronome.
        if points >= 5 {
            self.pops.push(Pop {
                col: b.at.0 as f32 + 0.5,
                row: b.at.1 as f32,
                points,
                life: 1.0,
            });
        }
        if self.bricks.is_empty() {
            self.score += CLEAR_BONUS;
            self.walls += 1;
            self.hitstop = self.hitstop.max(HITSTOP_CLEAR);
            self.punch = 1.0;
            self.shake = self.shake.max(3.0);
            self.kick = Some(Kick::Huge);
            self.shout = Some(("WALL CLEARED".into(), 1.0));
            self.pops.push(Pop {
                col: self.cols as f32 * 0.5,
                row: (WALL_TOP + WALL_ROWS as i32 / 2) as f32,
                points: CLEAR_BONUS,
                life: 1.0,
            });
            self.build_wall();
            self.serve();
        }
    }

    /// Advance the ball by `dt`, in sub-steps small enough that it can never
    /// tunnel a brick or the paddle at the top of the speed ladder.
    fn fly(&mut self, dt: f32) {
        let speed = self.speed();
        // Re-aim the velocity to the current ladder speed, keeping direction:
        // the ball gets faster the moment the rule says so, not at the next
        // serve.
        let len = (self.vel.0 * self.vel.0 + self.vel.1 * self.vel.1).sqrt();
        if len > 0.0 {
            let k = speed / len;
            self.vel = (self.vel.0 * k, self.vel.1 * k);
        }
        let steps = ((speed * dt) / 0.25).ceil().max(1.0) as usize;
        let sub = dt / steps as f32;
        for _ in 0..steps {
            if self.phase != Phase::Flying {
                break;
            }
            self.ball.0 += self.vel.0 * sub;
            self.ball.1 += self.vel.1 * sub;
            self.collide();
        }
    }

    fn collide(&mut self) {
        let (mut x, mut y) = self.ball;

        // Side walls.
        if x < 0.3 {
            x = 0.6 - x;
            self.vel.0 = self.vel.0.abs();
        }
        if x > self.cols as f32 - 0.3 {
            x = 2.0 * (self.cols as f32 - 0.3) - x;
            self.vel.0 = -self.vel.0.abs();
        }
        // The ceiling — and the first touch takes a piece of the paddle,
        // which is the game telling you it has stopped being polite.
        if y < 0.3 {
            y = 0.6 - y;
            self.vel.1 = self.vel.1.abs();
            if !self.shrunk {
                self.shrunk = true;
                self.paddle_w = PADDLE_SHRUNK;
                self.hitstop = self.hitstop.max(HITSTOP_PADDLE);
                self.shout = Some(("PADDLE SHRINKS".into(), 1.0));
            }
        }
        self.ball = (x, y);

        // The paddle. Only a falling ball can be caught, and where it lands
        // on the face decides where it goes — the whole aim of the game.
        let py = self.paddle_row() as f32 + 0.5;
        if self.vel.1 > 0.0 && y >= py - 0.5 && y <= py + 0.4 {
            let half = self.paddle_w * 0.5;
            let off = (x - self.paddle) / half;
            if off.abs() <= 1.1 {
                let english = (off * ENGLISH_MAX).clamp(-ENGLISH_MAX, ENGLISH_MAX);
                let s = self.speed();
                self.vel = (english * s, -(1.0 - english * english).sqrt() * s);
                self.ball.1 = py - 0.55;
                self.hits += 1;
                self.hitstop = self.hitstop.max(HITSTOP_PADDLE);
                self.punch = self.punch.max(0.15);
                self.heat = (self.heat + 0.04).min(1.0);
                return;
            }
        }
        // The floor.
        if y > self.rows as f32 {
            self.lose_ball();
            return;
        }

        // The wall. One brick per sub-step: the nearest whose two cells the
        // ball is inside. The bounce axis comes from which face is nearer,
        // which is as much physics as a brick ever had.
        let bx = x;
        let by = y;
        let hit = self
            .bricks
            .iter()
            .position(|b| {
                bx >= b.at.0 as f32 - 0.2
                    && bx <= b.at.0 as f32 + 2.2
                    && by >= b.at.1 as f32 - 0.2
                    && by <= b.at.1 as f32 + 1.2
            });
        if let Some(i) = hit {
            let b = self.bricks[i];
            let cx = b.at.0 as f32 + 1.0;
            let cy = b.at.1 as f32 + 0.5;
            // Wider than tall, so the vertical faces are the common ones.
            if ((bx - cx) / 1.2).abs() > (by - cy).abs() {
                self.vel.0 = if bx < cx {
                    -self.vel.0.abs()
                } else {
                    self.vel.0.abs()
                };
            } else {
                self.vel.1 = if by < cy {
                    -self.vel.1.abs()
                } else {
                    self.vel.1.abs()
                };
            }
            self.break_brick(i);
        }
    }
}

/// A free function so the painter can colour a row without a game in hand.
impl Default for Breakout {
    fn default() -> Self {
        Self::new()
    }
}

impl Game for Breakout {
    fn kind(&self) -> Kind {
        Kind::Breakout
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

        if self.over {
            self.death = (self.death + dts / DEATH_SECS).min(1.0);
            return;
        }
        self.elapsed += dts;

        // The paddle: held state, full speed, hard stop at the walls. No
        // acceleration ramp — a breakout paddle that eases in is a breakout
        // paddle that misses.
        let dir = match (input.left, input.right) {
            (true, false) => -1.0,
            (false, true) => 1.0,
            _ => 0.0,
        };
        let half = self.paddle_w * 0.5;
        self.paddle = (self.paddle + dir * PADDLE_SPEED * dts)
            .clamp(half + 0.2, self.cols as f32 - half - 0.2);

        match self.phase {
            Phase::Serve(ref mut t) => {
                *t -= dts;
                let done = *t <= 0.0;
                // Rides the paddle until it goes.
                self.ball = (self.paddle, self.paddle_row() as f32 - 0.6);
                if done {
                    self.launch();
                }
            }
            Phase::Lost(ref mut t) => {
                *t -= dts;
                if *t <= 0.0 {
                    self.serve();
                }
            }
            Phase::Flying => self.fly(dts),
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
        [("BRICKS", self.broken), ("WALLS", self.walls)]
    }

    fn pops(&self) -> &[Pop] {
        &self.pops
    }

    /// Track the ball the way a person does: only once it is coming down, and
    /// deliberately off-centre — catching on the paddle's shoulder is how you
    /// aim, and it is also how you eventually die, because the wild angles it
    /// produces are the ones half a flight's tracking cannot always cover.
    /// Competent enough to rally, mortal enough that the attract loop moves
    /// on, and it visibly *plays* rather than merely returns.
    fn autopilot(&self) -> Input {
        let mut input = Input::default();
        if !self.flying() || self.vel.1 <= 0.0 {
            return input;
        }
        // Slow hands: it lets some frames go by unmoved, which prices its
        // effective speed under the ball's late-ladder pace. Without this the
        // tracker is a wall and the demo never ends.
        if ((self.elapsed * 7.0) as u32) % 10 >= 6 {
            return input;
        }
        // Which shoulder it plays for swings slowly with the clock, so the
        // demo works both sides of the court.
        let aim = if (self.elapsed * 0.23).fract() < 0.5 { 1.0 } else { -1.0 };
        let err = self.ball.0 + aim * self.paddle_w * 0.42 - self.paddle;
        if err < -0.4 {
            input.left = true;
        } else if err > 0.4 {
            input.right = true;
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

    fn seeded() -> Breakout {
        Breakout::with_rng(Rng::from_seed(9))
    }

    /// Step `secs` of neutral input.
    fn run(g: &mut Breakout, secs: f32) {
        for _ in 0..(secs / 0.016) as u32 {
            g.step(&Input::default(), ms(16));
        }
    }

    #[test]
    fn the_serve_launches_itself_upward() {
        let mut g = seeded();
        assert!(g.serving(), "a fresh game opens on the serve");
        run(&mut g, SERVE_SECS + 0.1);
        assert!(g.flying(), "the ball launches without being asked");
        assert!(g.vel.1 < 0.0, "and it goes up");
    }

    #[test]
    fn the_wall_is_full_and_the_edge_lanes_are_open() {
        let g = seeded();
        let per_row = ((COLS - 2) / 2) as usize;
        assert_eq!(g.bricks().len(), WALL_ROWS * per_row);
        for b in g.bricks() {
            assert!(b.at.0 >= 1 && b.at.0 + 2 < COLS, "brick in the lane: {b:?}");
        }
    }

    #[test]
    fn bricks_break_score_and_deepen() {
        let mut g = seeded();
        let first = g.bricks()[0];
        g.ball = (first.at.0 as f32 + 1.0, first.at.1 as f32 + 0.5);
        g.vel = (0.0, -9.0);
        g.phase = Phase::Flying;
        g.collide();
        assert_eq!(g.broken, 1);
        assert_eq!(g.score, ROW_POINTS[first.row]);
        assert!(g.deepest <= first.row);
    }

    #[test]
    fn the_paddle_aims_the_ball() {
        let mut g = seeded();
        g.phase = Phase::Flying;
        g.paddle = 13.0;
        // Falling onto the paddle's right half: leaves to the right.
        g.ball = (14.2, g.paddle_row() as f32 + 0.2);
        g.vel = (0.0, 9.0);
        g.collide();
        assert!(g.vel.1 < 0.0, "caught and sent back up");
        assert!(g.vel.0 > 0.0, "off the right half it leaves to the right");
        // And the fastest english is capped, so it always comes down again.
        let s = (g.vel.0 * g.vel.0 + g.vel.1 * g.vel.1).sqrt();
        assert!(g.vel.0.abs() / s <= ENGLISH_MAX + 1e-4);
    }

    #[test]
    fn the_ceiling_shrinks_the_paddle_once() {
        let mut g = seeded();
        g.phase = Phase::Flying;
        g.ball = (5.0, 0.2);
        g.vel = (0.0, -9.0);
        g.collide();
        assert_eq!(g.paddle(), (g.paddle, PADDLE_SHRUNK));
        assert!(g.shrunk);
        // A second touch keeps it where it is rather than whittling it away.
        g.ball = (5.0, 0.2);
        g.vel = (0.0, -9.0);
        g.collide();
        assert_eq!(g.paddle_w, PADDLE_SHRUNK);
    }

    #[test]
    fn a_lost_ball_serves_the_next_and_the_last_ends_the_run() {
        let mut g = seeded();
        for ball in (1..=BALLS).rev() {
            g.phase = Phase::Flying;
            g.ball = (5.0, g.rows as f32 + 0.5);
            g.vel = (0.0, 9.0);
            g.collide();
            if ball > 1 {
                assert!(matches!(g.phase, Phase::Lost(_)));
                assert_eq!(g.balls_left(), ball - 1);
                run(&mut g, LOSS_SECS + 0.1);
                assert!(g.serving(), "the court reloads itself");
                run(&mut g, SERVE_SECS + 0.1);
            } else {
                assert!(g.over, "the last ball ends the run");
                assert!(!g.is_over(), "after its burn-out, not before");
                run(&mut g, DEATH_SECS + 0.1);
                assert!(g.is_over());
            }
        }
    }

    #[test]
    fn clearing_the_wall_pays_rebuilds_and_speeds_up() {
        let mut g = seeded();
        g.phase = Phase::Flying;
        let before_speed = g.speed();
        // Break everything but one by hand, then hit the last one properly.
        while g.bricks.len() > 1 {
            let last = g.bricks.len() - 1;
            g.break_brick(last);
        }
        let score_before = g.score;
        g.break_brick(0);
        assert!(g.score >= score_before + ROW_POINTS[WALL_ROWS - 1] + CLEAR_BONUS);
        assert_eq!(g.walls(), 1);
        assert!(!g.bricks().is_empty(), "the wall rebuilds");
        assert!(g.serving(), "and the ball re-serves");
        assert!(g.speed() > before_speed, "every wall is faster than the last");
        assert_eq!(g.take_kick(), Some(Kick::Huge));
        assert_eq!(g.take_hitstop(), HITSTOP_CLEAR);
    }

    #[test]
    fn the_speed_ladder_climbs_on_hits_and_depth() {
        let mut g = seeded();
        let open = g.speed();
        g.hits = SPEED_HITS[0];
        assert!(g.speed() > open);
        g.hits = SPEED_HITS[1];
        let mid = g.speed();
        assert!(mid > SPEED_LADDER[1]);
        g.deepest = 0;
        assert!(g.speed() > mid, "reaching the deep rows is the third step");
        g.walls = 50;
        assert_eq!(g.speed(), SPEED_CAP, "and the whole thing has a ceiling");
    }

    #[test]
    fn the_ball_cannot_tunnel_at_top_speed() {
        // Fire the fastest possible ball through a full wall for a while: it
        // must end every frame inside the court and never below the floor
        // without the loss being called.
        let mut g = seeded();
        g.walls = 50; // pin the ladder to the cap
        run(&mut g, SERVE_SECS + 0.1);
        for _ in 0..2000 {
            g.step(&Input::default(), ms(16));
            if g.over {
                break;
            }
            let (x, y) = g.ball;
            assert!(x >= -0.5 && x <= g.cols as f32 + 0.5, "ball left the court: {x}");
            assert!(y >= -0.5 && y <= g.rows as f32 + 1.5, "ball fell through: {y}");
        }
    }

    #[test]
    fn the_autopilot_rallies_but_is_mortal() {
        let mut g = Breakout::with_rng(Rng::from_seed(4));
        let mut steps = 0u32;
        while !g.is_over() && steps < 60 * 60 * 4 {
            let input = g.autopilot();
            g.step(&input, ms(16));
            steps += 1;
        }
        assert!(g.broken > 3, "the demo has to look like play: {}", g.broken);
        assert!(g.is_over(), "and it has to end so the attract loop moves on");
    }

    #[test]
    fn drawing_never_touches_the_logical_state() {
        let mut g = seeded();
        run(&mut g, 2.0);
        let before = (g.ball, g.vel, g.paddle, g.bricks.len(), g.score);
        let l = Layout::for_field(120, 34, g.cols as usize, g.rows as usize);
        let mut b = Buf::new(120, 34);
        for _ in 0..3 {
            g.paint(&mut b, &l);
        }
        let after = (g.ball, g.vel, g.paddle, g.bricks.len(), g.score);
        assert_eq!(before.4, after.4);
        assert_eq!(before.3, after.3);
        assert_eq!(before.0, after.0);
        assert_eq!(before.1, after.1);
        assert_eq!(before.2, after.2);
    }
}

#[cfg(test)]
mod balance {
    use super::*;

    #[test]
    fn the_paddle_can_always_get_there() {
        // From one wall to the other is the worst case the english can ask;
        // the ball's crossing at the cap must take longer than the paddle's.
        let crossing = COLS as f32 / (SPEED_CAP * ENGLISH_MAX);
        let paddle = (COLS as f32 - PADDLE_SHRUNK) / PADDLE_SPEED;
        assert!(
            paddle < crossing,
            "an unreachable ball is a coin toss, not a game: {paddle} vs {crossing}"
        );
    }

    #[test]
    fn a_run_is_the_rotation_s_size() {
        // Three balls at the opening speed: the first wall alone is over a
        // minute of rally if played well, and a whiffed run is still three
        // serves and three falls — never an instant exit.
        let fall = ROWS as f32 / SPEED_LADDER[0];
        assert!(fall > 1.0, "a serve must live long enough to be seen");
        const _: () = {
            assert!(BALLS == 3);
            assert!(WALL_ROWS == 6);
        };
    }

    #[test]
    fn the_deep_rows_pay_most() {
        for i in 1..ROW_POINTS.len() {
            assert!(ROW_POINTS[i - 1] >= ROW_POINTS[i], "the ladder inverts at {i}");
        }
        assert!(CLEAR_BONUS > ROW_POINTS[0] * 5, "the wall is worth chasing");
    }
}
