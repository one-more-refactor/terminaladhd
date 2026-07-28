//! The games. Each one is pure logic driven by a `step` call — no terminal, no
//! timers of its own — and draws itself onto the fixed [`Screen`] canvas, so
//! the same core runs headless in tests and paints identically everywhere.
//!
//! The shell knows games only through [`Kind`] and [`Game`]: how to spawn one,
//! how to advance it, what tone it lights the tube with, and how to draw it.
//! Adding a game is a variant and a module, nothing else.

pub mod snake;
pub mod tetris;

use std::time::Duration;

use crate::rng::Rng;
use crate::screen::{Phosphor, Screen};

pub use snake::Snake;
pub use tetris::Blocks;

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

/// What the shell needs from a running game.
pub trait Game {
    /// Which game this is.
    fn kind(&self) -> Kind;

    fn step(&mut self, input: &Input, dt: Duration);

    /// True only once there is nothing left to show — the death animation is
    /// the game's own, and it finishes before the shell moves on.
    fn is_over(&self) -> bool;

    fn score(&self) -> u32;

    /// How hot the player is, `0.0..=1.0`. The shell pushes the phosphor
    /// towards gold as this rises, so the screen itself gets louder the better
    /// the run is going.
    fn heat(&self) -> f32 {
        0.0
    }

    /// Render frames the whole machine should freeze for, and zero afterwards.
    /// An impact that stops time reads as an impact; one that does not reads
    /// as a colour change. Drained by the shell once a frame.
    fn take_hitstop(&mut self) -> u32 {
        0
    }

    /// How hard to blow the tube out white for what just happened, `0.0..=1.0`
    /// and zero afterwards. This is the game's loud channel: the field invert
    /// is local, the flash is the whole monitor saying so.
    fn take_flash(&mut self) -> f32 {
        0.0
    }

    /// One step of input for a game playing itself — the demo brain behind
    /// `--shot`, and the proof that the game is playable at all.
    fn autopilot(&self) -> Input {
        Input::default()
    }

    /// Draw the whole screen for this game: field, score, everything. The
    /// canvas is cleared; the shell adds nothing on top but the wrap rule.
    fn draw(&self, s: &mut Screen);
}

/// Every game the machine can land on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Blocks,
    Snake,
}

/// The order the reel carries them in. Adding a variant here is all it takes
/// to put a game in rotation.
pub const ALL: [Kind; 2] = [Kind::Blocks, Kind::Snake];

impl Kind {
    /// The name on the reel.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Blocks => "BLOCKS",
            Kind::Snake => "SNAKE",
        }
    }

    /// The tone this game lights the tube with.
    pub fn phosphor(self) -> Phosphor {
        match self {
            Kind::Blocks => Phosphor::ICE,
            Kind::Snake => Phosphor::LIME,
        }
    }

    /// One line of controls, short enough for the reel's hold. The rest is
    /// taught the way a cabinet taught: by playing.
    pub fn hint(self) -> &'static str {
        match self {
            Kind::Blocks => "X SPIN - SPACE DROP",
            Kind::Snake => "STEER - WALLS BITE",
        }
    }

    pub fn spawn(self, rng: Rng) -> Box<dyn Game> {
        match self {
            Kind::Blocks => Box::new(Blocks::with_rng(rng)),
            Kind::Snake => Box::new(Snake::with_rng(rng)),
        }
    }

    /// The key this game's scores are filed under. Deliberately not
    /// [`Kind::name`]: renaming a reel entry must not orphan a score table.
    pub fn slug(self) -> &'static str {
        match self {
            Kind::Blocks => "blocks",
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
        // installed — the reel is the only way in.
        for k in [Kind::Blocks, Kind::Snake] {
            assert!(ALL.contains(&k), "{k:?} is not in ALL");
        }
    }

    #[test]
    fn slugs_are_unique_and_tones_are_too() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.slug(), b.slug());
                // The phosphor is the one thing that says what you are playing
                // before you have read a word of it.
                assert_ne!(a.phosphor(), b.phosphor(), "{a:?} and {b:?} share a tone");
            }
        }
    }

    #[test]
    fn hints_fit_the_canvas() {
        for k in ALL {
            assert!(
                crate::screen::text_width(k.hint()) <= crate::screen::W as i32,
                "{k:?} hint is too wide to draw"
            );
        }
    }

    #[test]
    fn no_input_storm_panics_a_game() {
        // A cheap fuzz: every kind, random input every step, run well past
        // death, repeated across seeds. In a debug build this also catches
        // arithmetic overflow. It proves nothing about correctness — it exists
        // because "the game crashed while I was playing" is the one bug report
        // that must never be true, and random mashing is how players play.
        for (k, kind) in ALL.into_iter().enumerate() {
            for seed in 0..6u64 {
                let seed = seed * 31 + k as u64;
                let mut fz = Rng::from_seed(seed ^ 0x5eed);
                let mut game = kind.spawn(Rng::from_seed(seed + 1));
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
                    let _ = (game.score(), game.heat());
                    let _ = (game.take_hitstop(), game.take_flash());
                    if game.is_over() {
                        dead += 1;
                        // A dead game is still stepped by the shell during the
                        // settle; it has to survive that too.
                        if dead > 120 {
                            break;
                        }
                    }
                }
                // And drawing any state it ended in must not panic either.
                let mut s = Screen::new();
                game.draw(&mut s);
            }
        }
    }
}
