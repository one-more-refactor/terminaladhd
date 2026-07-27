//! The loop: attract, spin, play, crash, board, spin again — and the [`Host`]
//! that lets the same loop serve both the standalone arcade and a wrapped
//! command.
//!
//! One fixed-timestep clock drives everything. The sim advances in whole 16 ms
//! steps and the frame is painted from whatever state that leaves, so a slow
//! terminal drops frames rather than slowing the game down.
//!
//! The machine never lets you pick. Every crash spins a new game, which is the
//! point: you sat down to wait for a command, not to run a menu.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::games::{Game, Input, Kind, ALL};
use crate::rng::Rng;
use crate::scores::Table;
use crate::stage::{clock, Quality, Settle, Stage, Tick};
use crate::term::{self, Keys};

/// One sim step. ARR is one step, so the handling machine's real-millisecond
/// timers land on frame boundaries instead of straddling them.
pub const STEP: Duration = Duration::from_millis(16);
/// Never simulate more than this much wall time in one frame: after a suspend
/// or a laptop lid, catching up in real time would fast-forward the game.
const MAX_CATCHUP: Duration = Duration::from_millis(250);
const MAX_STEPS: u32 = 8;

/// How long the attract screen holds before a wrapped command spins the wheel
/// on its own. Long enough to read the marquee, short enough that it is not in
/// the way of the thing you actually started.
const AUTOSTART: Duration = Duration::from_millis(700);

/// How long the wheel turns, and how many slots it travels. Everything between
/// two games is dead time, so it is as short as it can be and still read as a
/// spin rather than as a cut.
const SPIN_TIME: Duration = Duration::from_millis(950);
const SPIN_SLOTS: usize = 9;

/// The settle after a crash: how long the picture takes to go down, how long
/// the score counter takes to climb, and how long the whole thing is held.
const SINK_TIME: f32 = 0.35;
const COUNT_TIME: f32 = 0.45;
const OVER_TIME: f32 = 1.4;
/// A record is worth looking at for longer than a bad run.
const OVER_TIME_RECORD: f32 = 2.2;

/// How long the board stays up before the wheel turns again.
const BOARD_TIME: f32 = 2.0;

/// How long the tube takes to cut out and to come back. Short: this is a cut
/// between two things, not a scene of its own.
const CUT_OUT: f32 = 0.16;
const CUT_IN: f32 = 0.24;

/// Impact past which the monitor itself is felt — the guns pull apart and the
/// chassis moves, rather than only the arena shaking.
const JOLT_FLOOR: f32 = 0.55;
/// And past which it loses its horizontal hold, for this many frames.
const TEAR_FLOOR: f32 = 0.8;
const SYNC_FRAMES: u32 = 7;

/// How fast the `1UP` marker alternates, in frames.
const BLINK_FRAMES: u32 = 24;

/// How long the attract loop holds each of its screens. A cabinet left alone
/// did not show one picture — it cycled the marquee, the board and a demo, and
/// that cycle is most of why an idle machine still looked alive from across the
/// room.
const ATTRACT_CYCLE: f32 = 5.5;

/// What the loop is running for. The standalone arcade is [`Forever`]; a
/// wrapped command reports through its own implementation, and the loop ends
/// when [`Host::finished`] says so.
pub trait Host {
    /// Left ticker slot: what the machine is doing right now.
    fn status(&mut self) -> String;

    /// `0.0..=1.0`, or `None` when there is no way to know. Drives the sun.
    fn progress(&mut self) -> Option<f32> {
        None
    }

    /// True once the loop should tear down on its own.
    fn finished(&mut self) -> bool {
        false
    }

    /// Skip the wait for a keypress and spin the wheel after [`AUTOSTART`].
    fn autostart(&self) -> bool {
        false
    }
}

/// The arcade with nothing behind it: it runs until the player leaves.
pub struct Forever;

impl Host for Forever {
    fn status(&mut self) -> String {
        "INSERT COIN - ENTER TO PLAY - ESC TO QUIT".to_string()
    }
}

/// Why the loop returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// The player quit.
    Quit,
    /// The host said it was done.
    Finished,
}

