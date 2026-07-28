//! terminaladhd — an arcade for the dead time.
//!
//! The picture is 80×48 one-bit pixels on an arcade monitor's black glass,
//! drawn with half-block characters and blown up in whole pixels to whatever
//! the terminal is. A reel picks the game, the game plays until you die, and
//! the reel turns again.
//!
//! The crate splits along one seam: [`games`] is pure logic that draws onto
//! the [`screen`] canvas, and [`app`] is the machine around it — the reel,
//! the clock, the monitor and the keys. [`wrap`] runs a command behind all of
//! it without ever touching the command's stdout.

pub mod app;
pub mod encode;
pub mod font;
pub mod games;
pub mod rng;
pub mod scores;
pub mod screen;
pub mod term;
pub mod wrap;
