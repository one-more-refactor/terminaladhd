// Linear-light RGB. All blending, blurring and gradient interpolation happens
// here; sRGB is only ever an encoding at the boundary.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const ZERO: Rgb = Rgb {
        r: 0.0,
        g: 0.0,
        b: 0.0,
    };

    pub fn new(r: f32, g: f32, b: f32) -> Self {
        Rgb { r, g, b }
    }

    // `add` and `mul` shadow the names of the std arithmetic traits on purpose.
    // Implementing `Add`/`Mul` would let `a + b` compile for a type where the
    // whole point is that light is added in linear space and only ever scaled
    // by a scalar; naming them keeps every call site saying which it meant.
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, o: Rgb) -> Rgb {
        Rgb::new(self.r + o.r, self.g + o.g, self.b + o.b)
    }

    #[allow(clippy::should_implement_trait)]
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
        let s = linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0 + dither;
        s.round().clamp(0.0, 255.0) as u8
    };
    [f(c.r), f(c.g), f(c.b)]
}

/// 4x4 Bayer, returned centred on zero in units of one 8-bit step.
pub fn bayer4(x: usize, y: usize) -> f32 {
    const M: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    (M[y & 3][x & 3] as f32 + 0.5) / 16.0 - 0.5
}