/// The mode the machine is in, and the cut it is part-way through on its way
/// to the next one. Every screen change goes through the tube: it collapses to
/// a line, the mode swaps behind the dark, and it opens on the new one.
///
/// Not every change, though. The settle after a crash is continuous with the
/// game that produced it, so [`Machine::slide`] exists for the transitions
/// where a cut would throw away the thing worth watching.
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
            // The machine opens on its first frame, so the very first thing the
            // player sees is a tube warming up.
            curtain: 1.0,
        }
    }

    /// Change screens behind a cut.
    fn go(&mut self, next: Mode) {
        if self.pending.is_none() {
            self.pending = Some(next);
        }
    }

    /// Change screens without one, for a transition that is a continuation.
    fn slide(&mut self, next: Mode) {
        self.mode = next;
    }

    /// Advance the cut. True while the picture is on its way out, which is when
    /// the sim holds — on the way back in the new mode is already running, so
    /// the tube warms up on a game that has started.
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
    Attract {
        since: Instant,
    },
    /// The wheel, turning toward `reel`'s last slot.
    Spin {
        reel: Vec<Kind>,
        age: f32,
    },
    Play,
    /// The clock stopped, holding whatever was on screen.
    Paused,
    /// The controls. `playing` records whether a run is waiting behind them, so
    /// backing out returns to the game rather than dropping into a stale one.
    Help {
        playing: bool,
    },
    /// The crash settle. `shown` climbs to the run's real score.
    Over {
        age: f32,
        score: u32,
        rank: Option<usize>,
    },
    /// The board, with the run that just landed still highlighted.
    Board {
        age: f32,
        rank: Option<usize>,
    },
}

