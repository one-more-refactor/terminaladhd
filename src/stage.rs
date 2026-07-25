//! The stage: everything that depends on one terminal size.
//!
//! A [`Stage`] owns the [`Layout`] and the three scratch buffers a frame passes
//! through — sub-pixel colour, post-effect colour, resolved cells — so a frame
//! allocates nothing. A resize throws the whole thing away and builds a new one,
//! which is what guarantees every rectangle reflows through `Layout` and never
//! through a stale coordinate.

use std::time::Duration;

use crate::games::tetris::Tetris;
use crate::world::diorama::{
    base_scene_sun, paint, selector, ticker, title_mark, DioramaState, Mino, PieceView,
};
use crate::world::layout::Layout;
use crate::world::{bloom, resolve_d, scanlines, Buf, Cell, Rgb};

/// Bloom radius, sample step and weight. Tuned on the reviewed stills: wide
/// enough that the sun and the neon bleed, tight enough that the grid stays a
/// grid rather than a smear.
const BLOOM: (usize, usize, f32) = (4, 2, 0.55);
/// How much darker every second sub-row is. Subtle — a CRT hint, not a mask.
const SCANLINE: f32 = 0.86;
/// Colour-match tolerance when resolving sub-pixels to cells. 2/255 is below
/// the perceptual floor and cuts the diff substantially.
const TOL: i32 = 2;

/// An empty well, for scenes that have no game behind them.
const NO_WELL: [[Option<Mino>; 10]; 20] = [[None; 10]; 20];

/// The wordmark standing in the sky.
pub const TITLE: &str = "ADHD";

pub struct Stage {
    pub w: usize,
    pub h: usize,
    pub layout: Layout,
    /// `0.0..=1.0` — how far the sun has set. Set by whoever knows what the
    /// machine is doing; the scene reads it every frame.
    pub sun_sink: f32,
    buf: Buf,
    px: Vec<Rgb>,
    pub cells: Vec<Cell>,
}

impl Stage {
    pub fn new(w: usize, h: usize) -> Self {
        Stage {
            w,
            h,
            layout: Layout::new(w, h),
            sun_sink: 0.0,
            buf: Buf::new(w, h),
            px: Vec::new(),
            cells: Vec::new(),
        }
    }

    /// The idle world: wide horizon, no well cut into it, the wordmark in the
    /// sky and a game name sitting on the weld. `phase` scrolls the grid.
    pub fn attract(&mut self, phase: f32, word: &str, lit_dot: usize, left: &str, right: &str) {
        let weld = self.layout.horizon_idle;
        self.buf = base_scene_sun(&self.layout, weld, phase, self.sun_sink);
        title_mark(&mut self.buf, &self.layout, TITLE);
        selector(&mut self.buf, &self.layout, weld, word, lit_dot);
        ticker(&mut self.buf, &self.layout, left, right);
        self.finish(0.0);
    }

    /// The world with a game cut into it. `phase` scrolls the grid, `fade` in
    /// `0.0..=1.0` sinks and desaturates the picture for the game-over settle.
    pub fn game(&mut self, game: &Tetris, phase: f32, fade: f32, left: &str, right: &str) {
        let well = game.cells();
        let active_cells = game.active().map(|(_, c)| c);
        let active = match (game.active().map(|(m, _)| m), &active_cells) {
            (Some(mino), Some(c)) => Some(PieceView { cells: &c[..], mino }),
            _ => None,
        };
        let ghost_cells = game.ghost();
        let clearing = game.clearing_rows();
        let st = DioramaState {
            layout: &self.layout,
            well_cells: &well,
            active,
            ghost: ghost_cells.as_ref().map(|g| &g[..]),
            hold: game.hold(),
            next: game.next(),
            score: game.score(),
            lines: game.lines(),
            level: game.level(),
            heat: phase,
            shake: game.shake(),
            clearing: &clearing,
            ticker_left: left,
            ticker_elapsed: right,
            sun_sink: self.sun_sink,
        };
        paint(&mut self.buf, &st);
        self.finish(fade);
    }

    /// The world with the well present but empty — the frame the wrapper shows
    /// while it is only reporting, with nobody playing.
    pub fn idle_well(&mut self, phase: f32, left: &str, right: &str) {
        let st = DioramaState {
            layout: &self.layout,
            well_cells: &NO_WELL,
            active: None,
            ghost: None,
            hold: None,
            next: &[],
            score: 0,
            lines: 0,
            level: 1,
            heat: phase,
            shake: 0,
            clearing: &[],
            ticker_left: left,
            ticker_elapsed: right,
            sun_sink: self.sun_sink,
        };
        paint(&mut self.buf, &st);
        self.finish(0.0);
    }

    /// Post: bloom, scanlines, optional fade, then resolve to cells.
    fn finish(&mut self, fade: f32) {
        bloom(&mut self.buf, BLOOM.0, BLOOM.1, BLOOM.2, &mut self.px);
        scanlines(&mut self.px, self.buf.w, self.buf.sh, SCANLINE);
        if fade > 0.0 {
            self.sink(fade);
        }
        resolve_d(&self.px, self.buf.w, self.buf.sh, TOL, true, &mut self.cells);
    }

    /// Desaturate, dim and drop the picture by a few sub-rows: the settle after
    /// a top-out. Rows move downward, so the walk runs bottom-up in place.
    fn sink(&mut self, fade: f32) {
        let (w, sh) = (self.buf.w, self.buf.sh);
        let drop = (fade * 8.0).round() as usize;
        let dim = 1.0 - 0.55 * fade;
        for y in (0..sh).rev() {
            for x in 0..w {
                let i = y * w + x;
                if y < drop {
                    self.px[i] = Rgb::ZERO;
                    continue;
                }
                let c = self.px[(y - drop) * w + x];
                let luma = 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
                self.px[i] = c.lerp(Rgb::new(luma, luma, luma), fade).mul(dim);
            }
        }
    }

    pub fn sub_pixels(&self) -> (&[Rgb], usize, usize) {
        (&self.px, self.buf.w, self.buf.sh)
    }
}

/// Binary P6 at native sub-pixel resolution. The dump is how a frame is
/// reviewed without a terminal in the way — and how a rendering change is
/// compared against the last one it is supposed to improve on.
pub fn write_ppm(path: &str, px: &[Rgb], w: usize, sh: usize) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(w * sh * 3 + 32);
    buf.extend_from_slice(format!("P6\n{w} {sh}\n255\n").as_bytes());
    for y in 0..sh {
        for x in 0..w {
            buf.extend_from_slice(&crate::world::to_srgb8(
                px[y * w + x],
                crate::world::bayer4(x, y),
            ));
        }
    }
    std::fs::write(path, buf)
}

/// `M:SS`, the only clock format the ticker has room for.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}
