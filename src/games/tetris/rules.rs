//! The board and the piece geometry: a 10×40 matrix with a twenty-row buffer
//! above the visible field, the seven tetromino shapes, SRS rotation with the
//! wall-kick tables ported verbatim from the original `blocks.rs`, line detect
//! and collapse, and the spin classifier (three-corner for the T, immobile for
//! the other spinnable pieces).
//!
//! Everything here is pure: a [`Board`] and a [`Piece`] in, a decision out. No
//! clock, no scoring state, no rendering.

use super::skin::{self, Mino};

/// Ten columns wide.
pub const COLS: usize = 10;
/// Forty rows tall: twenty visible plus a twenty-row buffer above the skyline.
/// Pieces spawn in the buffer and step down into view, which is what makes
/// Block-Out and Lock-Out distinct and lets a near-ceiling kick lift a piece
/// without being clipped.
pub const ROWS: usize = 40;
/// The bottom twenty rows a renderer draws.
pub const VISIBLE: usize = 20;
/// The first visible row: the skyline. Rows above it are the buffer.
pub const BUFFER: usize = ROWS - VISIBLE;

/// The seven tetrominoes (I O T S Z J L) as cells in an n×n box, in their SRS
/// spawn orientation. The box sizes are SRS's too, so a clockwise turn is just
/// a turn of the box and the kick tables line up with the published ones.
const PIECES: [([(i8, i8); 4], i8); 7] = [
    ([(0, 1), (1, 1), (2, 1), (3, 1)], 4), // I
    ([(0, 0), (1, 0), (0, 1), (1, 1)], 2), // O
    ([(1, 0), (0, 1), (1, 1), (2, 1)], 3), // T
    ([(1, 0), (2, 0), (0, 1), (1, 1)], 3), // S
    ([(0, 0), (1, 0), (1, 1), (2, 1)], 3), // Z
    ([(0, 0), (0, 1), (1, 1), (2, 1)], 3), // J
    ([(2, 0), (0, 1), (1, 1), (2, 1)], 3), // L
];

/// SRS wall kicks for J L S T Z, indexed by the rotation state being left and
/// then by direction (0 clockwise, 1 anticlockwise). The five offsets are
/// tried in order and the first that fits wins.
///
/// The published tables are drawn with y pointing up; these are written in
/// screen coordinates, so every vertical offset is negated and a positive `dy`
/// means downwards.
const KICKS: [[[(i8, i8); 5]; 2]; 4] = [
    [
        [(0, 0), (-1, 0), (-1, -1), (0, 2), (-1, 2)],
        [(0, 0), (1, 0), (1, -1), (0, 2), (1, 2)],
    ],
    [
        [(0, 0), (1, 0), (1, 1), (0, -2), (1, -2)],
        [(0, 0), (1, 0), (1, 1), (0, -2), (1, -2)],
    ],
    [
        [(0, 0), (1, 0), (1, -1), (0, 2), (1, 2)],
        [(0, 0), (-1, 0), (-1, -1), (0, 2), (-1, 2)],
    ],
    [
        [(0, 0), (-1, 0), (-1, 1), (0, -2), (-1, -2)],
        [(0, 0), (-1, 0), (-1, 1), (0, -2), (-1, -2)],
    ],
];

/// The I's own kicks. Its box is 4×4 and its centre of rotation sits on a box
/// edge rather than a cell, so the generic offsets would leave it two columns
/// from where it has to end up.
const KICKS_I: [[[(i8, i8); 5]; 2]; 4] = [
    [
        [(0, 0), (-2, 0), (1, 0), (-2, 1), (1, -2)],
        [(0, 0), (-1, 0), (2, 0), (-1, -2), (2, 1)],
    ],
    [
        [(0, 0), (-1, 0), (2, 0), (-1, -2), (2, 1)],
        [(0, 0), (2, 0), (-1, 0), (2, -1), (-1, 2)],
    ],
    [
        [(0, 0), (2, 0), (-1, 0), (2, -1), (-1, 2)],
        [(0, 0), (1, 0), (-2, 0), (1, 2), (-2, -1)],
    ],
    [
        [(0, 0), (1, 0), (-2, 0), (1, 2), (-2, -1)],
        [(0, 0), (-2, 0), (1, 0), (-2, 1), (1, -2)],
    ],
];

