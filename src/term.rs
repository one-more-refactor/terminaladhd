//! The terminal: raw mode, a restore that survives a panic, key decoding and
//! one atomic present.
//!
//! Everything is written to **stderr**. stdout belongs to whatever command we
//! are wrapping — a shell pipeline downstream of `adhd -- cmd` must receive the
//! command's bytes and nothing of ours.

use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    cursor,
    event::{
        self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};

use crate::games::{Taps, Turn};
use crate::world::{enc_diff, Cell, DiffOpts};

/// Used when the terminal will not say how big it is (a pipe, a CI log).
pub const FALLBACK_SIZE: (usize, usize) = (120, 34);

/// Below this the arena does not fit under the status strip. A twenty-row
/// playfield at the smallest legible block is twenty rows and the strip is
/// three, because the face is five sub-rows and a cell is two. That is
/// twenty-three, plus one for the sub-pixel the arena's frame is drawn on. The
/// only way lower is a second, shorter font.
///
/// Between twenty-three and twenty-six there is no room for the ticker, so the
/// command's output gives way rather than the game — which is the trade a phone
/// held in portrait wants.
pub const MIN_SIZE: (usize, usize) = (60, 24);

/// And above this nothing is a terminal any more. A size report is attacker
/// input as far as allocation is concerned: a claimed 65535×65535 frame is a
/// hundred-gigabyte buffer and an allocation abort that skips every restore
/// hook, so anything past the cap is clamped rather than believed.
pub const MAX_SIZE: (usize, usize) = (1024, 512);

/// Clamp a reported size to what the machine will actually build.
pub fn clamp_size(w: usize, h: usize) -> (usize, usize) {
    (w.min(MAX_SIZE.0), h.min(MAX_SIZE.1))
}

/// Whether this session came in over the network. A local terminal is a memcpy
/// away from the screen; an SSH session is paying for every byte, and on a
/// phone it may be paying in cellular data.
pub fn remote() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

/// Whether there is a terminal to draw on at all. stderr, because that is
/// where the picture goes — stdout may well be a pipe by design.
pub fn attached() -> bool {
    io::stderr().is_terminal()
}

/// Ask the terminal for its size. The answer is reported honestly — a
/// terminal that says 45×20 *is* 45×20, and the caller's minimum-size check
/// is the place that says so. Folding "too small" into the fallback here was
/// a bug with a long shadow: the polite refusal became unreachable for every
/// real terminal, and a phone in portrait got a 120-column picture sprayed
/// into 45 columns instead of one line telling it what to do.
pub fn size() -> (usize, usize) {
    match terminal::size() {
        Ok((c, r)) if c > 0 && r > 0 => clamp_size(c as usize, r as usize),
        _ => FALLBACK_SIZE,
    }
}

/// Restores raw mode, the alternate screen and the cursor on drop, so an early
/// return or a `?` never leaves the shell wedged. A panic hook does the same for
/// the unwind path, which `Drop` alone does not cover when the panic aborts.
pub struct Guard;

impl Guard {
    pub fn enter() -> Result<Self> {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            default(info);
        }));
        terminal::enable_raw_mode()?;
        let mut err = io::stderr();
        err.execute(EnterAlternateScreen)?;
        err.execute(cursor::Hide)?;
        Ok(Guard)
    }
}

/// Ask the terminal to report key releases, and say whether it agreed.
///
/// This is the single largest thing standing between this and a game that feels
/// like one. A terminal does not normally send a key-up: a held key arrives as
/// the operating system's own auto-repeat, which waits about half a second
/// before it starts. No amount of tuning inside the game can fix that, because
/// during those five hundred milliseconds nothing arrives at all — the piece
/// moves once and then sits there.
///
/// The kitty keyboard protocol reports press and release separately, which is
/// what the handling code has always assumed it had.
pub fn enable_key_release() -> bool {
    if !matches!(terminal::supports_keyboard_enhancement(), Ok(true)) {
        return false;
    }
    let pushed = io::stderr()
        .execute(PushKeyboardEnhancementFlags(
            // Releases for the arrows, and all keys as escape codes so plain
            // letters — wasd, hjkl — get them too.
            KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
        ))
        .is_ok();
    PUSHED.store(pushed, std::sync::atomic::Ordering::Relaxed);
    pushed
}

