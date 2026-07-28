//! Damage-tracked frame encoding: compare against the previously presented
//! frame and repaint only the cells that differ, jumping the cursor over
//! untouched spans.
//!
//! With a one-bit picture in a handful of flat tones the diff is small by
//! nature — most frames a game changes a few dozen cells — and the encoder's
//! job is just not to squander that: one CSI when both colours change, none
//! when neither does, and short clean gaps repainted through rather than
//! re-addressed.

use crate::screen::Cell;

pub const UPPER_HALF: &str = "\u{2580}";

fn near(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tol)
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

/// Both colours in one CSI: saves bytes, and one parser dispatch per cell in
/// the terminal.
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
    push_u8(o, (row + 1).min(255) as u8);
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

/// Encode the difference between `cells` and `prev` into `o`, clearing it
/// first. `gap` is the number of unchanged cells it is still cheaper to
/// repaint through than to re-address the cursor past.
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
            // Extend the run until `gap` consecutive clean cells end it.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(fg: [u8; 3], bg: [u8; 3]) -> Cell {
        Cell { half: true, fg, bg }
    }

    #[test]
    fn an_unchanged_frame_encodes_to_nothing() {
        let frame = vec![cell([1, 2, 3], [4, 5, 6]); 8];
        let mut o = Vec::new();
        enc_diff(&frame, &frame, 4, 2, 0, 2, &mut o);
        assert!(o.is_empty());
    }

    #[test]
    fn a_single_changed_cell_is_addressed_and_repainted() {
        let prev = vec![cell([0, 0, 0], [0, 0, 0]); 8];
        let mut next = prev.clone();
        next[5] = cell([255, 0, 0], [0, 0, 255]);
        let mut o = Vec::new();
        enc_diff(&next, &prev, 4, 2, 0, 2, &mut o);
        let s = String::from_utf8_lossy(&o);
        assert!(s.contains("\x1b[2;2H"), "cursor lands on row 2 col 2: {s:?}");
        assert!(s.contains("38;2;255;0;0"));
        assert!(s.contains("48;2;0;0;255"));
    }

    #[test]
    fn short_gaps_are_painted_through_rather_than_readdressed() {
        let prev = vec![cell([0, 0, 0], [0, 0, 0]); 8];
        let mut next = prev.clone();
        next[0] = cell([9, 9, 9], [0, 0, 0]);
        next[3] = cell([9, 9, 9], [0, 0, 0]);
        let mut o = Vec::new();
        enc_diff(&next, &prev, 8, 1, 0, 4, &mut o);
        let s = String::from_utf8_lossy(&o);
        assert_eq!(s.matches("\x1b[1;").count(), 1, "one run, not two: {s:?}");
    }
}
