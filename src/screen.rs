//! The picture: 80×48 one-bit pixels behind the glass of an arcade monitor.
//!
//! Every screen the machine has is drawn onto the same fixed canvas — lit
//! phosphor or dark glass, nothing else — and the [`Monitor`] blows it up to
//! whatever the terminal is, in whole pixels, centred on black. Eighty by
//! forty-eight because a half-block cell carries two pixels: the picture is
//! exactly an 80×24 terminal at scale one, and every bigger window only makes
//! the pixels larger.
//!
//! The colour lives in one place, the [`Phosphor`]: a lit tone and the tint it
//! leaves in the glass. Games own their tone, heat pushes it towards gold, a
//! hit blows it out white, and that single channel is the whole mood system —
//! a screen with one voice can afford to raise it.
//!
//! What makes it a monitor rather than a bitmap is applied on the way out:
//! unlit pixels next to lit ones pick up a faint halo of the phosphor,
//! alternate rows dim a shade, a hit shears the rows against each other, and a
//! screen change collapses the raster to a line and then to a dot, the way a
//! tube loses its picture when the power goes.

use crate::font;

/// Logical picture size, in pixels. Fixed: nothing in the machine ever asks
/// the terminal how big to be, only how big to draw.
pub const W: usize = 80;
pub const H: usize = 48;

/// The most the picture is displaced by a hit, in terminal cells. Rows shear
/// in opposite directions, so even one cell reads as the chassis being struck.
const MAX_SHAKE: i32 = 2;

/// How much of the lit tone an adjacent unlit pixel picks up. High enough to
/// read as glow, low enough that text stays text.
const HALO: f32 = 0.22;

/// How much of the lit tone survives on alternate terminal rows. On dark
/// glass the scanline dims the phosphor itself — dimming the glass would be
/// invisible.
const SCAN: f32 = 0.80;

// ------------------------------------------------------------------ picture

/// The one-bit canvas every screen is drawn on.
pub struct Screen {
    px: [bool; W * H],
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen {
    pub fn new() -> Screen {
        Screen { px: [false; W * H] }
    }

    pub fn clear(&mut self) {
        self.px.fill(false);
    }

    /// Set one pixel; drawing off the canvas is silently clipped.
    pub fn set(&mut self, x: i32, y: i32, on: bool) {
        if x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H {
            self.px[y as usize * W + x as usize] = on;
        }
    }

    pub fn get(&self, x: i32, y: i32) -> bool {
        if x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H {
            self.px[y as usize * W + x as usize]
        } else {
            false
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, on: bool) {
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                self.set(x + dx, y + dy, on);
            }
        }
    }

    /// 1px outline.
    pub fn rect(&mut self, x: i32, y: i32, w: u32, h: u32) {
        self.hline(x, y, w);
        self.hline(x, y + h as i32 - 1, w);
        self.vline(x, y, h);
        self.vline(x + w as i32 - 1, y, h);
    }

    pub fn hline(&mut self, x: i32, y: i32, len: u32) {
        for dx in 0..len as i32 {
            self.set(x + dx, y, true);
        }
    }

    pub fn vline(&mut self, x: i32, y: i32, len: u32) {
        for dy in 0..len as i32 {
            self.set(x, y + dy, true);
        }
    }

    /// Flip every pixel in the rect — the loudest thing a one-bit field can do.
    pub fn invert_rect(&mut self, x: i32, y: i32, w: u32, h: u32) {
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                let on = self.get(x + dx, y + dy);
                self.set(x + dx, y + dy, !on);
            }
        }
    }

    /// Hand-pixeled art: one `&str` per row, `#` is ink, anything else is
    /// transparent.
    pub fn sprite(&mut self, x: i32, y: i32, rows: &[&str]) {
        for (dy, row) in rows.iter().enumerate() {
            for (dx, ch) in row.chars().enumerate() {
                if ch == '#' {
                    self.set(x + dx as i32, y + dy as i32, true);
                }
            }
        }
    }

    /// 3×5 pixel text; lowercase is uppercased.
    pub fn text(&mut self, x: i32, y: i32, s: &str) {
        self.text_with(x, y, s, 1, true);
    }

    /// Text at an integer scale (2 = headline).
    pub fn text_scaled(&mut self, x: i32, y: i32, s: &str, scale: u32) {
        self.text_with(x, y, s, scale, true);
    }

    /// Text drawing *unlit* pixels — labels on inverted ground.
    pub fn text_off(&mut self, x: i32, y: i32, s: &str) {
        self.text_with(x, y, s, 1, false);
    }

    fn text_with(&mut self, x: i32, y: i32, s: &str, scale: u32, on: bool) {
        let scale = scale.max(1) as i32;
        let mut cx = x;
        for ch in s.chars() {
            let glyph = font::glyph(ch);
            for (gy, row) in glyph.iter().enumerate() {
                for gx in 0..3 {
                    if row & (0b100 >> gx) != 0 {
                        self.fill_rect(
                            cx + gx * scale,
                            y + gy as i32 * scale,
                            scale as u32,
                            scale as u32,
                            on,
                        );
                    }
                }
            }
            cx += 4 * scale;
        }
    }
}

