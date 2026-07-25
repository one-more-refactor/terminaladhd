//! Tetris core: a pure-logic tetromino well with a millisecond clock. No
//! rendering, no terminal — a caller drives [`Tetris::step`] with an [`Input`]
//! and a `dt`, then reads the board through the accessors a renderer needs.
//!
//! This is the clean re-express of the original `blocks.rs`, split into a rules
//! layer ([`rules`]), a scoring layer ([`scoring`]) and an input/lock state
//! machine ([`handling`]), all keyed on the [`Mino`] enum the Diorama
//! compositor already knows how to paint. The clock is the change that unlocks
//! everything: gravity accumulates in fractional rows (G-units) against real
//! elapsed time, so DAS/ARR/soft-drop/lock are all genuine milliseconds rather
//! than quantised 50 ms ticks.

pub mod handling;
pub mod rules;
pub mod scoring;
pub mod skin;

use std::time::Duration;

use crate::rng::Rng;

use handling::Handling;
use rules::{Board, Piece, BUFFER, COLS, ROWS, VISIBLE};
use scoring::{Action, Award};
pub use skin::Mino;

/// Pieces of lookahead the renderer draws.
const NEXT_SHOWN: usize = 5;

/// Lines cleared per level, and the wall-clock alternative — sitting on a tidy
/// board must not be a way to stay slow.
const LINES_PER_LEVEL: u32 = 5;
const LEVEL_TIME_SECS: u64 = 20;

/// The gravity curve is offset so play opens at Guideline level 4 (≈0.47 s/row)
/// rather than the punishing one-row-a-second of level 1 — the right pace for a
/// waiting room, still every value defensible against the spec curve.
const GRAVITY_LEVEL_OFFSET: u32 = 3;

/// Spawn the piece two rows above the skyline; one step of gravity brings it
/// into view. The buffer above the visible field is what makes this legal.
const SPAWN_Y: i32 = BUFFER as i32 - 2;

/// How long cleared rows sit lit before the stack collapses into the gap.
const CLEAR_DELAY: Duration = Duration::from_millis(200);

/// A cell of soft drop pays one point, a cell of hard drop two.
const SOFT_DROP_POINTS: u32 = 1;
const HARD_DROP_POINTS: u32 = 2;

/// Heat decays to nothing over this long; a hard drop nudges it, a clear or a
/// spin shoves it.
const HEAT_DECAY_SECS: f32 = 2.5;
/// Shake settles far faster — it is a recoil, not a mood.
const SHAKE_DECAY_SECS: f32 = 0.30;

/// One step's worth of input. Held keys (`left`/`right`/`soft`) stay true for
/// as long as they are down; the rest are edges — true only on the step the key
/// was pressed. Edge detection is the caller's job, since a cooked terminal
/// cannot always be trusted to report releases.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    pub left: bool,
    pub right: bool,
    pub soft: bool,
    pub cw: bool,
    pub ccw: bool,
    pub hard: bool,
    pub hold: bool,
}

pub struct Tetris {
    board: Board,
    piece: Piece,
    /// The pieces on deck, always [`NEXT_SHOWN`] long.
    queue: Vec<Mino>,
    hold: Option<Mino>,
    hold_used: bool,
    bag: Vec<Mino>,
    rng: Rng,
    handling: Handling,
    /// The offset the last rotation used, set only while the last thing that
    /// moved the piece was a rotation — the last-move rule for spins.
    spun: Option<usize>,
    /// Fractional rows of gravity accumulated but not yet applied.
    fall_accum: f32,
    score: u32,
    lines: u32,
    /// Consecutive line-clearing pieces; a clean lock breaks it.
    combo: u32,
    back_to_back: bool,
    elapsed: Duration,
    heat: f32,
    shake: f32,
    /// Matrix rows lit mid-clear; while any are present there is no live piece.
    clearing: Vec<usize>,
    clear_timer: Duration,
    last_action: Action,
    over: bool,
}

impl Tetris {
    pub fn new() -> Self {
        Self::with_rng(Rng::new())
    }

    pub fn with_rng(rng: Rng) -> Self {
        let mut game = Self {
            board: Board::new(),
            piece: Piece::new(Mino::I),
            queue: Vec::new(),
            hold: None,
            hold_used: false,
            bag: Vec::new(),
            rng,
            handling: Handling::new(SPAWN_Y),
            spun: None,
            fall_accum: 0.0,
            score: 0,
            lines: 0,
            combo: 0,
            back_to_back: false,
            elapsed: Duration::ZERO,
            heat: 0.0,
            shake: 0.0,
            clearing: Vec::new(),
            clear_timer: Duration::ZERO,
            last_action: Action::Nothing,
            over: false,
        };
        game.refill();
        game.spawn();
        game
    }

