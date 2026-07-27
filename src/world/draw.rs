//! Sub-pixel drawing primitives shared by the world and every game skin.
//!
//! A [`Buf`] carries two planes: `base` is what the surface looks like, `emis`
//! is what it throws into the bloom pass. Writing base clears emissive for that
//! sub-pixel, so an opaque draw never leaves the previous layer's glow behind
//! it — [`lit`] is the pairing for a surface that is both.

use super::scene::palette::*;
use super::scene::Buf;
use super::tiny as smallfont;
use crate::world::color::{hex, Rgb};

/// Opaque write. Clears the emissive plane at that sub-pixel: whatever glow was
/// there belonged to a layer this one is now covering.
pub fn put_base(b: &mut Buf, x: i32, y: i32, col: Rgb) {
    if x < 0 || y < 0 || x as usize >= b.w || y as usize >= b.sh {
        return;
    }
    let i = y as usize * b.w + x as usize;
    b.base[i] = col;
    b.emis[i] = Rgb::ZERO;
}

/// Additive write to the emissive plane only — light without a surface, which
/// is how sparks, halos and shockwaves stay transparent.
pub fn add_emis(b: &mut Buf, x: i32, y: i32, col: Rgb) {
    if x < 0 || y < 0 || x as usize >= b.w || y as usize >= b.sh {
        return;
    }
    let i = y as usize * b.w + x as usize;
    b.emis[i] = b.emis[i].add(col);
}

/// Opaque fill plus an emissive add, the common case for a lit surface.
pub fn lit(b: &mut Buf, x: i32, y: i32, base: Rgb, emis: Rgb) {
    put_base(b, x, y, base);
    add_emis(b, x, y, emis);
}

/// A solid neon segment with a single highlight along its top-left, and no
/// white bevel. Where [`capsule`] reads as a glass block, this reads as a lit
/// tube — which is what a snake is, and what a bevelled block cannot be at
/// three sub-pixels, where the bevels are the whole cell.
pub fn pill(b: &mut Buf, x0: i32, y0: i32, size: i32, fill: Rgb, halo: f32) {
    // Top row only. At three sub-pixels a two-sided highlight is five of the
    // nine, which is not a highlight — it is a paler block.
    let hi = fill.lerp(hex(WHITE), 0.22);
    for dy in 0..size {
        for dx in 0..size {
            let col = if dy == 0 { hi } else { fill };
            put_base(b, x0 + dx, y0 + dy, col);
        }
    }
    if halo > 0.0 {
        glow_rect(b, x0, y0, size, size, fill.mul(halo));
    }
}

pub fn glow_rect(b: &mut Buf, x0: i32, y0: i32, w: i32, h: i32, col: Rgb) {
    for dy in 0..h {
        for dx in 0..w {
            add_emis(b, x0 + dx, y0 + dy, col);
        }
    }
}

// -------------------------------------------------------------- small text

/// Draw `text` in the 3×5 handset face, top-left at sub-pixel `(x, y)`. `emis`
/// scales how much of the glyph feeds the bloom pass; ticker text passes 0 so it
/// never blooms (SPEC section 8: dim, never bloomed).
pub fn text(b: &mut Buf, s: &str, x: i32, y: i32, scale: usize, col: Rgb, emis: f32) {
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

pub fn text_w(s: &str, scale: usize) -> i32 {
    let n = s.chars().count() as i32;
    if n == 0 {
        0
    } else {
        n * 4 * scale as i32 - scale as i32
    }
}

/// `text`, centred on `cx` rather than started at it.
pub fn text_center(b: &mut Buf, s: &str, cx: i32, y: i32, scale: usize, col: Rgb, emis: f32) {
    text(b, s, cx - text_w(s, scale) / 2, y, scale, col, emis);
}

// ------------------------------------------------------------------- 7-seg

/// One 7-segment digit in a `4 × 7` sub-pixel box, lit segments in `hue`, off
/// segments the dark `GRID_TICK` rule (SPEC section 8).
pub fn seg_digit(b: &mut Buf, d: u8, x0: i32, y0: i32, hue: Rgb) {
    const MASK: [u8; 10] = [
        0b0111111, 0b0000110, 0b1011011, 0b1001111, 0b1100110, 0b1101101, 0b1111101, 0b0000111,
        0b1111111, 0b1101111,
    ];
    let m = MASK[(d % 10) as usize];
    let off = hex(IRON);
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

pub fn seg_number(b: &mut Buf, n: u32, digits: usize, x0: i32, y0: i32, hue: Rgb) {
    let s = format!("{:0width$}", n, width = digits);
    for (i, ch) in s.chars().enumerate() {
        let d = ch.to_digit(10).unwrap_or(0) as u8;
        seg_digit(b, d, x0 + i as i32 * 5, y0, hue);
    }
}

pub fn seg_number_w(digits: usize) -> i32 {
    digits as i32 * 5 - 1
}

// ------------------------------------------------------------------ blocks

/// One lit glass capsule (SPEC section 9.2): saturated fill, a bright top/left
/// bevel, a dark bottom/right bevel, and a bloom halo into `emis`.
pub fn capsule(b: &mut Buf, x0: i32, y0: i32, size: i32, fill: Rgb, halo: f32) {
    let hi = fill.lerp(hex(WHITE), 0.35);
    let lo = fill.lerp(hex(VOID), 0.45);
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
    if halo > 0.0 {
        glow_rect(b, x0, y0, size, size, fill.mul(halo));
    }
}

/// A filled disc in emissive light only — the shockwave and spark primitive.
pub fn ring(b: &mut Buf, cx: f32, cy: f32, r: f32, thick: f32, col: Rgb) {
    let r0 = (r - thick).max(0.0);
    let (x0, x1) = ((cx - r - 1.0) as i32, (cx + r + 1.0) as i32);
    let (y0, y1) = ((cy - r - 1.0) as i32, (cy + r + 1.0) as i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d >= r0 && d <= r {
                // Feather the outer edge so the ring reads as light rather than
                // as a jagged annulus of sub-pixels.
                let f = 1.0 - ((d - r0) / thick.max(0.001) - 0.5).abs() * 2.0;
                add_emis(b, x, y, col.mul(f.max(0.0)));
            }
        }
    }
}
