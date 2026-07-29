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
| warp field | streaks flying outward from behind the arena | a readout of how well you are doing, drawn as light — it never occludes |
| arena | the game | the game |
| frame | a hard rail round the arena | says where the field ends, and nothing else |
| columns | HOLD, NEXT, the readings | the numbers you are chasing |
| strip | `1UP` / score / name / `HI` | conventions every player can already read |
| monitor | bloom, scanlines, vignette, fringe, hum | says *tube* rather than *panel* |

The warp field is the one that does the most work. It is not decoration: at rest
it drifts, under the player's heat it stretches, and on a clear, an apple or a
death it punches into hyperspace for a fifth of a second and falls back. Play
well and the whole screen goes faster. No game knows it exists — games bank an
impact, the shell spends it.

## The games claim the width

Snake's field used to be a fixed 26×14 wherever it was played, which on a wide
terminal left it sitting in the middle of a lot of black. It is fourteen rows
because that is what the height affords, and now as many columns as the width
will carry — solved by taking the mino size the height forces and dividing the
usable width by it. On a 270-column terminal that is a field roughly four times
the area it used to be.

The arena is fixed when a game is *spawned*, not recomputed from the terminal
every frame, and the layout takes it from the running game rather than from the
game's kind. That is what makes a resize safe: it changes how big a cell is
drawn and never how many there are, so a snake can never find itself outside its
own walls.

Tetris cannot do this, and should not. Ten by twenty with square minos is twice
as tall as it is wide, and that is the game.

## Everything reflows from (width, height)

`world::layout` is the only place a screen coordinate is allowed to come from. A
raw literal reaching a draw call has to be a visible smell rather than a silent
bug, so `Layout::for_field(w, h, cols, rows)` derives every rectangle and every
draw call takes them from there.

The rule it enforces is that **the arena takes every row the chrome does not
need**. There is no scenery to protect any more, so the game gets the screen.

Two constants come out of hard arithmetic rather than taste:

- **`MIN_SIZE` is 60×24.** A twenty-row playfield at the smallest legible block
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

## The terminal will not tell you a key went up

This was the single biggest thing standing between the machine and a game, and
no amount of tuning inside the games could have found it, because the problem
was never inside them.

A terminal does not send a key-up. A held direction arrives as the operating
system's own auto-repeat, which waits about half a second before it starts — so
a piece moves once and then sits there, and then moves fast. All the DAS and
auto-repeat handling in `tetris::handling` was written as though it had held-key
state and it never had any: `keys.left` was only true on the frames a repeat
event happened to land on.

The kitty keyboard protocol reports press and release separately. `term::Pad`
asks for it, and keeps the four directions across frames — whether a key is
still down is a fact about the world rather than about this frame. On a terminal
that supports it (kitty, foot, ghostty, WezTerm, recent xterm) the handling code
finally means what it says.

On one that does not, a direction is assumed held for sixty milliseconds after
its last event. That is deliberately shorter than DAS, so a tap can never start
an auto-shift: on such a terminal the only honest reading of a key event is
"this happened once", and pretending otherwise would make a single press slide
a piece across the well.

## Everything glides

The logic of both games moves in whole cells. Nothing on screen does.

Snake was done first and tetris was left snapping, which is exactly what it felt
like: one game moved and the other stepped. A falling piece is drawn part of a
row down under gravity — `fall_accum` is already the fraction, so the position
was there all along and only the painter had to ask for it — and part of a cell
behind its own column after a sideways move, catching up over 55 ms. Whole-cell
steps are what make a falling game feel like a spreadsheet.

Two rules keep it honest. A **grounded piece sits exactly on its cell**: it is
about to be part of the stack, and a stack that floats reads as a bug. And a
**lock squares everything up**, so whatever the piece was doing on the way down,
the board is a grid again the moment it lands.

