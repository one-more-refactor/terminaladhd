//! The loop: reel, play, game over, reel again — and the [`Host`] that lets
//! the same loop serve both the standalone arcade and a wrapped command.
//!
//! There is no menu and no attract screen. The machine opens on the reel
//! already turning, drops you into whatever it lands on, and spins again
//! every time you die. That rotation is the product: you sat down to wait for
//! a command, not to run a menu, and the fastest way to stop deciding is to
//! have the machine decide.
//!
//! One fixed-timestep clock drives everything. The sim advances in whole
//! 16 ms steps and the frame is painted from whatever state that leaves, so a
//! slow terminal drops frames rather than slowing the game down.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::games::{Kind, ALL};
use crate::rng::Rng;
use crate::scores::Table;
use crate::screen::{self, Fx, Monitor, Phosphor, Screen};
use crate::term;

/// One sim step. Small enough that input latency is invisible; the games run
/// their own millisecond clocks against it.
pub const STEP: Duration = Duration::from_millis(16);
/// Never simulate more than this much wall time in one frame: after a suspend
/// or a laptop lid, catching up in real time would fast-forward the game.
const MAX_CATCHUP: Duration = Duration::from_millis(250);
const MAX_STEPS: u32 = 8;

/// The reel steps on its own 50 ms tick, whatever the frame rate is doing —
/// its physics constants are per-tick and the picture interpolates between
/// them, which is where the smoothness comes from.
const REEL_TICK: f32 = 0.05;
/// Pixel height of one name on the strip: enough air to separate them, tight
/// enough that the next one is always already showing at the window's lip.
const SLOT_H: f32 = 14.0;
/// Opening speed in slots per tick, and how much of it survives each tick.
/// The throw is spent in about 700 ms — long enough to read as a gamble,
/// short enough that it never stands between you and playing.
const REEL_V0: f32 = 1.8;
const REEL_DECAY: f32 = 0.72;
/// Hitting a key brakes hard instead of waiting it out.
const REEL_BRAKE: f32 = 0.55;
/// Below this the reel has arrived and drops into its detent.
const REEL_STOP: f32 = 0.02;
/// How far past the detent the reel carries and rocks back, one entry per
/// tick after it arrives. A real reel is stopped by a sprung pawl rather than
/// by arriving, and that little bounce is what says the thing was moving
/// under its own weight.
const REEL_SETTLE: [f32; 4] = [0.20, 0.06, -0.02, 0.0];
/// Ticks the winner is held before the cut — the settle, plus long enough to
/// read the one line of controls under it.
const REEL_HOLD: u8 = 18;

/// Seconds before the game-over screen accepts a skip — a held space (hard
/// drop) must not blow past the score — and how long it holds before the reel
/// takes over by itself. Losing costs a glance, not a decision.
const OVER_GRACE: f32 = 0.4;
const OVER_HOLD: f32 = 1.4;

/// How long the tube takes to collapse out and to warm back in. A cut between
/// two things, not a scene of its own.
const CUT_OUT: f32 = 0.14;
const CUT_IN: f32 = 0.22;

/// How fast a flash blows off the tube.
const FLASH_DECAY: f32 = 0.12;

/// What the diff believes was on screen before the first frame: a cell the
/// composer can never produce (a half-block with identical halves collapses
/// to a space), so every cell of the first frame is painted — the black ones
/// included. Diffing against a default (black) cell instead quietly assumed
/// the terminal's own background was black, and on a light theme the whole
/// surround simply never appeared.
const UNPAINTED: screen::Cell = screen::Cell {
    half: true,
    fg: [255, 0, 255],
    bg: [255, 0, 255],
};

/// What the loop is running for. The standalone arcade is [`Forever`]; a
/// wrapped command reports through its own implementation, and the loop ends
/// when [`Host::finished`] says so.
pub trait Host {
    /// `0.0..=1.0`, or `None` when there is no way to know. Drawn as the
    /// hairline across the top of the picture.
    fn progress(&mut self) -> Option<f32> {
        None
    }

    /// True once the loop should tear down on its own.
    fn finished(&mut self) -> bool {
        false
    }
}

/// The arcade with nothing behind it: it runs until the player leaves.
pub struct Forever;

impl Host for Forever {}

/// Why the loop returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// The player quit.
    Quit,
    /// The host said it was done.
    Finished,
}

// -------------------------------------------------------------------- reel

