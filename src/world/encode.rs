use super::color::*;

pub const UPPER_HALF: &str = "\u{2580}"; // 3 bytes UTF-8

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub struct Cell {
    pub half: bool, // true => upper-half block with fg+bg, false => space (bg only)
    pub fg: [u8; 3],
    pub bg: [u8; 3],
}

fn near(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tol)
}

/// Fold the sub-pixel buffer into cells. When the two sub-rows of a cell are
/// within `tol` of each other the cell collapses to a space, which halves the
/// escape bytes for that cell and removes the faint seam a half-block leaves on
/// terminals that anti-alias glyph edges.
///
/// `dither` toggles ordered (Bayer) dithering at the 8-bit quantisation step.
/// A truecolour terminal has a full 8-bit-per-channel palette, but a vertical
/// gradient spanning only ~28 rows still lands many neighbouring rows on the
/// same rounded value -> visible bands. Adding a per-sub-pixel Bayer offset of
/// under one LSB before rounding pushes adjacent columns across the rounding
/// boundary at staggered points, trading a hard band for 4x4 noise the eye
/// integrates back to a smooth ramp.
pub fn resolve(px: &[Rgb], w: usize, sh: usize, tol: i32, out: &mut Vec<Cell>) {
    resolve_d(px, w, sh, tol, true, out)
}

pub fn resolve_d(px: &[Rgb], w: usize, sh: usize, tol: i32, dither: bool, out: &mut Vec<Cell>) {
    let h = sh / 2;
    out.clear();
    out.reserve(w * h);
    for y in 0..h {
        for x in 0..w {
            let (dt, db) = if dither {
                (bayer4(x, 2 * y), bayer4(x, 2 * y + 1))
            } else {
                (0.0, 0.0)
            };
            let t = to_srgb8(px[(2 * y) * w + x], dt);
            let b = to_srgb8(px[(2 * y + 1) * w + x], db);
            if near(t, b, tol) {
                out.push(Cell {
                    half: false,
                    fg: t,
                    bg: t,
                });
            } else {
                out.push(Cell {
                    half: true,
                    fg: t,
                    bg: b,
                });
            }
        }
    }
}

fn push_u8(o: &mut Vec<u8>, v: u8) {
    if v >= 100 {
        o.push(b'0' + v / 100);
        o.push(b'0' + (v / 10) % 10);
        o.push(b'0' + v % 10);
    } else if v >= 10 {
        o.push(b'0' + v / 10);
        o.push(b'0' + v % 10);
    } else {
        o.push(b'0' + v);
    }
}

fn sgr_fg(o: &mut Vec<u8>, c: [u8; 3]) {
    o.extend_from_slice(b"\x1b[38;2;");
    push_u8(o, c[0]);
    o.push(b';');
    push_u8(o, c[1]);
    o.push(b';');
    push_u8(o, c[2]);
    o.push(b'm');
}

fn sgr_bg(o: &mut Vec<u8>, c: [u8; 3]) {
    o.extend_from_slice(b"\x1b[48;2;");
    push_u8(o, c[0]);
    o.push(b';');
    push_u8(o, c[1]);
    o.push(b';');
    push_u8(o, c[2]);
    o.push(b'm');
}

/// Both colours in one CSI. Saves 3 bytes over two separate sequences and, more
/// importantly, one parser dispatch per cell in the terminal.
fn sgr_both(o: &mut Vec<u8>, fg: [u8; 3], bg: [u8; 3]) {
    o.extend_from_slice(b"\x1b[38;2;");
    push_u8(o, fg[0]);
    o.push(b';');
    push_u8(o, fg[1]);
    o.push(b';');
    push_u8(o, fg[2]);
    o.extend_from_slice(b";48;2;");
    push_u8(o, bg[0]);
    o.push(b';');
    push_u8(o, bg[1]);
    o.push(b';');
    push_u8(o, bg[2]);
    o.push(b'm');
}

/// Nearest xterm-256 index for an sRGB cell colour: the 6x6x6 cube or the
/// grey ramp, whichever is closer. On the wire a palette pair is a third of a
/// truecolor pair, and a phone terminal parses it in a fraction of the time —
/// which is the whole reason lean mode speaks palette.
fn srgb_to_256(c: [u8; 3]) -> u8 {
    const LV: [i16; 6] = [0, 95, 135, 175, 215, 255];
    let near_lv = |v: u8| -> usize {
        let v = v as i16;
        let mut best = 0;
        let mut bd = i16::MAX;
        for (i, l) in LV.iter().enumerate() {
            let d = (v - l).abs();
            if d < bd {
                bd = d;
                best = i;
            }
        }
        best
    };
    let (ri, gi, bi) = (near_lv(c[0]), near_lv(c[1]), near_lv(c[2]));
    let cube = [LV[ri], LV[gi], LV[bi]];
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;
    // Grey ramp: 232..=255 at 8 + 10k.
    let avg = (c[0] as i16 + c[1] as i16 + c[2] as i16) / 3;
    let k = ((avg - 8).max(0) / 10).min(23);
    let grey = 8 + 10 * k;
    let d2 = |a: [i16; 3]| -> i32 {
        (0..3)
            .map(|i| (c[i] as i32 - a[i] as i32).pow(2))
            .sum::<i32>()
    };
    if d2([grey, grey, grey]) < d2(cube) {
        (232 + k) as u8
    } else {
        cube_idx as u8
    }
}

fn sgr_fg_256(o: &mut Vec<u8>, i: u8) {
    o.extend_from_slice(b"\x1b[38;5;");
    push_u8(o, i);
    o.push(b'm');
}

