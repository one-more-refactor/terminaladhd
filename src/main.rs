//! `adhd` — an arcade for the dead time.
//!
//!   adhd                       play until you quit
//!   adhd -- cargo build        play while that runs, then hand back its exit code
//!   adhd --size 120x34         force a size instead of asking the terminal
//!
//! You do not choose the game — the machine spins for one, and spins again
//! every time you die.
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

The machine picks the game. Every time you die it spins and picks another,
shows you where the run placed, and starts the next one. Needs 80x26.

keys: arrows or hjkl steer, x or up rotates, z counter-rotates,
      space hard drops, c holds, esc leaves the game then quits

env:  ADHD_SCORES        where the high-score table lives
                         (default $XDG_DATA_HOME/terminaladhd/scores)";

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

/// Dump the frames a change has to be judged on: every screen the machine can
/// show, for every game it can land on.
fn shots(dir: &str, w: usize, h: usize) -> Result<()> {
    use terminaladhd::games::{Kind, ALL};
    use terminaladhd::scores::Entry;
    use terminaladhd::stage::{write_ppm, Settle, Stage, Tick};

    std::fs::create_dir_all(dir)?;
    let tick = Tick {
        left: "COMPILING TERMINALADHD V0.1.0".into(),
        right: "1:07".into(),
    };
    let mut stage = Stage::new(Kind::Tetris, w, h);
    stage.progress = Some(0.62);
    let dump = |stage: &Stage, name: &str| -> Result<()> {
        let (px, pw, sh) = stage.sub_pixels();
        write_ppm(&format!("{dir}/{name}.ppm"), px, pw, sh)?;
        Ok(())
    };

    // Give the warp field a moment of flight so no still is of frame zero.
    for _ in 0..90 {
        stage.animate(0.016, 0.2);
    }

    let demo = play(Kind::Snake, 0);
    stage.attract(demo.as_ref(), 9200, true, 0.0, &tick);
    dump(&stage, "attract")?;

    stage.spin(&[Kind::Tetris, Kind::Snake, Kind::Tetris], 0.68, 9200, true, &tick);
    dump(&stage, "spin")?;

    // Mid-cut: the raster half collapsed, which is what every screen change
    // passes through and the thing a still cannot otherwise show.
    let mid = play(Kind::Tetris, 400);
    for (name, t) in [("cut-early", 0.30f32), ("cut-late", 0.72)] {
        stage.curtain = t;
        stage.game(mid.as_ref(), 9200, true, &tick);
        dump(&stage, name)?;
    }
    stage.curtain = 0.0;

    // The loudest frame the machine has: hold lost, guns apart, chassis moved.
    stage.tear = 9.0;
    stage.fringe = 6.0;
    stage.jolt = (2, 2);
    stage.game(mid.as_ref(), 9200, true, &tick);
    dump(&stage, "hit")?;

    for kind in ALL {
        stage.retarget(kind);
        // Stopped a couple of frames after a score lands, so the still shows a
        // marker in the air rather than a board at rest.
        let game = play(kind, 900);

        stage.game(game.as_ref(), 9200, true, &tick);
        dump(&stage, kind.slug())?;

        stage.over(
            game.as_ref(),
            &Settle {
                fade: 1.0,
                shown: game.score(),
                record: true,
            },
            9200,
            true,
            &tick,
        );
        dump(&stage, &format!("{}-over", kind.slug()))?;

        let rows: Vec<Entry> = [9200u32, 7710, 4820, 3300, 2150, 1400, 900, 420]
            .iter()
            .enumerate()
            .map(|(i, &score)| Entry {
                score,
                at: 1_700_000_000 + i as u64,
            })
            .collect();
        stage.board(kind, &rows, Some(2), 0.1, 9200, &tick);
        dump(&stage, &format!("{}-board", kind.slug()))?;
    }

    let (_, pw, sh) = stage.sub_pixels();
    eprintln!("wrote frames to {dir} at {pw}x{sh} sub-pixels");
    Ok(())
}

/// Play a game headlessly on its own autopilot — the same brain the attract
/// screen uses, so a still shows exactly what a player would see rather than
/// something a test harness arranged.
fn play(kind: terminaladhd::games::Kind, steps: u32) -> Box<dyn terminaladhd::games::Game> {
    use std::time::Duration;
    use terminaladhd::rng::Rng;

    let mut game = kind.spawn(Rng::from_seed(7));
    for i in 0..steps {
        if game.is_over() {
            break;
        }
        let input = game.autopilot();
        game.step(&input, Duration::from_millis(16));
        // Past the halfway mark, stop on the first frame that has a marker in
        // the air: a still of a board at rest says nothing about how the game
        // pays out.
        if i > steps / 2 && !game.pops().is_empty() {
            break;
        }
    }
    game
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
