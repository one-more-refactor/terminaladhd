# terminaladhd

An arcade for the dead time.

![the machine running](assets/demo.gif)

You hit enter on something slow. The terminal goes dark, a reel of game names
spins past a lit window, and you play until your command comes back.

```
adhd                    play until you quit
adhd -- cargo build     play while that runs, exit with its code
```

## You do not pick the game

The reel picks. Names fly past, any key brakes it, it rocks into its detent,
and you are playing. When you die it shows you the run against your best and
spins again — for a different game. The whole ceremony is about two seconds,
because it is time you are not playing in.

That is the whole design. You sat down to wait for a build, not to run a
menu, and the fastest way to stop deciding is to have the machine decide.

| | |
|---|---|
| **BLOCKS** | A Guideline-shaped well: SRS kicks, hold, ghost, three of lookahead, T-spins, all-spins, perfect clears, back-to-back and combo, with DAS and lock delay in real milliseconds. Gravity opens brisk, tightens every level, and floors where it is still a game — dying is something you did, not something the clock did. |
| **SNAKE** | A bordered field the body *glides* across rather than jumping, which is the whole difference between a snake and a cursor. Six speed tiers, three apples apart. Apples eaten back to back build a multiplier to 8×, tuned so the average apple chains and the far corner does not. Every fifth apple is golden: five times the points, gone in three and a half seconds. |

Best scores are kept per game in `$XDG_DATA_HOME/terminaladhd/scores` — or
wherever `ADHD_SCORES` points.

## The look

The screen is 80×48 one-bit pixels behind the glass of an arcade monitor:
every pixel is lit phosphor or dark glass, and the picture scales up in whole
pixels to fill whatever terminal it is given. At the smallest it is exactly
an 80×24 terminal.

One colour carries everything. Each game owns its phosphor — lime for SNAKE,
ice for BLOCKS — and that tone warms towards gold as you play well, strobes
neon while the reel flies, blows out white when something big lands, flares
red when you die, and turns the whole panel gold on a new best. The glass
does the rest: lit pixels halo into the dark around them, scanlines run
through the ink, a hit shears the rows against each other, and every screen
change collapses the raster to a bright line and a dot, the way a tube loses
its picture when the power goes.

An impact also stops the whole machine for a few frames, so a golden apple
and a tetris *land* rather than merely change colour. Points float up off
the cell that earned them, in the same 3×5 face everything on the machine is
written in.

## Wrapping a command

```
$ adhd -- cargo build --release
```

While it runs, its progress is a hairline across the top of the picture.
When it exits, the screen folds away and `adhd` exits with the command's
code — and on a failure, replays the last lines of its stderr, so nothing is
swallowed by the alternate screen.

**stdout is never touched.** The picture is drawn on stderr and the child's
stdout is inherited byte for byte, so this is still exactly `ls | wc -l`:

```
$ adhd -- ls | wc -l
```

With no terminal to draw on — a CI log, a pipe, a cron job — it does not
refuse. It runs the command, hands back its code, and gets out of the way.
And if the arcade itself ever crashes, the crash is caught, the terminal
restored, and the command waited out: a script that works keeps working with
`adhd` in front of it, no matter what.

## Keys

| | |
|---|---|
| `wasd`, arrows or `hjkl` | steer; up also rotates |
| `x` / `z` | rotate clockwise / counter-clockwise |
| `space` | hard drop |
| `c` | hold |
| any key | brake the reel, skip the settle |
| `esc` | leave the game for the reel; again to quit |

Terminals that speak the kitty keyboard protocol get true key-release
reporting, so held keys feel like a controller instead of OS auto-repeat.

## Over SSH, and on a phone

A one-bit picture in flat tones diffs down to almost nothing — a moving
snake is a few dozen cells a frame — and over SSH the frame rate halves
automatically, which is most of the saving with none of the picture gone.
It needs 80×24, so a phone terminal in landscape is comfortable and portrait
works if the font is small.

## Building

```
cargo build --release
./target/release/adhd
```

No system dependencies. `--size WxH` forces a size, and `--shot DIR` dumps
every screen — the reel, both games, the blowout, the collapse, the settle —
as PPM, for reviewing a rendering change without a terminal in the way.

`DESIGN.md` is the long version: why the picture is fixed and one bit deep,
what was tried and thrown away, and what it takes to add a third game.
