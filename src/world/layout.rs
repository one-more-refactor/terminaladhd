//! The reflow solver — the ONLY source of a screen coordinate.
//!
//! A raw literal reaching a draw call must be a visible smell, not a silent
//! bug. [`Layout::for_field`] takes the terminal size in cells and derives every
//! rectangle the cabinet needs — the status strip, the arena, the side columns,
//! the ticker — so no draw call downstream computes geometry of its own.
//!
//! The screen is a cabinet screen: a status strip on top, the ticker at the
//! bottom, and the arena taking every row in between that it can. There is no
//! scenery to leave room for.

/// An inclusive cell rectangle, `[x0..=x1] × [y0..=y1]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Rect {
    pub fn w(&self) -> usize {
        self.x1 - self.x0 + 1
    }
    pub fn h(&self) -> usize {
        self.y1 - self.y0 + 1
    }
}

/// Rows the chrome claims above and below the arena. The status face is five
/// sub-rows and a cell is two, so the strip needs three rows to itself before
/// the progress rule can have one — anything tighter and the rule is drawn
/// through the score.
const TOP_ROWS: usize = 3;
const BOTTOM_ROWS: usize = 3;

/// Columns a side column needs to carry a piece preview and two readings.
const SIDE_MIN: usize = 10;

/// Mino edges in sub-pixels, largest first. Ten is as big as a block reads
/// before it is just a rectangle; two is as small as one reads at all.
const MINO_SIZES: [usize; 7] = [10, 8, 6, 5, 4, 3, 2];
const MINO_MIN: usize = 2;

/// Every coordinate the cabinet draws from, derived once from `(w, h)`.
#[derive(Clone, Debug)]
pub struct Layout {
    pub w: usize,
    pub h: usize,

    /// Sub-pixels per mino edge — a whole number, which is the structural
    /// guarantee against fractional scaling. A mino is `mino_px` cols ×
    /// `mino_px` sub-rows, and a sub-pixel is square, so a mino is square.
    pub mino_px: usize,

    /// The arena in minos. Tetris asks for 10×20; a game with a different shape
    /// asks for its own, and every rectangle below reflows around it.
    pub cols: usize,
    pub rows: usize,

    /// The playfield, centred in what the chrome leaves.
    pub arena: Rect,

    /// Sub-rows, not rows: both of these are anchored inside a cell rather than
    /// on one, because a five-sub-row face does not fit a two-sub-row cell.
    pub strip_sub: usize,
    pub rule_sub: usize,
    pub ticker_sub: usize,

    /// Columns free either side of the arena. Below [`SIDE_MIN`] the side
    /// columns do not exist and a game must do without them.
    pub side: usize,
    /// Top-left cell of each side column's content, when there is one.
    pub left_col: Option<(usize, usize)>,
    pub right_col: Option<(usize, usize)>,
    /// Pieces of lookahead the right column has room to show.
    pub next_deep: usize,
}

impl Layout {
    /// The tetris arena — the default the tests are written against.
    pub fn new(w: usize, h: usize) -> Layout {
        Layout::for_field(w, h, 10, 20)
    }

    /// A layout for an arena `cols × rows` minos.
    pub fn for_field(w: usize, h: usize, cols: usize, rows: usize) -> Layout {
        // The biggest mino whose arena still clears the chrome rows and leaves
        // a side column on each flank. Nothing else competes for the space:
        // there is no scenery to protect, so the game takes what there is.
        let body = h.saturating_sub(TOP_ROWS + BOTTOM_ROWS);
        let mino_px = MINO_SIZES
            .into_iter()
            .find(|&p| rows * p / 2 <= body && cols * p + 2 * SIDE_MIN <= w)
            .or_else(|| {
                // A frame too narrow for the side columns still gets a game; it
                // just loses the previews.
                MINO_SIZES
                    .into_iter()
                    .find(|&p| rows * p / 2 <= body && cols * p <= w)
            })
            .unwrap_or(MINO_MIN);

        let arena_h = rows * mino_px / 2;
        let arena_w = cols * mino_px;

        let top = TOP_ROWS + body.saturating_sub(arena_h) / 2;
        let x0 = w.saturating_sub(arena_w) / 2;
        let arena = Rect {
            x0,
            y0: top,
            x1: x0 + arena_w.saturating_sub(1),
            y1: top + arena_h.saturating_sub(1),
        };

        let side = w.saturating_sub(arena_w) / 2;
        let has_side = side >= SIDE_MIN;
        let left_col = has_side.then_some((2, arena.y0));
        let right_col = has_side.then_some((arena.x1 + 3, arena.y0));
        // Each preview costs a mino plus a gap, and the queue may not run past
        // the arena's own floor.
        let next_deep = if has_side {
            (2 * arena.h() / (mino_px + 2)).clamp(1, 5)
        } else {
            0
        };

        Layout {
            w,
            h,
            mino_px,
            cols,
            rows,
            arena,
            strip_sub: 0,
            rule_sub: 2 * TOP_ROWS - 1,
            // The ticker hangs off the bottom of the frame, which is the only
            // way a five-sub-row face fits the last row.
            ticker_sub: 2 * h.saturating_sub(3),
            side,
            left_col,
            right_col,
            next_deep,
        }
    }

