//! `adhd` — an arcade for the dead time.
//!
//!   adhd                       play until you quit
//!   adhd -- cargo build        play while that runs, then hand back its exit code
//!   adhd --size 120x34         force a size instead of asking the terminal
//!
//! The world is drawn on stderr. stdout belongs to the wrapped command, so
//! `adhd -- ls | wc -l` is still just `ls | wc -l`.

use std::process::ExitCode;

use anyhow::{bail, Result};

use terminaladhd::app::{self, Exit, Forever};
use terminaladhd::term;
use terminaladhd::wrap::{split_argv, Command};

const USAGE: &str = "\
usage: adhd [--size WxH] [-- <command>...]

  adhd                    play until you quit
  adhd -- cargo build     play while that runs, exit with its code
  adhd --size 120x34      force a size instead of asking the terminal

keys: arrows or hjkl move, up/x rotate, z counter-rotate,
      space hard drop, c hold, esc leaves the game then quits";

struct Args {
    size: Option<(usize, usize)>,
    command: Option<Vec<String>>,
    help: bool,
    shot: Option<String>,
}

fn parse(argv: Vec<String>) -> Result<Args> {
    let (ours, command) = split_argv(&argv)?;
    let mut out = Args {
        size: None,
        command,
        help: false,
        shot: None,
    };
    let mut it = ours.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--size" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--size needs WxH"))?;
                let (w, h) = v
                    .split_once(['x', 'X'])
                    .ok_or_else(|| anyhow::anyhow!("--size wants WxH, got {v:?}"))?;
                out.size = Some((w.trim().parse()?, h.trim().parse()?));
            }
            "--shot" => {
                out.shot = Some(it.next().ok_or_else(|| anyhow::anyhow!("--shot needs a dir"))?);
            }
            "-h" | "--help" => out.help = true,
            "-V" | "--version" => {
                println!("adhd {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => bail!("unknown argument {other:?} (try --help)"),
        }
    }
    Ok(out)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("adhd: {e:#}");
            ExitCode::from(1)
        }
    }
}

/// Dump the frames a change has to be judged on: the attract screen, a live
/// board, and the same board with the sun most of the way down.
fn shots(dir: &str, w: usize, h: usize) -> Result<()> {
    use std::time::Duration;
    use terminaladhd::games::tetris::{Input, Tetris};
    use terminaladhd::stage::{write_ppm, Stage};

    std::fs::create_dir_all(dir)?;
    let mut stage = Stage::new(w, h);

    stage.attract(0.35, "BLOCKS", 2, "INSERT COIN - ENTER TO PLAY", "0:00 -----");
    let (px, pw, sh) = stage.sub_pixels();
    write_ppm(&format!("{dir}/attract.ppm"), px, pw, sh)?;

    // Drop a handful of pieces so the well is not empty in the still.
    let mut game = Tetris::new();
    for i in 0..600 {
        let mut input = Input::default();
        if i % 40 == 0 {
            input.hard = true;
        } else if i % 7 == 0 {
            input.left = true;
        }
        game.step(&input, Duration::from_millis(16));
    }

    for (name, sink) in [("play", 0.0f32), ("sunset", 0.75)] {
        stage.sun_sink = sink;
        stage.game(&game, 0.4, 0.0, "COMPILING TERMINALADHD V0.1.0", "1:07 ###..");
        let (px, pw, sh) = stage.sub_pixels();
        write_ppm(&format!("{dir}/{name}.ppm"), px, pw, sh)?;
    }

    eprintln!("wrote attract/play/sunset to {dir} at {pw}x{sh} sub-pixels");
    Ok(())
}

fn run() -> Result<i32> {
    let args = parse(std::env::args().skip(1).collect())?;
    if args.help {
        println!("{USAGE}");
        return Ok(0);
    }

    let (w, h) = args.size.unwrap_or_else(term::size);
    if w < term::MIN_SIZE.0 || h < term::MIN_SIZE.1 {
        bail!(
            "terminal is {w}x{h}; the world needs at least {}x{}",
            term::MIN_SIZE.0,
            term::MIN_SIZE.1
        );
    }

    if let Some(dir) = args.shot {
        return shots(&dir, w, h).map(|_| 0);
    }

    let Some(argv) = args.command else {
        app::run(&mut Forever, w, h)?;
        return Ok(0);
    };

    let mut cmd = Command::spawn(&argv)?;
    let exit = app::run(&mut cmd, w, h)?;

    // The player leaving is not the command leaving: keep waiting, quietly, so
    // the exit code we hand back is always the command's own.
    if exit == Exit::Quit {
        while !cmd.is_done() {
            std::thread::sleep(app::STEP);
        }
    }

    let code = cmd.exit_code();
    if code != 0 {
        eprintln!("adhd: `{}` exited {code}", cmd.label());
        cmd.replay_tail()?;
    }
    Ok(code)
}