pub fn run(host: &mut dyn Host, w0: usize, h0: usize, quality: Quality) -> Result<Exit> {
    let _guard = term::Guard::enter()?;

    let mut rng = Rng::new();
    let mut scores = Table::load();
    let mut kind = pick(&mut rng, None);

    let mut stage = Stage::new(kind, w0, h0);
    stage.quality = quality;
    let frame_time = Duration::from_micros(1_000_000 / quality.fps.clamp(10, 120) as u64);
    let mut prev = vec![Default::default(); stage.w * stage.h];
    let mut presenter = term::Presenter::new(stage.quality.tol);

    let mut game = kind.spawn(Rng::new());
    // The attract screen shows a game playing itself, and it is never the one
    // you are about to be given — the demo is a trailer, not a spoiler.
    let mut demo_kind = pick(&mut rng, Some(kind));
    let mut demo = demo_kind.spawn(Rng::new());
    let mut machine = Machine::new(Mode::Attract {
        since: Instant::now(),
    });
    let mut played = Duration::ZERO;
    let mut frame_no: u32 = 0;

    let mut term_size = (w0, h0);
    let mut accumulator = Duration::ZERO;
    let mut last = Instant::now();
    // Render frames the whole machine is frozen for. An impact that stops time
    // reads as an impact; one that does not reads as a colour change.
    let mut freeze: u32 = 0;
    // Frames of lost horizontal hold and of inverted picture still owed.
    let mut sync_loss: u32 = 0;
    let mut negative: u32 = 0;

    let exit = loop {
        let frame_start = Instant::now();
        frame_no = frame_no.wrapping_add(1);

        let poll = term::poll()?;
        if let Some((cw, ch)) = poll.resize {
            term_size = (cw, ch);
            if cw >= term::MIN_SIZE.0 && ch >= term::MIN_SIZE.1 && (cw != stage.w || ch != stage.h)
            {
                stage = Stage::new(kind, cw, ch);
                stage.quality = quality;
                prev = vec![Default::default(); stage.w * stage.h];
                term::clear()?;
            }
        }

        if host.finished() {
            // The command coming back must not quietly cost a good run: file it
            // before the machine goes away.
            if matches!(machine.mode, Mode::Play | Mode::Paused) {
                scores.submit(kind, game.score());
            }
            break Exit::Finished;
        }

        // A frame that shrank under the minimum cannot be drawn at all. Saying
        // so beats drawing the old size into a smaller window, which is what
        // ignoring it used to do.
        if stage.w > term_size.0 || stage.h > term_size.1 {
            stage.too_small();
            presenter.frame(&stage.cells, &prev, stage.w, stage.h)?;
            prev.copy_from_slice(&stage.cells);
            std::thread::sleep(frame_time.saturating_sub(frame_start.elapsed()));
            continue;
        }

        // Esc leaves the game for the attract screen, and leaves the attract
        // screen for the shell — so one key never drops the whole session by
        // surprise, but two always do.
        if poll.keys.quit {
            match machine.mode {
                Mode::Attract { .. } => break Exit::Quit,
                // Backing out of the controls or a pause returns to the game,
                // not out of it: one key must never cost a run.
                Mode::Help { playing } => machine.slide(leave_help(playing)),
                Mode::Paused => machine.slide(Mode::Play),
                _ => {
                    machine.go(Mode::Attract {
                        since: Instant::now(),
                    });
                    played = Duration::ZERO;
                }
            }
        }

        let closing = machine.cut(STEP.as_secs_f32());
        stage.curtain = machine.curtain;

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
            // A held screen still runs its own step — that is how it notices the
            // key that lets it go — but the game inside it does not advance.
            accumulator -= STEP;
            // Only the first step of a frame sees the input: catch-up steps run
            // neutral, so a hard drop never fires twice off one keypress.
            let keys = if steps == 0 { poll.keys } else { Keys::default() };
            steps += 1;
            advance(Sim {
                machine: &mut machine,
                kind: &mut kind,
                game: &mut game,
                stage: &mut stage,
                scores: &mut scores,
                rng: &mut rng,
                played: &mut played,
                keys: &keys,
                host,
            });
        }

        stage.progress = host.progress();
        // A held screen is held all the way down: the game, the background and
        // the monitor's own hum all stop, or a pause looks like a freeze.
        let held = matches!(machine.mode, Mode::Paused | Mode::Help { .. });

        // Whatever the game just did reaches the background and the screen
        // here, and nowhere else: no game knows the warp field exists.
        let punch = game.take_punch();
        if punch > 0.0 {
            stage.warp.punch(punch);
            stage.flash = 0.35 * punch;
            // Past a certain size a hit stops being something on the screen and
            // starts being something that happened to the monitor.
            if punch >= JOLT_FLOOR {
                let over = (punch - JOLT_FLOOR) / (1.0 - JOLT_FLOOR);
                stage.fringe = 6.0 * over;
                let kick = 1 + (3.0 * over) as i32;
                stage.jolt = (rng.range(2 * kick as u32 + 1) as i32 - kick, kick);
                // The biggest hits take the horizontal hold with them. Held for
                // a few frames rather than one, or it is over before the eye
                // has decided anything happened.
                if punch >= TEAR_FLOOR {
                    sync_loss = SYNC_FRAMES;
                }
                // And the very biggest flip the picture for two frames, which
                // is what a cabinet did when it had nothing louder left.
                if punch >= 0.95 {
                    negative = 2;
                }
            }
        }
        freeze = freeze.max(game.take_hitstop());
        // The background stops with everything else — a field still flying
        // through a frozen frame reads as a dropped frame, not as an impact.
        if freeze == 0 && !held {
            stage.animate(STEP.as_secs_f32() * steps.max(1) as f32, game.heat());
        }

        if sync_loss > 0 {
            // Decaying rather than flat: the hold comes back rather than
            // being switched back on.
            stage.tear = 9.0 * sync_loss as f32 / SYNC_FRAMES as f32;
            sync_loss -= 1;
        }
        if negative > 0 {
            stage.invert = 1.0;
            negative -= 1;
        }

        let blink = (frame_no / BLINK_FRAMES).is_multiple_of(2);
        let best = scores.best(kind);
        let right = clock(played);
        let tick = Tick {
            left: host.status(),
            right,
        };

        // The demo plays itself only while it is on screen, and a demo that
        // tops out is replaced with a different game rather than restarted —
        // the marquee should never show the same run twice.
        if matches!(machine.mode, Mode::Attract { .. }) {
            if demo.is_over() {
                demo_kind = pick(&mut rng, Some(demo_kind));
                demo = demo_kind.spawn(Rng::new());
            }
            let input = demo.autopilot();
            demo.step(&input, STEP);
        }

        match &machine.mode {
            Mode::Attract { since } => {
                // The attract ticker teaches instead of reporting: it is the one
                // screen where the player is not yet busy, and the only place
                // the controls can be read without covering the command's own
                // output later.
                let teach = Tick {
                    left: demo_kind.hint().to_string(),
                    right: tick.right.clone(),
                };
                let idle = since.elapsed().as_secs_f32();
                let t = frame_no as f32 * STEP.as_secs_f32();
                if ((idle / ATTRACT_CYCLE) as u32).is_multiple_of(2) {
                    stage.attract(demo.as_ref(), scores.best(demo_kind), blink, t, &teach);
                } else {
                    // The board the demo is playing for, so the two halves of
                    // the loop are about the same game.
                    let rows = scores.top(demo_kind);
                    stage.board(demo_kind, &rows, None, idle, scores.best(demo_kind), &teach);
                }
            }
            Mode::Spin { reel, age } => {
                let t = (age / SPIN_TIME.as_secs_f32()).clamp(0.0, 1.0);
                // Ease out hard: almost all the travel happens in the first
                // third, so the wheel is visibly fighting to stop.
                let travel = 1.0 - (1.0 - t).powi(4);
                // The landing is a hit, not an arrival.
                stage.flash = (t * 14.0 - 13.0).max(0.0) * 0.9;
                stage.spin(reel, travel, best, blink, &tick);
            }
            Mode::Play => stage.game(game.as_ref(), best, blink, &tick),
            Mode::Paused => stage.paused(game.as_ref(), best, blink, &tick),
            Mode::Help { .. } => stage.help(kind, best, &tick),
            Mode::Over { age, score, rank } => {
                let record = *rank == Some(0);
                let settle = Settle {
                    fade: (age / SINK_TIME).clamp(0.0, 1.0),
                    shown: counted(*score, *age),
                    record,
                };
                stage.over(game.as_ref(), &settle, best, blink, &tick);
            }
            Mode::Board { age, rank } => {
                let rows = scores.top(kind);
                stage.board(kind, &rows, *rank, *age, best, &tick);
            }
        }

        presenter.frame(&stage.cells, &prev, stage.w, stage.h)?;
        prev.copy_from_slice(&stage.cells);

        std::thread::sleep(frame_time.saturating_sub(frame_start.elapsed()));
    };

    term::drain();
    Ok(exit)
}