/// A slot-machine reel of game names. No menu, no choosing — you spin, it
/// lands, you play whatever came up.
pub struct Reel {
    /// Position in slots; the fractional part drives the scroll.
    pos: f32,
    vel: f32,
    /// Ticks since the reel stopped, once it has.
    landed: Option<u8>,
    /// Progress towards the next 50 ms tick.
    accum: f32,
}

impl Reel {
    fn new(rng: &mut Rng) -> Reel {
        Reel {
            // A random start means the same opening speed still lands
            // somewhere different every time.
            pos: rng.range(ALL.len() as u32) as f32,
            vel: REEL_V0,
            landed: None,
            accum: 0.0,
        }
    }

    /// A reel already at rest on `kind` — for `--shot` stills.
    #[doc(hidden)]
    pub fn parked(kind: Kind) -> Reel {
        Reel {
            pos: ALL.iter().position(|&k| k == kind).unwrap_or(0) as f32,
            vel: 0.0,
            landed: Some(REEL_SETTLE.len() as u8),
            accum: 0.0,
        }
    }

    /// Which game is under the payline right now.
    fn index(&self) -> usize {
        (self.pos.round() as i64).rem_euclid(ALL.len() as i64) as usize
    }

    pub fn kind(&self) -> Kind {
        ALL[self.index()]
    }

    /// Where the strip sits for this repaint. The reel steps at 20 Hz but the
    /// screen paints at 60, and a strip that moves in big jumps reads as a
    /// flick book rather than as something turning; the tick fraction carries
    /// it the rest of the way.
    fn strip(&self) -> f32 {
        let alpha = (self.accum / REEL_TICK).clamp(0.0, 1.0);
        match self.landed {
            Some(held) => {
                let at = |i: u8| REEL_SETTLE.get(i as usize).copied().unwrap_or(0.0);
                let from = at(held);
                self.pos + from + (at(held.saturating_add(1)) - from) * alpha
            }
            None => self.pos + self.vel * alpha,
        }
    }

    /// The two ticks the window is thrown into reverse video on arrival — the
    /// pixel half of the payout, the lamp being the other.
    fn paying(&self) -> bool {
        matches!(self.landed, Some(held) if held < 2)
    }

    pub fn has_landed(&self) -> bool {
        self.landed.is_some()
    }

    /// Advance by `dt`; returns the winner once it has been held long enough
    /// to hand over.
    fn step(&mut self, dt: f32, braking: bool) -> Option<Kind> {
        self.accum += dt;
        let mut done = None;
        while self.accum >= REEL_TICK {
            self.accum -= REEL_TICK;
            match self.landed {
                Some(held) => {
                    if held >= REEL_HOLD {
                        done = Some(self.kind());
                    } else {
                        self.landed = Some(held + 1);
                    }
                }
                None => {
                    self.pos += self.vel;
                    self.vel *= if braking { REEL_BRAKE } else { REEL_DECAY };
                    if self.vel < REEL_STOP {
                        self.vel = 0.0;
                        self.pos = self.pos.round();
                        self.landed = Some(0);
                    }
                }
            }
        }
        done
    }

    /// How lit the tube is: neon cycling while the strip flies, a white pop
    /// on the landing, then the winner's own tone.
    fn light(&self) -> Phosphor {
        match self.landed {
            // Blow out on impact and fall into the game's tone over the
            // settle, so the rest of the hold is a steady beat on the winner
            // rather than a fade the player waits out.
            Some(held) => {
                let t = 1.0 - (held as f32 / REEL_SETTLE.len() as f32);
                self.kind().phosphor().flash(t.max(0.0))
            }
            None => {
                // Cycling faster than the eye settles — the reel is the one
                // screen with no game on it to protect.
                let slot = self.pos * 0.8;
                let index = slot as usize;
                Phosphor::neon(index).mix(Phosphor::neon(index + 1), slot.fract())
            }
        }
    }
}

// ----------------------------------------------------------------- machine

/// The mode the machine is in, and the cut it is part-way through on its way
/// to the next one. A `go` collapses the raster, swaps behind the dark, and
/// warms back in; a `slide` swaps in place, for transitions that are a
/// continuation of what is already on screen.
struct Machine {
    mode: Mode,
    pending: Option<Mode>,
    curtain: f32,
}

impl Machine {
    fn new(mode: Mode) -> Machine {
        Machine {
            mode,
            pending: None,
            // The machine opens on its first frame, so the very first thing
            // the player sees is a tube warming up.
            curtain: 1.0,
        }
    }

