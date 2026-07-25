//! The Diorama compositor: paint a full synthwave frame from a description of
//! the current game state.
//!
//! This is pure rendering — it owns no terminal, no game clock and no ratatui.
//! A caller builds a [`Layout`] for a `(w, h)` in cells, fills a [`DioramaState`]
//! and calls [`paint`], then runs the same `bloom` / `scanlines` / `enc_diff`
//! pipeline the scene bin uses. Every element is placed through the [`Layout`],
//! so the frame reflows truthfully at any size.
//!
//! Both the scene bin's static `--shot` frames and a live game bin render
//! through these functions, so a playable frame looks identical to the reviewed
//! stills.

use super::layout::Layout;
use super::scene::chrome_word;
use super::scene::palette::*;
use super::scene::{Buf, Opts, Scene};
use super::tiny as smallfont;
use crate::world::color::{hex, Rgb};
use crate::world::scene::chrome_word_w;

fn c(v: u32) -> Rgb {
    hex(v)
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    a.lerp(b, t)
}

// ------------------------------------------------------------------- minos

/// The seven tetrominoes. The colour and its bevel/settled/ghost derivations
/// (SPEC section 3.2 / 9.2) live here so the scene bin and the game bin skin
/// every block identically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mino {
    I,
    O,
    T,
    S,
    Z,
    J,
    L,
}

impl Mino {
    /// SPEC section 3.2 neon hue, indexed by the palette constant.
    pub fn color(self) -> Rgb {
        hex(match self {
            Mino::I => MINO_I,
            Mino::O => MINO_O,
            Mino::T => MINO_T,
            Mino::S => MINO_S,
            Mino::Z => MINO_Z,
            Mino::J => MINO_J,
            Mino::L => MINO_L,
        })
    }

    /// Bright top/left bevel: the hue lifted toward chrome white.
    pub fn bevel_hi(self) -> Rgb {
        mix(self.color(), c(CHROME_HI), 0.35)
    }

    /// Dark bottom/right bevel: the hue sunk toward the well smoke.
    pub fn bevel_lo(self) -> Rgb {
        mix(self.color(), c(WELL_SMOKE), 0.45)
    }

    /// Settled fill: a locked block reads dimmer than the live piece.
    pub fn settled(self) -> Rgb {
        mix(self.color(), c(WELL_SMOKE), 0.30)
    }

    /// Ghost outline: the receded hue, so the drop preview never reads as solid.
    pub fn ghost(self) -> Rgb {
        mix(self.color(), c(WELL_SMOKE), 0.72)
    }
}

/// The falling piece for the rain-in / lock flash: its occupied cells in well
/// coordinates (column, row; rows may be negative while above the well) and its
/// colour.
pub struct PieceView<'a> {
    pub cells: &'a [(i32, i32)],
    pub mino: Mino,
}

/// Everything a single frame needs. `well_cells` is the settled stack with the
/// active piece already merged for the base draw; `active` re-draws that piece
/// brighter (and paints the rows still above the well as it rains in).
pub struct DioramaState<'a> {
    pub layout: &'a Layout,
    pub well_cells: &'a [[Option<Mino>; 10]; 20],
    pub active: Option<PieceView<'a>>,
    pub ghost: Option<&'a [(i32, i32)]>,
    pub hold: Option<Mino>,
    pub next: &'a [Mino],
    pub score: u32,
    pub lines: u32,
    pub level: u32,
    pub heat: f32,
    pub shake: i32,
    pub clearing: &'a [usize],
    pub ticker_left: &'a str,
    pub ticker_elapsed: &'a str,
    /// `0.0..=1.0` — how far the sun has set. The wrapper drives it with a
    /// command's progress, so the sky itself is the progress bar.
    pub sun_sink: f32,
}

// ------------------------------------------------------------- field writers

fn put_base(b: &mut Buf, x: i32, y: i32, col: Rgb) {
    if x < 0 || y < 0 || x as usize >= b.w || y as usize >= b.sh {
        return;
    }
    let i = y as usize * b.w + x as usize;
    b.base[i] = col;
    b.emis[i] = Rgb::ZERO;
}