/// Everything one sim step is allowed to touch. Bundled because the loop body
/// would otherwise take nine arguments and drift out of sync with itself.
struct Sim<'a> {
    machine: &'a mut Machine,
    kind: &'a mut Kind,
    game: &'a mut Box<dyn Game>,
    stage: &'a mut Stage,
    scores: &'a mut Table,
    rng: &'a mut Rng,
    played: &'a mut Duration,
    keys: &'a Keys,
    host: &'a mut dyn Host,
}

fn advance(s: Sim) {
    let dts = STEP.as_secs_f32();
    match &mut s.machine.mode {
        Mode::Attract { since } => {
            if s.keys.help {
                s.machine.slide(Mode::Help { playing: false });
                return;
            }
            let start = s.keys.skip();
            let auto = s.host.autostart() && since.elapsed() >= AUTOSTART;
            if start || auto {
                s.machine.go(spin_to(pick(s.rng, None), s.rng));
            }
        }
        Mode::Spin { reel, age } => {
            *age += dts;
            if *age >= SPIN_TIME.as_secs_f32() {
                let landed = *reel.last().unwrap_or(&Kind::Tetris);
                *s.kind = landed;
                s.stage.retarget(landed);
                *s.game = landed.spawn(Rng::new());
                *s.played = Duration::ZERO;
                s.machine.go(Mode::Play);
            }
        }
        Mode::Paused => {
            if s.keys.pause || s.keys.enter {
                s.machine.slide(Mode::Play);
            } else if s.keys.help {
                s.machine.slide(Mode::Help { playing: true });
            }
        }
        Mode::Help { playing } => {
            if s.keys.skip() || s.keys.help {
                let playing = *playing;
                s.machine.slide(leave_help(playing));
            }
        }
        Mode::Play => {
            if s.keys.pause {
                s.machine.slide(Mode::Paused);
                return;
            }
            if s.keys.help {
                s.machine.slide(Mode::Help { playing: true });
                return;
            }
            *s.played += STEP;
            s.game.step(&input(s.keys), STEP);
            if s.game.is_over() {
                let score = s.game.score();
                let rank = s.scores.submit(*s.kind, score);
                // No cut: the crash, the dissolve and the settle are one
                // continuous thing, and cutting away from it would throw out
                // the part worth watching.
                s.machine.slide(Mode::Over {
                    age: 0.0,
                    score,
                    rank,
                });
            }
        }
        Mode::Over { age, rank, .. } => {
            // A dead game is still stepped, so a death animation of its own
            // keeps running on the same clock as the settle around it.
            s.game.step(&Input::default(), STEP);
            *age += dts;
            let hold = if *rank == Some(0) {
                OVER_TIME_RECORD
            } else {
                OVER_TIME
            };
            // Any key skips ahead: nobody should have to sit through the
            // ceremony twice.
            if *age >= hold || s.keys.skip() {
                let rank = *rank;
                s.machine.go(Mode::Board { age: 0.0, rank });
            }
        }
        Mode::Board { age, .. } => {
            *age += dts;
            if *age >= BOARD_TIME || s.keys.skip() {
                let next = pick(s.rng, Some(*s.kind));
                s.machine.go(spin_to(next, s.rng));
            }
        }
    }
}

