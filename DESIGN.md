# How the machine is built

Notes on why this is shaped the way it is. The code says what it does; this
says what it is for, and what was tried and thrown away.

## The premise

You start something slow. For the next two minutes you are not working, you are
waiting, and waiting is the worst-designed part of a developer's day. `adhd`
takes those two minutes and makes them an arcade.

Two consequences fall out of that immediately and both are load-bearing:

**You do not pick the game.** A menu is a decision, and the whole reason you
reached for this is that you did not want to decide anything. A wheel picks, and
picks again after every death. The rotation is not a feature bolted on top; it
is the product.

**Nothing may cost you your command.** stdout belongs to the child, always. The
exit code is the child's. If the command fails, its stderr is replayed rather
than swallowed by the alternate screen. With no terminal at all it runs the
command and gets out of the way rather than refusing. A toy that eats your build
output — or that turns a working script into a failing one the moment you prefix
it — is not a toy you keep installed.

## One rule for the screen

> Every pixel is either the game or feedback for the game.

The first version broke this. It was a synthwave landscape — indigo sky, a
slit-striped sun, a perspective grid running to a vanishing point — with the
playfield cut into it. It was pretty. It was also a picture with a game on it,
which is a 2015 album cover rather than a 1983 arcade machine, and it failed the
rule on almost every pixel.

Deleting the scenery deleted about as much code again, and that is the tell that
the rule was right. The smoked plates behind every sign, the contrast fallbacks,
the folded stat stacks, the "will this read against the magenta band" branches —
all of that existed only to fight the picture. On black, ink is legible
everywhere, and none of it is needed.

What is on screen now:

| layer | what it is | why it earns its place |
|---|---|---|
| ground | flat near-black | the black a monitor is at rest |
| warp field | streaks flying outward from behind the arena | a readout of how well you are doing |
| arena | the game | the game |
| frame | a hard vector rectangle round the arena | says which game, and whether the walls kill |
| columns | HOLD, NEXT, the readings | the numbers you are chasing |
| strip | `1UP` / score / name / `HI` | conventions every player can already read |
| monitor | bloom, scanlines, vignette, fringe, hum | says *tube* rather than *panel* |

The warp field is the one that does the most work. It is not decoration: at rest
it drifts, under the player's heat it stretches, and on a clear, an apple or a
death it punches into hyperspace for a fifth of a second and falls back. Play
well and the whole screen goes faster. No game knows it exists — games bank an
impact, the shell spends it.

## Everything reflows from (width, height)

`world::layout` is the only place a screen coordinate is allowed to come from. A
raw literal reaching a draw call has to be a visible smell rather than a silent
bug, so `Layout::for_field(w, h, cols, rows)` derives every rectangle and every
draw call takes them from there.

The rule it enforces is that **the arena takes every row the chrome does not
need**. There is no scenery to protect any more, so the game gets the screen.

Two constants come out of hard arithmetic rather than taste:

- **`MIN_SIZE` is 80×26.** A twenty-row playfield at the smallest legible block
  is twenty rows. The status face is five sub-rows and a cell is two, so the
  strip and the ticker need three rows each. Twenty-six. No arrangement of the
  chrome gets it lower without a second, shorter font.
- **Text anchors in sub-rows, not rows.** Chasing that down took three bugs —
  the progress rule drawn through the score, the ticker hanging off the bottom
  of the frame, the board's title landing on the strip — all the same mistake.

## Feel is shared, not per-game

Two games that respond differently to the same event read as two programs. So
the vocabulary lives on the `Game` trait and the shell spends it in one place:

- **`take_hitstop()`** — render frames the whole machine freezes for. Time does
  not accumulate during it; it is a debt paid in frames, not wall time the sim
  owes itself afterwards. Three frames for an apple, ten for a death. This is
  the single largest difference between a clear that *lands* and one that merely
  changes colour.
- **`take_punch()`** — impact, `0..1`, which the shell spends on the warp field,
  the screen flash and, past a threshold, on the monitor itself.
- **`pops()`** — `+N` markers floating off the cell that earned them. One face,
  one rate, both games.
- **`shout()`** — what to say. It goes into the status strip's middle slot,
  taking it off the game's own name. A machine talks to you from its status
  strip; a banner floating over the playfield is a modern habit, and at most
  sizes there is nowhere to put one that does not collide with something.

And one grammar for the side columns: `cabinet::heading` draws a label with a
hairline fading out under it and returns where content starts. Both games stack
everything through it, so the queue and the readings share a left edge and a
rhythm.

## Snake glides

The reference for game feel is `blipscreen`'s snake, and the thing it does that
matters is that the body does not teleport. The logic moves in whole cells, but
every segment is drawn part of the way between where it was and where it is, on
an eased move budget.

The drawn body deliberately trails the logical one by one cell. That lag *is*
the animation. Collision, self-intersection and apple placement only ever look
at the logical cells; the interpolation exists strictly between the grid and the
screen.

A snake that jumps a cell at a time reads as a cursor. The same snake sliding
the same distance reads as an animal, and it is the same game underneath.

## Balance

Both games are tuned against `blipscreen`'s, which is the version that felt
right, and the numbers were diffed rather than guessed. Where they differ now it
is deliberate.

**Snake.** The speed curve is the reference's exactly: six tiers three apples
apart, 150 ms a cell down to 75 ms, reached around the fifteenth apple. What had
drifted was everything around it.

The field was 20×20. A square arena leaves the player equidistant from
everything and the game loses its rhythm — long runs into tight corners is what
snake *is*. It is 26×14 now, about twice as wide as it is tall, which is both
the shape the game has always been played on and the shape of a terminal.

