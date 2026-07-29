# terminaladhd

An arcade for the dead time.

![the machine running](assets/demo.gif)

You hit enter on something slow. The terminal goes black, a wheel picks a game,
and you play until your command comes back.

```
adhd                    play until you quit
adhd -- cargo build     play while that runs, exit with its code
```

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

## You do not pick the game

The wheel picks. Names fly through the warp toward you, the wheel fights to a
stop, the screen flashes white, and you are playing.

When you die it shows you the run, shows you where it placed on the board, and
spins again — for a different game. The whole ceremony is about two seconds,
because it is time you are not playing in.

That is the whole design. You sat down to wait for a build, not to run a menu,
and the fastest way to stop deciding is to have the machine decide.

| | |
|---|---|
| **BLOCKS** | Guideline tetris — SRS kicks, hold, ghost, T-spins, all-spins, perfect clears, back-to-back and combo. Gravity opens at 300 ms a row and climbs on lines *and* on the clock, so a tidy board is not a way to stay slow — but it floors at 100 ms, because a curve carried past three minutes is not a game any more. |
| **CHOMP** | A maze chase. Every maze is carved fresh — mirrored like the real cabinets, threaded with loops so there is always a second way out, pierced by a tunnel that wraps — and a cleared board carves the next one, faster. Four ghosts with four minds: one hunts you, one aims ahead of you, one closes the pincer, one loses its nerve up close; they breathe between scatter and chase and the whole pack turns on its heel when the tide changes. A pellet turns them blue and catchable — each one taken in the same hunt pays double the one before. |
| **SNAKE** | The body glides between cells rather than jumping them, which is the whole difference between a snake and a cursor. A wide field that follows the terminal — because long runs into tight corners is what snake is. Walls kill, and the one you are running at lights up before you reach it. Six speed tiers, three apples apart, 150 ms a cell down to 75. Apples eaten back to back build a multiplier to 8×, and the numbers are set so the average apple chains and the far corner does not. Every fifth is golden, worth five times its tier and gone in three and a half seconds. |

High scores are kept per game, top ten, in
`$XDG_DATA_HOME/terminaladhd/scores` — or wherever `ADHD_SCORES` points.

## The look

It is a cabinet screen, not a picture with a game on it. The ground is black,
the only light comes from something that is alive, and every pixel is either
play or feedback for play — in hard chunks: the picture is posterized to a
short ladder of levels, the streaks and sparks are fat two-pixel dashes, the
minos are bordered bricks with knocked corners, and the chrome wordmark is
silkscreened in flat bands. Nothing on this screen fades smoothly, because
nothing on a 1983 screen could.

Behind everything flies the warp field — streaks pouring out of a point
behind the arena, drifting at rest, stretching as you heat up, punching into
hyperspace when something lands. It is drawn as light, so it never covers a
thing you need to read. The idle screens get the rest of the spectacle: a
lamp chase stepping around the marquee, and a real slot reel — names on a
strip behind a lit window with detent marks, overshooting its stop and
rocking back when it lands.

And it has a soundtrack: a hand-rolled chip synth — square waves, noise and
an E-minor loop that is deliberately a little dirty — piped raw into
whatever player the system has. The tempo rides your heat, every kick the
screen strobes to lands as a stinger in the mix, the wheel gets a riser and
a death gets the bottom dropped out. `--mute` (or `ADHD_MUTE=1`) turns it
off; over SSH it stays quiet on its own.

The chrome is one row: the live score at the left, whatever the game has to
shout — TETRIS, GOLDEN, a streak — beside it, the wrapped command's last line,
and the clock at the right. The arena is a slab with a machined bevel for an
edge, and the bevel says what each side does: a wall that kills you wears the
game's own colour, a wall that is only furniture is quiet iron — snake ringed
all the way round, the tetris well all quiet, chomp's boundary quiet too —
in the maze it is the pack that kills, and the tunnel mouths cut visibly
through the frame. Snake plays on checkerboard tile; a wall lights up as you
close on it.

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
carries two stacked square pixels via `▀` — in linear light, posterized to eight
flat levels per channel, additive bloom over an emissive buffer, and a damage-tracked diff so
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
| `a` `d`, `←` `→` or `h` `l` | move / steer |
| `w` `s`, `↑` `↓` or `k` `j` | steer; up also rotates |
| `x` / `z` | rotate clockwise / counter-clockwise |
| `space` | hard drop |
| `c` | hold |
| `p` | pause — the picture is held, not lost |
| `?` | the controls, on their own screen |
| any key | skip the ceremony after a run |
| `esc` or `ctrl-c` | leave the game; again to quit |

## Over SSH, and on a phone

Every frame is bytes on a wire. `adhd --bench` measures it, and the chunky
pass turned out to be a bandwidth pass too: flat posterized colour diffs down
to almost nothing, where the old airbrushed picture nudged half the screen
every frame.

`--lean` halves the frame rate and loosens the colour match on top. It is the
**default whenever `SSH_CONNECTION` is set**, so a phone gets it without being
told; `--rich` forces the full renderer back on.

```
~250 KB/s   local, full, 60 fps — locally this is a memcpy
 ~30 KB/s   over SSH: lean speaks xterm-256 at 30 fps
```

It needs 60×24, so a terminal app in landscape is comfortable and portrait
works if the font is small enough. Add `-C` to your `ssh` command and the ANSI
compresses well on top of all this.

