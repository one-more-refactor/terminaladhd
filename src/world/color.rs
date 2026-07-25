// Linear-light RGB. All blending, blurring and gradient interpolation happens
// here; sRGB is only ever an encoding at the boundary.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const ZERO: Rgb = Rgb { r: 0.0, g: 0.0, b: 0.0 };

    pub fn new(r: f32, g: f32, b: f32) -> Self {
        Rgb { r, g, b }
    }

    pub fn add(self, o: Rgb) -> Rgb {
        Rgb::new(self.r + o.r, self.g + o.g, self.b + o.b)
    }

    pub fn mul(self, k: f32) -> Rgb {
        Rgb::new(self.r * k, self.g * k, self.b * k)
    }

    pub fn lerp(self, o: Rgb, t: f32) -> Rgb {
        Rgb::new(
            self.r + (o.r - self.r) * t,
            self.g + (o.g - self.g) * t,
            self.b + (o.b - self.b) * t,
        )
    }

    pub fn max_c(self) -> f32 {
        self.r.max(self.g).max(self.b)
    }
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Build a linear-light colour from 8-bit sRGB, the form the stop table is
/// authored in.
pub fn hex(v: u32) -> Rgb {
    let r = ((v >> 16) & 0xff) as f32 / 255.0;
    let g = ((v >> 8) & 0xff) as f32 / 255.0;
    let b = (v & 0xff) as f32 / 255.0;
    Rgb::new(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b))
}

/// Quantise to 8-bit sRGB. `dither` is a signed offset in [-0.5,0.5] LSB,
/// applied post-transfer so it perturbs the *encoded* value by well under one
/// step -- that is what breaks up flat gradient bands.
pub fn to_srgb8(c: Rgb, dither: f32) -> [u8; 3] {
    let f = |v: f32| -> u8 {
        let s = linear_to_srgb(v.max(0.0).min(1.0)) * 255.0 + dither;
        s.round().max(0.0).min(255.0) as u8
    };
    [f(c.r), f(c.g), f(c.b)]
}

/// 4x4 Bayer, returned centred on zero in units of one 8-bit step.
pub fn bayer4(x: usize, y: usize) -> f32 {
    const M: [[u8; 4]; 4] = [
        [0, 8, 2, 10],
        [12, 4, 14, 6],
        [3, 11, 1, 9],
        [15, 7, 13, 5],
    ];
    (M[y & 3][x & 3] as f32 + 0.5) / 16.0 - 0.5
}

// ---------------------------------------------------------------- quantisers

/// xterm-256: 6x6x6 colour cube (16..231) plus the 24-step grey ramp
/// (232..255). Matching happens in linear light, so mid greys do not go muddy.
pub fn to_256(c: Rgb) -> u8 {
    to_256_d(c, 0.0)
}

/// `dither` is a Bayer offset in [-0.5,0.5]; scaled to ~half the cube step it
/// spreads a colour that sits between two cube levels across both, which is the
/// difference between visible 40-unit banding and a smooth ramp in 256 mode.
pub fn to_256_d(c: Rgb, dither: f32) -> u8 {
    let d = dither * 34.0 / 255.0; // ~half a mid-cube step, in 0..1 sRGB
    let s = [
        (linear_to_srgb(c.r.clamp(0.0, 1.0)) + d).clamp(0.0, 1.0),
        (linear_to_srgb(c.g.clamp(0.0, 1.0)) + d).clamp(0.0, 1.0),
        (linear_to_srgb(c.b.clamp(0.0, 1.0)) + d).clamp(0.0, 1.0),
    ];
    // cube levels are 0,95,135,175,215,255
    const LV: [f32; 6] = [0.0, 95.0, 135.0, 175.0, 215.0, 255.0];
    let idx = |v: f32| -> usize {
        let t = v * 255.0;
        let mut best = 0;
        let mut bd = f32::MAX;
        for (i, l) in LV.iter().enumerate() {
            let d = (t - l).abs();
            if d < bd {
                bd = d;
                best = i;
            }
        }
        best
    };
    let (ri, gi, bi) = (idx(s[0]), idx(s[1]), idx(s[2]));
    let cube_err = {
        let e = |v: f32, i: usize| {
            let d = v * 255.0 - LV[i];
            d * d
        };
        e(s[0], ri) + e(s[1], gi) + e(s[2], bi)
    };
    // grey ramp: 8 + 10*n for n in 0..24
    let lum = 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
    let gn = (((lum * 255.0) - 8.0) / 10.0).round().clamp(0.0, 23.0) as usize;
    let gv = 8.0 + 10.0 * gn as f32;
    let grey_err = {
        let e = |v: f32| {
            let d = v * 255.0 - gv;
            d * d
        };
        e(s[0]) + e(s[1]) + e(s[2])
    };
    if grey_err < cube_err {
        (232 + gn) as u8
    } else {
        (16 + 36 * ri + 6 * gi + bi) as u8
    }
}

/// The 16 ANSI colours, as rendered by most modern terminals (xterm defaults).
pub const ANSI16: [(u32, u8); 16] = [
    (0x000000, 30),
    (0x800000, 31),
    (0x008000, 32),
    (0x808000, 33),
    (0x000080, 34),
    (0x800080, 35),
    (0x008080, 36),
    (0xc0c0c0, 37),
    (0x808080, 90),
    (0xff0000, 91),
    (0x00ff00, 92),
    (0xffff00, 93),
    (0x0000ff, 94),
    (0xff00ff, 95),
    (0x00ffff, 96),
    (0xffffff, 97),
];

/// Nearest of the 16, matched in linear light with luma weighting so hue
/// survives the collapse better than a flat euclidean match would allow.
pub fn to_16(c: Rgb) -> u8 {
    let mut best = 30u8;
    let mut bd = f32::MAX;
    for (v, code) in ANSI16 {
        let p = hex(v);
        let dr = c.r - p.r;
        let dg = c.g - p.g;
        let db = c.b - p.b;
        let d = 0.2126 * dr * dr + 0.7152 * dg * dg + 0.0722 * db * db;
        if d < bd {
            bd = d;
            best = code;
        }
    }
    best
}
