//! The reflow solver — the ONLY source of a scene coordinate.
//!
//! SPEC section 2.1's lesson: a raw literal reaching a draw call must be a
//! visible smell, not a silent bug. [`Layout::new`] takes the terminal size in
//! cells and derives every rectangle the Diorama needs — the well, the horizon
//! weld, the sun, the grid rungs, the selector word, the scenery signs, the
//! ticker — so no draw call downstream computes geometry of its own.
//!
//! The reflow rule is SPEC section 2.2; the results match the authoritative rect
//! table in section 2.6 at 80×24, 120×30 and 270×62 (asserted in the tests).

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

/// Every coordinate the scene draws from, derived once from `(w, h)`.
#[derive(Clone, Debug)]
pub struct Layout {
    pub w: usize,
    pub h: usize,

    /// Sub-pixels per mino edge — only ever 2, 4 or 6 (the structural guarantee
    /// against fractional scaling). A mino is `mino_px` cols × `mino_px` sub-rows.
    pub mino_px: usize,

    /// The 10×20-mino playfield, cut into the sky above the weld.
    pub well: Rect,
    pub well_x0: usize,
    pub well_w: usize,
    pub well_h: usize,

    /// The cyan weld the tetris stack rests on (`top_air + well_h`).
    pub horizon_row: usize,
    /// The broad idle/selector floor the camera tilts to (`round(0.44·H)`).
    pub horizon_idle: usize,

    pub top_air: usize,
    pub floor_rungs: usize,
    /// Inclusive grid-rung rows, `horizon_row+1 ..= H-2`.
    pub grid_rows: (usize, usize),

    pub scenery_each_side: usize,
    pub lanes: usize,

    /// Sun disc, at the tetris horizon (SPEC section 4.3). Sub-pixel centre and
    /// radius are the values the renderer consumes; the cell-rounded pair is the
    /// rect-table view.
    pub sun_cx: f32,
    pub sun_cy_sub: f32,
    pub sun_r_sub: f32,
    pub sun_center_row: usize,
    pub sun_radius_rows: usize,

    /// Selector word centred here on the weld, dot strip one rung below.
    pub selector_center_col: usize,
    pub dot_strip_row: usize,

    /// Scenery sign anchors (top-left cell). HOLD is omitted on the narrowest
    /// terminals, where the flanks cannot carry it (SPEC rect table, 80×24).
    pub hold: Option<(usize, usize)>,
    pub next: (usize, usize),
    /// NEXT queue depth (SPEC rect table: 5 at 270, 3 at 120, 2 at 80).
    pub next_deep: usize,
    pub score: (usize, usize),
    pub lines: (usize, usize),
    pub level: (usize, usize),
    /// LINES/LVL share one small-text row instead of stacked 7-seg blocks when
    /// the flank is too short to carry three signs above the ticker.
    pub compact_stats: bool,

    pub ticker_row: usize,
}

/// The sun geometry for a given weld, SPEC section 4.3. Returned as
/// `(cx, cy_sub, r_sub)` in sub-pixels. Split out because the idle camera tilts
/// the weld and the sun rises/sets with it.
pub fn sun_at(w: usize, h: usize, horizon_row: usize) -> (f32, f32, f32) {
    let sh = (h * 2) as f32;
    let r = (0.26 * sh).min(0.16 * w as f32);
    let y_h = 2.0 * horizon_row as f32;
    let cx = w as f32 * 0.5;
    let cy = y_h - 0.10 * r;
    (cx, cy, r)
}

