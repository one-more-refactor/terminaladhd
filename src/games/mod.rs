//! The games cut into the world. Each one is pure logic driven by a `step`
//! call — no terminal, no timers of its own — so the same core runs headless in
//! tests and paints identically from a `--shot` dump and from a live frame.
//!
//! The shell knows games only through [`Kind`] and [`Game`]: what arena to cut,
//! how to advance it, and how to paint it. Adding a game means adding a variant
//! and nothing else.

pub mod breakout;
pub mod snake;
pub mod tetris;

use std::time::Duration;

use crate::rng::Rng;
use crate::world::layout::Layout;
use crate::world::scene::palette::*;
use crate::world::{hex, Buf, Rgb};

pub use breakout::Breakout;
pub use snake::Snake;
pub use tetris::Tetris;

/// One step's worth of input. Held keys (the four directions) stay true for as
/// long as they are down; the rest are edges — true only on the step the key
/// was pressed. Edge detection is the caller's job, since a cooked terminal
/// cannot always be trusted to report releases.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    pub cw: bool,
    pub ccw: bool,
    pub hard: bool,
    pub hold: bool,
    /// Direction *presses* this step, in the order they arrived. The booleans
    /// above are state — true for as long as a key is down — which is what an
    /// auto-shift wants and exactly what a steering wheel does not: held state
    /// has no order, so two keys rolled around a corner collapse into
    /// whichever the reader happens to check first, and a key still held from
    /// three cells ago outvotes the tap that was meant to turn. A tap lands
    /// here once, in sequence, and is never outvoted by a hold.
    pub taps: Taps,
}

impl Input {
    /// An input that taps one direction — the way a snake is actually steered.
    pub fn turn(t: Turn) -> Input {
        let mut input = Input::default();
        input.taps.push(t);
        input
    }
}

/// A direction key going down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    Up,
    Down,
    Left,
    Right,
}

/// Direction presses in arrival order, at most four to a step — enough to bank
/// a double corner with room to spare, and small enough to stay `Copy`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Taps {
    buf: [Option<Turn>; 4],
}

impl Taps {
    /// Record a press; presses past the fourth are dropped, which by then is
    /// mashing rather than steering.
    pub fn push(&mut self, t: Turn) {
        if let Some(slot) = self.buf.iter_mut().find(|s| s.is_none()) {
            *slot = Some(t);
        }
    }

    /// The presses, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = Turn> + '_ {
        self.buf.iter().flatten().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.buf[0].is_none()
    }
}

/// A score marker floating up from where it was earned. Both games emit these
/// and the shell draws them, so `+150` off a Tetris and `+90` off a golden
/// apple are the same object in the same face — which is most of what makes two
/// different games feel like one machine.
#[derive(Clone, Copy, Debug)]
pub struct Pop {
    /// Arena cell the points were earned at. Fractional, because a snake is
    /// between cells more often than it is on one.
    pub col: f32,
    pub row: f32,
    pub points: u32,
    /// `1.0` at birth down to `0.0`. The shell rises and fades it.
    pub life: f32,
}

/// What just happened, in the only terms the shell needs. A game names the
/// event; the shell decides how loud the screen gets about it, which is what
/// keeps two games reacting identically to the same kind of thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kick {
    /// A clear worth acknowledging and nothing more.
    Small,
    /// A clear worth interrupting the picture for.
    Big,
    /// The best thing this game has: a Tetris, a perfect clear.
    Huge,
    /// A bonus taken. Paid rather than interrupted, so it reads warm.
    Bonus,
    /// The run ending.
    Death,
}

/// What the shell needs from a running game.
pub trait Game {
    /// Which game this is. The shell needs it to cut the right arena for a
    /// game it is only holding as a `dyn Game` — the attract demo, above all.
    fn kind(&self) -> Kind;

    /// The arena this run is being played on, in minos. Fixed when the game was
    /// spawned, so a resize never moves the walls.
    fn field(&self) -> (usize, usize);

    fn step(&mut self, input: &Input, dt: Duration);
    fn is_over(&self) -> bool;
    fn score(&self) -> u32;

    /// How hot the player is, `0.0..=1.0`. Drives the grid scroll, so playing
    /// harder visibly speeds the world up.
    fn heat(&self) -> f32;

    /// Screen shake in whole cells, applied to the arena only.
    fn shake(&self) -> i32;

    /// The loudest thing that happened since the last call, and nothing
    /// afterwards.
    fn take_kick(&mut self) -> Option<Kick> {
        None
    }

