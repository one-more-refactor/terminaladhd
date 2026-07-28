//! Running a command behind the arcade.
//!
//! The child is spawned with its stdout inherited — a pipeline downstream of
//! `adhd -- cmd` must receive the command's bytes untouched and in order. Only
//! stderr is captured, because that is where progress chatter goes and where
//! we can afford to hold it back until the picture folds away: on a failure
//! the tail is replayed, so the alternate screen never swallows the reason.
//!
//! Two threads: one draining stderr into the tail, one waiting on the child.
//! The loop asks through the [`Command`] host and never blocks on either.

use std::io::{BufRead, BufReader, Write};
use std::process::{self, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::app::Host;

/// After this long the progress hairline is treated as most of the way over.
/// It is a mood, not a measurement — no command says how far along it is.
const PACE: Duration = Duration::from_secs(90);

/// Captured lines are short; anything longer is a hang risk on a chatty build.
const LINE_CAP: usize = 200;

/// A command running behind the picture.
pub struct Command {
    started: Instant,
    label: String,
    done: Arc<AtomicBool>,
    code: Arc<AtomicI32>,
    tail: Arc<Mutex<Vec<String>>>,
}

/// How many trailing stderr lines are replayed after the world folds away, so a
/// failure is not swallowed by the alternate screen.
const TAIL_LINES: usize = 40;

impl Command {
    /// Spawn `argv` and return the host that reports on it.
    pub fn spawn(argv: &[String]) -> Result<Self> {
        let (program, rest) = argv.split_first().context("no command given")?;

        let mut child = process::Command::new(program)
            .args(rest)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not run {program:?}"))?;

        let tail = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        let code = Arc::new(AtomicI32::new(0));

        let stderr = child.stderr.take().context("no stderr pipe")?;
        {
            let tail = Arc::clone(&tail);
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for raw in reader.lines().map_while(Result::ok) {
                    let clean = sanitize(&raw);
                    if clean.trim().is_empty() {
                        continue;
                    }
                    if let Ok(mut t) = tail.lock() {
                        t.push(clean);
                        let overflow = t.len().saturating_sub(TAIL_LINES);
                        t.drain(..overflow);
                    }
                }
            });
        }

        {
            let done = Arc::clone(&done);
            let code = Arc::clone(&code);
            std::thread::spawn(move || {
                let status = child.wait();
                // A signal death has no exit code; 130 is the shell's own
                // convention for it, and something must be reported.
                code.store(status.ok().and_then(|s| s.code()).unwrap_or(130), Ordering::SeqCst);
                done.store(true, Ordering::SeqCst);
            });
        }

        Ok(Command {
            started: Instant::now(),
            label: argv.join(" "),
            done,
            code,
            tail,
        })
    }

    pub fn exit_code(&self) -> i32 {
        self.code.load(Ordering::SeqCst)
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Replay the captured stderr tail once the world is gone. Only worth doing
    /// on a failure — on success the noise was noise.
    pub fn replay_tail(&self) -> Result<()> {
        let tail = self.tail.lock().map_err(|_| anyhow::anyhow!("tail poisoned"))?;
        let mut err = std::io::stderr();
        for l in tail.iter() {
            writeln!(err, "{l}")?;
        }
        err.flush()?;
        Ok(())
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

impl Host for Command {
    /// No command tells us how far along it is, so the hairline tracks
    /// elapsed time against a nominal pace and is deliberately capped short
    /// of the far edge — a bar that completes before the command does would
    /// be a lie.
    fn progress(&mut self) -> Option<f32> {
        let t = self.started.elapsed().as_secs_f32() / PACE.as_secs_f32();
        Some((t.min(1.0) * 0.85).clamp(0.0, 0.85))
    }

    fn finished(&mut self) -> bool {
        self.is_done()
    }
}

/// Strip anything that could move the cursor, repaint the screen or set a
/// colour. Subprocess output lands inside our own frame, so a stray escape
/// sequence would corrupt the picture — and a rogue one could rewrite it.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(LINE_CAP));
    let mut kept = 0usize;
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if kept >= LINE_CAP {
            break;
        }
        match ch {
            '\x1b' => match chars.next() {
                // CSI: parameter and intermediate bytes, then a final byte in
                // 0x40..=0x7E. The introducer is itself in that range, which is
                // why it has to be consumed before the scan starts.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC and the other string-argument sequences run until BEL or
                // ST (ESC \), not until a final byte — a window title would
                // otherwise leak its text into the ticker.
                Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            let _ = chars.next();
                            break;
                        }
                    }
                }
                // Two-character escape (ESC c, ESC 7, …): the second char is
                // the whole sequence and is already consumed.
                _ => {}
            },
            '\t' => {
                out.push(' ');
                kept += 1;
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {}
            c => {
                out.push(c);
                kept += 1;
            }
        }
    }
    out
}

/// Split `argv` at the `--` separator. Everything before it is ours, everything
/// after is the command's — including its own flags, which is the whole point.
pub fn split_argv(args: &[String]) -> Result<(Vec<String>, Option<Vec<String>>)> {
    match args.iter().position(|a| a == "--") {
        Some(i) => {
            let cmd = args[i + 1..].to_vec();
            if cmd.is_empty() {
                bail!("`--` needs a command after it");
            }
            Ok((args[..i].to_vec(), Some(cmd)))
        }
        None => Ok((args.to_vec(), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_escapes_and_controls() {
        assert_eq!(sanitize("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(sanitize("a\x07b"), "ab");
        assert_eq!(sanitize("a\tb"), "a b");
        assert_eq!(sanitize("\x1b]0;title\x07after"), "after");
    }

    #[test]
    fn sanitize_caps_length() {
        let long = "x".repeat(LINE_CAP * 3);
        assert_eq!(sanitize(&long).chars().count(), LINE_CAP);
    }

    #[test]
    fn sanitize_keeps_ordinary_text() {
        assert_eq!(sanitize("   Compiling serde v1.0"), "   Compiling serde v1.0");
    }

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn split_argv_finds_the_command() {
        let (ours, cmd) = split_argv(&v(&["--size", "80x24", "--", "cargo", "build"])).unwrap();
        assert_eq!(ours, v(&["--size", "80x24"]));
        assert_eq!(cmd.unwrap(), v(&["cargo", "build"]));
    }

    #[test]
    fn split_argv_without_separator_is_all_ours() {
        let (ours, cmd) = split_argv(&v(&["--size", "80x24"])).unwrap();
        assert_eq!(ours, v(&["--size", "80x24"]));
        assert!(cmd.is_none());
    }

    #[test]
    fn split_argv_rejects_a_bare_separator() {
        assert!(split_argv(&v(&["--"])).is_err());
    }

    #[test]
    fn split_argv_keeps_the_commands_own_flags() {
        let (_, cmd) = split_argv(&v(&["--", "ls", "--", "-la"])).unwrap();
        assert_eq!(cmd.unwrap(), v(&["ls", "--", "-la"]));
    }
}
