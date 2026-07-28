//! BLOCKS — a Guideline-shaped tetromino well on a millisecond clock: 7-bag,
//! hold, three of lookahead, SRS kicks, T-spins and all-spins, back-to-back
//! and combo. The rules live in [`rules`], the awards in [`scoring`], and the
//! DAS/lock state machine in [`handling`]; this module is the game that plays
//! them and draws itself onto the canvas.
//!
//! The clock matters: gravity accumulates in fractional rows against real
//! elapsed time, so DAS, auto-repeat, soft drop and the lock delay are all
//! genuine milliseconds rather than quantised ticks.

pub mod handling;
pub mod rules;
pub mod scoring;
pub mod skin;

use std::time::Duration;

use crate::games::{Game, Input, Kind};
use crate::rng::Rng;
use crate::screen::{self, Screen};

use handling::Handling;
use rules::{Board, Piece, BUFFER, COLS, ROWS, VISIBLE};
use scoring::{Action, Award};
pub use skin::Mino;

/// Pieces of lookahead the panel shows. Two is enough to place the current
/// one, three is enough to plan a well — and three is what the panel has room
/// for.
const NEXT_SHOWN: usize = 3;

/// Lines cleared per level, and the wall-clock alternative — sitting on a tidy
/// board must not be a way to stay slow.
const LINES_PER_LEVEL: u32 = 5;
const LEVEL_TIME_SECS: u64 = 25;

/// The gravity curve, as a geometric fall from an opening pace to a floor.
///
/// The Guideline curve was the wrong shape for this. It is built for a
/// marathon — gentle for a long time, then unplayable — and this is two
/// minutes while a build runs. This one opens brisk enough to be a game from
/// the first piece, gives up a fixed share of what is left every level so the
/// early steps are felt and the late ones are not, and settles somewhere a
/// person can keep playing. Dying is then something the player did rather
/// than something the clock did.
const FALL_OPEN: f32 = 0.30;
const FALL_REST: f32 = 0.10;
const FALL_DECAY: f32 = 0.72;

/// Spawn the piece two rows above the skyline; one step of gravity brings it
/// into view. The buffer above the visible field is what makes this legal.
const SPAWN_Y: i32 = BUFFER as i32 - 2;

/// How long cleared rows sit lit before the stack collapses into the gap —
/// plus a share per extra line, so a tetris holds the screen longer than a
/// single.
const CLEAR_DELAY: Duration = Duration::from_millis(180);
const CLEAR_PER_LINE: Duration = Duration::from_millis(60);

/// A cell of soft drop pays one point, a cell of hard drop two.
const SOFT_DROP_POINTS: u32 = 1;
const HARD_DROP_POINTS: u32 = 2;

/// Heat decays to nothing over this long; a hard drop nudges it, a clear or a
/// spin shoves it.
const HEAT_DECAY_SECS: f32 = 2.5;

/// Render frames the machine freezes for. A lock that clears nothing is worth
/// nothing; a tetris and a top-out are worth the 150 ms an impact wants before
/// the eye stops reading it as one.
const HITSTOP_CLEAR: u32 = 4;
const HITSTOP_BIG: u32 = 8;
const HITSTOP_OVER: u32 = 10;

/// How long a `+N` marker stays in the air.
const POP_SECS: f32 = 0.45;

/// How long the death curtain takes to fill the well from the floor up — the
/// classic cabinet game-over, and the beat that separates the crash from the
/// score screen.
const DEATH_SECS: f32 = 0.6;

/// A `+N` rising from the lock that earned it.
struct Pop {
    x: i32,
    y: i32,
    points: u32,
    age: f32,
}

pub struct Blocks {
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
    ticks: f32,
    heat: f32,
    /// Matrix rows lit mid-clear; while any are present there is no live piece.
    clearing: Vec<usize>,
    clear_timer: Duration,
    last_action: Action,
    pops: Vec<Pop>,
    hitstop: u32,
    flash_out: f32,
    over: bool,
    /// Seconds into the death curtain once dead.
    dying: f32,
}

impl Blocks {
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
            ticks: 0.0,
            heat: 0.0,
            clearing: Vec::new(),
            clear_timer: Duration::ZERO,
            last_action: Action::Nothing,
            pops: Vec::new(),
            hitstop: 0,
            flash_out: 0.0,
            over: false,
            dying: 0.0,
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

    /// Seconds a piece takes to fall one row under gravity.
    pub fn fall_seconds(&self) -> f32 {
        let steps = (self.level() - 1).min(40) as i32;
        FALL_REST + (FALL_OPEN - FALL_REST) * FALL_DECAY.powi(steps)
    }

