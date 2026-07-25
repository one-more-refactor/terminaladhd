//! The games cut into the world. Each one is pure logic driven by a `step`
//! call — no terminal, no rendering — so the same core runs headless in tests.

pub mod tetris;

pub use tetris::Tetris;
