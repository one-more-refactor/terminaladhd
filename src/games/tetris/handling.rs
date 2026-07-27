//! Input handling in real milliseconds: delayed auto-shift, auto-repeat, the
//! soft-drop factor, and the lock-delay state machine with its reset cap and
//! lowest-row rule.
//!
//! This owns no board — collision is the caller's to resolve — but it owns
//! every timer, so the caller drives it with `dt` and applies whatever pulses
//! it hands back. The one real bug carried over from the tick-based original is
//! fixed here: a reset is consumed on any move or rotation *while grounded*,
//! not merely while the lock timer happens to be running.

use std::time::Duration;

/// Delay before a held direction begins to auto-repeat. A tenth of a second is
/// about as long as a hold can wait before it reads as the game not listening.
pub const DAS: Duration = Duration::from_millis(100);
/// Interval between auto-repeat shifts once charged. One render frame.
pub const ARR: Duration = Duration::from_millis(16);
/// Soft drop is this many times faster than gravity…
pub const SDF: u32 = 20;
/// …but never faster than one row per this long, so it stays watchable.
pub const SOFT_FLOOR: Duration = Duration::from_millis(15);
/// A grounded piece locks after this long unless disturbed.
pub const LOCK_DELAY: Duration = Duration::from_millis(500);
/// Moves and turns that may each buy back the lock window. Uncapped, a piece
/// left endlessly fiddled with would never lock at all.
pub const LOCK_RESETS: u32 = 15;
/// Entry delay between locking one piece and the next appearing — none.
pub const ARE: Duration = Duration::ZERO;

/// Soft-drop interval for a given natural gravity interval: the faster of
/// SDF× gravity and the floor, so at high levels it is capped rather than
/// racing gravity for the same rows.
pub fn soft_interval(gravity: Duration) -> Duration {
    (gravity / SDF).max(SOFT_FLOOR)
}

/// All the input timers for one live piece.
pub struct Handling {
    /// Direction the auto-shift is charging (-1, 0, +1).
    das_dir: i32,
    /// How long the current direction has been held.
    das_held: Duration,
    /// Auto-repeat pulses already emitted since the charge completed.
    arr_emitted: u32,
    /// How long the piece has sat grounded without locking.
    lock_timer: Duration,
    /// Resets spent buying back the lock window at the current low.
    lock_resets: u32,
    /// Lowest row (largest y) the piece has reached; getting lower refunds the
    /// resets, so descent is always rewarded and a slide never is forever.
    lock_low: i32,
}

impl Handling {
    /// A fresh machine for a piece spawning at row `low`.
    pub fn new(low: i32) -> Self {
        Self {
            das_dir: 0,
            das_held: Duration::ZERO,
            arr_emitted: 0,
            lock_timer: Duration::ZERO,
            lock_resets: 0,
            lock_low: low,
        }
    }

    /// Re-arm for a new piece at row `low` without reallocating.
    pub fn reset(&mut self, low: i32) {
        self.das_dir = 0;
        self.das_held = Duration::ZERO;
        self.arr_emitted = 0;
        self.lock_timer = Duration::ZERO;
        self.lock_resets = 0;
        self.lock_low = low;
    }

    /// How many horizontal shifts the auto-shift wants this step, given the
    /// direction currently held (`-1`, `0`, `+1`) and the elapsed `dt`. A fresh
    /// press moves once immediately; a held direction moves again once DAS has
    /// elapsed and then every ARR after that. The caller applies that many
    /// single-cell shifts in `dir` and stops early on a wall.
    pub fn autoshift(&mut self, dir: i32, dt: Duration) -> u32 {
        if dir == 0 {
            self.das_dir = 0;
            self.das_held = Duration::ZERO;
            self.arr_emitted = 0;
            return 0;
        }
        if dir != self.das_dir {
            // Fresh press: move on the spot and start the charge.
            self.das_dir = dir;
            self.das_held = Duration::ZERO;
            self.arr_emitted = 0;
            return 1;
        }
        self.das_held = self.das_held.saturating_add(dt);
        if self.das_held < DAS {
            return 0;
        }
        // Charged. The number of pulses owed is one at the DAS boundary plus
        // one per ARR beyond it; emit however many of those have not gone yet.
        let beyond = self.das_held - DAS;
        let want = (beyond.as_micros() / ARR.as_micros().max(1)) as u32 + 1;
        let count = want.saturating_sub(self.arr_emitted);
        self.arr_emitted = want;
        count
    }

    /// Note that the piece just fell to a new row. If it is lower than any it
    /// has occupied, the lock window and its resets start over.
    pub fn descend(&mut self, low: i32) {
        if low > self.lock_low {
            self.lock_low = low;
            self.lock_resets = 0;
            self.lock_timer = Duration::ZERO;
        }
    }