    /// Render frames the whole machine should freeze for, and zero afterwards.
    /// An impact that stops time reads as an impact; one that does not reads as
    /// a colour change. Drained by the shell once a frame.
    fn take_hitstop(&mut self) -> u32 {
        0
    }

    /// Score markers currently in the air.
    fn pops(&self) -> &[Pop] {
        &[]
    }

    /// The impact since the last call, `0.0..=1.0`, and zero afterwards. The
    /// shell drains it once a frame and spends it on the warp field and the
    /// screen flash — which is how a line clear reaches the background without
    /// any game knowing the background exists.
    fn take_punch(&mut self) -> f32 {
        0.0
    }

    /// What the game wants shouted under the arena, and how much of its life is
    /// left. `None` when it has nothing to say.
    fn shout(&self) -> Option<(&str, f32)> {
        None
    }

    /// One step of input for a game playing itself. This is what the attract
    /// screen shows, so it has to look like someone competent is at the
    /// controls — a demo that flails is worse than no demo.
    fn autopilot(&self) -> Input {
        Input::default()
    }

    /// The two numbers this run is remembered by, for the screen that is read
    /// after it rather than played on.
    fn tally(&self) -> [(&'static str, u32); 2] {
        [("", 0), ("", 0)]
    }

    /// Paint the arena and the game's own columns. The ground, the warp field
    /// and the chrome are the shell's, and are already on the buffer.
    fn paint(&self, b: &mut Buf, l: &Layout);
}

/// Every game the machine can land on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tetris,
    Snake,
    Breakout,
}

/// The order the roulette spins through. Adding a variant here is all it takes
/// to put a game in rotation.
pub const ALL: [Kind; 3] = [Kind::Tetris, Kind::Snake, Kind::Breakout];