    // ---------------------------------------------------------------- the bag

    /// The 7-bag: a shuffle of all seven pieces, refilled when empty, so no
    /// piece can ever be withheld for long.
    fn deal(&mut self) -> Mino {
        if self.bag.is_empty() {
            self.bag.extend(skin::ORDER);
            for i in (1..self.bag.len()).rev() {
                let j = self.rng.range(i as u32 + 1) as usize;
                self.bag.swap(i, j);
            }
        }
        self.bag.pop().unwrap_or(Mino::I)
    }

    fn refill(&mut self) {
        while self.queue.len() < NEXT_SHOWN {
            let piece = self.deal();
            self.queue.push(piece);
        }
    }

    // ------------------------------------------------------------ spawn/place

    /// Put `kind` at the top of the well: above the skyline, then one step into
    /// view if there is room. Returns false when the spawn cell is already
    /// occupied — a Block-Out.
    fn place(&mut self, kind: Mino) -> bool {
        let mut piece = Piece {
            kind,
            rot: 0,
            x: rules::spawn_x(kind),
            y: SPAWN_Y,
        };
        self.spun = None;
        self.fall_accum = 0.0;
        if !self.board.fits(kind, 0, piece.x, piece.y) {
            self.piece = piece;
            self.handling.reset(piece.y);
            return false;
        }
        if self.board.fits(kind, 0, piece.x, piece.y + 1) {
            piece.y += 1;
        }
        self.piece = piece;
        self.handling.reset(piece.y);
        true
    }

    /// Bring on the head of the queue; false means the well is full.
    fn spawn(&mut self) -> bool {
        let kind = self.queue.remove(0);
        self.refill();
        self.place(kind)
    }

    /// Swap the live piece into the hold slot, once per piece. Returns false if
    /// the piece coming back finds the well full.
    fn swap_hold(&mut self) -> bool {
        if self.hold_used {
            return true;
        }
        self.hold_used = true;
        match self.hold.replace(self.piece.kind) {
            Some(kind) => self.place(kind),
            None => self.spawn(),
        }
    }

    // ------------------------------------------------------------ the level

    pub fn level(&self) -> u32 {
        let by_lines = self.lines / LINES_PER_LEVEL;
        let by_time = (self.elapsed.as_secs() / LEVEL_TIME_SECS) as u32;
        1 + by_lines.max(by_time)
    }

    /// Seconds a piece takes to fall one row under gravity, on the Guideline
    /// curve offset to open at level 4.
    pub fn fall_seconds(&self) -> f32 {
        let l = (self.level() + GRAVITY_LEVEL_OFFSET) as f32;
        let base = (0.8 - (l - 1.0) * 0.007).max(0.001);
        base.powf(l - 1.0).max(0.0001)
    }

    // ------------------------------------------------------------ the clock

    /// Advance the game by `dt`, applying `input`. Held directions auto-shift,
    /// gravity accumulates in fractional rows, and a grounded piece locks after
    /// its window. Rotations, hold and hard drop are edges.
    pub fn step(&mut self, input: &Input, dt: Duration) {
        if self.over {
            return;
        }
        self.elapsed = self.elapsed.saturating_add(dt);
        let dts = dt.as_secs_f32();
        self.heat = (self.heat - dts / HEAT_DECAY_SECS).max(0.0);
        self.shake = (self.shake - dts / SHAKE_DECAY_SECS).max(0.0);

        // A clear freezes the world for its window, then collapses and reloads.
        if !self.clearing.is_empty() {
            self.clear_timer = self.clear_timer.saturating_sub(dt);
            if self.clear_timer.is_zero() {
                self.finish_clear();
            }
            return;
        }

        if input.cw {
            self.try_rotate(true);
        }
        if input.ccw {
            self.try_rotate(false);
        }
        if input.hold && !self.swap_hold() {
            self.over = true;
            return;
        }
        if input.hard {
            self.heat = (self.heat + 0.1).min(1.0);
            while self.step_down() {
                self.score += HARD_DROP_POINTS;
            }
            self.lock();
            return;
        }

        // Horizontal auto-shift: the handling machine says how many single-cell
        // shifts are owed this step; apply them until a wall stops us.
        let dir = match (input.left, input.right) {
            (true, false) => -1,
            (false, true) => 1,
            _ => 0,
        };
        let count = self.handling.autoshift(dir, dt);
        for _ in 0..count {
            if !self.try_shift(dir) {
                break;
            }
        }

        self.apply_gravity(input.soft, dts);
        self.settle(dt);
    }