fn sgr_bg_256(o: &mut Vec<u8>, i: u8) {
    o.extend_from_slice(b"\x1b[48;5;");
    push_u8(o, i);
    o.push(b'm');
}

fn sgr_both_256(o: &mut Vec<u8>, fg: u8, bg: u8) {
    o.extend_from_slice(b"\x1b[38;5;");
    push_u8(o, fg);
    o.extend_from_slice(b";48;5;");
    push_u8(o, bg);
    o.push(b'm');
}

fn goto(o: &mut Vec<u8>, row: usize, col: usize) {
    o.extend_from_slice(b"\x1b[");
    // Clamped, not truncated: a row past 254 would wrap to the top of the
    // screen and scramble the frame on very tall terminals; the size cap
    // keeps this theoretical, and the clamp keeps it harmless.
    push_u8(o, (row + 1).min(255) as u8);
    o.push(b';');
    let c = (col + 1).min(999);
    if c >= 100 {
        o.push(b'0' + (c / 100) as u8);
        o.push(b'0' + ((c / 10) % 10) as u8);
        o.push(b'0' + (c % 10) as u8);
    } else if c >= 10 {
        o.push(b'0' + (c / 10) as u8);
        o.push(b'0' + (c % 10) as u8);
    } else {
        o.push(b'0' + c as u8);
    }
    o.push(b'H');
}

/// How a diff is encoded: the colour-match tolerance, the run-joining gap
/// (unchanged cells cheaper to repaint through than to re-address past), and
/// whether the wire speaks xterm-256 instead of truecolor.
#[derive(Clone, Copy, Debug)]
pub struct DiffOpts {
    pub tol: i32,
    pub gap: usize,
    pub palette: bool,
}

/// `prev` is what is actually on the terminal, and this function keeps it
/// true: painted cells are written back, skipped cells keep their old value.
/// Diffing intended-against-intended instead let a slow fade walk under the
/// tolerance one step at a time and leave the screen permanently a few
/// shades stale, because every comparison was against a picture that was
/// never painted.
pub fn enc_diff(
    cells: &[Cell],
    prev: &mut [Cell],
    w: usize,
    h: usize,
    d: DiffOpts,
    o: &mut Vec<u8>,
) {
    let DiffOpts { tol, gap, palette } = d;
    o.clear();
    let mut cur_fg: Option<[u8; 3]> = None;
    let mut cur_bg: Option<[u8; 3]> = None;
    let mut cur_fg_i: Option<u8> = None;
    let mut cur_bg_i: Option<u8> = None;

    // In palette mode a cell is unchanged when its *emitted* colours are —
    // two truecolor values that quantise to the same xterm-256 index paint
    // the same ink, and repainting them was most of lean's remaining bytes
    // once the finer posterize let more sub-visible drift through.
    let same = |a: &Cell, b: &Cell| -> bool {
        if a.half != b.half {
            return false;
        }
        if palette {
            srgb_to_256(a.bg) == srgb_to_256(b.bg)
                && (!a.half || srgb_to_256(a.fg) == srgb_to_256(b.fg))
        } else {
            near(a.bg, b.bg, tol) && (!a.half || near(a.fg, b.fg, tol))
        }
    };

    for y in 0..h {
        let row = y * w;
        let mut x = 0;
        while x < w {
            if same(&cells[row + x], &prev[row + x]) {
                x += 1;
                continue;
            }
            // extend the run: keep going until `gap` consecutive clean cells
            let start = x;
            let mut end = x;
            let mut clean = 0;
            let mut j = x;
            while j < w {
                if same(&cells[row + j], &prev[row + j]) {
                    clean += 1;
                    if clean > gap {
                        break;
                    }
                } else {
                    clean = 0;
                    end = j;
                }
                j += 1;
            }
            goto(o, y, start);
            // the cursor jumped, so the colour registers are still valid but
            // position-independent -- no need to reset them
            for k in start..=end {
                let c = cells[row + k];
                prev[row + k] = c;
                if palette {
                    let fi = srgb_to_256(c.fg);
                    let bi = srgb_to_256(c.bg);
                    let need_fg = c.half && cur_fg_i != Some(fi);
                    let need_bg = cur_bg_i != Some(bi);
                    match (need_fg, need_bg) {
                        (true, true) => {
                            sgr_both_256(o, fi, bi);
                            cur_fg_i = Some(fi);
                            cur_bg_i = Some(bi);
                        }
                        (true, false) => {
                            sgr_fg_256(o, fi);
                            cur_fg_i = Some(fi);
                        }
                        (false, true) => {
                            sgr_bg_256(o, bi);
                            cur_bg_i = Some(bi);
                        }
                        (false, false) => {}
                    }
                    if c.half {
                        o.extend_from_slice(UPPER_HALF.as_bytes());
                    } else {
                        o.push(b' ');
                    }
                    continue;
                }
                let need_fg = c.half && !cur_fg.is_some_and(|p| near(p, c.fg, tol));
                let need_bg = !cur_bg.is_some_and(|p| near(p, c.bg, tol));
                match (need_fg, need_bg) {
                    (true, true) => {
                        sgr_both(o, c.fg, c.bg);
                        cur_fg = Some(c.fg);
                        cur_bg = Some(c.bg);
                    }
                    (true, false) => {
                        sgr_fg(o, c.fg);
                        cur_fg = Some(c.fg);
                    }
                    (false, true) => {
                        sgr_bg(o, c.bg);
                        cur_bg = Some(c.bg);
                    }
                    (false, false) => {}
                }
                if c.half {
                    o.extend_from_slice(UPPER_HALF.as_bytes());
                } else {
                    o.push(b' ');
                }
            }
            x = end + 1;
        }
    }
}
