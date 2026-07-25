//! The loop: attract, play, top-out, repeat — and the [`Host`] that lets the
//! same loop serve both the standalone arcade and a wrapped command.
//!
//! One fixed-timestep clock drives everything. The sim advances in whole 16 ms
//! steps and the frame is painted from whatever state that leaves, so a slow
//! terminal drops frames rather than slowing the game down.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::games::tetris::{Input, Tetris};
use crate::stage::{clock, Stage};
use crate::term::{self, Keys};

/// One sim step. ARR is one step, so the handling machine's real-millisecond
/// timers land on frame boundaries instead of straddling them.
pub const STEP: Duration = Duration::from_millis(16);
/// Never simulate more than this much wall time in one frame: after a suspend
/// or a laptop lid, catching up in real time would fast-forward the game.
const MAX_CATCHUP: Duration = Duration::from_millis(250);
const MAX_STEPS: u32 = 8;

/// Frames a top-out is held before the world respins.
const OVER_HOLD: u32 = 26;

/// Grid scroll per frame at rest, and the extra the player's heat buys — play
/// harder and the floor rushes at you.
const SCROLL_BASE: f32 = 0.010;
const SCROLL_HEAT: f32 = 0.030;

/// How long the attract screen holds before a wrapped command drops the player
/// straight into the game. Long enough to read the wordmark, short enough that
/// it is not in the way.
const AUTOSTART: Duration = Duration::from_millis(900);

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

    /// Skip the wait for a keypress and drop into the game after [`AUTOSTART`].
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

enum Mode {
    Attract { since: Instant },
    Play,
    Over { frame: u32 },
}

pub fn run(host: &mut dyn Host, w0: usize, h0: usize) -> Result<Exit> {
    let _guard = term::Guard::enter()?;

    let mut stage = Stage::new(w0, h0);
    let mut prev = vec![Default::default(); stage.w * stage.h];
    let mut presenter = term::Presenter::new();

    let mut game = Tetris::new();
    let mut mode = Mode::Attract {
        since: Instant::now(),
    };
    let mut scroll = 0.0f32;
    let mut played = Duration::ZERO;
    let mut dot = 0usize;

    let mut accumulator = Duration::ZERO;
    let mut last = Instant::now();

    let exit = loop {
        let frame_start = Instant::now();

        let poll = term::poll()?;
        if let Some((c, r)) = poll.resize {
            if c >= term::MIN_SIZE.0 && r >= term::MIN_SIZE.1 && (c != stage.w || r != stage.h) {
                let sink = stage.sun_sink;
                stage = Stage::new(c, r);
                stage.sun_sink = sink;
                prev = vec![Default::default(); stage.w * stage.h];
                term::clear()?;
            }
        }

        if host.finished() {
            break Exit::Finished;
        }

        // Esc leaves the game for the attract screen, and leaves the attract
        // screen for the shell — so one key never drops the whole session by
        // surprise, but two always do.
        if poll.keys.quit {
            match mode {
                Mode::Play | Mode::Over { .. } => {
                    mode = Mode::Attract {
                        since: Instant::now(),
                    };
                    game = Tetris::new();
                    played = Duration::ZERO;
                }
                Mode::Attract { .. } => break Exit::Quit,
            }
        }

        let now = Instant::now();
        accumulator += (now - last).min(MAX_CATCHUP);
        last = now;

        let mut steps = 0;
        while accumulator >= STEP && steps < MAX_STEPS {
            accumulator -= STEP;
            // Only the first step of a frame sees the input: catch-up steps run
            // neutral, so a hard drop never fires twice off one keypress.
            let keys = if steps == 0 { poll.keys } else { Keys::default() };
            steps += 1;
            advance(&mut mode, &mut game, &mut played, &keys, host);
        }

        if let Some(p) = host.progress() {
            stage.sun_sink = p.clamp(0.0, 1.0);
        }
        scroll = (scroll + SCROLL_BASE + game.heat() * SCROLL_HEAT).fract();

        let status = host.status();
        match mode {
            Mode::Attract { .. } => {
                dot = (dot + 1) % (5 * 24);
                let right = format!("{} {}", clock(played), gauge(host.progress()));
                stage.attract(scroll, "BLOCKS", dot / 24, &status, &right);
            }
            Mode::Play => {
                let right = format!("{} {}", clock(played), gauge(host.progress()));
                stage.game(&game, scroll, 0.0, &status, &right);
            }
            Mode::Over { frame } => {
                let right = format!("{} {}", clock(played), gauge(host.progress()));
                stage.game(&game, scroll, frame as f32 / OVER_HOLD as f32, &status, &right);
            }
        }

        presenter.frame(&stage.cells, &prev, stage.w, stage.h)?;
        prev.copy_from_slice(&stage.cells);

        std::thread::sleep(STEP.saturating_sub(frame_start.elapsed()));
    };

    term::drain();
    Ok(exit)
}

/// One sim step of whatever mode we are in.
fn advance(
    mode: &mut Mode,
    game: &mut Tetris,
    played: &mut Duration,
    keys: &Keys,
    host: &dyn Host,
) {
    match *mode {
        Mode::Attract { since } => {
            let start = keys.enter || keys.hard || keys.any();
            let auto = host.autostart() && since.elapsed() >= AUTOSTART;
            if start || auto {
                *game = Tetris::new();
                *played = Duration::ZERO;
                *mode = Mode::Play;
            }
        }
        Mode::Play => {
            *played += STEP;
            game.step(&input(keys), STEP);
            if game.is_over() {
                *mode = Mode::Over { frame: 0 };
            }
        }
        Mode::Over { frame } if frame + 1 >= OVER_HOLD => {
            *game = Tetris::new();
            *played = Duration::ZERO;
            *mode = Mode::Play;
        }
        Mode::Over { frame } => *mode = Mode::Over { frame: frame + 1 },
    }
}

fn input(k: &Keys) -> Input {
    Input {
        left: k.left,
        right: k.right,
        soft: k.down,
        cw: k.cw,
        ccw: k.ccw,
        hard: k.hard,
        hold: k.hold,
    }
}

/// Five characters of progress for the ticker's right slot. An unknown
/// progress still animates, so the gauge never reads as a hung command.
fn gauge(p: Option<f32>) -> String {
    match p {
        Some(p) => {
            let bars = (p.clamp(0.0, 1.0) * 5.0).round() as usize;
            (0..5).map(|i| if i < bars { '#' } else { '.' }).collect()
        }
        None => "-----".to_string(),
    }
}
