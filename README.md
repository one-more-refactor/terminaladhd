# terminaladhd

An arcade for the dead time.

You hit enter on something slow. The terminal becomes a synthwave horizon —
indigo sky, a slit-striped sun on the weld, a neon grid running to a vanishing
point — and you play in it until your command comes back.

```
adhd                    play until you quit
adhd -- cargo build     play while that runs, exit with its code
```

## The world

It is one continuous scene, not a set of panels. There are no borders and no
black bars: the sky, the sun and the grid fill every column at any terminal
size, and the game is *cut into* the world rather than boxed on top of it.
Everything reflows from `(width, height)` through a single layout solver, so an
80×24 window and a 270×62 one are the same picture at different scales.

The picture is drawn at half-block sub-pixel resolution — each cell carries two
stacked square pixels via `▀` — in linear light, with 4×4 ordered dithering on
the gradients, additive bloom over an emissive buffer, and a damage-tracked
diff so only the cells that actually changed are sent. Frames are bracketed in
DEC 2026 synchronized output, so a burst lands at once instead of tearing.

## Wrapping a command

```
$ adhd -- cargo build --release
```

The command's own output scrolls along the ticker at the bottom. The sun sinks
as it runs. When it exits, the world folds away and `adhd` exits with the
command's code — and on a failure, replays the last lines of its stderr so
nothing is swallowed by the alternate screen.

**stdout is never touched.** The world is drawn on stderr, and the child's
stdout is inherited byte for byte, so this is still exactly `ls | wc -l`:

```
$ adhd -- ls | wc -l
```

Escape sequences in the child's output are stripped before they reach the
ticker — subprocess text lands inside our frame, so an unsanitized one could
repaint or corrupt the screen.

## Keys

| | |
|---|---|
| `←` `→` or `h` `l` | move |
| `↑` `x` or `k` | rotate |
| `z` | rotate counter-clockwise |
| `↓` or `j` | soft drop |
| `space` | hard drop |
| `c` | hold |
| `esc` | leave the game; again to quit |

Play harder and the world notices — clears, spins and hard drops build heat,
and heat drives how fast the grid rushes at you.

## Building

```
cargo build --release
./target/release/adhd
```

No system dependencies. `--size WxH` forces a size, and `--shot DIR` dumps
frames as PPM for reviewing a rendering change without a terminal in the way.