/// Whether the enhancement push actually happened, so the restore only pops
/// what was pushed — popping an empty stack is undefined ground on some
/// terminals.
static PUSHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn disable_key_release() {
    if PUSHED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        let _ = io::stderr().execute(PopKeyboardEnhancementFlags);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    disable_key_release();
    let mut err = io::stderr();
    let _ = err.execute(cursor::Show);
    let _ = err.execute(LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
}

/// Ships frames to the terminal, reusing its buffers so a frame allocates
/// nothing.
///
/// The diff has to be encoded into a buffer of its own: every encoder in
/// [`crate::world::encode`] clears its output first, so writing the DEC 2026
/// introducer into that same buffer would silently drop it and every frame
/// would tear.
pub struct Presenter {
    body: Vec<u8>,
    /// Matched to the stage's own resolve tolerance, or the diff will re-send
    /// cells the resolver already decided were the same.
    pub tol: i32,
    /// Emit xterm-256 codes instead of truecolor — lean mode's wire dialect.
    pub palette: bool,
    tx: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    writer: Option<std::thread::JoinHandle<()>>,
    /// A composed frame the writer had no room for yet. At most one: while
    /// it waits, no new frames are composed.
    pending: Option<Vec<u8>>,
}

impl Drop for Presenter {
    fn drop(&mut self) {
        // Close the channel and wait the writer out, so the restore sequence
        // that follows cannot interleave with a frame still in flight.
        self.tx.take();
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
    }
}

/// The run-joining gap: cells this far apart are worth re-addressing rather
/// than repainting through.
const GAP: usize = 6;

impl Presenter {
    pub fn new(tol: i32, palette: bool) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);
        let writer = std::thread::spawn(move || {
            let mut err = io::stderr();
            while let Ok(frame) = rx.recv() {
                let _ = err.write_all(&frame);
                let _ = err.flush();
            }
        });
        Presenter {
            body: Vec::new(),
            tol,
            palette,
            tx: Some(tx),
            writer: Some(writer),
            pending: None,
        }
    }

    /// One atomic present: DEC 2026 brackets the whole diff so a burst lands
    /// as a single update instead of tearing across the scanout.
    ///
    /// The write happens on its own thread, and this is the fix for the
    /// machine feeling slow on any terminal that cannot drain the stream: a
    /// blocking `write_all` on the game loop meant that when the terminal
    /// fell behind, *everything* fell behind — input, sim, the lot — and no
    /// amount of game tuning could feel responsive through it. Now a slow
    /// terminal costs frames, never the game: if the writer is still busy
    /// with the last frame, this one is simply not composed — `prev` is left
    /// untouched, so the next composed diff is exactly what the terminal
    /// missed.
    pub fn frame(&mut self, cells: &[Cell], prev: &mut [Cell], w: usize, h: usize) -> Result<()> {
        let Some(tx) = &self.tx else {
            return Ok(());
        };
        // A frame that could not be handed over last time goes first: every
        // composed diff advanced `prev`, so every composed diff must reach
        // the terminal, in order, or the screen goes stale. While one is
        // still waiting, nothing new is composed — which is exactly the
        // self-throttle a slow terminal wants.
        if let Some(held) = self.pending.take() {
            match tx.try_send(held) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(held)) => {
                    self.pending = Some(held);
                    return Ok(());
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return Ok(()),
            }
        }
        enc_diff(
            cells,
            prev,
            w,
            h,
            DiffOpts {
                tol: self.tol,
                gap: GAP,
                palette: self.palette,
            },
            &mut self.body,
        );
        if self.body.is_empty() {
            return Ok(());
        }
        let mut out = Vec::with_capacity(self.body.len() + 16);
        out.extend_from_slice(b"\x1b[?2026h");
        out.extend_from_slice(&self.body);
        out.extend_from_slice(b"\x1b[?2026l");
        if let Err(std::sync::mpsc::TrySendError::Full(out)) = tx.try_send(out) {
            self.pending = Some(out);
        }
        Ok(())
    }
}