    // ------------------------------------------------------------ the clock

    /// Advance the game by `dt`, applying `input`. Held directions auto-shift,
    /// gravity accumulates in fractional rows, and a grounded piece locks
    /// after its window. Rotations, hold and hard drop are edges.
    fn advance(&mut self, input: &Input, dt: Duration) {
        let dts = dt.as_secs_f32();
        self.ticks += dts;
        self.heat = (self.heat - dts / HEAT_DECAY_SECS).max(0.0);
        for p in &mut self.pops {
            p.age += dts;
        }
        self.pops.retain(|p| p.age < POP_SECS);
        if self.over {
            self.dying += dts;
            return;
        }
        self.elapsed = self.elapsed.saturating_add(dt);

        // A clear freezes the well for its window, then collapses and reloads.
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
            self.die();
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

        // Horizontal auto-shift: the handling machine says how many
        // single-cell shifts are owed this step; apply them until a wall
        // stops us.
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

        self.apply_gravity(input.down, dts);
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
        self.fall_accum = 0.0;
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
        if points > 0 {
            // Off the piece that earned them, so the marker points at the lock
            // rather than at the middle of the well.
            let (sx, sy) = cells
                .iter()
                .fold((0i32, 0i32), |(ax, ay), &(cx, cy)| (ax + cx, ay + cy));
            let col = sx / cells.len() as i32;
            let row = (sy / cells.len() as i32 - BUFFER as i32).max(0);
            self.pops.push(Pop {
                x: WELL_X + col * 2,
                y: WELL_Y + row * 2,
                points,
                age: 0.0,
            });
        }
        if cleared {
            let huge = rows.len() >= 4 || perfect;
            self.hitstop = self.hitstop.max(if huge { HITSTOP_BIG } else { HITSTOP_CLEAR });
            // The flash is the machine's loud channel, and it is saved for the
            // moments that earn it: a tetris or a perfect clear blows the tube
            // out, a cleared spin most of the way — it is the hardest thing in
            // the game to set up and the easiest to miss — a triple noticeably,
            // and the everyday clears not at all.
            let spun = !matches!(action, Action::LineClear(_));
            self.flash_out = self.flash_out.max(match () {
                _ if huge => 1.0,
                _ if spun => 0.6,
                _ if rows.len() == 3 => 0.4,
                _ => 0.0,
            });
        }
        self.back_to_back = back_to_back;
        self.last_action = action;
        let combo_heat = 0.05 * self.combo as f32;
        let base = match action {
            Action::Nothing => 0.0,
            Action::LineClear(4) => 0.6,
            Action::LineClear(n) => 0.12 * n as f32,
            Action::TSpin { lines, .. } => 0.45 + 0.12 * lines as f32,
            Action::AllSpin(lines) => 0.40 + 0.12 * lines as f32,
        };
        self.heat = (self.heat + base + combo_heat + if perfect { 0.5 } else { 0.0 }).min(1.0);

        if !cleared {
            if lock_out || !self.spawn() {
                self.die();
            }
            return;
        }
        self.clearing = rows;
        self.clear_timer = CLEAR_DELAY + CLEAR_PER_LINE * (self.clearing.len() as u32 - 1);
    }

    fn finish_clear(&mut self) {
        let rows = std::mem::take(&mut self.clearing);
        self.board.collapse(&rows);
        self.lines += rows.len() as u32;
        if !self.spawn() {
            self.die();
        }
    }

    fn die(&mut self) {
        if self.over {
            return;
        }
        self.over = true;
        self.dying = 0.0;
        self.hitstop = self.hitstop.max(HITSTOP_OVER);
    }

    // ------------------------------------------------------ renderer readouts

    /// The visible 20×10 field as settled cells; the live piece is not merged
    /// in (see [`Blocks::active`]).
    pub fn cells(&self) -> [[Option<Mino>; COLS]; VISIBLE] {
        let mut out = [[None; COLS]; VISIBLE];
        for (r, row) in out.iter_mut().enumerate() {
            *row = self.board.cells[BUFFER + r];
        }
        out
    }

    /// The live piece kind and its cells in visible coordinates (column, row),
    /// where a negative row is still above the field, raining in. `None`
    /// during a clear or after game over.
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

    pub fn lines(&self) -> u32 {
        self.lines
    }

    pub fn combo(&self) -> u32 {
        self.combo
    }

    pub fn back_to_back(&self) -> bool {
        self.back_to_back
    }

    pub fn last_action(&self) -> Action {
        self.last_action
    }