/// Pixel width of a string in the 3×5 face at scale 1, for centring.
pub fn text_width(s: &str) -> i32 {
    let n = s.chars().count() as i32;
    if n == 0 {
        0
    } else {
        4 * n - 1
    }
}

// ----------------------------------------------------------------- phosphor

/// The tube's one voice: the tone lit pixels glow with, and the tint that
/// tone leaves in the dark glass around them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Phosphor {
    pub lit: [u8; 3],
    pub dark: [u8; 3],
}

impl Phosphor {
    /// Acid lime — snake.
    pub const LIME: Self = Self {
        lit: [150, 255, 80],
        dark: [6, 12, 4],
    };
    /// Ice — blocks.
    pub const ICE: Self = Self {
        lit: [120, 220, 255],
        dark: [4, 10, 14],
    };
    /// Jackpot gold — heat, records.
    pub const GOLD: Self = Self {
        lit: [255, 214, 92],
        dark: [14, 11, 4],
    };
    /// Alarm red — death.
    pub const ALARM: Self = Self {
        lit: [255, 92, 70],
        dark: [14, 5, 4],
    };

    /// A neon tone on near-black glass. The reel is the one screen with no
    /// game on it to protect, so it gets colours no playfield would survive.
    pub fn neon(index: usize) -> Self {
        Self {
            lit: NEON[index % NEON.len()],
            dark: [10, 7, 14],
        }
    }

    /// Blend towards `other`; `t` is clamped to 0..=1.
    pub fn mix(self, other: Self, t: f32) -> Self {
        Self {
            lit: mix_rgb(self.lit, other.lit, t),
            dark: mix_rgb(self.dark, other.dark, t),
        }
    }

    /// Blow the tube out towards white — a scoring hit, a landing reel.
    pub fn flash(self, t: f32) -> Self {
        self.mix(
            Self {
                lit: [255, 255, 255],
                dark: [235, 235, 235],
            },
            t,
        )
    }
}

/// The reel's neon, in chase order.
const NEON: [[u8; 3]; 6] = [
    [255, 64, 160],
    [255, 96, 32],
    [255, 208, 48],
    [140, 255, 72],
    [64, 232, 255],
    [168, 108, 255],
];

fn mix_rgb(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [lerp(a[0], b[0]), lerp(a[1], b[1]), lerp(a[2], b[2])]
}

// ------------------------------------------------------------------- cells

/// One terminal cell: an upper-half block carrying two stacked pixels, or a
/// space carrying only its background.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub struct Cell {
    pub half: bool,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
}

// ------------------------------------------------------------------ monitor

/// What the tube itself is doing, beyond showing pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fx {
    /// Displacement from a hit, `-1.0..=1.0`, spent in whole cells with
    /// alternate rows thrown opposite ways.
    pub shake: f32,
    /// How far the raster has collapsed, `0.0` open to `1.0` gone: the
    /// picture squeezes to a bright line, then the line to a dot.
    pub cut: f32,
}

/// Turns the picture into terminal cells: integer scale, centred on black,
/// with the glass effects applied on the way through.
pub struct Monitor {
    pub cols: usize,
    pub rows: usize,
    scale: usize,
    /// Per-pixel shade, rebuilt each frame: 0 dark, 1 halo, 2 lit.
    shades: [u8; W * H],
    cells: Vec<Cell>,
}