pub fn clear() -> Result<()> {
    io::stderr().execute(terminal::Clear(terminal::ClearType::All))?;
    Ok(())
}

/// The one thing said outside the picture: the window has shrunk below the
/// world's minimum, and any rendered screen would wrap into the very garbage
/// it was trying to explain.
pub fn say_too_small(w: usize, h: usize) -> Result<()> {
    clear()?;
    let mut err = io::stderr();
    write!(
        err,
        "\x1b[H\x1b[0madhd needs {}x{} - this window is {w}x{h}",
        MIN_SIZE.0, MIN_SIZE.1
    )?;
    err.flush()?;
    Ok(())
}

/// Swallow anything typed during a run, so keys meant for the game do not land
/// on the shell prompt after we hand the terminal back.
pub fn drain() {
    while event::poll(Duration::ZERO).unwrap_or(false) {
        let _ = event::read();
    }
}

/// What one frame's worth of keys means. Held keys (`left`/`right`/`soft`) are
/// true while the key repeats; the rest are edges. A cooked terminal cannot be
/// trusted to report releases, so the caller rebuilds this fresh every frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Keys {
    pub left: bool,
    pub right: bool,
    pub down: bool,
    pub up: bool,
    pub cw: bool,
    pub ccw: bool,
    pub hard: bool,
    pub hold: bool,
    pub enter: bool,
    pub back: bool,
    /// Stop the clock without losing the run.
    pub pause: bool,
    /// Show the controls.
    pub help: bool,
    /// Esc, q or Ctrl-C — leave whatever we are in.
    pub quit: bool,
    /// Direction keys *going down* this frame, in arrival order. The booleans
    /// above are held state; these are the presses themselves, which is what
    /// steering wants — a hold must never outvote a tap.
    pub taps: Taps,
}

impl Keys {
    fn press(&mut self, code: KeyCode, mods: KeyModifiers) {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        match code {
            KeyCode::Left => self.left = true,
            KeyCode::Right => self.right = true,
            KeyCode::Down => {
                self.down = true;
                self.cw = false;
            }
            KeyCode::Up => {
                self.up = true;
                self.cw = true;
            }
            // Three ways to move, because the machine has no idea whose hands
            // are on it: arrows, vim, and the left-hand grip everyone who has
            // ever played anything already has.
            KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Char('a') | KeyCode::Char('A')
                if !ctrl =>
            {
                self.left = true
            }
            KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Char('d') | KeyCode::Char('D')
                if !ctrl =>
            {
                self.right = true
            }
            KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Char('s') | KeyCode::Char('S')
                if !ctrl =>
            {
                // The same batch rule as arrow-Down: a rotate and a soft drop
                // in one poll resolve to the drop, whichever key said it.
                self.down = true;
                self.cw = false;
            }
            KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Char('w') | KeyCode::Char('W')
                if !ctrl =>
            {
                self.up = true;
                self.cw = true;
            }
            KeyCode::Char('p') | KeyCode::Char('P') if !ctrl => self.pause = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,
            KeyCode::Char('x') | KeyCode::Char('X') => self.cw = true,
            KeyCode::Char('z') | KeyCode::Char('Z') => self.ccw = true,
            KeyCode::Char(' ') => self.hard = true,
            KeyCode::Char('c') | KeyCode::Char('C') if ctrl => self.quit = true,
            KeyCode::Char('c') | KeyCode::Char('C') => self.hold = true,
            KeyCode::Enter => self.enter = true,
            KeyCode::Backspace | KeyCode::Tab => self.back = true,
            // Esc and Ctrl-C only. `q` sits under the same hand as everything
            // else and quitting is the one action with no undo.
            KeyCode::Esc => self.quit = true,
            // Readline arrows, for terminals that send them instead.
            KeyCode::Char('b') | KeyCode::Char('B') if ctrl => self.left = true,
            KeyCode::Char('f') | KeyCode::Char('F') if ctrl => self.right = true,
            KeyCode::Char('n') | KeyCode::Char('N') if ctrl => self.down = true,
            KeyCode::Char('p') | KeyCode::Char('P') if ctrl => self.cw = true,
            _ => {}
        }
    }