fn add_emis(b: &mut Buf, x: i32, y: i32, col: Rgb) {
    if x < 0 || y < 0 || x as usize >= b.w || y as usize >= b.sh {
        return;
    }
    let i = y as usize * b.w + x as usize;
    b.emis[i] = b.emis[i].add(col);
}

/// Opaque fill plus an emissive add, the common case for a lit surface.
fn lit(b: &mut Buf, x: i32, y: i32, base: Rgb, emis: Rgb) {
    put_base(b, x, y, base);
    add_emis(b, x, y, emis);
}

// -------------------------------------------------------------- small text

/// Draw `text` in the 3×5 handset face, top-left at sub-pixel `(x, y)`. `emis`
/// scales how much of the glyph feeds the bloom pass; ticker text passes 0 so it
/// never blooms (SPEC section 8: dim, never bloomed).
fn text(b: &mut Buf, s: &str, x: i32, y: i32, scale: usize, col: Rgb, emis: f32) {
    let sc = scale as i32;
    let mut cx = x;
    for ch in s.chars() {
        let g = smallfont::glyph(ch);
        for (gy, row) in g.iter().enumerate() {
            for gx in 0..3 {
                if row & (1 << (2 - gx)) == 0 {
                    continue;
                }
                for sy in 0..sc {
                    for sx in 0..sc {
                        let px = cx + gx * sc + sx;
                        let py = y + gy as i32 * sc + sy;
                        put_base(b, px, py, col);
                        if emis > 0.0 {
                            add_emis(b, px, py, col.mul(emis));
                        }
                    }
                }
            }
        }
        cx += 4 * sc;
    }
}

fn text_w(s: &str, scale: usize) -> i32 {
    (s.chars().count() * 4 * scale) as i32
}

// ----------------------------------------------------------------- minos

/// One lit glass capsule (SPEC section 9.2): saturated fill, a bright top/left
/// bevel, a dark bottom/right bevel, and a bloom halo into `emis`.
fn draw_mino(b: &mut Buf, x0: i32, y0: i32, size: i32, mino: Mino, settled: bool) {
    let fill = if settled { mino.settled() } else { mino.color() };
    let hi = mino.bevel_hi();
    let lo = mino.bevel_lo();
    for dy in 0..size {
        for dx in 0..size {
            let mut col = fill;
            if dy == 0 || dx == 0 {
                col = hi;
            }
            if dy == size - 1 || dx == size - 1 {
                col = lo;
            }
            put_base(b, x0 + dx, y0 + dy, col);
        }
    }
    let halo = fill.mul(if settled { 0.35 } else { 0.75 });
    for dy in 0..size {
        for dx in 0..size {
            add_emis(b, x0 + dx, y0 + dy, halo);
        }
    }
}

/// Ghost is texture, not a tint (SPEC section 9.2): a dotted outline in the
/// receded hue so it never reads as a solid block over the scrim.
fn draw_ghost(b: &mut Buf, x0: i32, y0: i32, size: i32, mino: Mino) {
    let col = mino.ghost();
    for dy in 0..size {
        for dx in 0..size {
            let edge = dx == 0 || dy == 0 || dx == size - 1 || dy == size - 1;
            if edge && (dx + dy) % 2 == 0 {
                lit(b, x0 + dx, y0 + dy, col, col.mul(0.4));
            }
        }
    }
}

// ----------------------------------------------------------------- 7-seg

/// One 7-segment digit in a `4 × 7` sub-pixel box, lit segments in `hue`, off
/// segments the dark `GRID_TICK` rule (SPEC section 8).
fn seg_digit(b: &mut Buf, d: u8, x0: i32, y0: i32, hue: Rgb) {
    const MASK: [u8; 10] = [
        0b0111111, 0b0000110, 0b1011011, 0b1001111, 0b1100110, 0b1101101, 0b1111101, 0b0000111,
        0b1111111, 0b1101111,
    ];
    let m = MASK[(d % 10) as usize];
    let off = c(GRID_TICK);
    let segs: [(u8, &[(i32, i32)]); 7] = [
        (0, &[(1, 0), (2, 0)]),
        (1, &[(3, 1), (3, 2)]),
        (2, &[(3, 4), (3, 5)]),
        (3, &[(1, 6), (2, 6)]),
        (4, &[(0, 4), (0, 5)]),
        (5, &[(0, 1), (0, 2)]),
        (6, &[(1, 3), (2, 3)]),
    ];
    for (bit, pts) in segs {
        let on = m & (1 << bit) != 0;
        for &(px, py) in pts {
            if on {
                lit(b, x0 + px, y0 + py, hue, hue.mul(0.7));
            } else {
                put_base(b, x0 + px, y0 + py, off);
            }
        }
    }
}