    fn go(&mut self, next: Mode) {
        if self.pending.is_none() {
            self.pending = Some(next);
        }
    }

    fn slide(&mut self, next: Mode) {
        self.mode = next;
    }

    /// Advance the cut. True while the picture is on its way out, which is
    /// when the sim holds — on the way back in the new mode is already
    /// running, so the tube warms up on a game that has started.
    fn cut(&mut self, dt: f32) -> bool {
        if let Some(next) = self.pending.take() {
            self.curtain += dt / CUT_OUT;
            if self.curtain >= 1.0 {
                self.curtain = 1.0;
                self.mode = next;
            } else {
                self.pending = Some(next);
            }
            return true;
        }
        if self.curtain > 0.0 {
            self.curtain = (self.curtain - dt / CUT_IN).max(0.0);
        }
        false
    }
}

enum Mode {
    /// The reel, turning toward whatever it lands on.
    Spin(Reel),
    Play,
    /// The score settle after a death.
    Over {
        age: f32,
        score: u32,
        best: u32,
        new_best: bool,
    },
}

// -------------------------------------------------------------------- loop

pub fn run(host: &mut dyn Host, w0: usize, h0: usize) -> Result<Exit> {
    let _guard = term::Guard::enter()?;
    let mut pad = term::Pad::open();

    // Over SSH every frame is bytes on a wire, and on a phone it may be bytes
    // on a cellular plan. Half the frames is most of the saving with none of
    // the picture gone.
    let fps: u64 = if term::remote() { 30 } else { 60 };
    let frame_time = Duration::from_micros(1_000_000 / fps);

    let mut rng = Rng::new();
    let mut scores = Table::load();
    let mut kind = ALL[rng.range(ALL.len() as u32) as usize];
    let mut game = kind.spawn(Rng::new());

    let mut canvas = Screen::new();
    let mut monitor = Monitor::fit(w0, h0);
    let mut prev = vec![UNPAINTED; w0 * h0];
    let mut presenter = term::Presenter::new();
    let mut term_size = (w0, h0);
    let mut small_said = false;

    let mut machine = Machine::new(Mode::Spin(Reel::new(&mut rng)));
    let mut accumulator = Duration::ZERO;
    let mut last = Instant::now();
    // Render frames the whole machine is frozen for. An impact that stops
    // time reads as an impact; one that does not reads as a colour change.
    let mut freeze: u32 = 0;
    let mut frame_no: u64 = 0;
    // The tube blowout still decaying from the last loud thing.
    let mut flash: f32 = 0.0;

    let exit = loop {
        let frame_start = Instant::now();
        frame_no = frame_no.wrapping_add(1);

        let poll = pad.poll()?;
        if let Some((cw, ch)) = poll.resize {
            term_size = (cw, ch);
            if Monitor::fits(cw, ch) {
                monitor = Monitor::fit(cw, ch);
                prev = vec![UNPAINTED; cw * ch];
                small_said = false;
                term::clear()?;
            }
        }

        if host.finished() {
            // The command coming back must not quietly cost a good run: file
            // it before the machine goes away.
            if matches!(machine.mode, Mode::Play) {
                scores.submit(kind, game.score());
            }
            break Exit::Finished;
        }

        // A window that shrank under the picture cannot show it at all.
        // Saying so beats painting garbage.
        if !Monitor::fits(term_size.0, term_size.1) {
            if !small_said {
                term::say_too_small()?;
                small_said = true;
            }
            std::thread::sleep(frame_time.saturating_sub(frame_start.elapsed()));
            continue;
        }

        // Esc leaves the game for the reel, and leaves the reel for the
        // shell — one key never drops the whole session by surprise, but two
        // always do.
        if poll.keys.quit {
            match machine.mode {
                Mode::Spin(_) => break Exit::Quit,
                Mode::Play => {
                    scores.submit(kind, game.score());
                    machine.go(Mode::Spin(Reel::new(&mut rng)));
                }
                Mode::Over { .. } => machine.go(Mode::Spin(Reel::new(&mut rng))),
            }
        }

        // A key still down when the screen changes must not steer whatever
        // comes next.
        if machine.pending.is_some() {
            pad.release_all();
        }
        let closing = machine.cut(STEP.as_secs_f32());

        let now = Instant::now();
        accumulator += (now - last).min(MAX_CATCHUP);
        last = now;

        // Time does not accumulate during hitstop: it is a debt paid in whole
        // render frames, not wall time the sim owes itself afterwards.
        if freeze > 0 {
            freeze -= 1;
            accumulator = Duration::ZERO;
        }
        let mut steps = 0;
        while freeze == 0 && !closing && accumulator >= STEP && steps < MAX_STEPS {
            accumulator -= STEP;
            // Only the first step of a frame sees the input: catch-up steps
            // run neutral, so a hard drop never fires twice off one keypress.
            let keys = if steps == 0 { poll.keys } else { Default::default() };
            steps += 1;

            match &mut machine.mode {
                Mode::Spin(reel) => {
                    // Any key slams the brake — the spin is a thrill, not a
                    // wait.
                    if let Some(landed) = reel.step(STEP.as_secs_f32(), keys.skip()) {
                        kind = landed;
                        game = kind.spawn(Rng::new());
                        machine.go(Mode::Play);
                    }
                }
                Mode::Play => {
                    let input = crate::games::Input {
                        left: keys.left,
                        right: keys.right,
                        up: keys.up,
                        down: keys.down,
                        cw: keys.cw,
                        ccw: keys.ccw,
                        hard: keys.hard,
                        hold: keys.hold,
                        taps: keys.taps,
                    };
                    game.step(&input, STEP);
                    freeze = freeze.max(game.take_hitstop());
                    flash = flash.max(game.take_flash());
                    if game.is_over() {
                        let score = game.score();
                        let best = scores.best(kind);
                        let new_best = scores.submit(kind, score) == Some(0);
                        // No cut: the death animation just finished on this
                        // same picture, and the settle is a continuation of
                        // it.
                        machine.slide(Mode::Over {
                            age: 0.0,
                            score,
                            best: best.max(score),
                            new_best,
                        });
                        pad.release_all();
                    }
                }
                Mode::Over { age, .. } => {
                    *age += STEP.as_secs_f32();
                    let skip = *age >= OVER_GRACE && keys.skip();
                    if skip || *age >= OVER_HOLD {
                        machine.go(Mode::Spin(Reel::new(&mut rng)));
                    }
                }
            }
        }

        flash = (flash - STEP.as_secs_f32() / FLASH_DECAY).max(0.0);

        // Draw the mode onto the canvas and pick the tube's tone.
        canvas.clear();
        let ph = match &machine.mode {
            Mode::Spin(reel) => {
                draw_reel(&mut canvas, reel, scores.best(reel.kind()));
                reel.light()
            }
            Mode::Play => {
                game.draw(&mut canvas);
                // The game's own tone, pushed towards gold as the run heats
                // up, blown towards white by whatever just happened.
                kind.phosphor()
                    .mix(Phosphor::GOLD, game.heat() * 0.85)
                    .flash(flash)
            }
            Mode::Over {
                age,
                score,
                best,
                new_best,
            } => {
                draw_over(&mut canvas, *score, *best, *new_best, *age);
                // A record blows the panel out gold; an ordinary death flares
                // red and settles back into the game's own tone.
                let base = if *new_best {
                    Phosphor::GOLD
                } else {
                    Phosphor::ALARM
                };
                let settle = (age / OVER_GRACE).min(1.0);
                base.mix(kind.phosphor(), settle * 0.6)
            }
        };

        // The wrap rule: the command's progress as a hairline across the very
        // top, on every screen, because it is the one thing the player is
        // actually waiting for.
        if let Some(p) = host.progress() {
            canvas.hline(0, 0, (screen::W as f32 * p.clamp(0.0, 1.0)) as u32);
        }

        // Hitstop and shake are the same event seen twice: the world stops,
        // and the picture recoils in its housing.
        let fx = Fx {
            shake: if freeze > 0 {
                let swing = if frame_no.is_multiple_of(2) { 1.0 } else { -1.0 };
                swing * (freeze as f32 / 6.0).min(1.0)
            } else {
                0.0
            },
            cut: machine.curtain,
        };
        let (cols, rows) = (monitor.cols, monitor.rows);
        let cells = monitor.compose(&canvas, ph, fx, true);
        presenter.frame(cells, &prev, cols, rows)?;
        prev.copy_from_slice(cells);

        std::thread::sleep(frame_time.saturating_sub(frame_start.elapsed()));
    };

    term::drain();
    Ok(exit)
}

