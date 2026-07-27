//! The monitor. Everything in here is an artefact of a real cathode-ray tube,
//! applied to the finished picture on its way to the cells.
//!
//! None of it is decoration in the sense that it could be swapped for something
//! else and mean the same thing. A phosphor screen bloomed, its corners fell
//! away, its guns never landed on top of each other, its supply hummed a bar
//! down the picture, and when you cut the power the raster collapsed to a line
//! and then to a dot. Those are the things that say *tube* rather than *panel*,
//! and they are the reason a black rectangle full of neon reads as 1983 and not
//! as a terminal with the lights off.
//!
//! Every pass here works on the resolved sub-pixel buffer — `w × sh` of [`Rgb`]
//! — and the ones that need to read while they write borrow a scratch buffer
//! from the caller rather than allocating one per frame.

use crate::world::Rgb;

/// How far into the frame the corners start falling away, and how much is left
/// at the very corner. Gentle: a vignette you can point at is a filter, one you
/// cannot is a monitor.
const VIGNETTE_START: f32 = 0.55;
const VIGNETTE_FLOOR: f32 = 0.62;

/// Darken toward the corners. The cheapest of these passes and the one that
/// does the most: a flat-lit rectangle reads as a window, a rectangle that
/// falls off at the edges reads as glass with a gun behind it.
pub fn vignette(px: &mut [Rgb], w: usize, sh: usize) {
    let (cx, cy) = (w as f32 * 0.5, sh as f32 * 0.5);
    let norm = 1.0 / (cx * cx + cy * cy).sqrt();
    for y in 0..sh {
        let dy = y as f32 + 0.5 - cy;
        for x in 0..w {
            let dx = x as f32 + 0.5 - cx;
            let r = (dx * dx + dy * dy).sqrt() * norm;
            if r <= VIGNETTE_START {
                continue;
            }
            let t = ((r - VIGNETTE_START) / (1.0 - VIGNETTE_START)).clamp(0.0, 1.0);
            let k = 1.0 - (1.0 - VIGNETTE_FLOOR) * t * t;
            let i = y * w + x;
            px[i] = px[i].mul(k);
        }
    }
}

/// Pull the red and blue guns apart horizontally. A tube never landed its three
/// beams on exactly the same spot, and the misconvergence grew toward the edges
/// — which is why this scales with distance from the centre rather than being
/// applied flat.
///
/// `amount` is the shift in sub-pixels at the frame edge. Under one it is the
/// permanent hum of a slightly misaligned monitor; over three it is an impact.
pub fn fringe(px: &mut [Rgb], scratch: &mut Vec<Rgb>, w: usize, sh: usize, amount: f32) {
    if amount <= 0.01 {
        return;
    }
    scratch.clear();
    scratch.extend_from_slice(px);
    let cx = w as f32 * 0.5;
    for y in 0..sh {
        let row = y * w;
        for x in 0..w {
            let off = ((x as f32 - cx) / cx) * amount;
            let r = sample_x(scratch, row, w, x as f32 - off);
            let b = sample_x(scratch, row, w, x as f32 + off);
            let g = scratch[row + x].g;
            px[row + x] = Rgb::new(r.r, g, b.b);
        }
    }
}

/// The supply hum: a band of slightly lifted brightness crawling down the
/// picture, with a darker edge chasing it. On a real tube this is a mains
/// frequency beating against the frame rate, and it is the single artefact that
/// most reliably reads as "this is not a still image".
pub fn hum(px: &mut [Rgb], w: usize, sh: usize, pos: f32, strength: f32) {
    if strength <= 0.0 {
        return;
    }
    let band = (sh as f32 * 0.22).max(6.0);
    let head = pos.rem_euclid(1.0) * (sh as f32 + band) - band;
    for y in 0..sh {
        let d = y as f32 - head;
        if d < 0.0 || d > band {
            continue;
        }
        let t = d / band;
        // Lifted through the body of the band, pinched dark just under its
        // trailing edge — that asymmetry is what makes it read as rolling
        // rather than as a gradient someone drew.
        let k = if t < 0.85 {
            1.0 + strength * (t * std::f32::consts::PI).sin() * 0.7
        } else {
            1.0 - strength * 0.5
        };
        let row = y * w;
        for x in 0..w {
            px[row + x] = px[row + x].mul(k);
        }
    }
}

