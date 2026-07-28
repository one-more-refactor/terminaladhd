//! terminaladhd — an arcade for the dead time.
//!
//! Hit enter on something slow, and the terminal turns into an arcade
//! cabinet you can play until the command comes back: a black screen, a warp
//! field behind the arena, and a wheel that picks the game — then picks a
//! different one every time you die.
//!
//! Three layers, each usable alone:
//!
//! - [`world`] — the renderer. Takes a size in cells, hands back bytes.
//! - [`games`] — pure game logic driven by `step(input, dt)`.
//! - [`term`] — raw mode, key decoding and a restore that survives a panic.

pub mod app;
pub mod games;
pub mod rng;
pub mod scores;
pub mod stage;
pub mod term;
pub mod world;
pub mod wrap;
