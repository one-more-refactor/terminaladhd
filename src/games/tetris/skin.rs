//! Piece identity: the seven-piece order the 7-bag and the SRS kick tables
//! are indexed by, and the conversions between that index and the [`Mino`]
//! enum. On a one-bit screen the pieces have no colour to own — identity is
//! shape, which is how the first cabinets told them apart too.

/// The seven tetrominoes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mino {
    I,
    O,
    T,
    S,
    Z,
    J,
    L,
}

/// The seven tetrominoes in canonical order. This is the order the 7-bag
/// shuffles and the order every kick/shape table is indexed by, so I is index
/// 0 (the only piece with its own kick table) and the rest follow the enum's
/// own declaration.
pub const ORDER: [Mino; 7] = [
    Mino::I,
    Mino::O,
    Mino::T,
    Mino::S,
    Mino::Z,
    Mino::J,
    Mino::L,
];

/// Table index for a piece — the position it occupies in [`ORDER`].
pub fn index(mino: Mino) -> usize {
    match mino {
        Mino::I => 0,
        Mino::O => 1,
        Mino::T => 2,
        Mino::S => 3,
        Mino::Z => 4,
        Mino::J => 5,
        Mino::L => 6,
    }
}

/// The piece at a table index; wraps so the bag can index modulo seven.
pub fn from_index(i: usize) -> Mino {
    ORDER[i % ORDER.len()]
}

/// The I has its own kick table because its box is 4×4 and its centre of
/// rotation sits on a box edge rather than on a cell.
pub fn is_i(mino: Mino) -> bool {
    matches!(mino, Mino::I)
}

/// Whether a piece takes part in the immobile all-spin rule. The T is
/// excluded here because it uses the stricter three-corner test instead, and
/// the O never rotates into a different footprint at all.
pub fn spins_by_immobility(mino: Mino) -> bool {
    matches!(mino, Mino::S | Mino::Z | Mino::J | Mino::L)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips_through_the_order() {
        for i in 0..ORDER.len() {
            assert_eq!(index(from_index(i)), i);
        }
    }

    #[test]
    fn only_the_i_owns_a_kick_table() {
        assert!(is_i(Mino::I));
        for &m in &ORDER[1..] {
            assert!(!is_i(m));
        }
    }
}
