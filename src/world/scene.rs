use super::color::*;
use super::font;

// SPEC section 3 palette. Every scenery colour is a named constant carrying the
// authored 8-bit sRGB hex; the ramps below reference these so a hex literal
// never reaches a draw call. Sky and sun interpolate in linear light with
// smoothstep and dither with a deterministic Bayer offset, which is what keeps
// static cells byte-identical frame to frame.
pub mod palette {
    // Sky, frame top (t=0) -> horizon (t=1).
    pub const SKY_ZENITH: u32 = 0x0B0221;
    pub const SKY_VIOLET: u32 = 0x2B0B52;
    pub const SKY_PURPLE: u32 = 0x6D1580;
    pub const SKY_MAGENTA: u32 = 0xC42D8E;
    pub const SKY_HOTPINK: u32 = 0xFF4D9E;
    pub const SKY_HAZE: u32 = 0x8E5BC7;
    pub const HORIZON_CYAN: u32 = 0x2DE2E6;

    // Sun, crown (t=0) -> waterline (t=1).
    pub const SUN_CROWN: u32 = 0xFFF06B;
    pub const SUN_GOLD: u32 = 0xFFD319;
    pub const SUN_ORANGE: u32 = 0xFF901F;
    pub const SUN_MAGENTA: u32 = 0xFF2975;
    pub const SUN_WATERLINE: u32 = 0xF222FF;

    // Grid, chrome, wells, status.
    pub const GRID_NEAR: u32 = 0xFF2FD0;
    pub const GRID_FAR: u32 = 0x00E5FF;
    pub const GRID_HOT: u32 = 0xF222FF;
    pub const WELL_SMOKE: u32 = 0x0B0417;
    pub const GRID_TICK: u32 = 0x16323A;
    pub const CHROME_HI: u32 = 0xFFFFFF;
    pub const CHROME_STEEL: u32 = 0x5C7FBF;
    pub const STATUS_OK: u32 = 0x35FFC7;
    pub const STATUS_FAIL: u32 = 0xFF2E5B;
    pub const TICKER_DIM: u32 = 0x7FB8C4;

    // SPEC section 3.2 tetromino hues, neon on near-black, each ≥24° apart.
    pub const MINO_I: u32 = 0x00E5FF;
    pub const MINO_T: u32 = 0xC13BFF;
    pub const MINO_O: u32 = 0xFFD400;
    pub const MINO_S: u32 = 0x2BFF9E;
    pub const MINO_Z: u32 = 0xFF2E5B;
    pub const MINO_J: u32 = 0x3C6BFF;
    pub const MINO_L: u32 = 0xFF7A18;
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
    #[inline]
    fn add_emis(&mut self, x: usize, y: usize, c: Rgb) {
        let i = y * self.w + x;
        self.emis[i] = self.emis[i].add(c);
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
                let k = if span <= 0.0 { 0.0 } else { (t - s[i].0) / span };
                // smoothstep between stops: removes the visible crease a plain
                // lerp leaves at every stop when only ~30 rows sample the ramp
                let k = k * k * (3.0 - 2.0 * k);
                return s[i].1.lerp(s[i + 1].1, k);
            }
        }
        s[s.len() - 1].1
    }
}

/// Sky, top of frame to horizon. Authored as 8-bit sRGB, interpolated linear.
pub fn sky_ramp() -> Ramp {
    Ramp::new(&[
        (0.00, SKY_ZENITH),
        (0.30, SKY_VIOLET),
        (0.52, SKY_PURPLE),
        (0.70, SKY_MAGENTA),
        (0.85, SKY_HOTPINK),
        (0.95, SKY_HAZE),
        (1.00, HORIZON_CYAN),
    ])
}

/// The Outrun sun, top of disc to bottom.
pub fn sun_ramp() -> Ramp {
    Ramp::new(&[
        (0.00, SUN_CROWN),
        (0.22, SUN_GOLD),
        (0.48, SUN_ORANGE),
        (0.74, SUN_MAGENTA),
        (1.00, SUN_WATERLINE),
    ])
}

