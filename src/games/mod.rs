//! The games cut into the world. Each one is pure logic driven by a `step`
//! call — no terminal, no timers of its own — so the same core runs headless in
//! tests and paints identically from a `--shot` dump and from a live frame.
//!
//! The shell knows games only through [`Kind`] and [`Game`]: what arena to cut,
//! how to advance it, and how to paint it. Adding a game means adding a variant
//! and nothing else.

pub mod snake;
pub mod tetris;

use std::time::Duration;

use crate::rng::Rng;
use crate::world::layout::Layout;
use crate::world::scene::palette::*;
use crate::world::{hex, Buf, Rgb};

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

/// What the shell needs from a running game.
pub trait Game {
    /// Which game this is. The shell needs it to cut the right arena for a
    /// game it is only holding as a `dyn Game` — the attract demo, above all.
    fn kind(&self) -> Kind;

    fn step(&mut self, input: &Input, dt: Duration);
    fn is_over(&self) -> bool;
    fn score(&self) -> u32;

    /// How hot the player is, `0.0..=1.0`. Drives the grid scroll, so playing
    /// harder visibly speeds the world up.
    fn heat(&self) -> f32;

    /// Screen shake in whole cells, applied to the arena only.
    fn shake(&self) -> i32;

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

    /// Paint the arena and the game's own columns. The ground, the warp field
    /// and the chrome are the shell's, and are already on the buffer.
    fn paint(&self, b: &mut Buf, l: &Layout);
}

/// Every game the machine can land on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tetris,
    Snake,
}

/// The order the roulette spins through. Adding a variant here is all it takes
/// to put a game in rotation.
pub const ALL: [Kind; 2] = [Kind::Tetris, Kind::Snake];

impl Kind {
    /// The name on the marquee. Kept to the chrome face's alphabet.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Tetris => "BLOCKS",
            Kind::Snake => "SNAKE",
        }
    }

    /// The arena to cut, in minos. The layout picks a scale that fits it.
    pub const fn field(self) -> (usize, usize) {
        match self {
            Kind::Tetris => (10, 20),
            // Wide, not square. A mino is square in sub-pixels, so 26x14 lands
            // as a field about twice as wide as it is tall — which is the shape
            // snake has always been played on, and the shape a terminal is.
            // A square arena leaves the player equidistant from everything and
            // the game loses its rhythm of long runs into tight corners.
            Kind::Snake => (26, 14),
        }
    }

    /// The colour of this game's arena frame. Not decoration: it is the first
    /// thing that says what you are playing, and a frame you must not touch is
    /// warm where one that is merely furniture is cool.
    ///
    /// Amber rather than red for the hazard. Red on black is the one
    /// combination on this palette that reads as an error dialogue, and a
    /// warning that looks like a failure makes the whole screen feel broken.
    pub fn hue(self) -> Rgb {
        hex(match self {
            Kind::Tetris => CYAN,
            Kind::Snake => ORANGE,
        })
    }

    /// The one line of controls the ticker carries while this game is up.
    pub fn hint(self) -> &'static str {
        match self {
            Kind::Tetris => "ARROWS MOVE - Z X ROTATE - SPACE DROPS - C HOLDS",
            Kind::Snake => "STEER WITH THE ARROWS - THE WALLS BITE",
        }
    }

    pub fn spawn(self, rng: Rng) -> Box<dyn Game> {
        match self {
            Kind::Tetris => Box::new(Tetris::with_rng(rng)),
            Kind::Snake => Box::new(Snake::with_rng(rng)),
        }
    }

    /// The layout this game wants for a terminal size.
    pub fn layout(self, w: usize, h: usize) -> Layout {
        let (cols, rows) = self.field();
        Layout::for_field(w, h, cols, rows)
    }

    /// The key this game's scores are filed under. Deliberately not
    /// [`Kind::name`]: renaming a marquee must not orphan a score table.
    pub fn slug(self) -> &'static str {
        match self {
            Kind::Tetris => "blocks",
            Kind::Snake => "snake",
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
        for k in [Kind::Tetris, Kind::Snake] {
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
