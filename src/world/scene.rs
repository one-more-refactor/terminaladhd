//! The picture surface: a half-block sub-pixel buffer, the palette everything
//! on screen is drawn from, the chrome face, and the two CRT passes.
//!
//! There is no scenery here any more. A cabinet screen is black, and the only
//! light on it comes from something that is alive — so the ground is a constant
//! and every other colour is a hot hue that belongs to a piece, a wall or a
//! number.

use super::color::*;
use super::font;

/// Every colour on screen. A hex literal never reaches a draw call.
pub mod palette {
    /// The ground. Not pure black: a trace of blue gives the bloom somewhere to
    /// sit, and stops a still frame reading as a monitor that is switched off.
    pub const VOID: u32 = 0x03020A;
    /// Inert structure — empty cells, dead segments, a frame at rest.
    pub const IRON: u32 = 0x16233F;
    /// Labels and anything that is text rather than a reading.
    pub const STEEL: u32 = 0x6E8CC8;

    // The hot hues. Everything lit is one of these or WHITE, which is reserved
    // for impact — if white appears, something just happened.
    pub const CYAN: u32 = 0x00F0FF;
    pub const MAGENTA: u32 = 0xFF23C8;
    pub const YELLOW: u32 = 0xFFE100;
    pub const GREEN: u32 = 0x00FF87;
    pub const ORANGE: u32 = 0xFF7A00;
    pub const RED: u32 = 0xFF2D55;
    pub const BLUE: u32 = 0x2A6BFF;
    pub const VIOLET: u32 = 0xB84BFF;
    pub const WHITE: u32 = 0xFFFFFF;
    /// A cool white rail. Not one of the hot hues: it is what an edge is made
    /// of when the edge is not trying to mean anything.
    pub const RAIL: u32 = 0xBFD8FF;
}

use palette::*;

/// The scene is rasterised at half-block resolution: `w` columns by `2*h`
/// sub-rows. A terminal cell is ~1:2, so a half-block sub-pixel is ~square and
/// circles need no aspect correction anywhere in this file.
pub struct Buf {
    pub w: usize,
    pub sh: usize, // sub-rows == 2 * cell rows
    pub base: Vec<Rgb>,
    pub emis: Vec<Rgb>,
}

impl Buf {
    pub fn new(w: usize, h: usize) -> Self {
        Buf {
            w,
            sh: h * 2,
            base: vec![Rgb::ZERO; w * h * 2],
            emis: vec![Rgb::ZERO; w * h * 2],
        }
    }
    pub fn clear(&mut self) {
        self.base.iter_mut().for_each(|p| *p = Rgb::ZERO);
        self.emis.iter_mut().for_each(|p| *p = Rgb::ZERO);
    }
}

// ------------------------------------------------------------------ gradient

pub struct Ramp {
    stops: Vec<(f32, Rgb)>,
}

impl Ramp {
    pub fn new(stops: &[(f32, u32)]) -> Self {
        Ramp {
            stops: stops.iter().map(|&(t, v)| (t, hex(v))).collect(),
        }
    }
    pub fn at(&self, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let s = &self.stops;
        for i in 0..s.len() - 1 {
            if t <= s[i + 1].0 {
                let span = s[i + 1].0 - s[i].0;
                let k = if span <= 0.0 {
                    0.0
                } else {
                    (t - s[i].0) / span
                };
                // smoothstep between stops: removes the visible crease a plain
                // lerp leaves at every stop when only ~30 rows sample the ramp
                let k = k * k * (3.0 - 2.0 * k);
                return s[i].1.lerp(s[i + 1].1, k);
            }
        }
        s[s.len() - 1].1
    }
}