fn seg_number(b: &mut Buf, n: u32, digits: usize, x0: i32, y0: i32, hue: Rgb) {
    let s = format!("{:0width$}", n, width = digits);
    for (i, ch) in s.chars().enumerate() {
        let d = ch.to_digit(10).unwrap_or(0) as u8;
        seg_digit(b, d, x0 + i as i32 * 5, y0, hue);
    }
}

// ------------------------------------------------------------- base scene

/// Sky + slit sun + scrolling grid, welded at `horizon_row`. Reuses the proven
/// [`Scene`] math, only retargeting the horizon (and the grid constants that
/// depend on it) so the same renderer serves the broad idle floor and the
/// tetris-pinned weld. `phase` in `[0,1)` freezes the grid scroll.
pub fn base_scene(l: &Layout, horizon_row: usize, phase: f32) -> Buf {
    base_scene_sun(l, horizon_row, phase, 0.0)
}

/// [`base_scene`] with the sun driven off its resting place — `sun_sink` in
/// `0.0..=1.0` sets the disc from full to swallowed, which is how a wrapped
/// command's progress reaches the sky.
pub fn base_scene_sun(l: &Layout, horizon_row: usize, phase: f32, sun_sink: f32) -> Buf {
    let mut sc = Scene::new(l.w, l.h);
    let sh = (l.h * 2) as f32;
    sc.yh = 2.0 * horizon_row as f32;
    sc.sun_sink = sun_sink;
    let below = sh - sc.yh;
    sc.k = below * 0.82;
    sc.slope = (l.w as f32 * 0.5) / (below * l.lanes as f32);

    let mut buf = Buf::new(l.w, l.h);
    let opts = Opts {
        title: None,
        ..Default::default()
    };
    sc.render(&mut buf, phase / opts.speed, &opts);
    buf
}

// ------------------------------------------------------------- the well

struct Well {
    l: Layout,
    shake: i32, // whole-cell vertical displacement, sub-rows (Tetris shake)
}

impl Well {
    fn cell_origin(&self, mc: i32, mr: i32) -> (i32, i32) {
        let p = self.l.mino_px as i32;
        let x0 = self.l.well.x0 as i32 + mc * p;
        let y0 = 2 * self.l.well.y0 as i32 + mr * p - self.shake;
        (x0, y0)
    }

    /// Smoked scrim over the well sub-region, then the dark `GRID_TICK`
    /// left-edge rule per empty cell — the horizon graph-paper stood upright.
    fn scrim(&self, b: &mut Buf, filled: &dyn Fn(i32, i32) -> bool) {
        let smoke = c(WELL_SMOKE);
        let tick = c(GRID_TICK);
        let p = self.l.mino_px as i32;
        let (x0, x1) = (self.l.well.x0 as i32, self.l.well.x1 as i32);
        let ytop = 2 * self.l.well.y0 as i32 - self.shake;
        let ybot = 2 * (self.l.well.y1 as i32 + 1) - self.shake;
        for y in ytop..ybot {
            for x in x0..=x1 {
                put_base(b, x, y, smoke);
            }
        }
        for mr in 0..20 {
            for mc in 0..10 {
                if filled(mc, mr) {
                    continue;
                }
                let (cx, cy) = self.cell_origin(mc, mr);
                for dy in 0..p {
                    put_base(b, cx, cy + dy, tick);
                }
            }
        }
    }
}

// ------------------------------------------------------- selector & ticker