    /// A successful move or rotation. While grounded it buys back the full lock
    /// window, up to [`LOCK_RESETS`] times — and it is the grounded state that
    /// gates this, not the timer, which is the fix over the original.
    pub fn touch(&mut self, grounded: bool) {
        if grounded && self.lock_resets < LOCK_RESETS {
            self.lock_resets += 1;
            self.lock_timer = Duration::ZERO;
        }
    }

    /// Advance the lock timer for a grounded piece; returns true once it has
    /// sat long enough to lock.
    pub fn ground(&mut self, dt: Duration) -> bool {
        self.lock_timer = self.lock_timer.saturating_add(dt);
        self.lock_timer >= LOCK_DELAY
    }

    /// The piece is no longer grounded — clear the lock timer.
    pub fn unground(&mut self) {
        self.lock_timer = Duration::ZERO;
    }

    /// Resets spent at the current low, for tests and HUD.
    pub fn resets(&self) -> u32 {
        self.lock_resets
    }

    /// Fraction of the lock window elapsed, for a grounded-piece pulse.
    pub fn lock_phase(&self) -> f32 {
        (self.lock_timer.as_secs_f32() / LOCK_DELAY.as_secs_f32()).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_fresh_press_moves_once_then_waits_out_das() {
        let mut h = Handling::new(0);
        assert_eq!(h.autoshift(1, ms(16)), 1, "the first press moves on the spot");
        // Still inside DAS after the immediate move: nothing repeats.
        let mut held = Duration::ZERO;
        while held + ms(16) < DAS {
            assert_eq!(h.autoshift(1, ms(16)), 0, "the auto-shift jumped the delay");
            held += ms(16);
        }
        // The step that crosses DAS releases the first repeat.
        assert_eq!(h.autoshift(1, ms(16)), 1, "the auto-shift never started");
    }

    #[test]
    fn once_charged_it_repeats_every_arr() {
        let mut h = Handling::new(0);
        h.autoshift(1, ms(0)); // press
        h.autoshift(1, DAS); // charge exactly to the boundary -> one pulse
        // From here each ARR is worth one more pulse.
        assert_eq!(h.autoshift(1, ARR), 1);
        assert_eq!(h.autoshift(1, ARR), 1);
        // A dt spanning three ARRs owes three shifts at once.
        assert_eq!(h.autoshift(1, ARR * 3), 3);
    }

    #[test]
    fn releasing_and_reversing_restarts_the_charge() {
        let mut h = Handling::new(0);
        h.autoshift(1, ms(0));
        h.autoshift(1, DAS + ARR * 4);
        assert_eq!(h.autoshift(0, ms(16)), 0, "release stops it");
        // A new direction moves once immediately, uncharged.
        assert_eq!(h.autoshift(-1, ms(16)), 1);
        assert_eq!(h.autoshift(-1, ms(16)), 0, "and has to charge afresh");
    }

    #[test]
    fn a_grounded_move_consumes_a_reset_even_with_a_cold_timer() {
        // The bug fix: a piece that has only just grounded (lock_timer == 0)
        // must still spend a reset when it moves, or a long slide is free.
        let mut h = Handling::new(20);
        assert_eq!(h.resets(), 0);
        h.touch(true);
        assert_eq!(h.resets(), 1, "a grounded move has to cost a reset");
        // An airborne move costs nothing.
        h.touch(false);
        assert_eq!(h.resets(), 1);
    }

    #[test]
    fn the_resets_are_capped_and_a_lower_row_refunds_them() {
        let mut h = Handling::new(20);
        for _ in 0..LOCK_RESETS * 2 {
            h.touch(true);
        }
        assert_eq!(h.resets(), LOCK_RESETS, "resets are capped");
        // Falling to a genuinely lower row hands the whole allowance back.
        h.descend(21);
        assert_eq!(h.resets(), 0);
        // The same row again does not.
        h.touch(true);
        h.descend(21);
        assert_eq!(h.resets(), 1);
    }

    #[test]
    fn a_grounded_piece_locks_after_the_delay() {
        let mut h = Handling::new(20);
        let mut waited = Duration::ZERO;
        while waited + ms(50) < LOCK_DELAY {
            assert!(!h.ground(ms(50)), "locked before its window ran out");
            waited += ms(50);
        }
        assert!(h.ground(ms(50)), "it never locked");
        // And lifting off clears the timer.
        h.unground();
        assert!(!h.ground(ms(50)));
    }

    #[test]
    fn soft_drop_is_capped_at_the_floor() {
        // Slow gravity: SDF× is well above the floor.
        assert_eq!(soft_interval(ms(500)), ms(25));
        // Fast gravity: SDF× would be below the floor, so the floor wins.
        assert_eq!(soft_interval(ms(100)), SOFT_FLOOR);
    }
}