#[inline]
pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if b <= a {
        return if x < a { 0.0 } else { 1.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ------------------------------------------------------------------- bloom

/// Separable box blur run twice ~= Gaussian, on the emissive buffer only.
/// Horizontal radius is doubled because a sub-pixel is square but the *glow*
/// should look isotropic in cell terms; in practice 2:1 matches the eye.
pub fn bloom(b: &mut Buf, rx: usize, ry: usize, gain: f32, out: &mut Vec<Rgb>) {
    let w = b.w;
    let sh = b.sh;
    let mut tmp = vec![Rgb::ZERO; w * sh];
    let mut cur = b.emis.clone();

    for _ in 0..2 {
        // horizontal, sliding window
        for y in 0..sh {
            let row = y * w;
            let n = (2 * rx + 1) as f32;
            let mut acc = Rgb::ZERO;
            for x in 0..=rx.min(w - 1) {
                acc = acc.add(cur[row + x]);
            }
            for x in 0..w {
                tmp[row + x] = acc.mul(1.0 / n);
                let add = x + rx + 1;
                if add < w {
                    acc = acc.add(cur[row + add]);
                }
                if x >= rx {
                    acc = acc.add(cur[row + x - rx].mul(-1.0));
                }
            }
        }
        // vertical
        for x in 0..w {
            let n = (2 * ry + 1) as f32;
            let mut acc = Rgb::ZERO;
            for y in 0..=ry.min(sh - 1) {
                acc = acc.add(tmp[y * w + x]);
            }
            for y in 0..sh {
                cur[y * w + x] = acc.mul(1.0 / n);
                let add = y + ry + 1;
                if add < sh {
                    acc = acc.add(tmp[add * w + x]);
                }
                if y >= ry {
                    acc = acc.add(tmp[(y - ry) * w + x].mul(-1.0));
                }
            }
        }
    }

    out.clear();
    out.reserve(w * sh);
    for ((base, emis), blurred) in b.base.iter().zip(&b.emis).zip(&cur) {
        out.push(base.add(*emis).add(blurred.mul(gain)));
    }
}

pub fn resolve_no_bloom(b: &Buf, out: &mut Vec<Rgb>) {
    out.clear();
    for i in 0..b.w * b.sh {
        out.push(b.base[i].add(b.emis[i]));
    }
}

/// The steel-to-magenta band the chrome face samples per glyph-row — the swept
/// metal every arcade wordmark of the period was airbrushed in.
pub fn chrome_ramp() -> Ramp {
    Ramp::new(&[
        (0.00, 0x081833),
        (0.26, STEEL),
        (0.42, 0xDCEEFF),
        (0.50, WHITE),
        (0.58, 0xBFF4FF),
        (0.68, CYAN),
        (0.82, 0x123A78),
        (1.00, MAGENTA),
    ])
}

/// Width in sub-pixel columns that [`chrome_word`] would occupy — what a caller
/// needs to choose a scale that fits before it draws.
pub fn chrome_word_w(text: &str, scale: usize) -> usize {
    let n = text.chars().count();
    if n == 0 {
        return 0;
    }
    n * (font::W + 1) * scale - scale
}

/// Draw `text` in the chrome face centred on sub-pixel point `(cx_px, cy_sub)`.
/// Opaque: replaces `base`, and only the bright bands feed `emis` so the
/// lettering blooms without the dark serifs smearing.
pub fn chrome_word(b: &mut Buf, text: &str, scale: usize, cx_px: f32, cy_sub: f32) {
    let ramp = chrome_ramp();
    let gw = (font::W + 1) * scale;
    let n = text.chars().count();
    if n == 0 {
        return;
    }
    let total = n * gw - scale;
    let x0 = (cx_px - total as f32 * 0.5).round() as i32;
    let gh = (font::H * scale) as f32;
    let y0 = (cy_sub - gh * 0.5).round() as i32;

    for (ci, ch) in text.chars().enumerate() {
        let g = font::glyph(ch);
        for (gy, row) in g.iter().enumerate().take(font::H) {
            for gx in 0..font::W {
                if row & (1 << (font::W - 1 - gx)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    let py = y0 + (gy * scale + sy) as i32;
                    if py < 0 || py as usize >= b.sh {
                        continue;
                    }
                    let t = (gy * scale + sy) as f32 / (gh - 1.0);
                    // Silkscreened rather than airbrushed: the same sweep, in
                    // six flat bands with hard edges. The gleam line survives
                    // as its own band, which is all a print run would have
                    // given it.
                    let t = ((t * 6.0).floor() + 0.5) / 6.0;
                    let c = ramp.at(t);
                    let lum = c.max_c();
                    for sx in 0..scale {
                        let px = x0 + (ci * gw + gx * scale + sx) as i32;
                        if px < 0 || px as usize >= b.w {
                            continue;
                        }
                        let i = py as usize * b.w + px as usize;
                        b.base[i] = c;
                        // Only the brightest bands of the sweep bloom, and
                        // gently: a chrome word is read by its shape, and a
                        // glow wide enough to close its counters is a blob.
                        b.emis[i] = if lum > 0.55 {
                            c.mul((lum - 0.55) * 0.55)
                        } else {
                            Rgb::ZERO
                        };
                    }
                }
            }
        }
    }
}

/// CRT scanline: attenuate the lower sub-row of every cell. Because a cell is
/// exactly two sub-rows, the scanline lands on a real pixel boundary and costs
/// one multiply -- no resampling, no shimmer when content scrolls.
pub fn scanlines(px: &mut [Rgb], w: usize, sh: usize, k: f32) {
    for y in (1..sh).step_by(2) {
        for x in 0..w {
            px[y * w + x] = px[y * w + x].mul(k);
        }
    }
}

/// Snap every channel to a fixed ladder of levels. This is the single pass
/// that makes the picture read as an old machine's rather than an airbrush's:
/// a bloom halo becomes concentric flat rings, a fade becomes steps, and
/// nothing on screen can be a colour the palette does not have. Run after
/// bloom and before the scanlines, so the raster texture stays a texture
/// instead of being quantised into bands of its own.
pub fn posterize(px: &mut [Rgb], levels: f32) {
    let snap = |v: f32| (v * levels).round() / levels;
    for p in px {
        p.r = snap(p.r);
        p.g = snap(p.g);
        p.b = snap(p.b);
    }
}