/// The selector word in chrome on the weld, flanked by guillemets, with the
/// five-dot position strip one rung below.
pub fn selector(b: &mut Buf, l: &Layout, weld_row: usize, word: &str, lit_dot: usize) {
    let cx = l.selector_center_col as f32;
    // Below the weld, standing on the grid. Above it is where the sun sits, and
    // chrome lettering on the disc is unreadable — the word's own bright band
    // and the sun's are the same colour.
    let base = 2.0 * weld_row as f32 + 8.0;
    chrome_word(b, &format!("‹ {} ›", word), 1, cx, base);

    let cyan = c(HORIZON_CYAN);
    let dim = c(GRID_TICK);
    let row = base as i32 + 8;
    let step = (l.mino_px as i32 * 2).max(4);
    for i in 0..5usize {
        let dx = l.selector_center_col as i32 + (i as i32 - 2) * step;
        let on = i == lit_dot;
        let col = if on { cyan } else { dim };
        for oy in 0..2 {
            for ox in 0..2 {
                if on {
                    lit(b, dx + ox, row + oy, col, col.mul(0.8));
                } else {
                    put_base(b, dx + ox, row + oy, col);
                }
            }
        }
    }
}

/// The big chrome wordmark high in the sky, opaque over the sun crown — the
/// signature synthwave title. Only drawn where the sky is tall enough to hold a
/// scale-3 face without crowding the weld; on short terminals the selector word
/// carries the hero alone.
pub fn title_mark(b: &mut Buf, l: &Layout, word: &str) {
    // Scale down before giving up: a long word at scale 3 would run off a
    // narrow frame, and no wordmark at all is worse than a smaller one.
    let scale = match (l.h, l.w) {
        (h, _) if h < 26 => return,
        (h, w) if h >= 40 && w >= chrome_word_w(word, 3) + 8 => 3,
        (_, w) if w >= chrome_word_w(word, 2) + 6 => 2,
        _ => 1,
    };
    let title_half = (7 * scale) as f32 * 0.5; // 5x7 chrome face
    chrome_word(b, word, scale, l.w as f32 * 0.5, title_half + 3.0);
}

/// Row H-1: sanitized command tail (`left`, dim, no bloom); timecode plus fuel
/// gauge (`right`) and the breathing state-LED. Callers pass the strings so the
/// same ticker serves the attract loop and a live run.
pub fn ticker(b: &mut Buf, l: &Layout, left: &str, right: &str) {
    let row = 2 * l.ticker_row as i32;
    let dim = c(TICKER_DIM);
    let budget = (((l.w as i32 - 16) / 4).max(0)) as usize;
    let s: String = left.chars().take(budget).collect();
    text(b, &s, 2, row, 1, dim, 0.0);

    let rx = l.w as i32 - text_w(right, 1) - 6;
    text(b, right, rx, row, 1, dim, 0.0);
    let led = c(HORIZON_CYAN);
    let lx = l.w as i32 - 3;
    for oy in 0..2 {
        for ox in 0..2 {
            lit(b, lx + ox, row + oy, led, led.mul(0.9));
        }
    }
}

// --------------------------------------------------------------- signs

fn hold_sign(b: &mut Buf, l: &Layout, hold: Option<Mino>) {
    let Some((hx, hy)) = l.hold else { return };
    let x = hx as i32;
    let y = 2 * hy as i32;
    text(b, "HOLD", x, y, 1, c(CHROME_STEEL), 0.0);
    if let Some(m) = hold {
        draw_mino(b, x, y + 7, l.mino_px as i32, m, false);
    }
}

fn next_sign(b: &mut Buf, l: &Layout, next: &[Mino]) {
    let (nx, ny) = l.next;
    let x = nx as i32;
    let y = 2 * ny as i32;
    text(b, "NEXT", x, y, 1, c(CHROME_STEEL), 0.0);
    let p = l.mino_px as i32;
    let deep = l.next_deep.min(5);
    for (i, &m) in next.iter().take(deep).enumerate() {
        draw_mino(b, x, y + 7 + i as i32 * (p + 2), p, m, false);
    }
}