/// The last offset in each generic row only ever reaches a genuine T-spin
/// slot, so a T turn that needed it counts as full however the corners fall.
const TSPIN_KICK: usize = 4;

/// Offsets to try when turning `kind` out of rotation state `from`.
pub fn kicks(kind: Mino, from: u8, clockwise: bool) -> [(i8, i8); 5] {
    let table = if skin::is_i(kind) { &KICKS_I } else { &KICKS };
    table[from as usize % 4][usize::from(!clockwise)]
}

/// Cells of `kind` after `rot` clockwise quarter turns of its box.
pub fn cells(kind: Mino, rot: u8) -> [(i8, i8); 4] {
    let (mut cells, size) = PIECES[skin::index(kind)];
    for _ in 0..rot % 4 {
        for cell in &mut cells {
            *cell = (size - 1 - cell.1, cell.0);
        }
    }
    cells
}

/// Edge length of a piece's rotation box.
pub fn box_size(kind: Mino) -> i8 {
    PIECES[skin::index(kind)].1
}

/// Spawn column that centres the piece's box in the well.
pub fn spawn_x(kind: Mino) -> i32 {
    (COLS as i32 - box_size(kind) as i32) / 2
}

/// The settled matrix, one colour per filled cell so a renderer can draw the
/// stack in each piece's own hue.
pub struct Board {
    pub cells: [[Option<Mino>; COLS]; ROWS],
}

impl Board {
    pub fn new() -> Self {
        Self {
            cells: [[None; COLS]; ROWS],
        }
    }

    /// Occupied, or outside the well — the corner test wants the walls, the
    /// floor and the ceiling to count as stack.
    pub fn blocked(&self, col: i32, row: i32) -> bool {
        if !(0..COLS as i32).contains(&col) || !(0..ROWS as i32).contains(&row) {
            return true;
        }
        self.cells[row as usize][col as usize].is_some()
    }

    /// Would the piece sit entirely inside the well on vacant cells?
    pub fn fits(&self, kind: Mino, rot: u8, x: i32, y: i32) -> bool {
        cells(kind, rot)
            .iter()
            .all(|&(cx, cy)| !self.blocked(x + cx as i32, y + cy as i32))
    }

    /// Rows that are completely filled, top to bottom.
    pub fn full_rows(&self) -> Vec<usize> {
        (0..ROWS)
            .filter(|&row| self.cells[row].iter().all(|cell| cell.is_some()))
            .collect()
    }

    /// Drop every named row out of the board, the rows above it falling in.
    pub fn collapse(&mut self, rows: &[usize]) {
        for &index in rows {
            for row in (1..=index).rev() {
                self.cells[row] = self.cells[row - 1];
            }
            self.cells[0] = [None; COLS];
        }
    }

    /// No filled cell anywhere — the Perfect-Clear condition.
    pub fn is_empty(&self) -> bool {
        self.cells
            .iter()
            .all(|row| row.iter().all(|cell| cell.is_none()))
    }

    /// Would clearing exactly `rows` leave the board empty? Every filled cell
    /// has to be in one of those rows. Checked before the collapse, which is
    /// where the Perfect Clear is detected.
    pub fn perfect_clear(&self, rows: &[usize]) -> bool {
        self.cells
            .iter()
            .enumerate()
            .all(|(row, cells)| rows.contains(&row) || cells.iter().all(|cell| cell.is_none()))
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

/// A live piece: which tetromino, its rotation state, and its box origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub kind: Mino,
    pub rot: u8,
    pub x: i32,
    pub y: i32,
}

impl Piece {
    pub fn new(kind: Mino) -> Self {
        Self {
            kind,
            rot: 0,
            x: spawn_x(kind),
            y: 0,
        }
    }

    /// The piece's four cells in absolute (column, row) board coordinates.
    pub fn cells(&self) -> [(i32, i32); 4] {
        cells(self.kind, self.rot).map(|(cx, cy)| (self.x + cx as i32, self.y + cy as i32))
    }

    /// Bottom-most row the piece occupies.
    pub fn bottom(&self) -> i32 {
        self.cells()
            .iter()
            .map(|&(_, row)| row)
            .max()
            .unwrap_or(self.y)
    }