    pub fn any(&self) -> bool {
        self.left
            || self.right
            || self.down
            || self.up
            || self.cw
            || self.ccw
            || self.hard
            || self.hold
            || self.enter
            || self.back
    }

    /// Anything the player could have meant as "get on with it". Deliberately
    /// not [`Keys::any`]: pause and help are requests of their own, and a
    /// ceremony that skipped because someone asked for the controls would be a
    /// bug the player could not explain.
    pub fn skip(&self) -> bool {
        self.any() && !self.pause && !self.help
    }

    /// Fold another frame's keys into this one — what the loop does with
    /// presses that arrive during hitstop, so a freeze banks input instead of
    /// eating it. Edges OR together; the other frame's taps queue behind this
    /// one's.
    pub fn merge(&mut self, other: &Keys) {
        self.left |= other.left;
        self.right |= other.right;
        self.down |= other.down;
        self.up |= other.up;
        self.cw |= other.cw;
        self.ccw |= other.ccw;
        self.hard |= other.hard;
        self.hold |= other.hold;
        self.enter |= other.enter;
        self.back |= other.back;
        self.pause |= other.pause;
        self.help |= other.help;
        self.quit |= other.quit;
        for t in other.taps.iter() {
            self.taps.push(t);
        }
    }
}

/// Everything the terminal reported since the last frame.
#[derive(Default)]
pub struct Poll {
    pub keys: Keys,
    pub resize: Option<(usize, usize)>,
}

/// How long a direction stays held after its last event when the terminal will
/// not report releases. Deliberately shorter than DAS, so a tap can never start
/// an auto-shift — on such a terminal the only honest reading of a key event is
/// "this happened once".
const ASSUMED_HOLD: Duration = Duration::from_millis(60);

/// The four directions, in the order [`Pad`] tracks them.
const DIRS: usize = 4;
const LEFT: usize = 0;
const RIGHT: usize = 1;
const UP: usize = 2;
const DOWN: usize = 3;

/// The keyboard, across frames.
///
/// Held state cannot live in [`Keys`], which is one frame's worth: whether a
/// direction is still down is a fact about the world, not about this frame. On
/// a terminal that reports releases this is exact, and everything the handling
/// code does with DAS and auto-repeat finally means what it says.
pub struct Pad {
    releases: bool,
    held: [bool; DIRS],
    seen: [Option<Instant>; DIRS],
}

impl Pad {
    /// Turn on key-release reporting if the terminal has it, and say so.
    ///
    /// Not over the network: the capability probe writes a query and waits
    /// for an answer, and crossterm's deadline for that answer is two full
    /// seconds. A phone SSH client that ignores the query therefore spent two
    /// seconds on a frozen black alternate screen before the first frame —
    /// which reads as "it hung", and is paid for a protocol essentially no
    /// SSH-from-a-phone terminal speaks. Locally every terminal answers in
    /// microseconds, so the probe stays.
    pub fn open() -> Pad {
        Pad {
            releases: !remote() && enable_key_release(),
            held: [false; DIRS],
            seen: [None; DIRS],
        }
    }

