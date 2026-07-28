//! `adhd` — an arcade for the dead time.
//!
//!   adhd                       play until you quit
//!   adhd -- cargo build        play while that runs, then hand back its exit code
//!   adhd --size 120x34         force a size instead of asking the terminal
//!
//! You do not choose the game — the machine spins for one, and spins again
//! every time you die.
//!
//! The picture is drawn on stderr. stdout belongs to the wrapped command, so
//! `adhd -- ls | wc -l` is still just `ls | wc -l`.

use std::panic::{catch_unwind, AssertUnwindSafe};
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

The machine picks the game. Every time you die it spins and picks another.
Needs an 80x24 terminal; anything bigger scales the picture up.

keys: wasd, arrows or hjkl steer; x or up rotates, z counter-rotates,
      space hard drops, c holds, esc leaves the game then quits

env:  ADHD_SCORES         where the high-score table lives
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

/// The arcade behind a crash barrier: a panic in the loop must not take the
/// process with it, because the process may be carrying someone's command.
/// `None` means it went down — the terminal is already restored (the panic
/// hook in [`term::Guard`] runs before the unwind) and the panic itself is
/// already on stderr.
fn arcade(host: &mut dyn app::Host, w: usize, h: usize) -> Result<Option<Exit>> {
    match catch_unwind(AssertUnwindSafe(|| app::run(host, w, h))) {
        Ok(result) => result.map(Some),
        Err(_) => Ok(None),
    }
}

/// Dump the frames a change has to be judged on: every screen the machine can
/// show, for every game it can land on, composed exactly as a terminal would
/// see them.
fn shots(dir: &str, w: usize, h: usize) -> Result<()> {
    use std::time::Duration;
    use terminaladhd::app::{draw_over, draw_reel, Reel};
    use terminaladhd::games::{Kind, ALL};
    use terminaladhd::rng::Rng;
    use terminaladhd::screen::{write_ppm, Fx, Monitor, Phosphor, Screen};

    if !Monitor::fits(w, h) {
        bail!("--shot needs at least {}x{}", term::MIN_SIZE.0, term::MIN_SIZE.1);
    }
    std::fs::create_dir_all(dir)?;
    let mut canvas = Screen::new();
    let mut monitor = Monitor::fit(w, h);
    let mut dump = |canvas: &Screen, ph: Phosphor, fx: Fx, name: &str| -> Result<()> {
        let cells = monitor.compose(canvas, ph, fx, true);
        write_ppm(&format!("{dir}/{name}.ppm"), cells, w, h)?;
        Ok(())
    };

    // The reel at rest on each game, with its hint line up.
    for kind in ALL {
        let reel = Reel::parked(kind);
        canvas.clear();
        draw_reel(&mut canvas, &reel, 9200);
        dump(&canvas, kind.phosphor(), Fx::default(), &format!("reel-{}", kind.slug()))?;
    }

    // Each game mid-run on its own autopilot — the same brain a demo would
    // use, so a still shows what a player would see rather than something a
    // test harness arranged.
    for kind in ALL {
        let mut game = kind.spawn(Rng::from_seed(7));
        for _ in 0..900 {
            if game.is_over() {
                break;
            }
            let input = game.autopilot();
            game.step(&input, Duration::from_millis(16));
        }
        canvas.clear();
        game.draw(&mut canvas);
        let ph = kind.phosphor().mix(Phosphor::GOLD, game.heat() * 0.85);
        dump(&canvas, ph, Fx::default(), kind.slug())?;
    }

    // The loud moments a still can otherwise never catch: the blowout, the
    // recoil, and the raster half-collapsed mid-cut.
    let mut game = Kind::Snake.spawn(Rng::from_seed(7));
    for _ in 0..400 {
        let input = game.autopilot();
        game.step(&input, Duration::from_millis(16));
    }
    canvas.clear();
    game.draw(&mut canvas);
    dump(&canvas, Phosphor::LIME.flash(0.7), Fx { shake: 1.0, cut: 0.0 }, "hit")?;
    dump(&canvas, Phosphor::LIME, Fx { shake: 0.0, cut: 0.55 }, "cut")?;

    canvas.clear();
    draw_over(&mut canvas, 9200, 4800, true, 0.1);
    dump(&canvas, Phosphor::GOLD, Fx::default(), "over")?;

    eprintln!("wrote frames to {dir} at {w}x{h} cells");
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
            "terminal is {w}x{h}; the picture needs at least {}x{}",
            term::MIN_SIZE.0,
            term::MIN_SIZE.1
        );
    }

    if let Some(dir) = args.shot {
        return shots(&dir, w, h).map(|_| 0);
    }

    let Some(argv) = args.command else {
        if !term::attached() {
            bail!("no terminal on stderr; there is nothing to play on");
        }
        return match arcade(&mut Forever, w, h)? {
            // The panic hook has already restored the terminal and printed
            // the panic; all that is left is to not pretend it went well.
            None => Ok(1),
            Some(_) => Ok(0),
        };
    };

    let mut cmd = Command::spawn(&argv)?;

    // No terminal — a CI log, a pipe, a cron job. Run the command and get out
    // of the way. Refusing here would mean a script that works becomes a
    // script that fails the moment someone prefixes it with `adhd`, and the
    // one promise this makes is that it never costs you your command.
    if !term::attached() {
        while !cmd.is_done() {
            std::thread::sleep(app::STEP);
        }
        let code = cmd.exit_code();
        if code != 0 {
            cmd.replay_tail()?;
        }
        return Ok(code);
    }

    let exit = arcade(&mut cmd, w, h)?;

    // The player leaving is not the command leaving — and neither is the
    // arcade crashing. Keep waiting, quietly, so the exit code we hand back
    // is always the command's own.
    if exit != Some(Exit::Finished) {
        if exit.is_none() {
            eprintln!(
                "adhd: the arcade crashed — `{}` is unaffected, waiting for it",
                cmd.label()
            );
        }
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