impl Monitor {
    /// A monitor for this terminal, blown up as far as it will take. Whole
    /// pixels only — a fractional scale stretches some pixels wider than
    /// others and the art stops being pixel art.
    pub fn fit(cols: usize, rows: usize) -> Monitor {
        Monitor {
            cols,
            rows,
            scale: best_scale(cols, rows),
            shades: [0; W * H],
            cells: vec![Cell::default(); cols * rows],
        }
    }

    /// Whether a terminal this size can show the picture at all.
    pub fn fits(cols: usize, rows: usize) -> bool {
        cols >= W && rows >= H / 2
    }

    pub fn scale(&self) -> usize {
        self.scale
    }

    /// Compose one frame: the picture under `ph`, with `fx` applied, on a
    /// black surround. The returned slice is `cols × rows`.
    pub fn compose(&mut self, s: &Screen, ph: Phosphor, fx: Fx, scanlines: bool) -> &[Cell] {
        // The halo is per logical pixel: unlit but next to something lit. One
        // pass here, then every terminal cell is a lookup.
        for y in 0..H as i32 {
            for x in 0..W as i32 {
                let shade = if s.get(x, y) {
                    2
                } else {
                    let lit_near = (-1..=1).any(|dy| {
                        (-1..=1).any(|dx| (dx != 0 || dy != 0) && s.get(x + dx, y + dy))
                    });
                    u8::from(lit_near)
                };
                self.shades[y as usize * W + x as usize] = shade;
            }
        }

        let halo = mix_rgb(ph.dark, ph.lit, HALO);
        let scan_lit = mix_rgb(ph.dark, ph.lit, SCAN);
        let scan_halo = mix_rgb(ph.dark, ph.lit, HALO * SCAN);

        // The collapse: the visible band narrows and what is left burns
        // brighter, because the same beam is being spent on fewer lines. Past
        // the line phase the dot takes over and the width goes too.
        let cut = fx.cut.clamp(0.0, 1.0);
        let vk = (1.0 - cut).max(0.001);
        let hk = if cut > 0.85 {
            (1.0 - (cut - 0.85) / 0.15).max(0.001)
        } else {
            1.0
        };
        let burn = |c: [u8; 3]| mix_rgb(c, [255, 255, 255], cut * 0.8);

        let span_cols = W * self.scale;
        let span_rows = H * self.scale / 2;
        let ox = self.cols.saturating_sub(span_cols) as i32 / 2;
        let oy = self.rows.saturating_sub(span_rows) as i32 / 2;
        let kick = (fx.shake.clamp(-1.0, 1.0) * MAX_SHAKE as f32).round() as i32;

        let black = Cell {
            half: false,
            fg: [0, 0, 0],
            bg: [0, 0, 0],
        };
        self.cells.fill(black);

        let mid_x = W as f32 / 2.0;
        let mid_y = H as f32 / 2.0;
        for cy in 0..self.rows as i32 {
            let py = cy - oy;
            if py < 0 || py >= span_rows as i32 {
                continue;
            }
            // Alternate rows kick opposite ways, so a shake tears the picture
            // rather than politely sliding it.
            let row_kick = if cy % 2 == 0 { kick } else { -kick };
            let lit_row = if scanlines && cy % 2 != 0 {
                (scan_lit, scan_halo)
            } else {
                (ph.lit, halo)
            };
            for cx in 0..self.cols as i32 {
                let px = cx - ox - row_kick;
                if px < 0 || px >= span_cols as i32 {
                    continue;
                }
                let sx = px as usize / self.scale;
                let top_y = (py as usize * 2) / self.scale;
                let bot_y = (py as usize * 2 + 1) / self.scale;
                let top = self.sample(sx, top_y, mid_x, mid_y, vk, hk);
                let bot = self.sample(sx, bot_y, mid_x, mid_y, vk, hk);
                let paint = |shade: Option<u8>| match shade {
                    None => [0, 0, 0],
                    Some(0) => ph.dark,
                    Some(1) => burn(lit_row.1),
                    _ => burn(lit_row.0),
                };
                let (t, b) = (paint(top), paint(bot));
                self.cells[cy as usize * self.cols + cx as usize] = if t == b {
                    Cell {
                        half: false,
                        fg: t,
                        bg: t,
                    }
                } else {
                    Cell {
                        half: true,
                        fg: t,
                        bg: b,
                    }
                };
            }
        }
        &self.cells
    }