/// Displace the whole picture. Games shake their own arena; this shakes the
/// monitor, which is what a hit big enough to be felt in the chassis looks
/// like.
pub fn shake(px: &mut [Rgb], scratch: &mut Vec<Rgb>, w: usize, sh: usize, dx: i32, dy: i32) {
    if dx == 0 && dy == 0 {
        return;
    }
    scratch.clear();
    scratch.extend_from_slice(px);
    for y in 0..sh {
        for x in 0..w {
            let sx = x as i32 - dx;
            let sy = y as i32 - dy;
            px[y * w + x] = if sx < 0 || sy < 0 || sx >= w as i32 || sy >= sh as i32 {
                Rgb::ZERO
            } else {
                scratch[sy as usize * w + sx as usize]
            };
        }
    }
}

/// Cut the power. `t` runs `0.0` (full picture) to `1.0` (gone): the raster
/// squeezes toward the centre line and brightens as its energy is packed into
/// fewer rows, then the line itself shrinks to a dot and goes out.
///
/// Run backwards it is the tube warming up, which is why every screen change in
/// the machine is one of these closing and the next one opening.
pub fn collapse(px: &mut [Rgb], scratch: &mut Vec<Rgb>, w: usize, sh: usize, t: f32) {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return;
    }
    scratch.clear();
    scratch.extend_from_slice(px);

    // The vertical squeeze finishes well before the horizontal one starts to
    // bite, so the two phases read as one movement rather than as a box
    // shrinking on both axes.
    let squeeze = 1.0 - smoothstep(0.0, 0.62, t);
    let width = 1.0 - smoothstep(0.55, 1.0, t);
    let (cx, cy) = (w as f32 * 0.5, sh as f32 * 0.5);
    // Energy is conserved as the raster narrows: the same light through fewer
    // rows is brighter light. Capped, or the last frames are a white screen.
    let gain = (1.0 / squeeze.max(0.02)).min(3.5);
    let half = cy * squeeze;

    for y in 0..sh {
        let dy = y as f32 + 0.5 - cy;
        let row = y * w;
        if squeeze <= 0.001 || dy.abs() > half {
            for x in 0..w {
                px[row + x] = Rgb::ZERO;
            }
            continue;
        }
        let sy = (cy + dy / squeeze).clamp(0.0, sh as f32 - 1.0) as usize;
        let src = sy * w;
        let edge = cx * width;
        for x in 0..w {
            let dx = x as f32 + 0.5 - cx;
            px[row + x] = if dx.abs() > edge {
                Rgb::ZERO
            } else {
                scratch[src + x].mul(gain)
            };
        }
    }

    // The dying line itself, once the picture is thinner than a sub-pixel: a
    // white core that shrinks with the width and takes the last of the light.
    if squeeze <= 0.06 && width > 0.0 {
        let y = cy as usize;
        if y < sh {
            let row = y * w;
            let edge = (cx * width).max(0.5);
            let core = Rgb::new(1.0, 1.0, 1.0).mul(width.min(1.0));
            for x in 0..w {
                if (x as f32 + 0.5 - cx).abs() <= edge {
                    px[row + x] = px[row + x].add(core);
                }
            }
        }
    }
}

/// Sync loss: bands of the picture slip sideways for a few frames. A tube that
/// lost its horizontal hold tore exactly like this, and it is the loudest thing
/// this machine can do without leaving the vocabulary of a monitor.
///
/// `amount` is the worst slip in sub-pixels; `seed` varies which bands go, so
/// consecutive frames tear differently rather than shimmering in place.
pub fn tear(px: &mut [Rgb], scratch: &mut Vec<Rgb>, w: usize, sh: usize, amount: f32, seed: u64) {
    if amount < 0.5 {
        return;
    }
    scratch.clear();
    scratch.extend_from_slice(px);
    let mut state = seed | 1;
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545f4914f6cdd1d)
    };

    let mut y = 0usize;
    while y < sh {
        // Bands rather than rows: a per-row offset reads as noise, a band of
        // rows moving together reads as the picture losing its hold.
        let band = 1 + (next() % 6) as usize;
        let end = (y + band).min(sh);
        // Most bands stay put. A tear where everything moves is just a blur.
        let slip = if next() % 5 == 0 {
            let mag = (next() % (2 * amount as u64 + 1)) as f32 - amount;
            mag as i32
        } else {
            0
        };
        if slip != 0 {
            for row in y..end {
                let base = row * w;
                for x in 0..w {
                    let sx = x as i32 - slip;
                    px[base + x] = if sx < 0 || sx >= w as i32 {
                        Rgb::ZERO
                    } else {
                        scratch[base + sx as usize]
                    };
                }
            }
        }
        y = end;
    }
}

