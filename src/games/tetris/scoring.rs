//! Scoring: one lock produces one [`Action`], and the Action alone decides its
//! points and what it does to the back-to-back chain. Turning the branchy
//! `award()` of the old `blocks.rs` into an enum with two predicates makes the
//! one subtle case provable rather than accidental — a spin that clears no
//! lines neither starts nor ends back-to-back, it is *neutral*.

use super::rules::Spin;
use super::skin::Mino;

/// Guideline line values for 1/2/3/4 rows, each times the level.
pub const LINE_POINTS: [u32; 4] = [100, 300, 500, 800];

/// A full T-spin (or immobile all-spin) clearing 0/1/2/3 rows.
pub const TSPIN_POINTS: [u32; 4] = [400, 800, 1200, 1600];

/// A mini T-spin clearing 0/1/2 rows.
pub const TSPIN_MINI_POINTS: [u32; 3] = [100, 200, 400];

/// Every link of a combo past the first pays this much a level.
pub const COMBO_POINTS: u32 = 50;

/// Perfect-Clear bonuses for a 1/2/3/4-row all-clear, on top of the line
/// points, all times the level. A back-to-back Tetris all-clear pays more.
pub const PC_POINTS: [u32; 4] = [800, 1200, 1800, 2000];
pub const PC_B2B_TETRIS: u32 = 3200;

/// The single outcome of one lock. Exactly one of these is produced per piece
/// that comes to rest, and it carries everything scoring needs to know.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// A clean lock: nothing cleared, no spin.
    Nothing,
    /// A plain line clear of 1..=4 rows (4 is a Tetris).
    LineClear(u32),
    /// A T-spin clearing `lines` rows (`mini` distinguishes the weak variant).
    TSpin { lines: u32, mini: bool },
    /// An immobile spin of an S/Z/J/L clearing `lines` rows.
    AllSpin(u32),
}

impl Action {
    /// Build the Action for a lock from the piece kind, the spin the rotation
    /// earned, and how many rows the lock filled.
    pub fn classify(kind: Mino, spin: Spin, lines: u32) -> Self {
        match spin {
            Spin::None => {
                if lines == 0 {
                    Action::Nothing
                } else {
                    Action::LineClear(lines)
                }
            }
            Spin::Mini => Action::TSpin { lines, mini: true },
            Spin::Full => {
                if kind == Mino::T {
                    Action::TSpin { lines, mini: false }
                } else {
                    Action::AllSpin(lines)
                }
            }
        }
    }

    /// Rows this action cleared.
    pub fn lines(self) -> u32 {
        match self {
            Action::Nothing => 0,
            Action::LineClear(n) => n,
            Action::TSpin { lines, .. } => lines,
            Action::AllSpin(lines) => lines,
        }
    }

    /// Points before the level multiplier, back-to-back and combo bonuses.
    pub fn base_points(self) -> u32 {
        match self {
            Action::Nothing => 0,
            Action::LineClear(n) => LINE_POINTS[(n as usize - 1).min(3)],
            Action::TSpin { lines, mini: true } => {
                TSPIN_MINI_POINTS[(lines as usize).min(TSPIN_MINI_POINTS.len() - 1)]
            }
            Action::TSpin { lines, mini: false } => {
                TSPIN_POINTS[(lines as usize).min(TSPIN_POINTS.len() - 1)]
            }
            Action::AllSpin(lines) => TSPIN_POINTS[(lines as usize).min(TSPIN_POINTS.len() - 1)],
        }
    }

    /// A "difficult" clear: a Tetris, or any spin that actually cleared a row.
    /// These are the clears that arm the back-to-back chain and, when one
    /// follows another, take the ×1.5 bonus.
    pub fn starts_back_to_back(self) -> bool {
        match self {
            Action::LineClear(4) => true,
            Action::TSpin { lines, .. } => lines > 0,
            Action::AllSpin(lines) => lines > 0,
            _ => false,
        }
    }