    /// Gravity (or soft drop) as fractional rows: accumulate `dt / interval`
    /// and fall a whole row for each unit, capped so a huge `dt` cannot loop
    /// past the floor. Soft-drop rows score; gravity rows do not.
    fn apply_gravity(&mut self, soft: bool, dts: f32) {
        let gravity = Duration::from_secs_f32(self.fall_seconds());
        let interval = if soft {
            handling::soft_interval(gravity)
        } else {
            gravity
        };
        self.fall_accum += dts / interval.as_secs_f32().max(1e-4);
        let mut rows = self.fall_accum.floor();
        if rows > ROWS as f32 {
            rows = ROWS as f32;
        }
        self.fall_accum -= rows;
        for _ in 0..rows as i32 {
            if self.step_down() {
                if soft {
                    self.score += SOFT_DROP_POINTS;
                }
            } else {
                self.fall_accum = 0.0;
                break;
            }
        }
    }

    /// Grounded pieces run down the lock timer; airborne ones clear it.
    fn settle(&mut self, dt: Duration) {
        if self.grounded() {
            if self.handling.ground(dt) {
                self.lock();
            }
        } else {
            self.handling.unground();
        }
    }

    fn grounded(&self) -> bool {
        !self
            .board
            .fits(self.piece.kind, self.piece.rot, self.piece.x, self.piece.y + 1)
    }

    /// Move the piece down one row if it fits. A move down is never a spin and
    /// refunds resets on reaching a new low.
    fn step_down(&mut self) -> bool {
        if self
            .board
            .fits(self.piece.kind, self.piece.rot, self.piece.x, self.piece.y + 1)
        {
            self.piece.y += 1;
            self.spun = None;
            self.handling.descend(self.piece.y);
            true
        } else {
            false
        }
    }

    fn try_shift(&mut self, dx: i32) -> bool {
        if self
            .board
            .fits(self.piece.kind, self.piece.rot, self.piece.x + dx, self.piece.y)
        {
            self.piece.x += dx;
            self.spun = None;
            let grounded = self.grounded();
            self.handling.touch(grounded);
            true
        } else {
            false
        }
    }

    fn try_rotate(&mut self, clockwise: bool) {
        if let Some(index) = rules::rotate(&self.board, &mut self.piece, clockwise) {
            self.spun = Some(index);
            let grounded = self.grounded();
            self.handling.touch(grounded);
        }
    }

    // ------------------------------------------------------------ locking

    /// Stamp the piece into the board, score the lock as one [`Action`], and
    /// either begin a line-clear or bring on the next piece.
    fn lock(&mut self) {
        let spin = rules::classify(&self.board, &self.piece, self.spun);
        let kind = self.piece.kind;
        let cells = self.piece.cells();
        for (col, row) in cells {
            if (0..COLS as i32).contains(&col) && (0..ROWS as i32).contains(&row) {
                self.board.cells[row as usize][col as usize] = Some(kind);
            }
        }
        self.hold_used = false;

        // Lock-Out: a piece that came to rest entirely above the skyline ends
        // the game (unless it cleared a line on the way).
        let lock_out = cells.iter().all(|&(_, row)| (row as usize) < BUFFER);

        let rows = self.board.full_rows();
        let cleared = !rows.is_empty();
        self.combo = if cleared { self.combo + 1 } else { 0 };

        let action = Action::classify(kind, spin, rows.len() as u32);
        let perfect = cleared && self.board.perfect_clear(&rows);
        let Award {
            points,
            back_to_back,
        } = scoring::award(action, self.level(), self.back_to_back, self.combo, perfect);
        self.score += points;
        self.back_to_back = back_to_back;
        self.last_action = action;
        self.bump(action, perfect);

        if !cleared {
            if lock_out || !self.spawn() {
                self.over = true;
            }
            return;
        }
        self.clearing = rows;
        self.clear_timer = CLEAR_DELAY;
    }

    fn finish_clear(&mut self) {
        let rows = std::mem::take(&mut self.clearing);
        self.board.collapse(&rows);
        self.lines += rows.len() as u32;
        if !self.spawn() {
            self.over = true;
        }
    }