/// Flip the picture to its negative. Two frames of this is what a cabinet did
/// when something enormous happened, and nothing else on a black screen carries
/// the same weight.
pub fn invert(px: &mut [Rgb], amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let k = amount.clamp(0.0, 1.0);
    for p in px.iter_mut() {
        let inv = Rgb::new(
            (1.0 - p.r).max(0.0),
            (1.0 - p.g).max(0.0),
            (1.0 - p.b).max(0.0),
        );
        *p = p.lerp(inv, k);
    }
}

/// Add a flat wash of colour to the whole frame — the moment a thing lands,
/// before any of the above has a chance to be subtle about it.
///
/// White is the default because white is what a phosphor screen does when it is
/// driven past what it can hold, but a hue carries which *kind* of thing landed:
/// gold for a bonus, the piece's own colour for a clear.
pub fn wash(px: &mut [Rgb], col: Rgb, amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let wash = col.mul(amount.clamp(0.0, 2.0));
    for p in px.iter_mut() {
        *p = p.add(wash);
    }
}

/// A hard band of light ripping down the picture. Where [`hum`] is the slow
/// artefact of a supply that never quite settled, this is the fast one: the
/// beam overdriven for a few lines, which is what a cabinet did when something
/// large happened and it had nothing louder left than the tube itself.
pub fn rip(px: &mut [Rgb], w: usize, sh: usize, pos: f32, col: Rgb, strength: f32) {
    if strength <= 0.0 || !(0.0..=1.0).contains(&pos) {
        return;
    }
    let band = (sh as f32 * 0.10).max(3.0);
    let head = pos * (sh as f32 + band) - band * 0.5;
    for y in 0..sh {
        let d = (y as f32 - head).abs();
        if d > band * 0.5 {
            continue;
        }
        // Hard in the middle and gone at the edges, so it reads as one bright
        // line rather than as a grey block sliding past.
        let k = 1.0 - (d / (band * 0.5));
        let add = col.mul(strength * k * k);
        let row = y * w;
        for x in 0..w {
            px[row + x] = px[row + x].add(add);
        }
    }
}

