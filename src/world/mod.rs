//! The world: one continuous synthwave field — indigo sky, a slit-striped
//! setting sun, a receding neon grid, a chrome wordmark — drawn at half-block
//! sub-pixel resolution and encoded with a damage-tracked diff.
//!
//! It owns no terminal and no game. A caller builds a [`scene::Scene`] for a
//! `(w, h)` in
//! cells, renders into a [`scene::Buf`] of `w × 2h` sub-pixels, runs [`bloom`]
//! and [`scanlines`], resolves to [`encode::Cell`]s, and ships the delta with
//! [`encode::enc_diff`].
//!
//! Everything scales from `(w, h)`: horizon at 0.46·height, lane count from
//! width, sun radius from both. There are no black bars and no wasted columns
//! at any aspect ratio.

pub mod color;
pub mod diorama;
pub mod encode;
pub mod font;
pub mod layout;
pub mod scene;
pub mod tiny;

pub use color::{
    bayer4, hex, linear_to_srgb, srgb_to_linear, to_16, to_256, to_256_d, to_srgb8, Rgb, ANSI16,
};
pub use encode::{
    enc_16, enc_256, enc_diff, enc_naive, enc_stateful, resolve, resolve_d, Cell, UPPER_HALF,
};
pub use layout::{sun_at, Layout, Rect};
pub use scene::{
    bloom, chrome_ramp, chrome_word, chrome_word_w, ground_ramp, palette, resolve_no_bloom,
    scanlines, sky_ramp, smoothstep, sun_ramp, Buf, Opts, Ramp, Scene,
};
