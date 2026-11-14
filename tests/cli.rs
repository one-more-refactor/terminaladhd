//! The binary, driven the way a user drives it.
//!
//! These run `adhd` as a child with every stream piped, which is exactly the
//! "no terminal" case: a CI log, a pipe, a cron job. That is the half of the
//! promise that can be checked without a screen — the command always runs,
//! its streams arrive untouched, and its exit code comes back — plus the
//! flags that never needed a terminal in the first place.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn adhd() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_adhd"));
    let scores = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-scores");
    // Never let a test make noise, or reach for the real score table.
    cmd.env("ADHD_MUTE", "1")
        .env("ADHD_SCORES", scores)
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY");
    cmd
}

fn run(args: &[&str]) -> Output {
    adhd().args(args).output().expect("the binary runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A fresh directory per test, under the target dir rather than /tmp so a
/// failed run leaves its evidence next to the build.
fn scratch(name: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let dir = base.join(format!("cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

// ── flags ───────────────────────────────────────────────────────────────

#[test]
fn help_prints_usage_on_stdout_and_succeeds() {
    for flag in ["--help", "-h"] {
        let o = run(&[flag]);
        assert!(o.status.success(), "{flag}: {o:?}");
        let out = stdout(&o);
        assert!(out.starts_with("usage: adhd"), "{flag}: {out}");
        for documented in ["--size", "--lean", "--rich", "--bench", "--shot", "--mute"] {
            assert!(out.contains(documented), "--help is missing {documented}");
        }
    }
}

#[test]
fn version_names_the_crate_version() {
    for flag in ["--version", "-V"] {
        let o = run(&[flag]);
        assert!(o.status.success());
        let out = stdout(&o);
        assert!(
            out.starts_with(&format!("adhd {}", env!("CARGO_PKG_VERSION"))),
            "{out}"
        );
    }
}

#[test]
fn an_unknown_flag_is_refused_with_a_hint() {
    let o = run(&["--nope"]);
    assert_eq!(o.status.code(), Some(1));
    let err = stderr(&o);
    assert!(err.contains("unknown argument"), "{err}");
    assert!(err.contains("--help"), "{err}");
}

#[test]
fn a_bare_separator_is_refused() {
    let o = run(&["--"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("needs a command"), "{}", stderr(&o));
}

#[test]
fn size_must_be_w_by_h() {
    let cases: [&[&str]; 3] = [&["--size"], &["--size", "120"], &["--size", "axb"]];
    for bad in cases {
        let o = run(bad);
        assert_eq!(o.status.code(), Some(1), "{bad:?}");
        assert!(stderr(&o).starts_with("adhd:"), "{bad:?}: {}", stderr(&o));
    }
}

#[test]
fn size_past_the_cap_is_refused_not_allocated() {
    let o = run(&["--size", "99999x99999", "--shot", "unused"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("cap"), "{}", stderr(&o));
}

#[test]
fn size_below_the_minimum_is_refused_politely() {
    let dir = scratch("tiny");
    let o = run(&["--size", "40x10", "--shot", dir.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("at least 60x24"), "{}", stderr(&o));
}

#[test]
fn playing_without_a_terminal_says_so() {
    let o = run(&[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("no terminal"), "{}", stderr(&o));
}

// ── wrapping a command with no screen to draw on ────────────────────────

#[test]
fn a_wrapped_command_keeps_its_stdout_byte_for_byte() {
    let o = run(&["--", "printf", "one\\ntwo\\n\\tthree"]);
    assert!(o.status.success(), "{o:?}");
    assert_eq!(o.stdout, b"one\ntwo\n\tthree");
}

#[test]
fn a_wrapped_command_keeps_its_stderr_even_when_it_succeeds() {
    let o = run(&["--", "sh", "-c", "echo warning: unused >&2; echo done"]);
    assert!(o.status.success(), "{o:?}");
    assert_eq!(stdout(&o), "done\n");
    assert!(stderr(&o).contains("warning: unused"), "{}", stderr(&o));
}

#[test]
fn a_wrapped_command_hands_back_its_exit_code() {
    for code in [0, 1, 3, 42, 255] {
        let o = run(&["--", "sh", "-c", &format!("exit {code}")]);
        assert_eq!(o.status.code(), Some(code));
    }
}

#[test]
fn a_signal_death_reports_like_a_shell_would() {
    let o = run(&["--", "sh", "-c", "kill -9 $$"]);
    assert_eq!(o.status.code(), Some(137));
}

#[test]
fn the_commands_own_flags_are_left_alone() {
    // `--help` after the separator belongs to the command, not to us.
    let o = run(&["--", "sh", "-c", "echo \"$1\"", "sh", "--help"]);
    assert!(o.status.success());
    assert_eq!(stdout(&o), "--help\n");
}

#[test]
fn stdin_reaches_the_wrapped_command() {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = adhd()
        .args(["--", "cat"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"through the arcade")
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success());
    assert_eq!(o.stdout, b"through the arcade");
}

#[test]
fn a_missing_program_is_an_error_not_a_hang() {
    let o = run(&["--", "terminaladhd-no-such-program"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("could not run"), "{}", stderr(&o));
}

// ── the headless renderer ───────────────────────────────────────────────

/// Parse a binary PPM header and check the body is exactly the size it says.
fn check_ppm(path: &Path) -> (usize, usize) {
    let bytes = std::fs::read(path).expect("the frame was written");
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        let start = i;
        while !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..i]).into_owned());
        i += 1;
    }
    assert_eq!(fields[0], "P6", "{path:?}");
    assert_eq!(fields[3], "255", "{path:?}");
    let w: usize = fields[1].parse().unwrap();
    let h: usize = fields[2].parse().unwrap();
    assert_eq!(bytes.len() - i, w * h * 3, "{path:?} body size");
    // A frame that is all one colour is a renderer that drew nothing.
    let body = &bytes[i..];
    assert!(
        body.chunks(3).any(|px| px != &body[..3]),
        "{path:?} is a flat frame"
    );
    (w, h)
}

#[test]
fn shot_dumps_every_screen_for_every_game() {
    let dir = scratch("shot");
    let o = run(&["--size", "80x26", "--shot", dir.to_str().unwrap()]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stderr(&o).contains("wrote frames"), "{}", stderr(&o));

    let mut screens = Vec::new();
    for name in ["attract", "coin", "spin", "land", "help", "paused", "lean"] {
        screens.push(name.to_string());
    }
    for game in ["blocks", "snake", "chomp"] {
        screens.push(game.into());
        screens.push(format!("{game}-over"));
        screens.push(format!("{game}-board"));
    }
    let mut size = None;
    for name in &screens {
        let got = check_ppm(&dir.join(format!("{name}.ppm")));
        // Every screen is the same machine at the same size.
        assert_eq!(*size.get_or_insert(got), got, "{name}");
    }
    let (w, h) = size.unwrap();
    assert_eq!(w, 80, "one sub-pixel per column");
    assert_eq!(h, 52, "two sub-pixels per row");
}