    /// Row the piece comes to rest on if dropped straight down from here.
    pub fn ghost_y(&self, board: &Board) -> i32 {
        let mut y = self.y;
        while board.fits(self.kind, self.rot, self.x, y + 1) {
            y += 1;
        }
        y
    }
}

/// Turn `piece` a quarter, trying the SRS offsets in order. Returns the index
/// of the offset that fitted (`Some` on a successful turn, and the index is
/// what the spin classifier reads), or `None` if the turn fits nowhere and is
/// refused. On success the piece is left in its new state.
pub fn rotate(board: &Board, piece: &mut Piece, clockwise: bool) -> Option<usize> {
    let from = piece.rot;
    let to = if clockwise {
        (from + 1) % 4
    } else {
        (from + 3) % 4
    };
    for (index, (dx, dy)) in kicks(piece.kind, from, clockwise).into_iter().enumerate() {
        let (x, y) = (piece.x + dx as i32, piece.y + dy as i32);
        if board.fits(piece.kind, to, x, y) {
            piece.rot = to;
            piece.x = x;
            piece.y = y;
            return Some(index);
        }
    }
    None
}

/// What the last rotation earned, if anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Spin {
    None,
    /// A T turned into a slot, but with its flat back buried rather than its
    /// point — worth less than a full T-spin.
    Mini,
    /// A full T-spin, or an immobile spin of one of the other spinnable pieces.
    Full,
}

/// Was the piece just turned into a spin? `kick` is the offset index the last
/// rotation used, or `None` if the last thing that moved the piece was not a
/// rotation — the Guideline's last-move rule, without which three pinned
/// corners on a piece that merely fell in would score a phantom spin.
pub fn classify(board: &Board, piece: &Piece, kick: Option<usize>) -> Spin {
    let Some(kick) = kick else {
        return Spin::None;
    };
    if piece.kind == Mino::T {
        classify_t(board, piece, kick)
    } else if skin::spins_by_immobility(piece.kind) && immobile(board, piece) {
        Spin::Full
    } else {
        Spin::None
    }
}

/// The three-corner T-spin test. Three of the four corners of the T's box
/// pinned is the Guideline's requirement; which corners decides full or mini,
/// except that the last kick in the table only ever reaches a real slot.
fn classify_t(board: &Board, piece: &Piece, kick: usize) -> Spin {
    const CORNERS: [(i32, i32); 4] = [(0, 0), (2, 0), (0, 2), (2, 2)];
    /// The two corners either side of the T's point, per rotation state, as
    /// indices into `CORNERS`.
    const FRONT: [(usize, usize); 4] = [(0, 1), (1, 3), (2, 3), (0, 2)];

    let pinned = |index: usize| {
        let (cx, cy) = CORNERS[index];
        board.blocked(piece.x + cx, piece.y + cy)
    };
    if (0..4).filter(|&index| pinned(index)).count() < 3 {
        return Spin::None;
    }
    let (left, right) = FRONT[piece.rot as usize % 4];
    if (pinned(left) && pinned(right)) || kick == TSPIN_KICK {
        Spin::Full
    } else {
        Spin::Mini
    }
}

