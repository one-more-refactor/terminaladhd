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

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::ExitCode;

use anyhow::{bail, Result};

use terminaladhd::app::{self, Exit, Forever};
use terminaladhd::stage::Quality;
use terminaladhd::term;
use terminaladhd::wrap::{split_argv, Command};

const USAGE: &str = "\
usage: adhd [--size WxH] [-- <command>...]

  adhd                    play until you quit
  adhd -- cargo build     play while that runs, exit with its code
  adhd --size 120x34      force a size instead of asking the terminal

The machine picks the game. Every time you die it spins and picks another,
shows you where the run placed, and starts the next one. Needs 80x26.

keys: wasd, arrows or hjkl steer; x or up rotates, z counter-rotates,
      space hard drops, c holds, p pauses, ? shows them all,
      esc leaves the game then quits

  --lean / --rich       force the low-bandwidth or the full renderer.
                        Over SSH lean is the default: it is a fifth of the
                        bytes and half the frames, for the same picture.
  --bench               report what a frame costs down a wire

env:  ADHD_SCORES        where the high-score table lives
                         (default $XDG_DATA_HOME/terminaladhd/scores)";

struct Args {
    size: Option<(usize, usize)>,
    command: Option<Vec<String>>,
    help: bool,
    shot: Option<String>,
    bench: bool,
    lean: Option<bool>,
}