The stack falls into a cleared gap rather than teleporting into it. The board
collapses logically at once and the picture catches up over 110 ms: a block is
drawn as many rows above where it now sits as there were cleared rows beneath
it, scaled by how far the collapse has left to go. A hard drop leaves a wake in
the piece's own colour from the top of the fall to the bottom, brightest where
the piece was longest ago.

And the handling was slow. DAS was 133 ms, which is long enough to read as the
game not listening; it is 100. The soft-drop floor was 25 ms a row and is 15.

## The edge of the field

The frame used to carry the hazard: warm where the walls kill, cool where they
are furniture. That idea produced first a red rail and then an amber one, and
both were the ugliest thing on the screen — which is the tell that the idea was
wrong rather than the colours. A boundary is not the place to say what a
boundary does. The wall warning already says it, locally, at the moment it
matters and in the direction it matters, which is everything a resting colour
cannot do.

What replaced it is the machine's own object language. Every brick on this
screen is shaded one way: light where the light catches, dark where the shape
falls away. The field is now the biggest brick of all — a slab, with a
machined bevel for an edge. A side that is only furniture gets the quiet
bevel: pale steel along the top and left, dark iron along the bottom and
right. A side that ends the run gets two sub-pixels of the game's own hue
instead — not a warning colour, *its* colour, which is how the boundary says
what it does without ever reaching for red.

So snake, where every wall kills, is ringed in its own cool white; the
tetris well, where no wall kills anything, is a quiet pocket left open at
the top for the pieces to fall into; and chomp's boundary is all quiet too,
because in a maze it is the pack that kills, not the walls — the maze draws
its own walls as lit outlines, and the tunnel mouths cut straight through
the frame so the wrap has a visible door at both ends. The corners took care
of themselves: bands overlapping at the ends of a rectangle is what a butt
joint looks like, and a butt joint is what a cabinet has.

The fields got floors at the same time. Snake's lattice of pips read as grid
paper; it is a checkerboard now, alternate cells lifted to the darkest navy
the posterize ladder can hold, which reads as tile the way every pit floor
in the arcade did. The tetris well stays empty — a falling game needs no
help seeing its columns — and chomp needs no floor at all, because the maze
is its own furniture and a dot field over tile would be noise on noise.

Nothing on it is brighter than the game inside it, either. A frame that
outshines the playfield is a picture frame, and the only time it is allowed to
be the brightest thing is the moment it flares for a clear or a crash.

The layout reserves the depth. An arena pushed against the frame edge would
otherwise lose one of the two rules down that side, which is the sort of thing
that only shows up at one terminal size and is then very hard to unsee.

An apple inverts the *inside* of the arena for three frames. The border and the
strip holding still is what makes it read as the field firing rather than the
monitor glitching — which is also why an ordinary apple gets that rather than
one of the full-screen reactions.

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

The first two games are tuned against `blipscreen`'s, which is the version
that felt right, and the numbers were diffed rather than guessed. Where they
differ now it is deliberate. Chomp is tuned against the arcade originals:
the player a touch faster than the pack, the hunt shrinking with the levels
but never below usable, the scatter never below a breath.

**Snake.** The speed curve is the reference's exactly: six tiers three apples
apart, 150 ms a cell down to 75 ms, reached around the fifteenth apple. What had
drifted was everything around it.

The field was 20×20. A square arena leaves the player equidistant from
everything and the game loses its rhythm — long runs into tight corners is what
snake *is*. It is fourteen rows and as many columns as the width affords — never
square, always the shape the game has been played on and the shape of a
terminal.

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

The shape was wrong as well as unbounded. The Guideline curve is built for a
marathon — gentle for a long time and then a wall — and this is two minutes
while a build runs. It opened too slowly to be interesting and arrived at
something unplayable with very little in between, which is exactly what "too
easy at the start and too hard too soon" describes.

It is a geometric fall now: 300 ms at the opening, giving up a fixed share of
what is left every level, settling at 100 ms. The early steps are large and
felt, the late ones are small, and the end is somewhere a person can keep
playing — so dying is something the player did rather than something the clock
did. A grounded piece still keeps its full 500 ms lock window on top of that.