    /// Visible rows currently lit mid-clear.
    pub fn clearing_rows(&self) -> Vec<usize> {
        self.clearing
            .iter()
            .filter(|&&row| row >= BUFFER)
            .map(|&row| row - BUFFER)
            .collect()
    }
}

// ------------------------------------------------------------------ drawing

/// The well: 10×20 cells of 2×2 pixels inside a 1px border, on the left of
/// the canvas. Row 0 is the shell's wrap rule.
const WELL_X: i32 = 1;
const WELL_Y: i32 = 2;

/// The two panel columns to the right of the well, each a label with its
/// contents under it, so the eye only ever tracks two verticals.
const HOLD_X: i32 = 28;
const NEXT_X: i32 = 56;
/// Top of the first preview under either label, and the pitch between queued
/// pieces.
const PREVIEW_Y: i32 = 10;
const PREVIEW_STEP: i32 = 9;
/// Baselines of the readouts under the hold slot, and of the combo badge
/// under the queue.
const LEVEL_Y: i32 = 24;
const LINES_Y: i32 = 32;
const COMBO_Y: i32 = 24;
/// The strip under the well: score at the left, back-to-back at the right.
const STRIP_Y: i32 = 43;

impl Blocks {
    fn paint(&self, s: &mut Screen) {
        s.rect(WELL_X - 1, WELL_Y - 1, COLS as u32 * 2 + 2, VISIBLE as u32 * 2 + 2);

        // The settled stack, solid. A texture was tried here — a knocked-out
        // corner per cell — and at two pixels a cell it reads as static, not
        // masonry. At this scale a cell is either there or it is not.
        let cells = self.cells();
        for (row, line) in cells.iter().enumerate() {
            for (col, cell) in line.iter().enumerate() {
                if cell.is_some() {
                    let (x, y) = well_px(col as i32, row as i32);
                    s.fill_rect(x, y, 2, 2, true);
                }
            }
        }

        // Rows mid-clear blink solid/hollow as their window runs down.
        let blink = ((self.clear_timer.as_secs_f32() / 0.07) as u32).is_multiple_of(2);
        for row in self.clearing_rows() {
            let (x, y) = well_px(0, row as i32);
            s.fill_rect(x, y, COLS as u32 * 2, 2, blink);
        }

        // The ghost as one pixel per cell: present enough to aim by, faint
        // enough that it can never be mistaken for the piece.
        if let Some(ghost) = self.ghost() {
            for (col, row) in ghost {
                if row >= 0 {
                    let (x, y) = well_px(col, row);
                    s.set(x, y, true);
                }
            }
        }

        // The live piece, solid — the one thing on the field with no texture,
        // which is what makes it the live one.
        if let Some((_, active)) = self.active() {
            for (col, row) in active {
                if row >= 0 {
                    let (x, y) = well_px(col, row);
                    s.fill_rect(x, y, 2, 2, true);
                }
            }
        }

        // The death curtain: the well fills from the floor up, the way a
        // cabinet said game over before it had words for it.
        if self.over {
            let rows = ((self.dying / DEATH_SECS) * VISIBLE as f32) as i32;
            for row in (VISIBLE as i32 - rows).max(0)..VISIBLE as i32 {
                let (x, y) = well_px(0, row);
                s.fill_rect(x, y, COLS as u32 * 2, 2, true);
            }
        }

        // The two panels.
        s.text(HOLD_X, 2, "HOLD");
        if let Some(kind) = self.hold {
            // Hollow while spent: the slot shows what it holds and whether it
            // can be asked for it.
            piece_sprite(s, HOLD_X, PREVIEW_Y, kind, self.hold_ready());
        }
        s.text(HOLD_X, LEVEL_Y, &format!("LV {}", self.level()));
        s.text(HOLD_X, LINES_Y, &format!("LN {}", self.lines));

        s.text(NEXT_X, 2, "NEXT");
        for (i, &kind) in self.queue.iter().take(NEXT_SHOWN).enumerate() {
            piece_sprite(s, NEXT_X, PREVIEW_Y + i as i32 * PREVIEW_STEP, kind, true);
        }
        if self.combo >= 2 {
            s.text(HOLD_X, COMBO_Y - 8, &format!("X{}", self.combo));
        }

        // The strip: score on the left, the back-to-back flag on the right,
        // one baseline so the bottom edge reads as a single line.
        s.text(WELL_X, STRIP_Y, &self.score.to_string());
        if self.back_to_back {
            s.text(screen::W as i32 - screen::text_width("B2B") - 1, STRIP_Y, "B2B");
        }

        // Markers last, so they stay legible over whatever fired them.
        for p in &self.pops {
            let label = format!("+{}", p.points);
            let w = screen::text_width(&label);
            let x = (p.x - w / 2).clamp(1, screen::W as i32 - w - 1);
            let y = (p.y - (p.age / 0.06) as i32).max(1);
            s.text(x, y, &label);
        }
    }
}

