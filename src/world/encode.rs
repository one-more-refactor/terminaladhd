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

fn goto(o: &mut Vec<u8>, row: usize, col: usize) {
    o.extend_from_slice(b"\x1b[");
    push_u8(o, (row + 1) as u8);
    o.push(b';');
    let c = col + 1;
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

// -------------------------------------------------------------- strategy A

/// Emit both colours for every single cell. This is the shape most naive
/// terminal renderers take and it is the number the design has to beat.
pub fn enc_naive(cells: &[Cell], w: usize, h: usize, o: &mut Vec<u8>) {
    o.clear();
    for y in 0..h {
        goto(o, y, 0);
        for x in 0..w {
            let c = cells[y * w + x];
            sgr_both(o, c.fg, c.bg);
            if c.half {
                o.extend_from_slice(UPPER_HALF.as_bytes());
            } else {
                o.push(b' ');
            }
        }
    }
}

// -------------------------------------------------------------- strategy B

/// Track the terminal's current fg/bg and emit only what actually changes.
/// `tol` lets a colour within N 8-bit steps of the one already set pass without
/// an escape at all -- the single biggest win on smooth gradients.
pub fn enc_stateful(cells: &[Cell], w: usize, h: usize, tol: i32, o: &mut Vec<u8>) {
    o.clear();
    let mut cur_fg: Option<[u8; 3]> = None;
    let mut cur_bg: Option<[u8; 3]> = None;
    for y in 0..h {
        goto(o, y, 0);
        for x in 0..w {
            let c = cells[y * w + x];
            // A space shows only its background, so the foreground register can
            // be left stale -- skipping it costs nothing visually.
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
    }
}

// -------------------------------------------------------------- strategy C

/// Damage-tracked: compare against the previously presented frame and repaint
/// only cells that differ, jumping the cursor over untouched spans. `gap` is
/// the number of unchanged cells it is still cheaper to repaint than to skip.
pub fn enc_diff(
    cells: &[Cell],
    prev: &[Cell],
    w: usize,
    h: usize,
    tol: i32,
    gap: usize,
    o: &mut Vec<u8>,
) {
    o.clear();
    let mut cur_fg: Option<[u8; 3]> = None;
    let mut cur_bg: Option<[u8; 3]> = None;

    let same = |a: &Cell, b: &Cell| -> bool {
        a.half == b.half && near(a.bg, b.bg, tol) && (!a.half || near(a.fg, b.fg, tol))
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

// ---------------------------------------------------------- reduced palettes

pub fn enc_256(px: &[Rgb], w: usize, h: usize, o: &mut Vec<u8>) {
    o.clear();
    let mut cur_fg: Option<u8> = None;
    let mut cur_bg: Option<u8> = None;
    for y in 0..h {
        goto(o, y, 0);
        for x in 0..w {
            let t = to_256(px[(2 * y) * w + x]);
            let b = to_256(px[(2 * y + 1) * w + x]);
            if t == b {
                if cur_bg != Some(b) {
                    o.extend_from_slice(b"\x1b[48;5;");
                    push_u8(o, b);
                    o.push(b'm');
                    cur_bg = Some(b);
                }
                o.push(b' ');
            } else {
                if cur_fg != Some(t) {
                    o.extend_from_slice(b"\x1b[38;5;");
                    push_u8(o, t);
                    o.push(b'm');
                    cur_fg = Some(t);
                }
                if cur_bg != Some(b) {
                    o.extend_from_slice(b"\x1b[48;5;");
                    push_u8(o, b);
                    o.push(b'm');
                    cur_bg = Some(b);
                }
                o.extend_from_slice(UPPER_HALF.as_bytes());
            }
        }
    }
}

pub fn enc_16(px: &[Rgb], w: usize, h: usize, o: &mut Vec<u8>) {
    o.clear();
    let mut cur_fg: Option<u8> = None;
    let mut cur_bg: Option<u8> = None;
    for y in 0..h {
        goto(o, y, 0);
        for x in 0..w {
            let t = to_16(px[(2 * y) * w + x]);
            let b = to_16(px[(2 * y + 1) * w + x]);
            // background can only use the 8 non-bright slots on many terminals
            let bbg = if b >= 90 { b - 90 + 40 } else { b - 30 + 40 };
            if cur_fg != Some(t) {
                o.extend_from_slice(b"\x1b[");
                push_u8(o, t);
                o.push(b'm');
                cur_fg = Some(t);
            }
            if cur_bg != Some(bbg) {
                o.extend_from_slice(b"\x1b[");
                push_u8(o, bbg);
                o.push(b'm');
                cur_bg = Some(bbg);
            }
            o.extend_from_slice(UPPER_HALF.as_bytes());
        }
    }
}