fn parse(argv: Vec<String>) -> Result<Args> {
    let (ours, command) = split_argv(&argv)?;
    let mut out = Args {
        size: None,
        command,
        help: false,
        shot: None,
        bench: false,
        lean: None,
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
            "--bench" => out.bench = true,
            "--lean" => out.lean = Some(true),
            "--rich" => out.lean = Some(false),
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
fn arcade(
    host: &mut dyn app::Host,
    w: usize,
    h: usize,
    quality: Quality,
) -> Result<Option<Exit>> {
    match catch_unwind(AssertUnwindSafe(|| app::run(host, w, h, quality))) {
        Ok(result) => result.map(Some),
        Err(_) => Ok(None),
    }
}

/// Dump the frames a change has to be judged on: every screen the machine can
/// show, for every game it can land on.
fn shots(dir: &str, w: usize, h: usize) -> Result<()> {
    use terminaladhd::games::{Game, Kind, ALL};
    use terminaladhd::rng::Rng;
    use terminaladhd::scores::Entry;
    use terminaladhd::stage::{write_ppm, Settle, Stage, Tick};

    std::fs::create_dir_all(dir)?;
    let tick = Tick {
        left: "COMPILING TERMINALADHD V0.1.0".into(),
        right: "1:07".into(),
    };
    let mut stage = Stage::new(Kind::Tetris, Kind::Tetris.field(w, h), w, h);
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

    let demo = play(Kind::Snake, 0, w, h);
    stage.attract(demo.as_ref(), 9200, 0.0, &tick);
    dump(&stage, "attract")?;

    stage.coin(demo.as_ref(), 0.1, &tick);
    dump(&stage, "coin")?;

    stage.spin(&[Kind::Tetris, Kind::Snake, Kind::Tetris], 0.68, &tick);
    dump(&stage, "spin")?;

    // The frame the wheel stops on, with the winner doubled off itself.
    stage.slam = 1.0;
    stage.spin(&[Kind::Snake, Kind::Tetris, Kind::Snake], 1.0, &tick);
    dump(&stage, "land")?;
    stage.slam = 0.0;

    // Mid-cut: the raster half collapsed, which is what every screen change
    // passes through and the thing a still cannot otherwise show.
    let mid = play(Kind::Tetris, 400, w, h);
    for (name, t) in [("cut-early", 0.30f32), ("cut-late", 0.72)] {
        stage.curtain = t;
        stage.game(mid.as_ref(), &tick);
        dump(&stage, name)?;
    }
    stage.curtain = 0.0;

    // The same frame at the tolerance a metered link gets, for judging what
    // the saving actually costs to look at.
    stage.quality = terminaladhd::stage::Quality::lean();
    stage.game(mid.as_ref(), &tick);
    dump(&stage, "lean")?;
    stage.quality = terminaladhd::stage::Quality::full();

    // A row coming apart, which is the one thing a still of a working game
    // almost never catches.
    {
        use std::time::Duration;
        use terminaladhd::games::{Input, Tetris};
        let mut g = Tetris::with_rng(Rng::from_seed(2));
        g.debris(6);
        for _ in 0..5 {
            g.step(&Input::default(), Duration::from_millis(16));
        }
        stage.game(&g, &tick);
        dump(&stage, "sparks")?;
    }

    // A piece caught between rows and a hard drop's streak still in the air —
    // the two things a still normally cannot show about how it moves.
    {
        use std::time::Duration;
        use terminaladhd::games::Input;
        let mut g = terminaladhd::games::Tetris::with_rng(Rng::from_seed(11));
        for _ in 0..40 {
            g.step(&Input::default(), Duration::from_millis(16));
        }
        g.step(
            &Input {
                hard: true,
                ..Default::default()
            },
            Duration::from_millis(16),
        );
        g.step(&Input::default(), Duration::from_millis(16));
        stage.game(&g, &tick);
        dump(&stage, "motion")?;
    }

    // The quiet screens before the loud ones: a fired strobe outlives its
    // frame by design, and a help screen shot through its afterglow reads as
    // a rendering bug rather than as the help screen.
    stage.help(Kind::Tetris, &tick);
    dump(&stage, "help")?;

    stage.paused(mid.as_ref(), &tick);
    dump(&stage, "paused")?;

    // Every beat of the loudest reaction the machine has, so a still can show
    // what a Tetris actually looks like rather than only describing it.
    stage.fire(
        terminaladhd::stage::strobe::HUGE,
        terminaladhd::world::hex(0xFFFFFF),
    );
    for i in 0..4 {
        stage.game(mid.as_ref(), &tick);
        dump(&stage, &format!("huge-{i}"))?;
    }

    stage.tear = 9.0;
    stage.fringe = 6.0;
    stage.jolt = (2, 2);
    stage.game(mid.as_ref(), &tick);
    dump(&stage, "hit")?;

    for kind in ALL {
        stage.retarget(kind, kind.field(w, h));
        // Stopped a couple of frames after a score lands, so the still shows a
        // marker in the air rather than a board at rest.
        let game = play(kind, 900, w, h);

        stage.game(game.as_ref(), &tick);
        dump(&stage, kind.slug())?;

        stage.over(
            game.as_ref(),
            &Settle {
                fade: 1.0,
                shown: game.score(),
                record: true,
                tally: game.tally(),
            },
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
        stage.board(kind, &rows, Some(2), 0.1, &tick);
        dump(&stage, &format!("{}-board", kind.slug()))?;
    }

    let (_, pw, sh) = stage.sub_pixels();
    eprintln!("wrote frames to {dir} at {pw}x{sh} sub-pixels");
    Ok(())
}

/// Play a game headlessly on its own autopilot — the same brain the attract
/// screen uses, so a still shows exactly what a player would see rather than
/// something a test harness arranged.
fn play(
    kind: terminaladhd::games::Kind,
    steps: u32,
    w: usize,
    h: usize,
) -> Box<dyn terminaladhd::games::Game> {
    use std::time::Duration;
    use terminaladhd::rng::Rng;

    let mut game = kind.spawn(Rng::from_seed(7), w, h);
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
            // Well past the hit, so the still shows the game rather than the
            // frames the screen happened to be reacting on.
            for _ in 0..14 {
                let input = game.autopilot();
                game.step(&input, Duration::from_millis(16));
            }
            break;
        }
    }
    game
}

/// What a frame costs down a wire, and what each pass costs of that.
///
/// The number that matters is bytes per second, not per frame: a link that can
/// carry the picture at sixty frames cannot necessarily carry it at all, and
/// the only honest way to find out is to encode real frames of a real game and
/// count the diff.
fn bench(w: usize, h: usize) -> Result<()> {
    use std::time::{Duration, Instant};
    use terminaladhd::games::Kind;
    use terminaladhd::stage::{Quality, Stage, Tick};
    use terminaladhd::world::{enc_diff, Cell};

    const FRAMES: usize = 300;
    let tick = Tick {
        left: "COMPILING TERMINALADHD V0.1.0".into(),
        right: "1:07".into(),
    };

    let measure = |kind: Kind, q: Quality| -> (usize, f64, f64) {
        let mut stage = Stage::new(kind, kind.field(w, h), w, h);
        stage.quality = q;
        let mut game = kind.spawn(terminaladhd::rng::Rng::from_seed(7), w, h);
        let mut prev: Vec<Cell> = vec![Default::default(); w * h];
        let mut out: Vec<u8> = Vec::new();
        let mut total = 0usize;
        let started = Instant::now();
        for _ in 0..FRAMES {
            if game.is_over() {
                game = kind.spawn(terminaladhd::rng::Rng::from_seed(7), w, h);
            }
            let input = game.autopilot();
            game.step(&input, Duration::from_millis(16));
            stage.animate(0.016, game.heat());
            stage.game(game.as_ref(), &tick);
            enc_diff(&stage.cells, &prev, w, h, q.tol, 6, &mut out);
            total += out.len();
            prev.copy_from_slice(&stage.cells);
        }
        let cpu_ms = started.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
        (total / FRAMES, total as f64 * 60.0 / FRAMES as f64, cpu_ms)
    };

    println!("terminaladhd bench — {w}x{h}, {FRAMES} frames of real play
");
    println!("{:<26} {:>10} {:>12} {:>9}", "", "bytes/frame", "KB/s at 60", "ms/frame");

    let full = Quality::full();
    let variants: [(&str, Quality); 9] = [
        ("full", full),
        ("full, no warp", Quality { warp: false, ..full }),
        ("full, no hum", Quality { hum: false, ..full }),
        ("full, no fringe", Quality { fringe: false, ..full }),
        ("full, no bloom", Quality { bloom: false, ..full }),
        ("full, tol 6", Quality { tol: 6, ..full }),
        ("full, tol 14", Quality { tol: 14, ..full }),
        ("full, tol 24", Quality { tol: 24, ..full }),
        ("lean", Quality::lean()),
    ];
    for kind in [Kind::Tetris, Kind::Snake] {
        println!("\n  {}", kind.name());
        for (name, q) in variants {
            let (per, rate, cpu) = measure(kind, q);
            println!(
                "  {:<24} {:>10} {:>12.0} {:>9.2}",
                name,
                per,
                rate / 1024.0,
                cpu
            );
        }
    }
    println!(
        "\nA cell costs up to 40 bytes: two truecolor SGRs and a three-byte glyph.\n\
         {} cells on this frame, so a full repaint is {} KB.",
        w * h,
        w * h * 40 / 1024
    );
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

    if args.bench {
        return bench(w, h).map(|_| 0);
    }

    // Over SSH every frame is bytes on a wire, and on a phone it may be bytes
    // on a cellular plan. Measured on a 120x34 frame: 498 KB/s rich, 54 KB/s
    // lean. Nobody should have to know that before it is usable, so the default
    // follows the link.
    let quality = match args.lean {
        Some(true) => Quality::lean(),
        Some(false) => Quality::full(),
        None if term::remote() => Quality::lean(),
        None => Quality::full(),
    };

    let Some(argv) = args.command else {
        if !term::attached() {
            bail!("no terminal on stderr; there is nothing to play on");
        }
        return match arcade(&mut Forever, w, h, quality)? {
            // The panic hook has already restored the terminal and printed
            // the panic; all that is left is to not pretend it went well.
            None => Ok(1),
            Some(_) => Ok(0),
        };
    };

    let mut cmd = Command::spawn(&argv)?;

    // No terminal — a CI log, a pipe, a cron job. Run the command and get out
    // of the way. Refusing here would mean a script that works becomes a script
    // that fails the moment someone prefixes it with `adhd`, and the one
    // promise this makes is that it never costs you your command.
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

    let exit = arcade(&mut cmd, w, h, quality)?;

    // The player leaving is not the command leaving — and neither is the
    // arcade crashing. Keep waiting, quietly, so the exit code we hand back is
    // always the command's own.
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
