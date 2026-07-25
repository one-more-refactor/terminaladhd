//! terminaladhd — an arcade for the dead time.
//!
//! Hit enter on something slow, and the terminal turns into a synthwave
//! horizon you can play in until the command comes back. The world is one
//! continuous scene rather than a set of panels: sky, sun and grid fill every
//! column at any size, and the game is cut into it.
//!
//! Three layers, each usable alone:
//!
//! - [`world`] — the renderer. Takes a size in cells, hands back bytes.
//! - [`games`] — pure game logic driven by `step(input, dt)`.
//! - [`term`] — raw mode, key decoding and a restore that survives a panic.

pub mod app;
pub mod games;
pub mod rng;
pub mod stage;
pub mod term;
pub mod world;
pub mod wrap;