fn score_signs(b: &mut Buf, l: &Layout, score: u32, lines: u32, level: u32) {
    let (sx, sy) = l.score;
    let x = sx as i32;
    let y = 2 * sy as i32;
    text(b, "SCORE", x, y, 1, c(CHROME_STEEL), 0.0);
    seg_number(b, score, 6, x, y + 7, c(SUN_GOLD));

    if l.compact_stats {
        // narrow flank: fold LINES/LVL onto one small-text row, no 7-seg stacks
        let (lx, ly) = l.lines;
        text(b, &format!("LN {lines}"), lx as i32, 2 * ly as i32, 1, c(HORIZON_CYAN), 0.0);
        text(b, &format!("LV {level}"), lx as i32, 2 * ly as i32 + 6, 1, c(SUN_MAGENTA), 0.0);
        return;
    }

    let (lx, ly) = l.lines;
    text(b, "LINES", lx as i32, 2 * ly as i32, 1, c(CHROME_STEEL), 0.0);
    seg_number(b, lines, 3, lx as i32, 2 * ly as i32 + 7, c(HORIZON_CYAN));

    let (vx, vy) = l.level;
    text(b, "LVL", vx as i32, 2 * vy as i32, 1, c(CHROME_STEEL), 0.0);
    seg_number(b, level, 2, vx as i32, 2 * vy as i32 + 7, c(SUN_MAGENTA));
}

/// Tetris flare: rising sparks off the cleared band and an ignited horizon weld
/// (the grid shock-ring). Bounded to grid + well per SPEC section 10.
fn flare_overlay(b: &mut Buf, l: &Layout) {
    let white = c(CHROME_HI);
    let p = l.mino_px as i32;
    for mc in 0..10 {
        let x = l.well.x0 as i32 + mc * p + p / 2;
        let base_y = 2 * l.well.y0 as i32 + 15 * p;
        for k in 0..6 {
            let y = base_y - k * p - (mc % 3) * p;
            let fade = 1.0 - k as f32 / 6.0;
            add_emis(b, x, y, white.mul(fade * 0.8));
        }
    }
    let weld = 2 * l.horizon_row as i32;
    let ring = c(GRID_HOT);
    for x in 0..l.w as i32 {
        add_emis(b, x, weld, ring.mul(0.5));
        add_emis(b, x, weld + 1, ring.mul(0.3));
    }
}

// ------------------------------------------------------------- the frame

/// Paint a full Diorama frame — sky, sun, grid, well, minos, signs, ticker —
/// into `b`, pre-bloom. The caller runs bloom / scanlines / encode afterward.
pub fn paint(b: &mut Buf, st: &DioramaState) {
    let l = st.layout;
    *b = base_scene_sun(l, l.horizon_row, st.heat, st.sun_sink);

    let well = Well {
        l: l.clone(),
        shake: st.shake,
    };
    well.scrim(b, &|mc, mr| st.well_cells[mr as usize][mc as usize].is_some());

    let p = l.mino_px as i32;
    for mr in 0..20i32 {
        for mc in 0..10i32 {
            let Some(m) = st.well_cells[mr as usize][mc as usize] else {
                continue;
            };
            let (x, y) = well.cell_origin(mc, mr);
            if st.clearing.contains(&(mr as usize)) {
                let white = c(CHROME_HI);
                for dy in 0..p {
                    for dx in 0..p {
                        lit(b, x + dx, y + dy, white, white.mul(0.9));
                    }
                }
                continue;
            }
            draw_mino(b, x, y, p, m, true);
        }
    }

    if let Some(pv) = &st.active {
        for &(mc, mr) in pv.cells {
            let (x, y) = well.cell_origin(mc, mr);
            draw_mino(b, x, y, p, pv.mino, false);
        }
        if let Some(ghost) = st.ghost {
            for &(mc, mr) in ghost {
                let (x, y) = well.cell_origin(mc, mr);
                draw_ghost(b, x, y, p, pv.mino);
            }
        }
    }

    if !st.clearing.is_empty() {
        flare_overlay(b, l);
    }

    hold_sign(b, l, st.hold);
    next_sign(b, l, st.next);
    score_signs(b, l, st.score, st.lines, st.level);
    ticker(b, l, st.ticker_left, st.ticker_elapsed);
}