impl Layout {
    pub fn new(w: usize, h: usize) -> Layout {
        let ticker_row = h.saturating_sub(1);

        // Largest mino edge whose 20-mino stack still leaves room for the air,
        // the weld, the floor and the ticker (10·p rows plus 4 of chrome).
        let mino_px = [6usize, 4, 2]
            .into_iter()
            .find(|&p| 10 * p <= h.saturating_sub(4))
            .unwrap_or(2);

        let well_h = 10 * mino_px;
        let well_w = 10 * mino_px;

        // Rows left over once the stack, its weld and the ticker are placed.
        let leftover = h.saturating_sub(well_h + 2);
        let top_air = ((0.30 * leftover as f32).round() as usize).clamp(1, 6);
        let floor_rungs = leftover.saturating_sub(top_air);
        let horizon_row = top_air + well_h;
        let horizon_idle = (0.44 * h as f32).round() as usize;

        let well_x0 = ((w as f32 - well_w as f32) / 2.0).round() as usize;
        let scenery_each_side = (w - well_w) / 2;
        let lanes = ((w as f32 / 26.0).round() as usize).clamp(5, 22);

        let well = Rect {
            x0: well_x0,
            y0: top_air,
            x1: well_x0 + well_w - 1,
            y1: top_air + well_h - 1,
        };
        let grid_rows = (horizon_row + 1, h.saturating_sub(2));

        let (sun_cx, sun_cy_sub, sun_r_sub) = sun_at(w, h, horizon_row);

        // Scenery signs stand in the flanks, hung off the well edges so they
        // reflow with it. The rect table's exact sign cells are eyeballed from
        // the mockups and disagree between sizes; these anchors keep the labels
        // in the correct flank and clear of the well at every width.
        let well_x1 = well.x1;
        let sign_top = top_air.max(1);
        let next = (well_x1 + 4, sign_top);
        let next_deep = if h >= 50 {
            5
        } else if h >= 28 {
            3
        } else {
            2
        };
        let hold = if w >= 100 {
            let hw = mino_px * 2 + 4;
            Some((well_x0.saturating_sub(hw + 3).max(1), sign_top))
        } else {
            None
        };
        // The stats stack begins below the full NEXT queue so the 7-seg values
        // never collide with the queued minos (both hang off the same column).
        // Queue bottom in sub-rows: label (7) + (deep-1) gaps of (p+2) + one mino.
        let next_end_sub = 2 * sign_top + 7 + (next_deep - 1) * (mino_px + 2) + mino_px;
        let score = (well_x1 + 4, next_end_sub / 2 + 2);
        // A label + 7-seg block is exactly 7 rows: 5 sub-rows of small text at
        // the top, the digits 7 sub-rows below it, themselves 7 sub-rows tall.
        // Pitching the stack at that same 7 would put every label on the
        // previous value, so the pitch carries a row of air.
        const BLOCK_ROWS: usize = 7;
        let block = BLOCK_ROWS + 1;
        // Three stacked blocks reach `2*block + BLOCK_ROWS` below the first, and
        // must still clear the ticker; otherwise LINES/LVL fold onto one row.
        let compact_stats = score.1 + 2 * block + BLOCK_ROWS >= ticker_row;
        let (lines, level) = if compact_stats {
            ((score.0, score.1 + block), (score.0, score.1 + block))
        } else {
            ((score.0, score.1 + block), (score.0, score.1 + 2 * block))
        };

        Layout {
            w,
            h,
            mino_px,
            well,
            well_x0,
            well_w,
            well_h,
            horizon_row,
            horizon_idle,
            top_air,
            floor_rungs,
            grid_rows,
            scenery_each_side,
            lanes,
            sun_cx,
            sun_cy_sub,
            sun_r_sub,
            sun_center_row: (sun_cy_sub / 2.0).round() as usize,
            sun_radius_rows: (sun_r_sub / 2.0).round() as usize,
            selector_center_col: w / 2,
            dot_strip_row: horizon_row + 1,
            hold,
            next,
            next_deep,
            score,
            lines,
            level,
            compact_stats,
            ticker_row,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC section 2.6, the authoritative rect table. These are cheap and catch
    // any reflow regression the moment a coordinate drifts.

    #[test]
    fn matches_rect_table_80x24() {
        let l = Layout::new(80, 24);
        assert_eq!(l.mino_px, 2);
        assert_eq!(l.well, Rect { x0: 30, y0: 1, x1: 49, y1: 20 });
        assert_eq!(l.horizon_row, 21);
        assert_eq!(l.top_air, 1);
        assert_eq!(l.floor_rungs, 1);
        assert_eq!(l.grid_rows, (22, 22));
        assert_eq!(l.scenery_each_side, 30);
        assert_eq!(l.lanes, 5);
        assert_eq!(l.selector_center_col, 40);
        assert_eq!(l.dot_strip_row, 22);
        assert_eq!(l.ticker_row, 23);
        assert_eq!(l.sun_cx as usize, 40);
        // HOLD omitted on the narrowest terminal (rect table 80×24).
        assert!(l.hold.is_none());
    }

    #[test]
    fn matches_rect_table_120x30() {
        let l = Layout::new(120, 30);
        assert_eq!(l.mino_px, 2);
        assert_eq!(l.well, Rect { x0: 50, y0: 2, x1: 69, y1: 21 });
        assert_eq!(l.horizon_row, 22);
        assert_eq!(l.top_air, 2);
        assert_eq!(l.floor_rungs, 6);
        assert_eq!(l.grid_rows, (23, 28));
        assert_eq!(l.scenery_each_side, 50);
        assert_eq!(l.lanes, 5);
        assert_eq!(l.selector_center_col, 60);
        assert_eq!(l.dot_strip_row, 23);
        assert_eq!(l.ticker_row, 29);
        assert_eq!(l.sun_cx as usize, 60);
        assert!(l.hold.is_some());
    }

    #[test]
    fn matches_rect_table_270x62() {
        let l = Layout::new(270, 62);
        assert_eq!(l.mino_px, 4);
        assert_eq!(l.well, Rect { x0: 115, y0: 6, x1: 154, y1: 45 });
        assert_eq!(l.horizon_row, 46);
        assert_eq!(l.top_air, 6);
        assert_eq!(l.floor_rungs, 14);
        assert_eq!(l.grid_rows, (47, 60));
        assert_eq!(l.scenery_each_side, 115);
        assert_eq!(l.lanes, 10);
        assert_eq!(l.selector_center_col, 135);
        assert_eq!(l.dot_strip_row, 47);
        assert_eq!(l.ticker_row, 61);
        // SPEC section 4.3 sun lands exactly on the rect table here: (135,44)·16.
        assert_eq!(l.sun_cx as usize, 135);
        assert_eq!(l.sun_center_row, 44);
        assert_eq!(l.sun_radius_rows, 16);
        assert!(l.hold.is_some());
    }

    #[test]
    fn only_three_mino_sizes_ever_occur() {
        for h in 24..=120 {
            let p = Layout::new(80.max(2 * h), h).mino_px;
            assert!(p == 2 || p == 4 || p == 6, "mino_px {p} at H={h}");
        }
    }

    #[test]
    fn signs_sit_in_the_correct_flank() {
        for &(w, h) in &[(120, 30), (270, 62)] {
            let l = Layout::new(w, h);
            // NEXT and the score signs stand right of the well.
            assert!(l.next.0 > l.well.x1);
            assert!(l.score.0 > l.well.x1);
            // HOLD, when present, stands left of the well.
            if let Some((hx, _)) = l.hold {
                assert!(hx < l.well.x0);
            }
        }
    }

    #[test]
    fn well_is_centered_and_square_in_minos() {
        let l = Layout::new(200, 50);
        assert_eq!(l.well_w, l.well_h);
        assert_eq!(l.well_w, 10 * l.mino_px);
        // centred: equal scenery on both flanks (even leftover).
        assert_eq!(l.well.x0, l.scenery_each_side);
    }
}