pub fn ground_ramp() -> Ramp {
    Ramp::new(&[
        (0.00, 0x2A0A47), // just under the horizon
        (0.45, 0x180330),
        (1.00, 0x0A0118), // near the viewer
    ])
}

// ------------------------------------------------------------------ geometry

pub struct Scene {
    pub w: usize,
    pub h: usize,
    pub yh: f32,    // horizon, in sub-rows
    pub xc: f32,    // vanishing point column
    pub k: f32,     // f*h_cam/S -- sets how fast horizontal lines bunch up
    pub slope: f32, // W_world/h_cam -- column spread per sub-row of depth
    pub sun_r: f32,
    /// How far the disc has slipped below its resting place, in units of its
    /// own radius. 0 is the attract sun; 1 has it fully swallowed by the
    /// horizon. The wrapper drives this with a command's progress.
    pub sun_sink: f32,
    pub sky: Ramp,
    pub sun: Ramp,
    pub ground: Ramp,
}

impl Scene {
    pub fn new(w: usize, h: usize) -> Self {
        let sh = (h * 2) as f32;
        // Horizon high enough that the grid gets room to open out, low enough
        // that the sky keeps its gradient. 46% reads well from 24 to 62 rows.
        let yh = (sh * 0.46).round();
        let below = sh - yh;
        // Fill the half-width at the bottom edge: slope*below >= w/2.
        // Solve for the number of lanes that lands nearest a target lane count.
        let lanes = ((w as f32 / 26.0).round() as usize).clamp(5, 22) as f32;
        let slope = (w as f32 * 0.5) / (below * lanes);
        Scene {
            w,
            h,
            yh,
            xc: w as f32 * 0.5,
            k: below * 0.82,
            slope,
            // leave headroom above the disc for a title; past ~0.28 of the
            // frame the sun crowds the top of the sky and there is nowhere left
            // to put lettering that is not on top of it
            sun_r: (sh * 0.26).min(w as f32 * 0.16),
            sun_sink: 0.0,
            sky: sky_ramp(),
            sun: sun_ramp(),
            ground: ground_ramp(),
        }
    }

    // -------------------------------------------------------------- sky/ground