## Things come apart

When something is destroyed on this machine it comes apart rather than
disappearing. A row that vanishes was never there; a row that blows out across
the well was something you did.

`world::spark` is a small particle system both games emit into. Sparks live in
*arena cells* — the coordinates the games already think in — so a game emits at
the cell where a thing happened and never has to know how big a cell currently
is on screen. They are drawn emissive only, so debris never occludes the game
under it and always blooms, for the same reason the warp field is.

Three emitters, because three things happen:

- **`shear`** — a row opening outward, hardest at the edges, so a clear reads as
  the line being forced apart rather than as confetti.
- **`burst`** — a cell blown up: an apple taken, a segment of a dead snake.
- **`glimmer`** — light rather than matter. It rises and does not fall, which is
  what a bonus paying out looks like.

There is no pool and no fixed buffer. The cap is a count and the oldest go
first, which is both simpler than a pool and the behaviour you want: the sparks
that matter are the ones that just left.

## Nothing on screen sits still

The screen is deliberately sparse, and the price of sparse is that whatever is
left had better be alive. Every element that survived the cut moves:

- **The arena frame holds still — on purpose.** It carried a light chase for
  a while, and on the fat bevel the lights read as white blobs crawling the
  one line the player judges every distance against. The warp field already
  says how fast you are going, everywhere, so the boundary now says only what
  it is. The stillness is load-bearing.
- **A lock is absorbed.** The block a piece comes to rest on flattens for a
  tenth of a second and springs back. Only the top of each column does it, or
  the whole well would breathe.
- **The queue is a conveyor.** Take a piece off the front and the rest slide up
  into the gap while the new one fades in at the bottom, rather than the whole
  list being rewritten in a frame.
- **The snake pulses** head to tail. It is the one thing always on screen, and a
  rope that only moves when the snake does is a rope nobody looks at twice.
- **The score lifts when it moves.** It is the number the whole machine is
  about; one that only ever sits there is one nobody watches change.
- **A high stack is felt before it is counted.** Nothing says "you are about to
  lose" in words. The frame pulls off its own colour toward the hazard, pulses,
  and runs its lights faster — a piece or two before you would have noticed the
  height yourself.
- **The tube bends as it comes back.** Every screen change opens on a picture
  still wobbling and settling, the way a real one did after a degauss.
- **The marquee breathes.** A wordmark perfectly still on an otherwise moving
  screen is the single thing that reads as a screenshot.

What went, to pay for it: the hairline under every column label, which was
drawing furniture; snake's apple count, which was the score by another name; and
tetris's LINES and LEVEL, which moved to the game-over screen. They are what you
read *after* a run rather than what you play with, and keeping them out of the
column is most of what makes the game screen quiet enough to play on.

Three ways to move, because the machine has no idea whose hands are on it:
arrows, vim, and the left-hand grip everyone who has ever played anything
already has.

## Putting a coin in

Between pressing a key and playing there used to be a 950 ms wheel, shortened on
the principle that everything before the game is dead time. That principle is
right everywhere except here. A cabinet that started the instant you touched it
never felt like it had *accepted* anything, and the half-second of the machine
noticing is most of why putting a coin in was worth doing.

It is three beats now, about a second and two thirds end to end:

1. **The credit.** The marquee stays up, dimmed, and `CREDIT 01` lands on it
   behind a ring going out from the middle. Nothing has been shown yet — but
   the wheel is already decided, so the screen is holding an answer it has not
   given. Re-rolling after the ceremony would be a different game.
2. **The reel**, 0.8 s, eased out to the fifth power: names on a vertical
   strip turning behind a lit window, sliced mid-letter at the lips, detent
   marks pinching the payline. On landing the strip overshoots its detent and
   rocks back — a thing stopped by a pawl was visibly moving under its own
   weight — and the glass fires inverse for the first beats of the slam. What
   matters is not the duration but that the last slots crawl past slowly
   enough to read; a reel that decelerates evenly is a fade with extra steps.