The bonus apple used to grow the snake by three cells. The bonus is already the
risk of crossing the field for it under a clock; charging extra length on top
makes taking it a punishment for succeeding. It grows by one, like any apple.

Its clock was six seconds and the streak window was 3.2, both of which made the
game slack. They are 3.5 and 2.5, which puts them either side of a line worth
being precise about:

| | tier 0 | tier 5 |
|---|---:|---:|
| average apple | 2.0 s | 1.0 s |
| far corner | 5.7 s | 2.9 s |
| streak window | 2.5 s | 2.5 s |

The average apple is inside the window and the far corner is outside it, at
every tier. That is the whole tension: if every apple chained the multiplier
would be free, and if none did it would be decoration. And because the window
does not shrink as the snake speeds up, chaining gets *easier* the further you
get — which is the reward for surviving the climb. Tests assert both halves.

**Tetris** had a real bug rather than a drift. The Guideline gravity curve is
exponential and has no floor, and this machine hands out a level on the clock
whether or not anyone is clearing lines. Carried three minutes it passed twenty
rows a frame, so a long build ended in a board nobody could have played.

There is a floor now, at 60 ms a row, reached at about two and a half minutes.
It is brutal and still human, because a grounded piece keeps its full
500 ms lock window whatever gravity is doing — the floor governs how long you
have to think, not how long you have to place. The curve runs 355 ms at the
start, 190 at a minute, 135 at ninety seconds, and settles.

## The monitor

`world::crt` is the last pass, and everything in it is an artefact of a real
cathode-ray tube rather than a filter someone liked:

- **vignette** — the corners of a tube fall away. Cheapest pass, does the most.
- **fringe** — three guns never landed on the same spot, and the misconvergence
  grew toward the edges, which is why it scales with distance from the centre.
- **hum** — a band of lifted brightness crawling down the picture, mains
  frequency beating against the frame rate. The single artefact that most
  reliably says "this is not a still image".
- **shake** — the chassis, not the arena. Games already shake their own field.
- **tear** — lost horizontal hold, in bands rather than rows. Per-row offsets
  read as noise; a band of rows moving together reads as the picture slipping.
- **collapse** — cut the power and the raster squeezes to a line and then to a
  dot, brightening as its energy packs into fewer rows.

That last one is why every screen change in the machine is a cut. The tube
collapses, the mode swaps behind the dark, and it opens on the next screen. The
exception is the crash: the death, the dissolve and the settle are one
continuous thing, and cutting away from it would throw out the part worth
watching. `Machine::go` cuts, `Machine::slide` does not.

Ordering in the chain is not arbitrary. The flash is light arriving at the
glass, so it is first. The guns are behind the glass, so the fringe is next. Hum
and vignette are properties of the tube. The shake moves the whole chassis. The
collapse is the power going, and it happens to whatever the screen was showing.

## What a frame costs

Every pass in the chain costs bytes on a wire, not just cycles, and on a phone
over cellular that is the whole bill. `adhd --bench` encodes three hundred
frames of real play and counts the diff. On a 120×34 frame:

| | bytes/frame | KB/s at 60 |
|---|---:|---:|
| full | 8503 | 498 |
| full, no warp field | 595 | 35 |
| full, no hum | 8299 | 486 |
| full, no fringe | 8503 | 498 |
| full, tolerance 14 | 1821 | 107 |
| lean | 1792 | 105 |

The result is not what it looks like from the code. The CRT passes are nearly
free — the hum costs two hundred bytes a frame and the fringe measures as zero,
because it only rewrites cells that were already being re-sent. **The warp field
is ninety-three per cent of the traffic**, and not because it is large: it is
faint, and bloom spreads that faintness across almost every cell, and a
two-level colour-match tolerance calls almost every one of those a change.

So the fix is not to delete the field. It is to stop re-sending changes nobody
can see: at tolerance 14 the same picture costs a fifth. That, half the frame
rate, and dropping the two passes that dirty cells nothing asked to change is
what `--lean` is, and it is the default whenever `SSH_CONNECTION` is set.

Measured end to end through a pty: 601 KB/s local, 90 KB/s over SSH.

A cell costs up to forty bytes — two truecolor SGRs and a three-byte glyph — so
a full repaint of that frame is 159 KB. Everything above is the diff doing its
job; the question was only ever how much of the frame it decides has changed.

## Things that were deliberately not done

- **A third game.** Two games done properly beat three done adequately, and the
  rotation reads as variety at two.
- **Configurable keybindings.** A TOML file is a decision, and see the premise.
- **A menu, difficulty select, or game modes.** Same reason.
- **Screen curvature.** It is the one CRT artefact that would have to resample
  text, and text that wobbles is not authenticity, it is a bug.
- **Letting the player replay the same game.** Tempting, and rejected: the
  moment you can choose, the machine is a menu with extra steps.
- **An adaptive snake arena.** It should have one — snake does not care whether
  it is 20×20 or 12×12, and on a 60-column phone the fixed size eats the flank
  its readings need. What stopped it is that a resize would then change the
  arena under a running snake whose body is already outside the new one. It
  wants the layout to follow the game's field rather than the game's kind, and
  that is a change worth making carefully rather than at the end of a session.

## Adding a game

One `games::Kind` variant and one module. `Kind` carries the arena size, the
marquee name, the slug its scores are filed under, the frame hue, the control
hint and how to spawn it; the module implements `games::Game`. Everything else
— layout, columns, hitstop, pops, the wheel, the board — already works.

The autopilot is not optional in practice. The attract screen is the machine
playing itself, and a game without one shows a still frame on the marquee.