// ------------------------------------------------------------------ screens

/// The window the strip turns behind, outer edges. Everything else on the
/// screen is measured off it: air above and below, one line of type centred
/// in each.
const WIN_X: i32 = 4;
const WIN_Y: i32 = 11;
const WIN_W: u32 = 72;
const WIN_H: u32 = 26;

/// Top row of a name sitting exactly on the payline — centred between the
/// window's inner faces.
const PAYLINE: f32 = 18.5;

/// Top row of the marks that pinch in on the payline from either post.
const MARK_Y: i32 = 21;

/// How much of the strip survives at the window's lip, in pixels out of four,
/// one entry per row inward. A name does not vanish at the edge, it rolls out
/// of sight.
const LIP: [u32; 3] = [1, 2, 3];

/// The reel: names on a strip turning behind a lit window, the one held
/// between the marks being what you are about to play.
pub fn draw_reel(s: &mut Screen, reel: &Reel, best: u32) {
    let inner_x = WIN_X + 1;
    let inner_w = WIN_W - 2;
    let inner_top = WIN_Y + 1;
    let inner_bottom = WIN_Y + WIN_H as i32 - 2;
    let inner_h = (inner_bottom - inner_top + 1) as u32;

    // Lay the strip down first — one name per slot, all the same size, so it
    // reads as a continuous band rather than a list. The range covers every
    // slot that can have so much as one row inside the window at any point in
    // the turn.
    let pos = reel.strip();
    let base = pos.floor();
    let frac = pos - base;
    for offset in -1..=2i64 {
        let slot = base as i64 + offset;
        let name = ALL[slot.rem_euclid(ALL.len() as i64) as usize].name();
        let y = PAYLINE + (offset as f32 - frac) * SLOT_H;
        let w = screen::text_width(name) * 2;
        s.text_scaled((screen::W as i32 - w) / 2, y.round() as i32, name, 2);
    }

    // Then cut the strip back to the window, so names are sliced off
    // mid-letter as they enter and leave — that slicing is what sells the
    // spin.
    s.fill_rect(0, 0, screen::W as u32, inner_top as u32, false);
    s.fill_rect(
        0,
        inner_bottom + 1,
        screen::W as u32,
        (screen::H as i32 - inner_bottom - 1) as u32,
        false,
    );
    s.fill_rect(0, inner_top, inner_x as u32, inner_h, false);
    s.fill_rect(
        inner_x + inner_w as i32,
        inner_top,
        screen::W as u32 - (inner_x as u32 + inner_w),
        inner_h,
        false,
    );

    // A one-bit panel has no greys, so the curve of the drum is spent in the
    // only currency there is: pixels knocked out of the strip.
    for (row, keep) in LIP.iter().enumerate() {
        shade_row(s, inner_x, inner_top + row as i32, inner_w, *keep);
        shade_row(s, inner_x, inner_bottom - row as i32, inner_w, *keep);
    }

    s.rect(WIN_X, WIN_Y, WIN_W, WIN_H);
    // The payline is marked by the window pinching in on the name rather than
    // by rules laid across the picture: less ink, and it puts the eye on the
    // name instead of on the furniture.
    let mark = ["#..", "##.", "###", "##.", "#.."];
    s.sprite(WIN_X, MARK_Y, &mark);
    for (dy, row) in mark.iter().enumerate() {
        for (dx, ch) in row.chars().enumerate() {
            if ch == '#' {
                let x = WIN_X + WIN_W as i32 - 1 - dx as i32;
                s.set(x, MARK_Y + dy as i32, true);
            }
        }
    }
    if reel.paying() {
        s.invert_rect(inner_x, inner_top, inner_w, inner_h);
    }

    // Two lines of type, both centred, both always in the same place: what
    // the machine is offering, and what it wants from you. The best score
    // waits for the landing — under a turning strip the number would only
    // flicker between games.
    if reel.has_landed() {
        if best > 0 {
            let label = format!("BEST {best}");
            s.text((screen::W as i32 - screen::text_width(&label)) / 2, 3, &label);
        }
        let hint = reel.kind().hint();
        s.text((screen::W as i32 - screen::text_width(hint)) / 2, 40, hint);
    } else {
        let hint = "ANY KEY STOPS IT";
        s.text((screen::W as i32 - screen::text_width(hint)) / 2, 40, hint);
    }
}