3. **The hold.** The winner sits for 0.3 s before the cut, and for the first
   frames of it the name is drawn a second time a step off itself. Two frames of
   double image is what a thing arriving hard looks like when the face cannot
   actually be scaled.

The landing strobe hits, flips, hits again and rings down — a detent catching
rather than a light coming on.

A test pins the total between 1.2 and 2.0 seconds. Under that it does not read
as a ceremony; over it, it is a wait — and it is paid on every single death,
which is why the first tuning at two and a half turned out to be a quarter of
a session spent ceremonising. A wrapped command's autostart skips the coin
entirely: nobody put one in.

## The loud moments

A reaction is written as data — a list of beats, one per render frame — rather
than as code, because the only way to tune a strobe is to read it as a rhythm.
`on, off, on, off` is legible; the same pattern spread across branches is not.

```rust
pub const HUGE: &[Beat] = &[
    beat(1.0, 0.3, 0.0),   // invert, wash, tear
    beat(0.0, 0.9, 6.0),
    beat(1.0, 0.0, 4.0),
    ...
];
```

Games never name a pattern. They name an *event* — `Kick::Small`, `Big`,
`Huge`, `Bonus`, `Death` — and the shell decides how loud the screen gets, which
is what stops two games reacting differently to the same kind of thing. A
louder reaction displaces a quieter one that is already playing rather than
queueing behind it: by the time a queued strobe played, whatever asked for it
would be long over.

Two rules hold for every pattern, and are tested. It ends dark, or the frame
after it inherits whatever the pattern was doing. And it is at most twelve
frames, because a strobe that outlasts the thing it is reacting to stops being
an impact and becomes a fault.

What is deliberately quiet: an ordinary apple. One arrives every second or two,
and a screen that flashes that often is a screen nobody looks at. It has the
hitstop, the marker and the frame flare already — the noise is being saved for
the golden one.

These are the most expensive frames the machine can draw, because a full-screen
invert repaints every cell. On a metered link a pattern keeps its opening hit
and loses its tail. Suppressing the inverts and keeping only the washes was
tried first, on the theory that a wash over black falls under a loose tolerance
and is nearly free; measured, it saved five per cent and cost most of the punch,
so the length of the pattern is the dial and the flip stays.

## The monitor

`world::crt` is the last pass, and everything in it is an artefact of a real
cathode-ray tube rather than a filter someone liked:

- **fringe** — three guns never landed on the same spot, and the misconvergence
  grew toward the edges, which is why it scales with distance from the centre.
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

The hum and the vignette are still in the code but dark by default: both are
smooth, filmic effects, and on the chunked picture (next section) anything
smooth reads as airbrush. The scanlines and the fringe carry the tube alone.

Ordering in the chain is not arbitrary. The flash is light arriving at the
glass, so it is first. The guns are behind the glass, so the fringe is next.
The shake moves the whole chassis. The collapse is the power going, and it
happens to whatever the screen was showing.

## The chunk

Everything on screen is hard pixels now, and it is enforced in the pipeline
rather than begged of each painter:

- the post pass **posterizes** every channel to eight levels, so a bloom halo
  becomes concentric flat rings, a fade becomes steps, and nothing on screen
  can be a colour the palette does not have
- **dithering is off** — dither exists to hide banding, and the bands are the
  look
- **bloom is a rim**, radius two, not an atmosphere
- the warp field is runs of **two-sub-pixel dashes** with a two-step fade —
  the taper was never the point of a speed streak, the length was
- **sparks are fat squares** snapped to the same grid, going bright, dim, gone
- the chrome wordmark is **silkscreened in six flat bands** instead of
  airbrushed
