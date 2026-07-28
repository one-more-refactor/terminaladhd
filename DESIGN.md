# How the machine is built

Notes on why this is shaped the way it is. The code says what it does; this
says what it is for, and what was tried and thrown away.

## The premise

You start something slow. For the next two minutes you are not working, you
are waiting, and waiting is the worst-designed part of a developer's day.
`adhd` takes those two minutes and makes them an arcade.

Two consequences fall out of that immediately and both are load-bearing:

**You do not pick the game.** A menu is a decision, and the whole reason you
reached for this is that you did not want to decide anything. A reel picks,
and picks again after every death. The rotation is not a feature bolted on
top; it is the product.

**Nothing may cost you your command.** stdout belongs to the child, always.
The exit code is the child's. If the command fails, its stderr is replayed
rather than swallowed by the alternate screen. With no terminal at all it runs
the command and gets out of the way rather than refusing. Even a panic in the
arcade is caught, the terminal restored, and the wait resumed — a toy that
eats your build the day it has a bug is not a toy you keep installed.

## One picture

The screen is 80×48 one-bit pixels. Every pixel is either lit phosphor or
dark glass. The picture is blown up in whole pixels to whatever the terminal
is and centred on black, and at scale one it is exactly an 80×24 terminal —
the size every terminal has been since terminals.

This is the structural decision everything else hangs off, and it was learned
the hard way. Two earlier machines lived in this repository. The first was a
synthwave landscape with the game cut into it — a picture with a game on it,
which is a 2015 album cover rather than an arcade machine. The second kept
the black and built an adaptive cabinet: a layout solver that reflowed an
arena, side columns and a status strip from `(width, height)`, full colour,
bloom, dithering, a warp field, a CRT filter chain. It was technically
impressive and it played safe and it looked like a dashboard, and every third
bug in the tracker was the solver being caught out by a size nobody had tried
— edges that clipped, fields a resize could strand, states you could see but
not play.

A fixed canvas deletes that entire class of problem *by construction*. There
is no layout to solve, so there is no layout to get wrong. A resize changes
one integer. Every coordinate in the machine is a hand-placed constant that
someone looked at, which is the only way "every detail right" is even a
claim that can be checked. And one bit per pixel deletes the palette the same
way: there is no contrast to manage, no "does this read against that" branch,
no colour that needs a fallback. Ink is legible everywhere, because ink is
the only thing there is.

The cost is honest: margins on terminals between whole scales, and a hard
floor of 80×24. Fractional scaling was rejected on sight — one source pixel
two columns wide next to one a single column wide stops being pixel art and
starts being mangled art.

## One voice

All the colour lives in a single value, the `Phosphor`: the tone lit pixels
glow with, and the tint that tone leaves in the glass around them. That pair
is the machine's whole emotional range, and it is played loudly precisely
because it is one channel:

- each game owns a tone — lime for SNAKE, ice for BLOCKS — so what you are
  playing is said before a word is read
- heat pushes the tone towards gold: play well and the screen itself warms
- the reel strobes neon while it flies, because it is the one screen with no
  game on it to protect
- a landing, a golden apple, a tetris blow the tube out white for a beat
- death flares alarm red; a record turns the whole panel gold

A screen with six colour systems has to whisper with all of them. A screen
with one can shout.

## The glass

What makes it a monitor rather than a bitmap is applied on the way out, in
the composer, and there are exactly four effects:

- **halo** — an unlit pixel next to a lit one picks up a faint wash of the
  tone. This is the phosphor bleeding into the glass, and it is the single
  pass that makes the picture read as *lit* rather than drawn.
- **scanlines** — alternate terminal rows dim the phosphor a shade. On dark
  glass it has to be the ink that dims; dimming the dark would be invisible.
- **shake** — on a hit, rows shear in opposite directions by up to two cells.
  The picture is not displaced, it is *struck*.
- **collapse** — between screens the raster squeezes to a bright line and
  the line to a dot, burning brighter as it narrows because the same beam is
  being spent on fewer lines. Every screen change is one of these closing
  and the next one opening. The one exception is death: the settle after a
  crash is continuous with the game that produced it, so it slides.

Everything else the old machine had — bloom in linear light, ordered
dithering, chromatic fringe, supply hum, vignette, a warp field, a particle
system — is gone. Each was defensible alone; together they were a mood board
sitting between the player and the game.

## Three screens

The shell is a loop over three modes, and none of them asks a question.

**The reel.** Game names on a strip turning behind a lit window, sliced off
mid-letter at the lip, with a sprung-pawl settle when it lands — it overshoots
its detent and rocks back, because a thing stopped by a pawl was visibly
moving under its own weight. Any key brakes it hard: the spin is a thrill,
not a wait. The whole throw is spent in about 700 ms. Once landed it holds
just long enough to read the winner's one line of controls, then cuts in.

**The game.** The entire canvas belongs to it. Score in the 3×5 face, the
field, and nothing else standing.

**The settle.** GAME OVER, the score, the best, and a bar draining towards
the next spin — the wait reads as the machine reloading, not as the game
having stopped. A grace period ignores the keys you were still mashing when
you died; after it, any key spins immediately. There is no "play again": the
next game is a different game, because that is the premise.