impl Kind {
    /// The name on the marquee. Kept to the chrome face's alphabet.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Tetris => "BLOCKS",
            Kind::Snake => "SNAKE",
            Kind::Breakout => "BREAKOUT",
        }
    }

    /// The arena to cut, in minos, for a given frame.
    ///
    /// Tetris is 10×20 wherever it is played. A mino is square in sub-pixels,
    /// so the well is twice as tall as it is wide — that is the game, and
    /// stretching it to fill a wide terminal would make it a different one.
    /// A tetris cabinet used a monitor stood on its end for exactly this
    /// reason, and there is no equivalent of that here.
    ///
    /// Snake has no such shape. It is fourteen rows because that is what the
    /// height affords, and as many columns as the width will carry — so on a
    /// wide terminal the field claims the screen rather than sitting in the
    /// middle of it.
    pub fn field(self, w: usize, h: usize) -> (usize, usize) {
        match self {
            Kind::Tetris => (10, 20),
            // Sixteen rows: six of wall, and enough air under it that a
            // rally is a rally rather than a reflex test. Columns follow the
            // width the way snake's do.
            Kind::Breakout => {
                const ROWS: usize = 16;
                let body = h.saturating_sub(4);
                let px = [10usize, 8, 6, 5, 4, 3, 2]
                    .into_iter()
                    .find(|&p| ROWS * p / 2 <= body)
                    .unwrap_or(2);
                let usable = w.saturating_sub(2 * (18 + 2));
                let cols = (usable / px).clamp(18, 40);
                (cols, ROWS)
            }
            Kind::Snake => {
                const ROWS: usize = 14;
                // The same mino the layout will pick, chosen from the height
                // alone, so the column count can be solved for the width.
                let body = h.saturating_sub(4);
                let px = [10usize, 8, 6, 5, 4, 3, 2]
                    .into_iter()
                    .find(|&p| ROWS * p / 2 <= body)
                    .unwrap_or(2);
                let usable = w.saturating_sub(2 * (18 + 2));
                let cols = (usable / px).clamp(18, 48);
                (cols, ROWS)
            }
        }
    }

    /// The colour of this game's arena frame.
    ///
    /// It used to carry the hazard — warm where the walls kill, cool where they
    /// are furniture — and that idea produced first a red frame and then an
    /// amber one, both of which were the ugliest thing on the screen. A
    /// boundary is not the right place to say it: the wall warning already
    /// says it, locally, at the moment it matters and in the direction it
    /// matters, which is everything a resting colour cannot do.
    ///
    /// So the frame is only an edge now. Snake gets a cool white rail because
    /// its body already runs cyan to magenta and its morsel is green; there is
    /// no hot hue left that would not be competing with the game inside it.
    pub fn hue(self) -> Rgb {
        hex(match self {
            Kind::Tetris => CYAN,
            Kind::Snake => RAIL,
            Kind::Breakout => VIOLET,
        })
    }

    /// The one line of controls the ticker carries while this game is up.
    pub fn hint(self) -> &'static str {
        match self {
            Kind::Tetris => "WASD OR ARROWS - Z X ROTATE - SPACE DROPS - C HOLDS",
            Kind::Snake => "STEER WITH WASD OR THE ARROWS - THE WALLS BITE",
            Kind::Breakout => "LEFT AND RIGHT - THE PADDLE IS THE AIM",
        }
    }

    /// Spawn a game sized for this frame. The field it gets is fixed for the
    /// life of the run: a resize changes how big a cell is drawn, never how
    /// many there are, or a snake would find itself outside its own arena.
    pub fn spawn(self, rng: Rng, w: usize, h: usize) -> Box<dyn Game> {
        let (cols, rows) = self.field(w, h);
        match self {
            Kind::Tetris => Box::new(Tetris::with_rng(rng)),
            Kind::Snake => Box::new(Snake::with_field(rng, cols as i32, rows as i32)),
            Kind::Breakout => Box::new(Breakout::with_field(rng, cols as i32, rows as i32)),
        }
    }

    /// The layout a fresh game of this kind wants for a terminal size. A game
    /// already running has its own field; use [`Game::field`].
    pub fn layout(self, w: usize, h: usize) -> Layout {
        let (cols, rows) = self.field(w, h);
        Layout::for_field(w, h, cols, rows)
    }

    /// The key this game's scores are filed under. Deliberately not
    /// [`Kind::name`]: renaming a marquee must not orphan a score table.
    pub fn slug(self) -> &'static str {
        match self {
            Kind::Tetris => "blocks",
            Kind::Snake => "snake",
            Kind::Breakout => "breakout",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_in_the_rotation() {
        // A game missing from ALL would be unreachable while still looking
        // installed — the roulette is the only way in.
        for k in [Kind::Tetris, Kind::Snake, Kind::Breakout] {
            assert!(ALL.contains(&k), "{k:?} is not in ALL");
        }
    }

    #[test]
    fn slugs_are_unique() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.slug(), b.slug());
            }
        }
    }

    #[test]
    fn no_input_storm_at_any_size_panics_a_game() {
        // A cheap fuzz: every kind, a spread of frames, random input every
        // step, run well past death. In a debug build this also catches
        // arithmetic overflow. It proves nothing about correctness — it exists
        // because "the game crashed while I was playing" is the one bug report
        // that must never be true, and random mashing is how players play.
        for (k, kind) in ALL.into_iter().enumerate() {
            for (i, (w, h)) in [(60, 24), (80, 25), (120, 34), (200, 50), (400, 100)]
                .into_iter()
                .enumerate()
            {
                let seed = (k * 8 + i) as u64 + 1;
                let mut fz = Rng::from_seed(seed ^ 0x5eed);
                let mut game = kind.spawn(Rng::from_seed(seed), w, h);
                let mut dead = 0;
                for _ in 0..4000 {
                    let mut input = Input {
                        left: fz.range(4) == 0,
                        right: fz.range(4) == 0,
                        up: fz.range(8) == 0,
                        down: fz.range(4) == 0,
                        cw: fz.range(8) == 0,
                        ccw: fz.range(8) == 0,
                        hard: fz.range(16) == 0,
                        hold: fz.range(16) == 0,
                        taps: Taps::default(),
                    };
                    for _ in 0..fz.range(3) {
                        input.taps.push(match fz.range(4) {
                            0 => Turn::Up,
                            1 => Turn::Down,
                            2 => Turn::Left,
                            _ => Turn::Right,
                        });
                    }
                    game.step(&input, Duration::from_millis(16));
                    let _ = (game.score(), game.heat(), game.shake(), game.tally());
                    let _ = (game.take_punch(), game.take_hitstop(), game.take_kick());
                    if game.is_over() {
                        dead += 1;
                        // A dead game is still stepped by the shell during the
                        // settle; it has to survive that too.
                        if dead > 120 {
                            break;
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_game_gets_its_own_frame_colour() {
        // The frame is the one thing that says what you are playing before you
        // have read a word of it, so two games must never share a hue.
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.hue(), b.hue(), "{a:?} and {b:?} share a frame");
            }
        }
    }
}