    /// The shade at picture position `(x, y)` seen through the collapse
    /// mapping, or `None` where the raster has already left.
    fn sample(&self, x: usize, y: usize, mid_x: f32, mid_y: f32, vk: f32, hk: f32) -> Option<u8> {
        let sy = mid_y + (y as f32 + 0.5 - mid_y) / vk;
        if sy < 0.0 || sy >= H as f32 {
            return None;
        }
        let sx = mid_x + (x as f32 + 0.5 - mid_x) / hk;
        if sx < 0.0 || sx >= W as f32 {
            return None;
        }
        Some(self.shades[sy as usize * W + sx as usize])
    }
}

/// The largest whole-pixel scale this terminal takes: fill it, keep pixels
/// square, never go below 1:1.
fn best_scale(cols: usize, rows: usize) -> usize {
    let by_width = cols / W;
    let by_height = rows * 2 / H;
    by_width.min(by_height).max(1)
}

/// Dump composed cells as a PPM, two stacked pixels per cell — for reviewing
/// a rendering change without a terminal in the way.
pub fn write_ppm(path: &str, cells: &[Cell], cols: usize, rows: usize) -> std::io::Result<()> {
    let mut out = format!("P6\n{} {}\n255\n", cols, rows * 2).into_bytes();
    for row in 0..rows * 2 {
        for col in 0..cols {
            let c = cells[(row / 2) * cols + col];
            let px = if row % 2 == 0 && c.half { c.fg } else { c.bg };
            out.extend_from_slice(&px);
        }
    }
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_clipping() {
        let mut s = Screen::new();
        s.set(0, 0, true);
        s.set(W as i32 - 1, H as i32 - 1, true);
        s.set(-1, 5, true);
        s.set(W as i32, 5, true);
        s.set(5, H as i32, true);
        assert!(s.get(0, 0));
        assert!(s.get(W as i32 - 1, H as i32 - 1));
        assert!(!s.get(-1, 5));
        assert!(!s.get(W as i32, 5));
    }

    #[test]
    fn text_marks_pixels_and_width_matches() {
        let mut s = Screen::new();
        s.text(0, 0, "HI");
        assert!((0..7).any(|x| s.get(x, 0)));
        assert_eq!(text_width("HI"), 7);
        assert_eq!(text_width(""), 0);
    }

    #[test]
    fn the_picture_is_exactly_a_stock_terminal_at_scale_one() {
        // 80×24 is the size every terminal has been since terminals: the
        // machine must fit it with nothing to spare and nothing missing.
        assert!(Monitor::fits(80, 24));
        assert!(!Monitor::fits(79, 24));
        assert!(!Monitor::fits(80, 23));
        assert_eq!(best_scale(80, 24), 1);
    }

    #[test]
    fn scaling_is_whole_pixels_and_fills_what_it_can() {
        assert_eq!(best_scale(159, 100), 1);
        assert_eq!(best_scale(160, 48), 2);
        assert_eq!(best_scale(240, 72), 3);
        // Height-bound windows pick their own limit.
        assert_eq!(best_scale(1000, 47), 1);
        assert_eq!(best_scale(1000, 48), 2);
        for cols in [80, 100, 160, 200, 400] {
            for rows in [24, 30, 48, 80, 200] {
                let scale = best_scale(cols, rows);
                assert!(scale >= 1);
                assert!(W * scale <= cols.max(W));
                assert!(H * scale / 2 <= rows.max(H / 2));
            }
        }
    }

    #[test]
    fn a_lit_pixel_shows_and_the_surround_stays_black() {
        let mut s = Screen::new();
        s.set(0, 0, true);
        let mut m = Monitor::fit(120, 40);
        let ph = Phosphor::LIME;
        let cells = m.compose(&s, ph, Fx::default(), false);
        // At 120×40 the scale is still 1 (40 rows carry 80 pixel-rows at
        // scale 2, which is too many), so 40 spare columns split 20/20 and 16
        // spare rows split 8/8.
        let at = |cx: usize, cy: usize| cells[cy * 120 + cx];
        assert_eq!(at(0, 0).bg, [0, 0, 0], "the surround is black");
        let corner = at(20, 8);
        assert_eq!(corner.fg, ph.lit, "the lit pixel is on top of its cell");
        assert_eq!(corner.bg, mix_rgb(ph.dark, ph.lit, HALO), "haloed below");
    }

    #[test]
    fn the_halo_clings_to_lit_pixels_and_no_further() {
        let mut s = Screen::new();
        s.set(10, 10, true);
        let mut m = Monitor::fit(80, 24);
        let ph = Phosphor::ICE;
        m.compose(&s, ph, Fx::default(), false);
        assert_eq!(m.shades[10 * W + 10], 2);
        assert_eq!(m.shades[10 * W + 11], 1, "next door glows");
        assert_eq!(m.shades[9 * W + 9], 1, "diagonals glow too");
        assert_eq!(m.shades[10 * W + 12], 0, "two away is dark glass");
    }

    #[test]
    fn the_collapse_narrows_the_picture_then_burns_it_out() {
        let mut s = Screen::new();
        s.fill_rect(0, 0, W as u32, H as u32, true);
        let mut m = Monitor::fit(80, 24);
        let ph = Phosphor::LIME;

        let lit_rows = |cells: &[Cell]| {
            (0..24)
                .filter(|cy| (0..80).any(|cx| cells[cy * 80 + cx].fg != [0, 0, 0]))
                .count()
        };
        let full = lit_rows(m.compose(&s, ph, Fx::default(), false));
        assert_eq!(full, 24);
        let half = lit_rows(m.compose(
            &s,
            ph,
            Fx {
                cut: 0.5,
                ..Default::default()
            },
            false,
        ));
        assert!(half <= 13, "half cut should show about half the rows: {half}");
        // Late in the cut what is left is brighter than the phosphor at rest.
        let cells = m.compose(
            &s,
            ph,
            Fx {
                cut: 0.9,
                ..Default::default()
            },
            false,
        );
        let line = (0..24 * 80).filter(|&i| cells[i].fg != [0, 0, 0]).count();
        assert!(line > 0, "the line phase still shows something");
        let bright = cells.iter().find(|c| c.fg != [0, 0, 0]).unwrap();
        assert!(
            bright.fg[0] as u32 + bright.fg[1] as u32 + bright.fg[2] as u32
                > ph.lit[0] as u32 + ph.lit[1] as u32 + ph.lit[2] as u32,
            "the collapsing raster should burn brighter"
        );
    }

    #[test]
    fn a_shake_shears_alternate_rows_against_each_other() {
        let mut s = Screen::new();
        s.vline(40, 0, H as u32);
        let mut m = Monitor::fit(80, 24);
        let ph = Phosphor::LIME;
        let cells: Vec<Cell> = m
            .compose(
                &s,
                ph,
                Fx {
                    shake: 1.0,
                    ..Default::default()
                },
                false,
            )
            .to_vec();
        let lit_x = |cy: usize| (0..80).find(|&cx| cells[cy * 80 + cx].fg == ph.lit);
        let even = lit_x(0).expect("row 0 lost the line");
        let odd = lit_x(1).expect("row 1 lost the line");
        assert_ne!(even, odd, "the rows should tear apart");
        assert_eq!(even.abs_diff(odd), 2 * MAX_SHAKE as usize);
    }

    #[test]
    fn scanlines_dim_alternate_rows_of_phosphor_only() {
        let mut s = Screen::new();
        s.fill_rect(0, 0, W as u32, H as u32, true);
        let mut m = Monitor::fit(80, 24);
        let ph = Phosphor::LIME;
        let cells = m.compose(&s, ph, Fx::default(), true);
        assert_eq!(cells[0].fg, ph.lit, "even rows at full brightness");
        assert_eq!(cells[80].fg, mix_rgb(ph.dark, ph.lit, SCAN), "odd rows dimmed");
    }

    #[test]
    fn phosphor_mix_and_flash_stay_in_range() {
        let warm = Phosphor::LIME.mix(Phosphor::GOLD, 0.5);
        assert_ne!(warm, Phosphor::LIME);
        assert_ne!(warm, Phosphor::GOLD);
        let blown = Phosphor::ICE.flash(1.0);
        assert_eq!(blown.lit, [255, 255, 255]);
        // Overshoot clamps rather than wrapping.
        assert_eq!(Phosphor::LIME.mix(Phosphor::GOLD, 7.0), Phosphor::GOLD);
    }
}