There is no attract mode, no help screen, no score board, no pause. Each was
built, and each was cut in the rebuild for the same reason: it was a screen
the player had to be *somewhere else* to be on. The machine teaches the way a
cabinet taught — one hint line on the reel, and dying.

## Feel is shared

Both games speak the same physical language, and the shell enforces it by
owning the vocabulary:

- **hitstop** — an impact freezes the whole machine for a few render frames.
  An apple is a tap (3), a golden one a hit (6), a death the full 150 ms the
  eye needs to read an impact as one (10). Time does not accumulate during
  the freeze; it is a debt paid in frames, not wall time owed afterwards.
- **shake** is derived from hitstop, so nothing can shake without having
  stopped time first — a shake without weight is a shiver.
- **the flash** is banked by the game (`take_flash`) and spent by the shell
  on the phosphor, so both games are loud through the same lamp. It is
  rationed deliberately: an ordinary apple arrives every second or two, and
  a screen that blows out that often is a screen nobody looks at.
- **markers** — `+N` rises off the cell that earned it, in the same face at
  the same rate, in both games.
- **local flashes stay local** — snake's eat inverts the field interior while
  the border holds still, which is what makes the field itself look like it
  fired; blocks' cleared rows blink in place while their window runs down.

## The snake glides

The one thing that makes a snake a snake rather than a cursor: the logic
moves in whole cells, but every segment is drawn part of the way between
where it was and where it is, on a mild ease-out — ahead of linear the whole
way but never past the cell, because a full ease leaves the head visibly
drifting into a cell it has already logically reached. Each segment
interpolates from *its own* previous position, so the whole body slides along
its own path and corners stay corners. Death freezes the glide at the far
end: a snake still coasting into its last cell reads as if the collision had
not landed.

## Steering is taps, never state

The input layer hands games two different things and the distinction is the
fix for the worst bug the machine ever shipped:

- **held state** (four booleans) — what an auto-shift wants. Blocks' DAS
  charges against it in real milliseconds.
- **taps** (direction presses, in arrival order) — what a steering wheel
  wants. The snake reads only these.

Steering by held state fails two ways at once: held booleans have no order,
so two keys rolled around a corner collapse into whichever the code checks
first; and a key still held from three cells ago outvotes the tap that was
meant to turn, which is exactly the moment the player calls the game
unresponsive. A tap is queued once, validated against the last queued turn
(a reversal into your own neck is always a misfire, so it is refused rather
than allowed to kill), and three can be banked — enough for a double corner
between moves without the snake driving itself.

Where the terminal supports the kitty keyboard protocol, releases are real
and held state is exact. Where it does not, a key is assumed held for 60 ms
after its last event — deliberately shorter than DAS, so a tap can never
start an auto-shift on such a terminal.

## Balance is asserted, not remembered

Every tuning claim that matters is a test, because a claim in a comment
drifts and a claim in an assertion cannot:

- snake's average apple is chainable and the far corner is not, at every
  tier — that is the whole tension of the multiplier
- the golden apple is reachable on average and not from the far corner: a
  decision, not a collection
- chaining gets easier as the snake speeds up, which is the reward for
  surviving the climb
- blocks' gravity opens between 0.25 and 0.35 seconds a row, only tightens,
  bites within two minutes, and floors where a piece still takes two seconds
  to cross the well — three minutes in, it is still a game and not a countdown
- the reel's whole ceremony fits between "long enough to read as a gamble"
  and "short enough to sit through twice"

And one test simply mashes: every game, thousands of steps of random input,
stepped well past death, in a debug build so overflow panics too. It proves
nothing about correctness. It exists because "it crashed while I was playing"
is the one bug report that must never be true.

## What a frame costs

The renderer diffs against the previous frame and repaints only cells that
changed, bracketed in DEC 2026 synchronized output so a burst lands at once
instead of tearing. A one-bit picture in flat tones makes the diff small by
nature — a moving snake is a few dozen cells — and the loud moments that
repaint everything are exactly the moments that are worth their bytes. Over
SSH the frame rate halves (30 instead of 60), which is most of the saving
with none of the picture gone; the old machine needed a whole quality tier,
a tolerance knob and a bench harness to get its wire cost down, and the new
picture gets further by simply having less picture.

## Adding a game

One `Kind` variant and one module implementing `Game`: step it with input and
a `dt`, say when it is over, bank hitstop and flash, draw yourself onto the
canvas, and provide an autopilot good enough to watch — it is the proof the
game is playable, and what `--shot` photographs. The reel, the scores, the
phosphor and the settle come for free. The autopilot is not optional.

## Things deliberately not done

- **No pause.** A run is under two minutes and the machine respins anyway;
  a pause is a menu in disguise.
- **No `q` to quit.** It sits under the same hand as the movement keys and
  quitting is the one action with no undo. Esc leaves the game for the reel,
  and the reel for the shell — one key never drops the session by surprise,
  and two always do.
- **No score board screen.** Ten scores per game are still kept on disk;
  the screen that showed them was a screen nobody was on. BEST on the reel
  and NEW BEST! on the settle carry what the player actually feels.
- **No ticker of the command's output.** It made the picture a dashboard.
  The command gets a hairline of progress across the top of the picture, and
  its stderr tail replayed on failure — the signal without the stream.
- **No fractional scaling, no adaptive layout.** See "One picture". The
  margin is the price of never again shipping a screen nobody had tried.