/// Where backing out of the controls goes. Straight back into the run if there
/// is one, and to the attract screen if the player was only reading.
fn leave_help(playing: bool) -> Mode {
    if playing {
        Mode::Play
    } else {
        Mode::Attract {
            since: Instant::now(),
        }
    }
}

/// A game at random, never the one just played — the whole promise is that the
/// machine gives you something else.
fn pick(rng: &mut Rng, avoid: Option<Kind>) -> Kind {
    let choices: Vec<Kind> = ALL.iter().copied().filter(|k| Some(*k) != avoid).collect();
    // With one game installed the filter empties and the only honest answer is
    // to play it again.
    let choices = if choices.is_empty() {
        ALL.to_vec()
    } else {
        choices
    };
    choices[rng.range(choices.len() as u32) as usize]
}

/// Build a reel that runs through the installed games and stops on `target`.
/// The wheel is padded so it turns for a while even with two games on it.
fn spin_to(target: Kind, rng: &mut Rng) -> Mode {
    let mut reel: Vec<Kind> = (0..SPIN_SLOTS)
        .map(|_| ALL[rng.range(ALL.len() as u32) as usize])
        .collect();
    // The last slot is the answer; everything before it is only motion.
    if let Some(slot) = reel.last_mut() {
        *slot = target;
    }
    // A reel that shows the answer in the slot before it lands gives the result
    // away a beat early.
    let n = reel.len();
    if n >= 2 && reel[n - 2] == target && ALL.len() > 1 {
        reel[n - 2] = *ALL.iter().find(|k| **k != target).unwrap_or(&target);
    }
    Mode::Spin { reel, age: 0.0 }
}

/// The score counter's current reading: eased so it slams most of the way up
/// immediately and then crawls the last of it.
fn counted(score: u32, age: f32) -> u32 {
    let t = (age / COUNT_TIME).clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t).powi(3);
    (score as f32 * eased).round() as u32
}

fn input(k: &Keys) -> Input {
    Input {
        left: k.left,
        right: k.right,
        up: k.up,
        down: k.down,
        cw: k.cw,
        ccw: k.ccw,
        hard: k.hard,
        hold: k.hold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_holds_the_sim_until_the_picture_is_gone() {
        let mut m = Machine::new(Mode::Play);
        m.curtain = 0.0;
        m.go(Mode::Board {
            age: 0.0,
            rank: None,
        });
        let mut frames = 0;
        // Closing: the sim is held and the mode has not changed yet.
        while m.cut(0.016) {
            frames += 1;
            assert!(frames < 200, "the cut never finished");
            if matches!(m.mode, Mode::Board { .. }) {
                break;
            }
            assert!(matches!(m.mode, Mode::Play), "the swap happened too early");
        }
        assert!(matches!(m.mode, Mode::Board { .. }), "and then it swapped");
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
    fn a_slide_changes_screens_without_a_cut() {
        let mut m = Machine::new(Mode::Play);
        m.curtain = 0.0;
        m.slide(Mode::Board {
            age: 0.0,
            rank: None,
        });
        assert!(matches!(m.mode, Mode::Board { .. }));
        assert_eq!(m.curtain, 0.0, "the picture never left");
    }

    #[test]
    fn a_second_request_mid_cut_does_not_jump_the_queue() {
        let mut m = Machine::new(Mode::Play);
        m.curtain = 0.0;
        m.go(Mode::Board {
            age: 0.0,
            rank: None,
        });
        m.go(Mode::Attract {
            since: Instant::now(),
        });
        while m.cut(0.016) && !matches!(m.mode, Mode::Board { .. }) {}
        assert!(matches!(m.mode, Mode::Board { .. }), "the first one won");
    }

    #[test]
    fn the_wheel_never_stops_where_it_just_was() {
        let mut rng = Rng::from_seed(11);
        for _ in 0..200 {
            assert_ne!(pick(&mut rng, Some(Kind::Snake)), Kind::Snake);
        }
    }

    #[test]
    fn the_reel_ends_on_its_target_without_spoiling_it() {
        let mut rng = Rng::from_seed(12);
        for _ in 0..50 {
            let Mode::Spin { reel, .. } = spin_to(Kind::Snake, &mut rng) else {
                panic!("spin_to built the wrong mode");
            };
            assert_eq!(reel.last(), Some(&Kind::Snake));
            assert_ne!(reel[reel.len() - 2], Kind::Snake);
        }
    }

    #[test]
    fn the_counter_lands_exactly_on_the_score() {
        assert_eq!(counted(1234, 0.0), 0);
        assert_eq!(counted(1234, COUNT_TIME), 1234);
        assert_eq!(counted(1234, 99.0), 1234);
    }
}