    /// Top-left sub-pixel of arena cell `(col, row)`, displaced by `shake`
    /// sub-rows. Rows may sit above the arena while a piece rains in, so this
    /// takes signed coordinates and lets the writers clip.
    pub fn cell_origin(&self, col: i32, row: i32, shake: i32) -> (i32, i32) {
        let p = self.mino_px as i32;
        (
            self.arena.x0 as i32 + col * p,
            2 * self.arena.y0 as i32 + row * p - shake,
        )
    }

    /// The arena in sub-pixels: `(x0, y0, x1, y1)`, inclusive, shaken.
    pub fn arena_sub(&self, shake: i32) -> (i32, i32, i32, i32) {
        (
            self.arena.x0 as i32,
            2 * self.arena.y0 as i32 - shake,
            self.arena.x1 as i32,
            2 * (self.arena.y1 as i32 + 1) - 1 - shake,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::ALL;
    use crate::term::MIN_SIZE;

    fn sizes() -> Vec<(usize, usize)> {
        vec![
            MIN_SIZE,
            (100, 30),
            (120, 30),
            (160, 44),
            (200, 50),
            (270, 62),
            (400, 100),
        ]
    }

    #[test]
    fn the_arena_never_leaves_the_frame() {
        for kind in ALL {
            for (w, h) in sizes() {
                let l = kind.layout(w, h);
                assert!(l.arena.x1 < w, "{kind:?} {w}x{h}: arena runs off the side");
                assert!(
                    2 * (l.arena.y1 + 1) <= l.ticker_sub,
                    "{kind:?} {w}x{h}: arena reaches the ticker"
                );
                assert!(
                    2 * l.arena.y0 > l.rule_sub,
                    "{kind:?} {w}x{h}: arena covers the strip"
                );
            }
        }
    }

    #[test]
    fn the_arena_is_centred() {
        for kind in ALL {
            for (w, h) in sizes() {
                let l = kind.layout(w, h);
                let (left, right) = (l.arena.x0, w - 1 - l.arena.x1);
                assert!(
                    left.abs_diff(right) <= 1,
                    "{kind:?} {w}x{h}: {left} left vs {right} right"
                );
            }
        }
    }

    #[test]
    fn the_arena_takes_the_screen() {
        // The whole point of losing the scenery: on any frame that can afford
        // it, the game is most of the height rather than a box in a picture.
        for kind in ALL {
            for (w, h) in sizes().into_iter().filter(|&(w, h)| w >= 120 && h >= 30) {
                let l = kind.layout(w, h);
                let share = l.arena.h() as f32 / h as f32;
                assert!(share > 0.6, "{kind:?} {w}x{h}: arena is only {share:.2}");
            }
        }
    }

    #[test]
    fn the_mino_is_always_a_whole_number_of_sub_pixels() {
        for kind in ALL {
            for h in 24..=140 {
                let p = kind.layout((3 * h).max(80), h).mino_px;
                assert!((MINO_MIN..=10).contains(&p), "mino_px {p} at H={h}");
            }
        }
    }

    #[test]
    fn side_columns_appear_only_when_they_fit() {
        for kind in ALL {
            for (w, h) in sizes() {
                let l = kind.layout(w, h);
                assert_eq!(l.right_col.is_some(), l.side >= SIDE_MIN);
                if let Some((x, _)) = l.right_col {
                    assert!(x > l.arena.x1 && x < w, "{kind:?} {w}x{h}: column off-frame");
                }
                if let Some((x, _)) = l.left_col {
                    assert!(x < l.arena.x0, "{kind:?} {w}x{h}: column overlaps arena");
                }
            }
        }
    }
}
