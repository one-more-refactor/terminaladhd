//! The world: a cabinet screen. Black ground, a warp field flying outward
//! behind the game, a hard vector frame around it, and a status strip — drawn
//! at half-block sub-pixel resolution and encoded with a damage-tracked diff.
//!
//! It owns no terminal and no game. A caller builds a [`Layout`] for a `(w, h)`
//! in cells, lays down [`cabinet::ground`], draws the [`Warp`] and whatever the
//! game wants into a [`scene::Buf`] of `w × 2h` sub-pixels, runs [`bloom`] and
//! [`scanlines`], resolves to [`encode::Cell`]s, and ships the delta with
//! [`encode::enc_diff`].
//!
//! Everything scales from `(w, h)` and nothing is reserved for decoration: the
//! arena takes every row the chrome does not need.

pub mod color;
pub mod cabinet;
pub mod crt;
pub mod draw;
pub mod encode;
pub mod font;
pub mod layout;
pub mod scene;
pub mod spark;
pub mod warp;
pub mod tiny;

pub use color::{
    bayer4, hex, linear_to_srgb, srgb_to_linear, to_16, to_256, to_256_d, to_srgb8, Rgb, ANSI16,
};
pub use encode::{
    enc_16, enc_256, enc_diff, enc_naive, enc_stateful, resolve, resolve_d, Cell, UPPER_HALF,
};
pub use layout::{Layout, Rect};
pub use scene::{
    bloom, chrome_ramp, chrome_word, chrome_word_w, palette, posterize, resolve_no_bloom, scanlines,
    smoothstep, Buf, Ramp,
};
pub use spark::Sparks;
pub use warp::Warp;