- the minos are **bricks**: a light border with knocked corners, a darker
  body, a square gleam — the border is what keeps every cell in a stack its
  own tile instead of a colour region, which is most of what makes an
  eighties well read as brickwork instead of a bar chart

The rule of thumb the pass enforces: gradients are for readouts (the snake's
body, the progress rule), and even those step. Anything that fades smoothly is
telling the player nothing and costing the period everything.

## One row of chrome

The play screen carried a bar across the top with a `1UP` marker, the live
score, the game's name and the all-time best; a second row under the well for
readings; and a third at the bottom for the wrapped command. Three rows of
furniture around a game is a dashboard.

Every one of those had an answer. The marker says nothing. The name is answered
by looking at the game. The record belongs on the screen where records are read,
which is the attract loop and the board. And the readings the player is not
using while playing belong on the screen they read afterwards.

So there is one row now, at the bottom: the score at the left in white, whatever
the game has to shout beside it, the command's last line dim after that, and the
clock at the right. The standalone arcade says nothing at all there — a line of
instructions along the bottom of a game is read once and then in the way
forever.

What still touches the well is what you play with: the hold slot, the queue, and
snake's multiplier.

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
is roughly a third of the traffic**, and not because it is large: it is
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

- ~~**A third game.**~~ This held until the third slot earned its keep — see
  "The third game" below; the bar was being done properly, not merely
  existing. BREAKOUT held the slot first and CHOMP holds it now.
- **Configurable keybindings.** A TOML file is a decision, and see the premise.
- **A menu, difficulty select, or game modes.** Same reason.
- **Screen curvature.** It is the one CRT artefact that would have to resample
  text, and text that wobbles is not authenticity, it is a bug.
- **Letting the player replay the same game.** Tempting, and rejected: the
  moment you can choose, the machine is a menu with extra steps.
- **A wide tetris well.** Ten by twenty with square minos is twice as tall as
  it is wide, and that *is* the game — stretching it to fill a wide terminal
  would make it a different one. A tetris cabinet solved this by standing its
  monitor on end, and there is no equivalent of that here. So the well is as
  large as the height allows and the flanks stay black.

## The third game

The slot has been held twice, which is the real test of the seam: BREAKOUT
went on the wheel in one evening, and when it was retired, CHOMP replaced it
the same way — a `Kind` variant, a module with the logic, a painter, and
nothing else moved.

CHOMP is a maze chase with the classic bones and one modern organ: the maze
is *carved*, fresh, for every level and every arena size. The carver works on
an odd lattice — a spanning tree first, then a share of the walls between
corridors knocked through so there are loops, because being able to run a
circle around a ghost is the entire skill of the game. The left half is
mirrored onto the right the way the real cabinets were, so a glance at a new
level is enough to start running; a tunnel pierces both walls and wraps. A
flood fill then seals anything the carver orphaned and knocks doors until
the maze is one piece — a dot that cannot be eaten is a level that cannot be
finished, so reachability is asserted in tests across twenty seeds.

The ghosts are the classic pack: a hunter on your cell, an ambusher four
ahead of your nose, a flank that mirrors you through the hunter, a coward
that hunts from range and breaks off up close. They breathe between scatter
and chase, reversing on every mode change so the tide is legible without a
HUD, and leave home staggered on a clock so a level opens as a mounting
problem. A pellet reverses and blues the pack; each ghost taken in one hunt
pays double the one before, and the eyes fly home to be reborn. Its demo is
a breadth-first search that gives live ghosts a wide berth and turns hunter
the moment the pack is blue — competent enough to clear a board, mortal
enough that the attract loop always moves on.

## Adding a game

One `games::Kind` variant and one module. `Kind` carries the arena size, the
marquee name, the slug its scores are filed under, the frame hue, the control
hint and how to spawn it; the module implements `games::Game`. Everything else
— layout, columns, hitstop, pops, the wheel, the board — already works.

The autopilot is not optional in practice. The attract screen is the machine
playing itself, and a game without one shows a still frame on the marquee.