/// Knock pixels out of one row of the strip: `keep` of every four survive, on
/// a diagonal so the holes never line up into stripes.
fn shade_row(s: &mut Screen, x: i32, y: i32, w: u32, keep: u32) {
    for dx in 0..w as i32 {
        let x = x + dx;
        let cut = match keep {
            1 => (x + y).rem_euclid(4) != 0,
            2 => (x + y).rem_euclid(2) != 0,
            _ => (x + y).rem_euclid(4) == 2,
        };
        if cut {
            s.set(x, y, false);
        }
    }
}

/// The score settle: what the run was worth, against what the game has ever
/// paid, over a bar draining towards the next spin — so the wait reads as the
/// machine reloading rather than as the game having stopped.
pub fn draw_over(s: &mut Screen, score: u32, best: u32, new_best: bool, age: f32) {
    let title = "GAME OVER";
    let w = screen::text_width(title) * 2;
    s.text_scaled((screen::W as i32 - w) / 2, 6, title, 2);
    let line = format!("SCORE {score}");
    s.text((screen::W as i32 - screen::text_width(&line)) / 2, 24, &line);
    let sub = if new_best {
        "NEW BEST!".to_string()
    } else {
        format!("BEST {best}")
    };
    // A record announces itself on a blink; a plain best just reads.
    if !new_best || ((age / 0.25) as u32).is_multiple_of(2) {
        s.text((screen::W as i32 - screen::text_width(&sub)) / 2, 32, &sub);
    }
    let left = (OVER_HOLD - age).max(0.0) / OVER_HOLD;
    let w = (screen::W as f32 * left) as u32;
    s.fill_rect(0, screen::H as i32 - 3, w, 3, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reel_spins_down_and_lands_on_a_real_game() {
        let mut rng = Rng::from_seed(3);
        let mut reel = Reel::new(&mut rng);
        let mut secs = 0.0;
        let winner = loop {
            if let Some(kind) = reel.step(0.016, false) {
                break kind;
            }
            secs += 0.016;
            assert!(secs < 10.0, "the reel never stopped");
        };
        assert!(ALL.contains(&winner));
        // Throw, settle and hold together: over inside about two seconds, and
        // long enough to read as a gamble at all.
        assert!((0.8..=2.2).contains(&secs), "the spin took {secs}s");
    }

    #[test]
    fn braking_lands_the_reel_sooner() {
        let mut rng = Rng::from_seed(4);
        let mut free = Reel::new(&mut rng);
        let mut braked = Reel::new(&mut rng);
        let mut free_t = 0.0;
        while free.step(0.016, false).is_none() {
            free_t += 0.016;
        }
        let mut braked_t = 0.0;
        while braked.step(0.016, true).is_none() {
            braked_t += 0.016;
        }
        assert!(braked_t < free_t, "brake {braked_t} vs free {free_t}");
    }

    #[test]
    fn the_reel_strobes_while_spinning_then_takes_the_winners_tone() {
        let mut rng = Rng::from_seed(5);
        let mut reel = Reel::new(&mut rng);
        let first = reel.light();
        reel.step(REEL_TICK, false);
        assert_ne!(first, reel.light(), "the lamp should be moving");
        while reel.step(REEL_TICK, false).is_none() {}
        assert_eq!(reel.light(), reel.kind().phosphor());
    }

    #[test]
    fn the_reel_overshoots_its_detent_and_rocks_back() {
        let mut rng = Rng::from_seed(6);
        let mut reel = Reel::new(&mut rng);
        while reel.landed.is_none() {
            reel.step(REEL_TICK, false);
        }
        let home = reel.pos;
        // Past the detent first, then back through it, then still — and
        // whatever the settle is doing, the winner never changes.
        assert!(reel.strip() > home);
        reel.step(REEL_TICK * 2.0, false);
        assert!(reel.strip() < home);
        for _ in 0..REEL_SETTLE.len() {
            reel.step(REEL_TICK, false);
        }
        assert_eq!(reel.strip(), home);
        assert_eq!(reel.index(), (home.round() as usize) % ALL.len());
    }

    #[test]
    fn the_strip_moves_between_ticks() {
        let mut rng = Rng::from_seed(7);
        let mut reel = Reel::new(&mut rng);
        reel.step(REEL_TICK, false);
        let a = reel.strip();
        reel.step(REEL_TICK * 0.5, false);
        assert!(reel.strip() > a, "the picture interpolates between ticks");
    }

    #[test]
    fn a_landed_reel_frames_the_winner_on_the_payline() {
        let reel = Reel::parked(Kind::Snake);
        let mut s = Screen::new();
        draw_reel(&mut s, &reel, 0);

        // The window: unbroken rules top and bottom, posts either side.
        for x in WIN_X..WIN_X + WIN_W as i32 {
            assert!(s.get(x, WIN_Y), "window top broken at {x}");
            assert!(s.get(x, WIN_Y + WIN_H as i32 - 1), "window bottom broken at {x}");
        }
        // The name under the marks is drawn whole where the payline says —
        // the lip shading must not have eaten it.
        let name = Kind::Snake.name();
        let w = screen::text_width(name) * 2;
        let x = (screen::W as i32 - w) / 2;
        let mut want = Screen::new();
        want.text_scaled(x, PAYLINE.round() as i32, name, 2);
        for y in 0..screen::H as i32 {
            for px in 0..screen::W as i32 {
                assert!(
                    !want.get(px, y) || s.get(px, y),
                    "{name} missing a pixel at {px},{y}"
                );
            }
        }
        // And it sits clear of the marks, inside the glass.
        assert!(x > WIN_X + 3 && x + w < WIN_X + WIN_W as i32 - 3);
    }

    #[test]
    fn a_turning_reel_stays_inside_the_window() {
        let mut rng = Rng::from_seed(8);
        let mut reel = Reel::new(&mut rng);
        // Every frame of a whole spin, including the settle: the strip may
        // never paint over the type or the air around the window.
        loop {
            let done = reel.step(0.016, false).is_some();
            let mut s = Screen::new();
            draw_reel(&mut s, &reel, 42);
            for y in 0..WIN_Y {
                for x in 0..screen::W as i32 {
                    // Row 3 carries the BEST readout once landed.
                    if (3..9).contains(&y) {
                        continue;
                    }
                    assert!(!s.get(x, y), "strip leaked above the window at {x},{y}");
                }
            }
            for y in WIN_Y + WIN_H as i32..40 {
                for x in 0..screen::W as i32 {
                    assert!(!s.get(x, y), "strip leaked below the window at {x},{y}");
                }
            }
            if done {
                break;
            }
        }
    }

    #[test]
    fn the_best_score_waits_for_the_landing() {
        let mut rng = Rng::from_seed(9);
        let mut reel = Reel::new(&mut rng);
        let readout = |reel: &Reel| {
            let mut s = Screen::new();
            draw_reel(&mut s, reel, 42);
            (0..screen::W as i32).any(|x| s.get(x, 3))
        };
        assert!(!readout(&reel), "a turning strip has no score to report");
        while reel.step(0.016, false).is_none() {}
        assert!(readout(&reel));
    }

    #[test]
    fn a_cut_holds_the_sim_until_the_picture_is_gone() {
        let mut m = Machine::new(Mode::Play);
        m.curtain = 0.0;
        m.go(Mode::Over {
            age: 0.0,
            score: 0,
            best: 0,
            new_best: false,
        });
        let mut frames = 0;
        while m.cut(0.016) {
            frames += 1;
            assert!(frames < 200, "the cut never finished");
            if matches!(m.mode, Mode::Over { .. }) {
                break;
            }
            assert!(matches!(m.mode, Mode::Play), "the swap happened too early");
        }
        assert!(matches!(m.mode, Mode::Over { .. }), "and then it swapped");
        assert_eq!(m.curtain, 1.0, "behind a dark screen");
    }

    #[test]
    fn the_picture_comes_back_on_its_own() {
        let mut m = Machine::new(Mode::Play);
        for _ in 0..200 {
            // Opening never holds the sim: the new screen is already running
            // while the tube warms up on it.
            assert!(!m.cut(0.016));
            if m.curtain == 0.0 {
                return;
            }
        }
        panic!("the tube never came back");
    }

    #[test]
    fn the_game_over_screen_reports_and_reloads() {
        let mut s = Screen::new();
        draw_over(&mut s, 1234, 2000, false, 0.0);
        // A full countdown bar at age zero, drained near the hold.
        let lit = |s: &Screen| {
            (0..screen::W as i32)
                .filter(|&x| s.get(x, screen::H as i32 - 2))
                .count()
        };
        assert_eq!(lit(&s), screen::W, "the bar opens full");
        let mut late = Screen::new();
        draw_over(&mut late, 1234, 2000, false, OVER_HOLD * 0.95);
        assert!(lit(&late) < screen::W / 10, "and drains as the reel nears");
    }
}