    /// A plain 1/2/3-row clear breaks the chain. Everything else — a Tetris, a
    /// spin (cleared or not), or a clean lock — leaves it as it was; a
    /// spin-clearing-nothing is neutral by construction here.
    pub fn ends_back_to_back(self) -> bool {
        matches!(
            self,
            Action::LineClear(1) | Action::LineClear(2) | Action::LineClear(3)
        )
    }
}

/// The chain state after scoring a lock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Award {
    /// Points scored by this lock.
    pub points: u32,
    /// Back-to-back armed after this lock.
    pub back_to_back: bool,
}

/// Perfect-Clear bonus for a `lines`-row all-clear, told whether it is a
/// back-to-back Tetris.
pub fn perfect_clear_bonus(lines: u32, back_to_back: bool) -> u32 {
    match lines {
        1..=3 => PC_POINTS[lines as usize - 1],
        4 => {
            if back_to_back {
                PC_B2B_TETRIS
            } else {
                PC_POINTS[3]
            }
        }
        _ => 0,
    }
}

/// Score one lock. `combo` is the run of consecutive line-clearing pieces
/// *including this one* (so it is one on the first clear and the combo bonus,
/// 50×(combo−1)×level, is zero there). `back_to_back` is the chain state
/// *before* this lock, which is what the ×1.5 and the b2b-Tetris all-clear
/// read. `perfect_clear` says the board is empty after the collapse.
pub fn award(
    action: Action,
    level: u32,
    back_to_back: bool,
    combo: u32,
    perfect_clear: bool,
) -> Award {
    let lines = action.lines();
    let mut points = action.base_points() * level;

    // A difficult clear following another is worth half again — the base only,
    // never the combo or the all-clear bonus.
    if action.starts_back_to_back() && back_to_back {
        points += points / 2;
    }

    if lines > 0 {
        points += COMBO_POINTS * combo.saturating_sub(1) * level;
    }

    if perfect_clear && lines > 0 {
        points += perfect_clear_bonus(lines, back_to_back) * level;
    }

    let new_b2b = if action.ends_back_to_back() {
        false
    } else if action.starts_back_to_back() {
        true
    } else {
        back_to_back
    };

    Award {
        points,
        back_to_back: new_b2b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guideline_line_table_pays_by_level() {
        for (rows, expected) in [(1, 100), (2, 300), (3, 500), (4, 800)] {
            let a = award(Action::LineClear(rows), 1, false, 1, false);
            assert_eq!(a.points, expected, "{rows} rows at level 1");
        }
        // The table scales with the level and nothing else.
        assert_eq!(
            award(Action::LineClear(1), 5, false, 1, false).points,
            100 * 5
        );
    }

    #[test]
    fn back_to_back_tetrises_pay_half_again() {
        // The first tetris is flat rate and arms the chain.
        let first = award(Action::LineClear(4), 1, false, 1, false);
        assert_eq!(first.points, 800);
        assert!(first.back_to_back);

        // A second tetris, still level 1, following the first: 800×1.5, plus
        // one combo link (50×1×1) for clearing on consecutive pieces.
        let second = award(Action::LineClear(4), 1, true, 2, false);
        assert_eq!(second.points, 1200 + COMBO_POINTS);
        assert!(second.back_to_back);

        // A single is not difficult: it breaks the chain and pays flat.
        let single = award(Action::LineClear(1), 1, true, 3, false);
        assert_eq!(single.points, 100 + 2 * COMBO_POINTS);
        assert!(!single.back_to_back);
    }

    #[test]
    fn the_combo_bonus_is_fifty_a_link_a_level() {
        for (combo, level, expected) in [(1, 1, 0), (2, 1, 50), (4, 1, 150), (4, 3, 450)] {
            let a = award(Action::LineClear(1), level, false, combo, false);
            assert_eq!(
                a.points,
                LINE_POINTS[0] * level + expected,
                "combo {combo} at level {level}"
            );
        }
    }

    #[test]
    fn a_t_spin_double_pays_the_spin_table() {
        let a = award(
            Action::TSpin {
                lines: 2,
                mini: false,
            },
            1,
            false,
            1,
            false,
        );
        assert_eq!(a.points, TSPIN_POINTS[2]);
        assert!(a.back_to_back, "a T-spin arms the chain like a tetris does");
    }

    #[test]
    fn a_t_spin_mini_pays_the_mini_table() {
        let a = award(
            Action::TSpin {
                lines: 1,
                mini: true,
            },
            1,
            false,
            1,
            false,
        );
        assert_eq!(a.points, TSPIN_MINI_POINTS[1]);
    }

    #[test]
    fn a_spin_that_clears_nothing_pays_but_is_neutral_for_b2b() {
        // Chain armed going in; a zero-line T-spin must not disturb it.
        let a = award(
            Action::TSpin {
                lines: 0,
                mini: false,
            },
            1,
            true,
            0,
            false,
        );
        assert_eq!(a.points, TSPIN_POINTS[0], "no clear, so no half again");
        assert!(a.back_to_back, "the chain is left exactly as it was");

        // And it neither starts nor ends the chain when the chain was cold.
        let cold = award(
            Action::TSpin {
                lines: 0,
                mini: false,
            },
            1,
            false,
            0,
            false,
        );
        assert!(!cold.back_to_back, "a spin clearing nothing cannot arm b2b");
        assert!(!Action::TSpin {
            lines: 0,
            mini: false
        }
        .starts_back_to_back());
        assert!(!Action::TSpin {
            lines: 0,
            mini: false
        }
        .ends_back_to_back());
    }

    #[test]
    fn an_all_spin_scores_and_feeds_the_chain() {
        // An immobile S/Z/J/L spin that clears scores on the full spin table
        // and arms back-to-back, exactly like a T-spin.
        let a = award(Action::AllSpin(2), 1, false, 1, false);
        assert_eq!(a.points, TSPIN_POINTS[2]);
        assert!(a.back_to_back);

        // A second difficult clear takes the ×1.5.
        let b = award(Action::AllSpin(2), 1, true, 1, false);
        assert_eq!(b.points, TSPIN_POINTS[2] + TSPIN_POINTS[2] / 2);

        // Zero-line all-spin is neutral for b2b, like the T.
        assert!(!Action::AllSpin(0).starts_back_to_back());
        assert!(!Action::AllSpin(0).ends_back_to_back());
    }

    #[test]
    fn a_perfect_clear_pays_its_bonus_on_top_of_the_lines() {
        // Tetris all-clear, cold chain: 800 line + 2000 PC, at level 1.
        let tetris = award(Action::LineClear(4), 1, false, 1, true);
        assert_eq!(tetris.points, 800 + 2000);

        // A back-to-back Tetris all-clear pays the 3200 PC and the ×1.5 on
        // the line points: 800×1.5 + 3200, level 1, first-of-combo so no link.
        let b2b = award(Action::LineClear(4), 1, true, 1, true);
        assert_eq!(b2b.points, 1200 + 3200);

        // A single-line perfect clear: 100 line + 800 PC.
        let single = award(Action::LineClear(1), 1, false, 1, true);
        assert_eq!(single.points, 100 + 800);

        // All of it scales with the level.
        let level = award(Action::LineClear(2), 3, false, 1, true);
        assert_eq!(level.points, (300 + 1200) * 3);
    }

    #[test]
    fn classify_maps_spin_and_lines_to_the_right_action() {
        assert_eq!(Action::classify(Mino::O, Spin::None, 0), Action::Nothing);
        assert_eq!(
            Action::classify(Mino::I, Spin::None, 4),
            Action::LineClear(4)
        );
        assert_eq!(
            Action::classify(Mino::T, Spin::Full, 2),
            Action::TSpin {
                lines: 2,
                mini: false
            }
        );
        assert_eq!(
            Action::classify(Mino::T, Spin::Mini, 1),
            Action::TSpin {
                lines: 1,
                mini: true
            }
        );
        // A "full" spin on a non-T is an all-spin, not a T-spin.
        assert_eq!(Action::classify(Mino::S, Spin::Full, 1), Action::AllSpin(1));
    }
}