    /// Non-blocking: fold every pending event into one [`Poll`], and carry the
    /// held directions across from the last frame.
    pub fn poll(&mut self) -> Result<Poll> {
        let mut p = Poll::default();
        let now = Instant::now();
        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(k) => {
                    // Releases only ever clear a direction. Everything else —
                    // rotate, drop, hold — is an edge, and a release of it is
                    // not a second press.
                    if k.kind == KeyEventKind::Release {
                        for (i, d) in direction(k.code)
                            .into_iter()
                            .enumerate()
                            .filter(|(_, d)| *d)
                        {
                            let _ = d;
                            self.held[i] = false;
                            self.seen[i] = None;
                        }
                        continue;
                    }
                    p.keys.press(k.code, k.modifiers);
                    // A tap is the key going down, once. Repeats are the same
                    // hold restated, and folding them in would hand the old
                    // priority bug back on any terminal that reports them.
                    if k.kind == KeyEventKind::Press {
                        if let Some(t) = turn(k.code) {
                            p.keys.taps.push(t);
                        }
                    }
                    for (i, d) in direction(k.code).into_iter().enumerate() {
                        if d {
                            self.held[i] = true;
                            self.seen[i] = Some(now);
                        }
                    }
                }
                Event::Resize(c, r) => p.resize = Some((c as usize, r as usize)),
                _ => {}
            }
        }

        if !self.releases {
            // Nothing said the key went up, so assume it did unless something
            // said otherwise recently.
            for i in 0..DIRS {
                if self.seen[i].is_some_and(|t| now.duration_since(t) > ASSUMED_HOLD) {
                    self.held[i] = false;
                    self.seen[i] = None;
                }
            }
        }

        p.keys.left |= self.held[LEFT];
        p.keys.right |= self.held[RIGHT];
        p.keys.up |= self.held[UP];
        p.keys.down |= self.held[DOWN];
        Ok(p)
    }

    /// Sleep out the rest of the frame, but wake the moment a key arrives.
    /// A plain `thread::sleep` here was up to sixteen milliseconds of the
    /// machine being deaf on every frame — the difference between an input
    /// landing on this sim step and the next one, which is the difference
    /// the hands actually feel. Events are left queued for the next poll;
    /// an early wake re-renders an unchanged frame, which the diff turns
    /// into a handful of bytes.
    pub fn wait(&mut self, budget: Duration) {
        if budget.is_zero() {
            return;
        }
        let _ = event::poll(budget);
    }

    /// Forget everything held. Used when the machine changes screens, so a
    /// direction still down when a run ends does not steer the next one.
    pub fn release_all(&mut self) {
        self.held = [false; DIRS];
        self.seen = [None; DIRS];
    }
}

/// The turn a key press means, if any — same map as [`direction`], as an event
/// rather than a state.
fn turn(code: KeyCode) -> Option<Turn> {
    let d = direction(code);
    [
        (LEFT, Turn::Left),
        (RIGHT, Turn::Right),
        (UP, Turn::Up),
        (DOWN, Turn::Down),
    ]
    .into_iter()
    .find(|&(i, _)| d[i])
    .map(|(_, t)| t)
}

/// Which of the four directions a key means, if any. Arrows, vim and the
/// left-hand grip all land here.
fn direction(code: KeyCode) -> [bool; DIRS] {
    let mut d = [false; DIRS];
    match code {
        KeyCode::Left => d[LEFT] = true,
        KeyCode::Right => d[RIGHT] = true,
        KeyCode::Up => d[UP] = true,
        KeyCode::Down => d[DOWN] = true,
        KeyCode::Char(c) => match c.to_ascii_lowercase() {
            'h' | 'a' => d[LEFT] = true,
            'l' | 'd' => d[RIGHT] = true,
            'k' | 'w' => d[UP] = true,
            'j' | 's' => d[DOWN] = true,
            _ => {}
        },
        _ => {}
    }
    d
}