/// The immobile rule: a piece that, right after a successful rotation, cannot
/// move left, right or down is wedged into a slot it could only have been
/// turned into. This generalises the spin to S/Z/J/L.
fn immobile(board: &Board, piece: &Piece) -> bool {
    !board.fits(piece.kind, piece.rot, piece.x - 1, piece.y)
        && !board.fits(piece.kind, piece.rot, piece.x + 1, piece.y)
        && !board.fits(piece.kind, piece.rot, piece.x, piece.y + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bottom row of the well.
    const FLOOR: usize = ROWS - 1;

    fn piece_at(kind: Mino, rot: u8, x: i32, y: i32) -> Piece {
        Piece { kind, rot, x, y }
    }

    #[test]
    fn every_turn_keeps_the_piece_inside_its_box() {
        for &kind in &skin::ORDER {
            let size = box_size(kind);
            for rot in 0..4 {
                for (cx, cy) in cells(kind, rot) {
                    assert!(
                        (0..size).contains(&cx) && (0..size).contains(&cy),
                        "{kind:?} rot {rot} left its box at {cx},{cy}"
                    );
                }
            }
        }
        // The O is the piece whose box turn leaves it exactly where it was.
        for rot in 0..4 {
            let mut turned = cells(Mino::O, rot);
            turned.sort_unstable();
            assert_eq!(turned, [(0, 0), (0, 1), (1, 0), (1, 1)]);
        }
    }

    #[test]
    fn pieces_spawn_where_the_guideline_puts_them() {
        assert_eq!(spawn_x(Mino::T), 3);
        assert_eq!(spawn_x(Mino::O), 4);
        assert_eq!(spawn_x(Mino::I), 3);
    }

    #[test]
    fn the_i_kicks_out_of_the_left_wall() {
        let board = Board::new();
        // Vertical, sitting in box column 2, flush against the left wall.
        let mut piece = piece_at(Mino::I, 1, -2, 18);
        assert!(board.fits(piece.kind, piece.rot, piece.x, piece.y));
        let kick = rotate(&board, &mut piece, true);
        assert_eq!(piece.rot, 2, "the turn was refused");
        // R→2 for the I tries (0,0) then (-1,0) then (+2,0); only the last
        // puts the flat bar back inside the well.
        assert_eq!(piece.x, 0);
        assert!(kick.is_some());
        for (col, _) in piece.cells() {
            assert!((0..COLS as i32).contains(&col));
        }
    }

    #[test]
    fn the_i_kicks_out_of_the_right_wall() {
        let board = Board::new();
        let mut piece = piece_at(Mino::I, 3, COLS as i32 - 2, 18);
        assert!(board.fits(piece.kind, piece.rot, piece.x, piece.y));
        rotate(&board, &mut piece, true);
        assert_eq!(piece.rot, 0, "the turn was refused");
        for (col, _) in piece.cells() {
            assert!((0..COLS as i32).contains(&col));
        }
    }

    #[test]
    fn a_j_over_a_trench_is_lifted_two_rows_to_turn() {
        // A vertical J standing over a one-wide gap has no room to lay itself
        // flat at its own height, nor one row down — only the fourth offset,
        // which lifts it two rows clear, fits.
        let mut board = Board::new();
        for row in FLOOR - 2..=FLOOR {
            for col in 0..COLS {
                if col != 4 {
                    board.cells[row][col] = Some(Mino::L);
                }
            }
        }
        let mut piece = piece_at(Mino::J, 1, 3, (ROWS - 4) as i32);
        assert!(
            board.fits(piece.kind, piece.rot, piece.x, piece.y),
            "the staging position is not legal"
        );
        rotate(&board, &mut piece, true);
        assert_eq!(piece.rot, 2, "the turn was refused");
        assert_eq!(piece.y, (ROWS - 6) as i32, "the kick never lifted it clear");
        assert!(board.fits(piece.kind, piece.rot, piece.x, piece.y));
    }

    #[test]
    fn anticlockwise_is_the_other_way_round() {
        let board = Board::new();
        let mut piece = piece_at(Mino::L, 0, 3, 18);
        rotate(&board, &mut piece, false);
        assert_eq!(piece.rot, 3);
        rotate(&board, &mut piece, false);
        assert_eq!(piece.rot, 2);
        rotate(&board, &mut piece, true);
        assert_eq!(piece.rot, 3);
    }

    /// A T-slot with a roof: three walls, an overhang on one side, and a notch
    /// only a rotation can reach. Filled so the spin clears two rows.
    fn stage_t_spin() -> (Board, Piece) {
        let mut board = Board::new();
        for col in [0, 1, 2, 5, 6, 7, 8, 9] {
            board.cells[ROWS - 3][col] = Some(Mino::I);
        }
        for col in [0, 1, 2, 6, 7, 8, 9] {
            board.cells[ROWS - 2][col] = Some(Mino::I);
        }
        for col in [0, 1, 2, 3, 5, 6, 7, 8, 9] {
            board.cells[FLOOR][col] = Some(Mino::I);
        }
        let piece = piece_at(Mino::T, 1, 2, (ROWS - 4) as i32);
        (board, piece)
    }

    #[test]
    fn a_kicked_t_lands_in_a_slot_it_could_not_have_fallen_into() {
        let (board, mut piece) = stage_t_spin();
        assert!(
            board.fits(piece.kind, piece.rot, piece.x, piece.y),
            "the staging position is not legal"
        );
        // Falling can never reach the notch: the overhang stops a flat T two
        // rows short, which is what makes the turn a spin. Drop it from high up
        // so the ghost falls onto the overhang lip rather than starting beneath
        // it.
        let flat = piece_at(Mino::T, 2, 3, 4);
        assert!(
            flat.ghost_y(&board) < (ROWS - 3) as i32,
            "the slot was reachable by dropping, so it is no slot"
        );
        let kick = rotate(&board, &mut piece, true);
        assert_eq!(piece.rot, 2, "the turn was refused");
        assert_eq!((piece.x, piece.y), (3, (ROWS - 3) as i32));
        assert_eq!(classify(&board, &piece, kick), Spin::Full);
    }

    #[test]
    fn a_t_that_was_not_turned_into_place_scores_nothing() {
        let (board, mut piece) = stage_t_spin();
        let kick = rotate(&board, &mut piece, true);
        assert_eq!(classify(&board, &piece, kick), Spin::Full);
        // The last-move rule: three pinned corners is not enough on its own,
        // the turn has to be what put the piece there.
        assert_eq!(classify(&board, &piece, None), Spin::None);
    }

    #[test]
    fn an_immobile_s_spins_but_a_free_one_does_not() {
        // Wall the S in so it cannot move in any direction: an all-spin.
        let mut board = Board::new();
        let piece = piece_at(Mino::S, 0, 3, (ROWS - 3) as i32);
        let occupied: Vec<(i32, i32)> = piece.cells().to_vec();
        for row in 0..ROWS {
            for col in 0..COLS {
                if !occupied.contains(&(col as i32, row as i32)) {
                    board.cells[row][col] = Some(Mino::Z);
                }
            }
        }
        assert!(
            board.fits(piece.kind, piece.rot, piece.x, piece.y),
            "the S's own cells must be clear"
        );
        assert_eq!(
            classify(&board, &piece, Some(0)),
            Spin::Full,
            "wedged S spins"
        );

        // The same S with room around it is not a spin.
        let open = Board::new();
        let free = piece_at(Mino::S, 0, 3, 18);
        assert_eq!(classify(&open, &free, Some(0)), Spin::None);
    }

    #[test]
    fn the_o_never_scores_a_spin() {
        // Even walled in, the O is excluded from the immobile rule.
        let mut board = Board::new();
        let piece = piece_at(Mino::O, 0, 4, (ROWS - 2) as i32);
        let occupied: Vec<(i32, i32)> = piece.cells().to_vec();
        for row in 0..ROWS {
            for col in 0..COLS {
                if !occupied.contains(&(col as i32, row as i32)) {
                    board.cells[row][col] = Some(Mino::Z);
                }
            }
        }
        assert_eq!(classify(&board, &piece, Some(0)), Spin::None);
    }

    #[test]
    fn a_full_row_is_detected_and_collapses_the_stack_above() {
        let mut board = Board::new();
        for col in 0..COLS {
            board.cells[FLOOR][col] = Some(Mino::I);
        }
        board.cells[FLOOR - 1][5] = Some(Mino::T); // a lone block riding above
        let full = board.full_rows();
        assert_eq!(full, vec![FLOOR]);
        board.collapse(&full);
        assert!(
            board.cells[FLOOR].iter().all(|c| c.is_none()) || board.cells[FLOOR][5].is_some(),
            "the lone block fell into the cleared row"
        );
        assert_eq!(board.cells[FLOOR][5], Some(Mino::T));
        assert!(board.full_rows().is_empty());
    }

    #[test]
    fn perfect_clear_is_only_when_nothing_survives() {
        let mut board = Board::new();
        for col in 0..COLS {
            board.cells[FLOOR][col] = Some(Mino::I);
        }
        assert!(
            board.perfect_clear(&[FLOOR]),
            "a single full row and nothing else"
        );
        board.cells[FLOOR - 1][0] = Some(Mino::L);
        assert!(
            !board.perfect_clear(&[FLOOR]),
            "a block survives the clear, so it is no perfect clear"
        );
    }
}