/// Pixel corner of well cell (col, row).
fn well_px(col: i32, row: i32) -> (i32, i32) {
    (WELL_X + col * 2, WELL_Y + row * 2)
}

/// A piece in a panel at 2×2 px a cell; hollow when it is not available.
fn piece_sprite(s: &mut Screen, x: i32, y: i32, kind: Mino, solid: bool) {
    for (cx, cy) in rules::cells(kind, 0) {
        let (px, py) = (x + cx as i32 * 2, y + cy as i32 * 2);
        if solid {
            s.fill_rect(px, py, 2, 2, true);
        } else {
            s.set(px, py, true);
            s.set(px + 1, py + 1, true);
        }
    }
}

impl Default for Blocks {
    fn default() -> Self {
        Self::new()
    }
}

impl Game for Blocks {
    fn kind(&self) -> Kind {
        Kind::Blocks
    }

    fn step(&mut self, input: &Input, dt: Duration) {
        self.advance(input, dt);
    }

    fn is_over(&self) -> bool {
        self.over && self.dying >= DEATH_SECS
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn heat(&self) -> f32 {
        self.heat.clamp(0.0, 1.0)
    }

    fn take_hitstop(&mut self) -> u32 {
        std::mem::take(&mut self.hitstop)
    }

    fn take_flash(&mut self) -> f32 {
        std::mem::take(&mut self.flash_out)
    }

    /// Walk the piece toward the shallowest column and drop it. Not a solver —
    /// it will bury itself eventually — but it stacks flat for long enough to
    /// look like play, which is all a demo owes anyone.
    fn autopilot(&self) -> Input {
        let Some((_, cells)) = self.active() else {
            return Input::default();
        };
        let well = self.cells();
        let depth = |col: usize| {
            (0..VISIBLE)
                .position(|r| well[r][col].is_some())
                .unwrap_or(VISIBLE)
        };
        let target = (0..COLS).max_by_key(|&c| depth(c)).unwrap_or(0) as i32;
        let left = cells.iter().map(|&(c, _)| c).min().unwrap_or(0);
        match left.cmp(&target) {
            std::cmp::Ordering::Greater => Input {
                left: true,
                ..Default::default()
            },
            std::cmp::Ordering::Less => Input {
                right: true,
                ..Default::default()
            },
            std::cmp::Ordering::Equal => Input {
                hard: true,
                ..Default::default()
            },
        }
    }

    fn draw(&self, s: &mut Screen) {
        self.paint(s);
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
    fn rig(kind: Mino, next: Mino) -> Blocks {
        let mut game = Blocks::with_rng(Rng::from_seed(7));
        game.queue = vec![next, Mino::O, Mino::T];
        game.place(kind);
        game
    }

    #[test]
    fn the_well_and_panels_fit_the_canvas() {
        // The well border, the panel columns and the strip all live inside
        // 80×48 with the wrap rule's row to spare.
        assert_eq!(WELL_Y + VISIBLE as i32 * 2, STRIP_Y - 1, "border meets strip");
        assert!(STRIP_Y + 5 <= screen::H as i32);
        assert!(NEXT_X + 8 < screen::W as i32);
        assert!(PREVIEW_Y + (NEXT_SHOWN as i32 - 1) * PREVIEW_STEP + 4 < STRIP_Y);
    }

    #[test]
    fn the_bag_deals_each_piece_once_and_the_queue_shows_three() {
        let mut game = Blocks::with_rng(Rng::from_seed(42));
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
            down: true,
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
        // Sit through the clear window; the row collapses and the O's top
        // half falls into the gap.
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
        // the lock delay (rather than a hard drop) keeps drop points out of
        // the score so the awards can be checked exactly.
        stage_tetris(&mut game);
        lock_by_delay(&mut game);
        assert_eq!(game.clearing.len(), 4);
        assert!(game.back_to_back, "the first tetris arms the chain");
        assert_eq!(game.score, 800, "the first tetris is flat rate at level 1");
        assert_eq!(game.take_flash(), 1.0, "and it is the loudest thing there is");
        game.step(&none(), CLEAR_DELAY + CLEAR_PER_LINE * 3);
        assert_eq!(game.lines, 4);

        // A second tetris. Four lines have already been paid for, so the
        // level has moved on; the point of the assertion is the ×1.5.
        stage_tetris(&mut game);
        let level = game.level();
        let before = game.score;
        lock_by_delay(&mut game);
        let paid = game.score - before;
        assert_eq!(
            paid,
            1200 * level + scoring::COMBO_POINTS * level,
            "b2b tetris + one combo link at level {level}"
        );
    }

    /// Fill the bottom four rows bar the last column and stand an I on end in
    /// the trench, plus a lone leftover block so clearing the four does not
    /// also perfect-clear the board.
    fn stage_tetris(game: &mut Blocks) {
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
    fn lock_by_delay(game: &mut Blocks) {
        game.piece.y = game.piece.ghost_y(&game.board);
        game.step(&none(), handling::LOCK_DELAY);
    }

    #[test]
    fn the_touch_fix_bounds_a_long_slide_but_lets_it_buy_time() {
        // A piece resting on the floor, shoved left/right every step. Each
        // grounded move must cost a reset even when the lock timer is cold, or
        // the slide is infinite; the resets are capped, so it still locks.
        let mut game = rig(Mino::O, Mino::T);
        game.piece.y = (ROWS - 2) as i32;
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
        assert!(steps > handling::LOCK_RESETS as usize, "sliding bought no lock time at all");
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
    fn a_block_out_at_spawn_ends_the_game_through_its_curtain() {
        let mut game = rig(Mino::O, Mino::O);
        // Block the spawn columns so the next piece cannot appear — but not
        // the whole row, or it would read as a line to clear instead.
        for col in 3..7 {
            game.board.cells[SPAWN_Y as usize][col] = Some(Mino::I);
        }
        game.step(&hard(), ms(16));
        assert!(game.over, "the well is topped out");
        assert!(!game.is_over(), "the curtain has to fall first");
        assert_eq!(game.take_hitstop(), HITSTOP_OVER);
        for _ in 0..((DEATH_SECS / 0.016) as u32 + 2) {
            game.step(&none(), ms(16));
        }
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
        game.step(&none(), CLEAR_DELAY);
        for _ in 0..40 {
            game.step(&none(), ms(50));
        }
        assert!(game.heat() < hot, "heat has to decay when the player idles");
    }

    #[test]
    fn everyday_clears_stay_quiet_on_the_loud_channel() {
        let mut game = rig(Mino::O, Mino::T);
        for col in 0..8 {
            game.board.cells[FLOOR][col] = Some(Mino::I);
        }
        game.piece.x = 8;
        game.step(&hard(), ms(16));
        assert_eq!(game.take_flash(), 0.0, "a single must not blow the tube out");
    }

    #[test]
    fn the_renderer_readouts_stay_consistent() {
        let game = rig(Mino::T, Mino::I);
        assert_eq!(game.next().len(), NEXT_SHOWN);
        let (kind, cells) = game.active().expect("a live piece");
        assert_eq!(kind, Mino::T);
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

#[cfg(test)]
mod balance {
    use super::*;

    /// The level at `secs` of play with nothing cleared — the clock alone.
    fn at(secs: u64) -> Blocks {
        let mut g = Blocks::with_rng(Rng::from_seed(1));
        g.elapsed = Duration::from_secs(secs);
        g
    }

    #[test]
    fn the_curve_opens_playable_and_never_runs_away() {
        // Brisk enough to be a game from the first piece, and it settles
        // somewhere a person can keep playing rather than at a wall.
        let open = at(0).fall_seconds();
        assert!(
            (0.25..=0.35).contains(&open),
            "opening gravity {open} is outside the playable band"
        );
        for secs in [0, 30, 60, 120, 300, 900] {
            let fall = at(secs).fall_seconds();
            assert!(
                fall >= FALL_REST,
                "gravity ran past its floor to {fall} at {secs}s"
            );
        }
        // And most of the way there inside the first two minutes, or the ramp
        // is happening after the build has finished.
        let bite = at(120).fall_seconds();
        assert!(bite < 0.17, "still gentle at two minutes: {bite}");
    }

    #[test]
    fn the_curve_only_ever_tightens() {
        let mut last = f32::MAX;
        for secs in (0..600).step_by(10) {
            let fall = at(secs).fall_seconds();
            assert!(fall <= last, "gravity loosened at {secs}s");
            last = fall;
        }
    }

    #[test]
    fn a_long_build_still_leaves_a_game() {
        // Three minutes is a long build, and it has to still be a game rather
        // than a countdown. At rest a piece takes two seconds to cross the
        // well, and a grounded one keeps its full lock window on top of that.
        let fall = at(180).fall_seconds();
        assert!(fall * VISIBLE as f32 > 1.8, "a piece crosses the well too fast");
        assert!(handling::LOCK_DELAY >= Duration::from_millis(400));
    }
}