fn sample_x(src: &[Rgb], row: usize, w: usize, x: f32) -> Rgb {
    let xi = x.round().clamp(0.0, w as f32 - 1.0) as usize;
    src[row + xi]
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if b <= a {
        return if x < a { 0.0 } else { 1.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(w: usize, sh: usize) -> Vec<Rgb> {
        vec![Rgb::new(0.5, 0.5, 0.5); w * sh]
    }

    fn luma(px: &[Rgb]) -> f32 {
        px.iter().map(|p| p.r + p.g + p.b).sum()
    }

    #[test]
    fn the_vignette_darkens_the_corners_and_leaves_the_centre() {
        let (w, sh) = (40, 40);
        let mut px = field(w, sh);
        vignette(&mut px, w, sh);
        let mid = px[(sh / 2) * w + w / 2];
        assert_eq!(mid.r, 0.5, "the centre is untouched");
        assert!(px[0].r < 0.5, "the corner is not");
        assert!(px[0].r > 0.2, "but it is not crushed either");
    }

    #[test]
    fn a_full_collapse_leaves_nothing_lit() {
        let (w, sh) = (32, 32);
        let mut px = field(w, sh);
        let mut scratch = Vec::new();
        collapse(&mut px, &mut scratch, w, sh, 1.0);
        assert_eq!(luma(&px), 0.0, "the tube is off");
    }

    #[test]
    fn collapsing_packs_the_picture_into_fewer_brighter_rows() {
        let (w, sh) = (32, 32);
        let mut px = field(w, sh);
        let mut scratch = Vec::new();
        collapse(&mut px, &mut scratch, w, sh, 0.4);
        let lit_rows = (0..sh)
            .filter(|&y| px[y * w..(y + 1) * w].iter().any(|p| p.r > 0.0))
            .count();
        assert!(lit_rows < sh, "the raster has squeezed: {lit_rows} of {sh}");
        assert!(lit_rows > 0, "and has not gone out yet");
        let brightest = px.iter().map(|p| p.r).fold(0.0f32, f32::max);
        assert!(brightest > 0.5, "what is left is brighter: {brightest}");
    }

    #[test]
    fn t_zero_is_the_picture_untouched() {
        let (w, sh) = (16, 16);
        let mut px = field(w, sh);
        let before = px.clone();
        let mut scratch = Vec::new();
        collapse(&mut px, &mut scratch, w, sh, 0.0);
        assert_eq!(px, before);
    }

    #[test]
    fn the_fringe_leaves_the_centre_column_alone() {
        let (w, sh) = (33, 4);
        let mut px = vec![Rgb::ZERO; w * sh];
        for y in 0..sh {
            px[y * w + w / 2] = Rgb::new(1.0, 1.0, 1.0);
        }
        let mut scratch = Vec::new();
        fringe(&mut px, &mut scratch, w, sh, 4.0);
        // Misconvergence grows from the centre out, so the middle is where the
        // guns agree — anywhere else and the whole picture would smear.
        assert_eq!(px[w / 2].g, 1.0);
    }

    #[test]
    fn the_hum_touches_one_band_and_moves() {
        let (w, sh) = (8, 64);
        let mut a = field(w, sh);
        let mut b = field(w, sh);
        hum(&mut a, w, sh, 0.2, 0.3);
        hum(&mut b, w, sh, 0.6, 0.3);
        assert_ne!(a, b, "the band is somewhere else a moment later");
        let untouched = (0..sh)
            .filter(|&y| (a[y * w].r - 0.5).abs() < 1e-6)
            .count();
        assert!(untouched > sh / 2, "most of the picture is not in the band");
    }

    #[test]
    fn a_tear_moves_some_bands_and_leaves_most_alone() {
        let (w, sh) = (32, 64);
        let mut px = vec![Rgb::ZERO; w * sh];
        for y in 0..sh {
            for x in 0..w {
                px[y * w + x] = Rgb::new(x as f32 / w as f32, 0.5, 0.5);
            }
        }
        let before = px.clone();
        let mut scratch = Vec::new();
        tear(&mut px, &mut scratch, w, sh, 6.0, 99);
        let moved = (0..sh)
            .filter(|&y| px[y * w..(y + 1) * w] != before[y * w..(y + 1) * w])
            .count();
        assert!(moved > 0, "something tore");
        assert!(moved < sh, "but the whole picture did not slide");
    }

    #[test]
    fn a_tear_of_nothing_is_a_no_op() {
        let (w, sh) = (8, 8);
        let mut px = field(w, sh);
        let before = px.clone();
        let mut scratch = Vec::new();
        tear(&mut px, &mut scratch, w, sh, 0.0, 1);
        assert_eq!(px, before);
    }

    #[test]
    fn a_rip_lights_one_band_and_leaves_the_rest() {
        let (w, sh) = (8, 60);
        let mut px = vec![Rgb::ZERO; w * sh];
        rip(&mut px, w, sh, 0.5, Rgb::new(1.0, 1.0, 1.0), 1.0);
        let lit = (0..sh).filter(|&y| px[y * w].r > 0.01).count();
        assert!(lit > 0 && lit < sh / 2, "one band, not the picture: {lit}");
        // Brightest at its centre.
        let peak = (0..sh).max_by(|a, b| px[a * w].r.total_cmp(&px[b * w].r)).unwrap();
        assert!((peak as i32 - sh as i32 / 2).abs() < 4, "peak at {peak}");
    }

    #[test]
    fn a_wash_of_nothing_changes_nothing() {
        let mut px = field(4, 4);
        let before = px.clone();
        wash(&mut px, Rgb::new(1.0, 1.0, 1.0), 0.0);
        assert_eq!(px, before);
    }

    #[test]
    fn a_full_invert_is_the_negative() {
        let mut px = vec![Rgb::new(1.0, 0.25, 0.0)];
        invert(&mut px, 1.0);
        assert_eq!(px[0], Rgb::new(0.0, 0.75, 1.0));
    }

    #[test]
    fn a_shake_moves_the_picture_and_blacks_what_it_uncovers() {
        let (w, sh) = (8, 8);
        let mut px = field(w, sh);
        let mut scratch = Vec::new();
        shake(&mut px, &mut scratch, w, sh, 2, 0);
        assert_eq!(px[0], Rgb::ZERO, "the uncovered edge is black");
        assert_eq!(px[4].r, 0.5, "and the picture itself came with it");
    }
}
