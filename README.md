# terminaladhd

An arcade for the dead time.

![the machine running](assets/demo.gif)

You hit enter on something slow. The terminal goes black, a wheel picks a game,
and you play until your command comes back.

```
adhd                    play until you quit
adhd -- cargo build     play while that runs, exit with its code
```

## You do not pick the game

The wheel picks. Names fly through the warp toward you, the wheel fights to a
stop, the screen flashes white, and you are playing.

When you die it shows you the run, shows you where it placed on the board, and
spins again — for a different game. The whole ceremony is under four seconds,
because it is time you are not playing in.

That is the whole design. You sat down to wait for a build, not to run a menu,
and the fastest way to stop deciding is to have the machine decide.

| | |
|---|---|
| **BLOCKS** | Guideline tetris — SRS kicks, hold, ghost, T-spins, all-spins, perfect clears, back-to-back and combo. Gravity opens at 355 ms a row and climbs on lines *and* on the clock, so a tidy board is not a way to stay slow — but it floors at 60 ms, because the Guideline curve carried past three minutes is not a game any more. |
| **SNAKE** | The body glides between cells rather than jumping them, which is the whole difference between a snake and a cursor. A 26×14 field — wide, because long runs into tight corners is what snake is. Walls kill, and the one you are running at lights up before you reach it. Six speed tiers, three apples apart, 150 ms a cell down to 75. Apples eaten back to back build a multiplier to 8×, and the numbers are set so the average apple chains and the far corner does not. Every fifth is golden, worth five times its tier and gone in three and a half seconds. |

High scores are kept per game, top ten, in
`$XDG_DATA_HOME/terminaladhd/scores` — or wherever `ADHD_SCORES` points.

## The look

It is a cabinet screen, not a picture with a game on it. The ground is black,
the only light comes from something that is alive, and every pixel is either
play or feedback for play.

Behind the arena a warp field flies outward from a vanishing point, and it is a
readout rather than decoration: at rest it drifts, under heat it stretches, and
on a line clear, an apple or a death it punches into hyperspace for a fifth of a
second and falls back. Play well and the whole screen goes faster.

The rest is the arcade convention, near enough verbatim, because it is chrome
every player already knows how to read: `1UP` blinking over the live score at
the top left, the game's name in the middle, `HI` and the record at the right,
and a hard vector frame around the arena — cyan where the walls are furniture,
amber where they kill. When a game has something to shout — TETRIS, GOLDEN, a
streak — it takes the middle slot off its own name. A machine talks to you from
its status strip.

Both games share one feel and one grammar. An impact stops the whole machine
for a few frames, so a Tetris and a swallowed apple land rather than merely
change colour. Points float up off the cell that earned them in the same face
at the same rate. And the side columns are one shape used twice: a label, a
hairline under it, then content — the queue and the readings on the same left
edge and the same rhythm, in both games. Two games laid out differently read as
two programs.

Everything reflows from `(width, height)` through a single layout solver, and
the arena takes every row the chrome does not need. An 80×26 window and a
400×100 one are the same screen at different scales.

And then it is put behind glass. The last pass is a cathode-ray tube: bloom,
scanlines, corners that fall away, three guns that never quite converge, and a
supply hum crawling down the picture. Cut a screen and the raster collapses to a
line and then to a dot, the way a monitor does when the power goes — every
screen change in the machine is one of those closing and the next one opening.
Hit something big enough and it takes the horizontal hold with it.

Underneath, the picture is drawn at half-block sub-pixel resolution — each cell
carries two stacked square pixels via `▀` — in linear light, with 4×4 ordered
dithering, additive bloom over an emissive buffer, and a damage-tracked diff so
only the cells that actually changed are sent. Frames are bracketed in DEC 2026
synchronized output, so a burst lands at once instead of tearing.

## Wrapping a command

```
$ adhd -- cargo build --release
```

The command's own output scrolls along the ticker at the bottom, and the rule
under the status strip fills as it runs. When it exits, the screen folds away
and `adhd` exits with the command's code — and on a failure, replays the last
lines of its stderr so nothing is swallowed by the alternate screen.

**stdout is never touched.** The screen is drawn on stderr, and the child's
stdout is inherited byte for byte, so this is still exactly `ls | wc -l`:

```
$ adhd -- ls | wc -l
```

With no terminal to draw on — a CI log, a pipe, a cron job — it does not
refuse. It runs the command, hands back its code, and gets out of the way. A
script that works should not stop working the moment someone prefixes it with
`adhd`.

Escape sequences in the child's output are stripped before they reach the
ticker — subprocess text lands inside our frame, so an unsanitized one could
repaint or corrupt the screen.

## Keys

| | |
|---|---|
| `←` `→` or `h` `l` | move / steer |
| `↑` `↓` or `k` `j` | steer; `↑` also rotates |
| `x` / `z` | rotate clockwise / counter-clockwise |
| `space` | hard drop |
| `c` | hold |
| `p` | pause — the picture is held, not lost |
| `?` | the controls, on their own screen |
| any key | skip the ceremony after a run |
| `esc` | leave the game; again to quit |

## Over SSH, and on a phone

Every frame is bytes on a wire. `adhd --bench` measures it: on a 120×34 frame
the full renderer is about 500 KB/s, and almost all of that is the warp field —
faint light that bloom spreads over nearly every cell, which a strict
colour-match then calls a change.

`--lean` stops re-sending changes nobody can see. Same picture, a fifth of the
bytes, thirty frames instead of sixty. It is the **default whenever
`SSH_CONNECTION` is set**, so a phone gets it without being told; `--rich`
forces the full renderer back on.

```
601 KB/s   local, full
 90 KB/s   over SSH, lean
```

It needs 60×24, so a terminal app in landscape is comfortable and portrait
works if the font is small enough. Add `-C` to your `ssh` command and the ANSI
compresses well on top of all this.

## Building

```
cargo build --release
./target/release/adhd
```

No system dependencies. `--size WxH` forces a size, `--bench` reports what a
frame costs, and `--shot DIR` dumps every screen — attract, spin, both games,
game over, the board — as PPM, for reviewing a rendering change without a
terminal in the way.

`DESIGN.md` is the long version: why the screen is shaped this way, what a
frame costs and where it goes, what was tried and thrown away, and what it
would take to add a third game.