    fn draw_sky(&self, b: &mut Buf, dither: bool) {
        for y in 0..b.sh {
            let fy = y as f32;
            if fy >= self.yh {
                let t = (fy - self.yh) / (b.sh as f32 - self.yh);
                let c = self.ground.at(t);
                for x in 0..b.w {
                    b.base[y * b.w + x] = c;
                }
            } else {
                let t = fy / self.yh;
                let c = self.sky.at(t);
                // Dither in the parameter, not the output: nudging t by a
                // fraction of one sub-row makes neighbouring columns straddle
                // the band edge, so the boundary dissolves into 4x4 noise
                // instead of a hard line across the full width.
                if dither {
                    let step = 1.0 / self.yh;
                    for x in 0..b.w {
                        let tt = (t + bayer4(x, y) * step).clamp(0.0, 1.0);
                        b.base[y * b.w + x] = self.sky.at(tt);
                    }
                } else {
                    for x in 0..b.w {
                        b.base[y * b.w + x] = c;
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------- sun

    fn draw_sun(&self, b: &mut Buf) {
        let cx = self.xc;
        // Centre sits slightly above the horizon so a sliver of disc is cut by
        // it -- that clip is what makes it read as a setting sun.
        let cy = self.yh - self.sun_r * 0.10 + self.sun_sink * self.sun_r * 1.20;
        let r = self.sun_r;
        let top = cy - r;
        let x0 = (cx - r - 2.0).max(0.0) as usize;
        let x1 = ((cx + r + 2.0) as usize).min(b.w - 1);
        let y0 = (top - 2.0).max(0.0) as usize;
        let y1 = (self.yh as usize).min(b.sh - 1);

        // Slit phase: period grows linearly with depth d below the disc top,
        // P(d) = p0 + a*d, so cumulative phase is the integral
        //   phi(d) = (1/a) * ln(1 + a*d/p0)
        // Cutting on frac(phi) keeps every slit boundary exact no matter how
        // the period grows -- no accumulated drift, no half-slit at the end.
        // p0 is the period at the disc top and `a` its growth per sub-row. A
        // period under ~1.4 sub-rows cannot be resolved by half-blocks and
        // turns into shimmer, so the floor gives small terminals fewer, coarser
        // slits rather than an aliased comb.
        let p0 = (r * 0.07).max(1.4);
        let a = 0.18;
        let duty_lo = 0.00;
        let duty_hi = 0.58;

        for y in y0..=y1 {
            let fy = y as f32 + 0.5;
            let dy = fy - cy;
            let inside = r * r - dy * dy;
            if inside <= 0.0 {
                continue;
            }
            let half = inside.sqrt();

            let d = fy - top;
            let phi = (1.0 + a * d / p0).ln() / a;
            let f = phi.fract();
            let ramp = ((d / (2.0 * r)) * 1.35).clamp(0.0, 1.0);
            let duty = duty_lo + (duty_hi - duty_lo) * ramp;
            // soft slit edge, in phase units scaled back to sub-rows
            let dphi_dd = 1.0 / (p0 + a * d);
            let edge = dphi_dd * 0.9;
            let cut = smoothstep(duty - edge, duty + edge, f);
            if cut <= 0.001 {
                continue;
            }

            let t = ((fy - top) / (2.0 * r)).clamp(0.0, 1.0);
            let col = self.sun.at(t);

            let lx = (cx - half).max(0.0);
            let rx = (cx + half).min(b.w as f32 - 1.0);
            let li = lx.floor() as usize;
            let ri = rx.ceil() as usize;
            for x in li..=ri.min(b.w - 1) {
                if x < x0 || x > x1 {
                    continue;
                }
                let fx = x as f32 + 0.5;
                // analytic edge AA from the signed distance to the rim
                let dist = half - (fx - cx).abs();
                let cov = dist.clamp(0.0, 1.0);
                let amt = cov * cut;
                if amt <= 0.002 {
                    continue;
                }
                b.add_emis(x, y, col.mul(amt));
            }
        }
    }

    // ------------------------------------------------------------------ grid

    /// `phase` in [0,1) advances the grid toward the viewer. Because the lines
    /// are indexed by an integer k plus this phase, phase wrapping 1 -> 0
    /// relabels line k as line k+1 and the animation is seamless by
    /// construction -- no snap, no accumulating float error.
    fn draw_grid(&self, b: &mut Buf, phase: f32) {
        let cyan = hex(HORIZON_CYAN);
        let mag = hex(SUN_MAGENTA);
        let hw = 0.85; // line half-width, sub-pixels

        for y in (self.yh.ceil() as usize)..b.sh {
            let fy = y as f32 + 0.5;
            let d = fy - self.yh;
            if d <= 0.25 {
                continue;
            }

            // haze: lines fade into the horizon rather than stopping dead
            let haze = smoothstep(0.0, 7.0, d);
            // ---- horizontal lines --------------------------------------
            // q = k + phase = K/d, so screen spacing is |dd/dq| = d^2/K
            let q = self.k / d;
            let u = q - phase;
            let fr = u - u.floor();
            let dq = fr.min(1.0 - fr);
            let dyds = d * d / self.k;
            let sdist = dq * dyds;
            let hline = 1.0 - smoothstep(hw * 0.5, hw * 1.5, sdist);
            // Nyquist: once lines are closer than ~2 sub-rows they alias, so
            // fade them out and let the haze carry the density instead.
            let hfade = smoothstep(1.2, 3.0, dyds);
            let hint = hline * hfade * haze;

            // ---- vertical lines ----------------------------------------
            // Lane m is the screen line through the vanishing point with
            // direction (m*slope, 1). Toward the edges that direction tilts far
            // from vertical, so the horizontal gap to the lane is a bad proxy
            // for its real thickness -- measuring along x alone is what breaks
            // the outer lanes into dashes. Divide by the direction's length to
            // get true perpendicular distance:
            //   perp = |slope*d*(p - m)| / sqrt(1 + (m*slope)^2)
            // and the perpendicular spacing between adjacent lanes falls out of
            // the same normalisation, which is what the Nyquist fade needs.
            let spacing = self.slope * d;
            let inv_sp = 1.0 / spacing;

            for x in 0..b.w {
                let fx = x as f32 + 0.5;
                let p = (fx - self.xc) * inv_sp;
                let m = p.round();
                let ms = m * self.slope;
                let norm = (1.0 + ms * ms).sqrt();
                let vdist = ((p - m) * spacing).abs() / norm;
                let vfade = smoothstep(1.2, 3.5, spacing / norm);
                let vint = (1.0 - smoothstep(hw * 0.5, hw * 1.5, vdist)) * vfade * haze;

                let i = hint.max(vint);
                if i <= 0.004 {
                    continue;
                }
                // colour shifts cyan -> magenta with distance from centre,
                // which is what stops the floor reading as flat wireframe
                let sway = ((fx - self.xc).abs() / (self.w as f32 * 0.5)).clamp(0.0, 1.0);
                let c = cyan.lerp(mag, sway * 0.75);
                b.add_emis(x, y, c.mul(i * 1.15));
            }
        }
    }

    // --------------------------------------------------------------- chrome

    /// Chrome is a vertical band ramp sampled per glyph-row, not per screen-row:
    /// every letter gets the identical highlight sweep, which is what makes it
    /// read as metal rather than as a tinted gradient.
    fn draw_title(&self, b: &mut Buf, text: &str, scale: usize, cy: f32) {
        let ramp = Ramp::new(&[
            (0.00, 0x081833),
            (0.26, CHROME_STEEL),
            (0.42, 0xDCEEFF),
            (0.50, CHROME_HI),
            (0.58, 0xBFF4FF),
            (0.68, HORIZON_CYAN),
            (0.82, 0x123A78),
            (1.00, SUN_MAGENTA),
        ]);
        let gw = 6 * scale; // 5px glyph + 1px gap
        let total = text.len() * gw - scale;
        let x0 = (self.w as f32 * 0.5 - total as f32 * 0.5).round() as i32;
        let gh = (font::H * scale) as f32;
        let y0 = (cy - gh * 0.5).round() as i32;

        for (ci, ch) in text.chars().enumerate() {
            let g = font::glyph(ch);
            for gy in 0..font::H {
                for gx in 0..font::W {
                    if g[gy] & (1 << (font::W - 1 - gx)) == 0 {
                        continue;
                    }
                    for sy in 0..scale {
                        let py = y0 + (gy * scale + sy) as i32;
                        if py < 0 || py as usize >= b.sh {
                            continue;
                        }
                        let t = (gy * scale + sy) as f32 / (gh - 1.0);
                        let c = ramp.at(t);
                        for sx in 0..scale {
                            let px = x0 + (ci * gw + gx * scale + sx) as i32;
                            if px < 0 || px as usize >= b.w {
                                continue;
                            }
                            let i = py as usize * b.w + px as usize;
                            // chrome is opaque: it replaces, and only the
                            // bright bands feed the bloom pass
                            b.base[i] = c;
                            b.emis[i] = Rgb::ZERO;
                            let lum = c.max_c();
                            if lum > 0.45 {
                                b.emis[i] = c.mul((lum - 0.45) * 0.9);
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn render(&self, b: &mut Buf, t: f32, opts: &Opts) {
        b.clear();
        self.draw_sky(b, opts.dither);
        self.draw_sun(b);
        if opts.grid {
            let phase = t * opts.speed;
            self.draw_grid(b, phase - phase.floor());
        }
        if let Some(title) = opts.title {
            // above the disc, not on it: the sun top sits at yh - 1.1r
            let sun_top = self.yh - self.sun_r * 1.10;
            let gh = (font::H * opts.title_scale) as f32;
            let margin = 2.0;
            self.draw_title(
                b,
                title,
                opts.title_scale,
                (sun_top - gh * 0.5 - 2.0).max(gh * 0.5 + margin),
            );
        }
        if let Some(pf) = opts.playfield {
            self.dim_playfield(b, pf);
        }
    }

    /// Reserve a rectangle for the game. The scenery keeps running underneath
    /// but is pushed down to a fraction of its brightness, so the playfield
    /// reads as a lit panel floating over the horizon rather than a hole cut in
    /// it. Cell coords in, sub-pixel rows out.
    fn dim_playfield(&self, b: &mut Buf, (cx0, cy0, cw, ch): (usize, usize, usize, usize)) {
        let (x0, x1) = (cx0, (cx0 + cw).min(b.w));
        let (y0, y1) = (cy0 * 2, ((cy0 + ch) * 2).min(b.sh));
        for y in y0..y1 {
            for x in x0..x1 {
                // feather the border so the panel does not have a hard edge
                let ex = (x - x0).min(x1 - 1 - x) as f32;
                let ey = (y - y0).min(y1 - 1 - y) as f32;
                let e = smoothstep(0.0, 3.0, ex.min(ey * 2.0));
                let k = 1.0 - 0.88 * e;
                let i = y * b.w + x;
                b.base[i] = b.base[i].mul(k);
                b.emis[i] = b.emis[i].mul(k);
            }
        }
    }
}

pub struct Opts<'a> {
    pub dither: bool,
    pub grid: bool,
    pub bloom: bool,
    pub scanlines: bool,
    pub speed: f32,
    pub title: Option<&'a str>,
    pub title_scale: usize,
    pub playfield: Option<(usize, usize, usize, usize)>,
}

impl<'a> Default for Opts<'a> {
    fn default() -> Self {
        Opts {
            dither: true,
            grid: true,
            bloom: true,
            scanlines: true,
            speed: 0.9,
            title: None,
            title_scale: 3,
            playfield: None,
        }
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
    for i in 0..w * sh {
        out.push(b.base[i].add(b.emis[i]).add(cur[i].mul(gain)));
    }
}

pub fn resolve_no_bloom(b: &Buf, out: &mut Vec<Rgb>) {
    out.clear();
    for i in 0..b.w * b.sh {
        out.push(b.base[i].add(b.emis[i]));
    }
}

/// The steel-to-magenta band the chrome wordmark samples per glyph-row. Kept
/// public so the composite frames letter the selector word and the SAVE mark
/// with the identical sweep the attract title uses.
pub fn chrome_ramp() -> Ramp {
    Ramp::new(&[
        (0.00, 0x081833),
        (0.26, CHROME_STEEL),
        (0.42, 0xDCEEFF),
        (0.50, CHROME_HI),
        (0.58, 0xBFF4FF),
        (0.68, HORIZON_CYAN),
        (0.82, 0x123A78),
        (1.00, SUN_MAGENTA),
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
/// Unlike [`Scene::draw_title`] the anchor is explicit, so a caller can stand a
/// word on the horizon weld or anywhere else in the field. Opaque: replaces
/// `base`, and only the bright bands feed `emis` so the lettering blooms without
/// the dark serifs smearing.
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
        for gy in 0..font::H {
            for gx in 0..font::W {
                if g[gy] & (1 << (font::W - 1 - gx)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    let py = y0 + (gy * scale + sy) as i32;
                    if py < 0 || py as usize >= b.sh {
                        continue;
                    }
                    let t = (gy * scale + sy) as f32 / (gh - 1.0);
                    let c = ramp.at(t);
                    let lum = c.max_c();
                    for sx in 0..scale {
                        let px = x0 + (ci * gw + gx * scale + sx) as i32;
                        if px < 0 || px as usize >= b.w {
                            continue;
                        }
                        let i = py as usize * b.w + px as usize;
                        b.base[i] = c;
                        b.emis[i] = if lum > 0.45 {
                            c.mul((lum - 0.45) * 0.9)
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