    /// Raise heat and shake for what just happened. Heat carries the felt
    /// momentum, shake the impact; a single-line clear gets no shake at all.
    fn bump(&mut self, action: Action, perfect: bool) {
        let base = match action {
            Action::Nothing => 0.0,
            Action::LineClear(4) => 0.6,
            Action::LineClear(n) => 0.12 * n as f32,
            Action::TSpin { lines, .. } => 0.45 + 0.12 * lines as f32,
            Action::AllSpin(lines) => 0.40 + 0.12 * lines as f32,
        };
        let combo = 0.05 * self.combo as f32;
        let pc = if perfect { 0.5 } else { 0.0 };
        self.heat = (self.heat + base + combo + pc).min(1.0);

        let shake = if perfect {
            3.0
        } else {
            match action {
                Action::LineClear(4) => 2.0,
                Action::TSpin { lines, .. } if lines > 0 => 1.5,
                Action::LineClear(2) | Action::LineClear(3) => 1.0,
                Action::AllSpin(lines) if lines > 0 => 1.0,
                _ => 0.0,
            }
        };
        self.shake = self.shake.max(shake);
    }

    // ------------------------------------------------------ renderer readouts

    /// The visible 20×10 field as settled colours; the live piece is not merged
    /// in (see [`Tetris::active`]).
    pub fn cells(&self) -> [[Option<Mino>; COLS]; VISIBLE] {
        let mut out = [[None; COLS]; VISIBLE];
        for (r, row) in out.iter_mut().enumerate() {
            *row = self.board.cells[BUFFER + r];
        }
        out
    }

    /// The live piece kind and its cells in visible coordinates (column, row),
    /// where a negative row is still above the field, raining in. `None` during
    /// a clear or after game over.
    pub fn active(&self) -> Option<(Mino, [(i32, i32); 4])> {
        if self.over || !self.clearing.is_empty() {
            return None;
        }
        let cells = self.piece.cells().map(|(c, r)| (c, r - BUFFER as i32));
        Some((self.piece.kind, cells))
    }

    /// The landing footprint of the live piece, in visible coordinates.
    pub fn ghost(&self) -> Option<[(i32, i32); 4]> {
        if self.over || !self.clearing.is_empty() {
            return None;
        }
        let gy = self.piece.ghost_y(&self.board);
        let cells = rules::cells(self.piece.kind, self.piece.rot)
            .map(|(cx, cy)| (self.piece.x + cx as i32, gy + cy as i32 - BUFFER as i32));
        Some(cells)
    }

    pub fn hold(&self) -> Option<Mino> {
        self.hold
    }

    /// Whether the hold slot can be used again this piece.
    pub fn hold_ready(&self) -> bool {
        !self.hold_used
    }

    pub fn next(&self) -> &[Mino] {
        &self.queue
    }

    pub fn score(&self) -> u32 {
        self.score
    }

    pub fn lines(&self) -> u32 {
        self.lines
    }

    pub fn combo(&self) -> u32 {
        self.combo
    }

    pub fn back_to_back(&self) -> bool {
        self.back_to_back
    }

    /// How hot the player is, 0.0..=1.0 — rising on clears, drops, spins and
    /// combos, decaying over ~2.5 s.
    pub fn heat(&self) -> f32 {
        self.heat.clamp(0.0, 1.0)
    }

    /// Current screen-shake amount in whole cells.
    pub fn shake(&self) -> i32 {
        self.shake.round() as i32
    }

    /// Visible rows currently lit mid-clear.
    pub fn clearing_rows(&self) -> Vec<usize> {
        self.clearing
            .iter()
            .filter(|&&row| row >= BUFFER)
            .map(|&row| row - BUFFER)
            .collect()
    }

    /// Fraction of the lock window elapsed for a grounded piece, for a
    /// grounded-piece ghost pulse; zero when airborne.
    pub fn lock_phase(&self) -> f32 {
        if self.grounded() {
            self.handling.lock_phase()
        } else {
            0.0
        }
    }

    pub fn last_action(&self) -> Action {
        self.last_action
    }

    pub fn is_over(&self) -> bool {
        self.over
    }
}

impl Default for Tetris {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOOR: usize = ROWS - 1;

    fn dur(secs: f32) -> Duration {
        Duration::from_secs_f32(secs)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn none() -> Input {
        Input::default()
    }

    fn hard() -> Input {
        Input {
            hard: true,
            ..Input::default()
        }
    }

    /// A deterministic game holding a chosen live piece with a known queue.
    fn rig(kind: Mino, next: Mino) -> Tetris {
        let mut game = Tetris::with_rng(Rng::from_seed(7));
        game.queue = vec![next, Mino::O, Mino::T, Mino::S, Mino::Z];
        game.place(kind);
        game
    }

    #[test]
    fn the_bag_deals_each_piece_once_and_the_queue_shows_five() {
        let mut game = Tetris::with_rng(Rng::from_seed(42));
        assert_eq!(game.next().len(), NEXT_SHOWN);
        let mut seen: Vec<Mino> = vec![game.piece.kind];
        seen.extend(game.queue.iter().copied());
        while seen.len() < 7 {
            seen.push(game.deal());
        }
        let mut idx: Vec<usize> = seen.iter().map(|&m| skin::index(m)).collect();
        idx.sort_unstable();
        assert_eq!(idx, vec![0, 1, 2, 3, 4, 5, 6], "the opening bag holds each piece once");
    }

    #[test]
    fn gravity_accumulates_in_fractional_rows_against_dt() {
        let mut game = rig(Mino::O, Mino::T);
        let y0 = game.piece.y;
        let spr = game.fall_seconds();
        game.step(&none(), dur(spr * 0.5));
        assert_eq!(game.piece.y, y0, "half a row of gravity does not move it");
        game.step(&none(), dur(spr * 0.6));
        assert_eq!(game.piece.y, y0 + 1, "the accumulated row falls once it fills");
    }

    #[test]
    fn soft_drop_is_much_faster_than_gravity_and_scores() {
        let mut game = rig(Mino::O, Mino::T);
        let y0 = game.piece.y;
        let spr = game.fall_seconds();
        let input = Input {
            soft: true,
            ..Input::default()
        };
        // Half a natural row's worth of time drops many rows under soft.
        game.step(&input, dur(spr * 0.5));
        assert!(game.piece.y > y0 + 1, "soft drop covers several rows at once");
        assert!(game.score > 0, "soft drop rows score a point each");
    }

    #[test]
    fn hard_drop_scores_two_a_row_and_locks_at_once() {
        let mut game = rig(Mino::O, Mino::T);
        game.step(&hard(), ms(16));
        assert!(game.board.cells[FLOOR][4].is_some() && game.board.cells[FLOOR][5].is_some());
        assert_eq!(game.piece.kind, Mino::T, "locking brings on the next piece");
        assert!(game.score > 0);
    }

    #[test]
    fn a_completed_row_lights_then_collapses_and_scores() {
        let mut game = rig(Mino::O, Mino::T);
        for col in 0..8 {
            game.board.cells[FLOOR][col] = Some(Mino::I);
        }
        game.piece.x = 8;
        let before = game.score;
        game.step(&hard(), ms(16));
        assert_eq!(game.clearing.len(), 1, "the full row is held lit");
        assert_eq!(game.lines, 0, "and not yet collapsed");
        assert!(
            game.score - before >= scoring::LINE_POINTS[0],
            "a single scores its line points"
        );
        // Sit through the clear window; the row collapses and the O's top half
        // falls into the gap.
        game.step(&none(), CLEAR_DELAY);
        assert_eq!(game.lines, 1);
        assert!(game.clearing.is_empty());
        assert!(game.board.cells[FLOOR][8].is_some() && game.board.cells[FLOOR][9].is_some());
        assert!(game.board.cells[FLOOR][0].is_none());
    }

    #[test]
    fn a_back_to_back_tetris_pays_half_again_through_the_clock() {
        let mut game = rig(Mino::I, Mino::I);
        // Stage and clear a first tetris off an I stood on its end. Locking by
        // the lock delay (rather than a hard drop) keeps drop points out of the
        // score so the awards can be checked exactly.
        stage_tetris(&mut game);
        lock_by_delay(&mut game);
        assert_eq!(game.clearing.len(), 4);
        assert!(game.back_to_back, "the first tetris arms the chain");
        assert_eq!(game.score, 800, "the first tetris is flat rate at level 1");
        game.step(&none(), CLEAR_DELAY);
        assert_eq!(game.lines, 4);

        // A second tetris, still level 1: 800×1.5 plus one combo link.
        stage_tetris(&mut game);
        let before = game.score;
        lock_by_delay(&mut game);
        let paid = game.score - before;
        assert_eq!(paid, 1200 + scoring::COMBO_POINTS, "b2b tetris + one combo link");
    }

    /// Fill the bottom four rows bar the last column and stand an I on end in
    /// the trench, plus a lone leftover block so clearing the four does not also
    /// perfect-clear the board.
    fn stage_tetris(game: &mut Tetris) {
        game.board = Board::new();
        for row in ROWS - 4..ROWS {
            for col in 0..COLS - 1 {
                game.board.cells[row][col] = Some(Mino::Z);
            }
        }
        game.board.cells[ROWS - 6][0] = Some(Mino::Z); // survives the clear
        game.place(Mino::I);
        // The vertical I is a bar in box-column 2, so x = 7 lands it in the
        // open last column.
        game.piece.rot = 1;
        game.piece.x = 7;
        game.piece.y = SPAWN_Y;
    }

    /// Send the live piece to its landing row and hold it there until the lock
    /// delay expires, without any drop scoring.
    fn lock_by_delay(game: &mut Tetris) {
        game.piece.y = game.piece.ghost_y(&game.board);
        game.step(&none(), handling::LOCK_DELAY);
    }

    #[test]
    fn the_touch_fix_bounds_a_long_slide_but_lets_it_buy_time() {
        // A piece resting on the floor, shoved left/right every step. Each
        // grounded move must cost a reset even when the lock timer is cold, or
        // the slide is infinite; the resets are capped, so it still locks.
        let mut game = rig(Mino::O, Mino::T);
        game.piece.y = (ROWS - 2) as i32; // O resting on the floor, grounded
        let mut steps = 0;
        while !game.board.cells[FLOOR].iter().any(|c| c.is_some()) {
            let input = Input {
                left: steps % 2 == 0,
                right: steps % 2 == 1,
                ..Input::default()
            };
            game.step(&input, ms(16));
            steps += 1;
            assert!(steps < 300, "the piece never locked — a reset leaked");
        }
        assert!(steps > handling::LOCK_RESETS, "sliding bought no lock time at all");
    }

    #[test]
    fn hold_takes_the_piece_and_returns_it_on_the_next_hold() {
        let mut game = rig(Mino::T, Mino::I);
        let held = Input {
            hold: true,
            ..Input::default()
        };
        game.step(&held, ms(16));
        assert_eq!(game.hold, Some(Mino::T));
        assert_eq!(game.piece.kind, Mino::I);
        assert!(!game.hold_ready());
        // A second hold this piece is refused.
        game.step(&held, ms(16));
        assert_eq!(game.piece.kind, Mino::I);
    }

    #[test]
    fn a_block_out_at_spawn_ends_the_game() {
        let mut game = rig(Mino::O, Mino::O);
        // Block the spawn columns so the next piece cannot appear — but not the
        // whole row, or it would read as a line to clear instead.
        for col in 3..7 {
            game.board.cells[SPAWN_Y as usize][col] = Some(Mino::I);
        }
        game.step(&hard(), ms(16));
        assert!(game.is_over());
    }

    #[test]
    fn heat_rises_on_a_clear_and_decays_back_down() {
        let mut game = rig(Mino::O, Mino::T);
        assert!(game.heat() < 0.05, "a fresh board is cold");
        for col in 0..8 {
            game.board.cells[FLOOR][col] = Some(Mino::I);
        }
        game.piece.x = 8;
        game.step(&hard(), ms(16));
        let hot = game.heat();
        assert!(hot > 0.1, "a clear lights it: {hot}");
        assert!((0.0..=1.0).contains(&hot));
        // Let time pass with nothing happening: it cools.
        game.step(&none(), CLEAR_DELAY);
        for _ in 0..40 {
            game.step(&none(), ms(50));
        }
        assert!(game.heat() < hot, "heat has to decay when the player idles");
    }

    #[test]
    fn the_renderer_readouts_stay_consistent() {
        let mut game = rig(Mino::T, Mino::I);
        assert_eq!(game.next().len(), NEXT_SHOWN);
        let (kind, cells) = game.active().expect("a live piece");
        assert_eq!(kind, Mino::T);
        // The active cells sit within the visible field or just above it.
        for (col, row) in cells {
            assert!((0..COLS as i32).contains(&col));
            assert!(row < VISIBLE as i32);
        }
        // The ghost never sits above the active piece.
        let ghost = game.ghost().expect("a ghost");
        let active_bottom = cells.iter().map(|&(_, r)| r).max().unwrap();
        let ghost_bottom = ghost.iter().map(|&(_, r)| r).max().unwrap();
        assert!(ghost_bottom >= active_bottom);
    }
}
