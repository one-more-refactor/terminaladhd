//! The stage: everything that depends on one terminal size and one game.
//!
//! A [`Stage`] owns the [`Layout`], the warp field and the three scratch
//! buffers a frame passes through — sub-pixel colour, post-effect colour,
//! resolved cells — so a frame allocates nothing. A resize, or a spin onto a
//! game with a different arena, throws the whole thing away and builds a new
//! one, which is what guarantees every rectangle reflows through `Layout` and
//! never through a stale coordinate.
//!
//! Every screen the machine can show is a method here. They all lay the same
//! black ground and fly the same warp field, so a cut between two of them is
//! continuous rather than a jump to another program.

use std::time::Duration;

use crate::games::{Game, Kind};
use crate::scores::Entry;
use crate::world::cabinet::{ground, pop, rule, strip, Strip};
use crate::world::crt;
use crate::world::draw::{lit, put_base, ring, text, text_center, text_w};
use crate::world::layout::Layout;
use crate::world::scene::palette::*;
use crate::world::Warp;
use crate::world::{bloom, chrome_word, chrome_word_w, hex, resolve_d, scanlines, Buf, Cell, Rgb};

/// Bloom radius, sample step and weight. Wider and hotter than a lit picture
/// would want: on a black ground the bloom *is* the light, and a phosphor
/// monitor bled a long way.
/// Tight and modest: on a chunky picture the glow is a rim, not an
/// atmosphere. The old (4, 2, 0.60) airbrushed everything it touched.
const BLOOM: (usize, usize, f32) = (2, 1, 0.32);
/// How much darker every second sub-row is. A CRT hint, not a mask.
const SCANLINE: f32 = 0.80;
/// Levels per channel after the post pass. Eight is enough that the hot hues
/// keep their identity and few enough that every gradient visibly steps.
// Fourteen: at eight the bands around every moving glow crawled visibly,
// which read as a rendering bug and wore the eyes out. Fourteen keeps the
// flat retro read and the rings stop shimmering.
const POSTER_LEVELS: f32 = 14.0;
/// Colour-match tolerance when resolving sub-pixels to cells and when diffing
/// them against the last frame, in 8-bit levels.
///
/// This is the single most expensive number in the machine. At 2 it is below
/// the perceptual floor and every faint nudge of the warp field over the whole
/// screen is a cell that has to be re-sent — which is most of the bytes leaving
/// the process. Raising it throws away changes nobody can see anyway.
const TOL_RICH: i32 = 2;
const TOL_LEAN: i32 = 14;

/// The wordmark on the marquee.
pub const TITLE: &str = "ADHD";

fn c(v: u32) -> Rgb {
    hex(v)
}

pub struct Stage {
    pub w: usize,
    pub h: usize,
    pub kind: Kind,
    pub layout: Layout,
    pub warp: Warp,
    /// `0.0..=1.0` from the wrapped command, or `None` when nothing is running.
    /// Drives the rule under the status strip.
    pub progress: Option<f32>,
    /// A full-frame white wash, `0.0..=1.0`. Consumed by the frame that draws.
    pub flash: f32,
    /// How hard the tube is still bending as it warms back up. The shell drives
    /// it down from one as the curtain opens.
    pub warmup: f32,
    /// How far the tube has cut out, `0.0..=1.0`. The shell drives it closed on
    /// its way out of a screen and open on its way into the next.
    pub curtain: f32,
    /// Guns pulled apart at the frame edge, in sub-pixels. There is always a
    /// little; an impact adds a lot.
    pub fringe: f32,
    /// Whole-monitor displacement in sub-pixels, for a hit big enough to be
    /// felt in the chassis.
    pub jolt: (i32, i32),
    /// Horizontal hold lost, in sub-pixels of slip. The loudest thing the
    /// machine does.
    pub tear: f32,
    /// The picture flipped to its negative, `0.0..=1.0`.
    pub invert: f32,
    /// Varies which bands tear, so consecutive frames do not shimmer in place.
    torn: u64,
    /// The reaction currently playing, and how far into it we are.
    fired: Option<(&'static [Beat], usize)>,
    /// What colour the current reaction washes in.
    fired_hue: Rgb,
    /// Where the light rip is, `0..1` down the picture, and how hard.
    rip: (f32, f32),
    /// How hard the wheel has just landed, `0.0..=1.0`, decaying.
    pub slam: f32,
    /// The last score drawn, and how much the strip is still lifting from the
    /// change — the score is what the machine is about, and a number that never
    /// reacts is one nobody watches.
    shown_score: u32,
    score_hot: f32,
    /// Where the supply hum currently is, `0.0..=1.0` down the picture.
    hum: f32,
    /// What this frame is allowed to spend.
    pub quality: Quality,
    buf: Buf,
    px: Vec<Rgb>,
    scratch: Vec<Rgb>,
    /// The bloom's two working buffers, kept so a frame allocates nothing.
    blur_a: Vec<Rgb>,
    blur_b: Vec<Rgb>,
    pub cells: Vec<Cell>,
}

/// What the machine is allowed to spend on a frame.
///
/// Every one of these costs bytes down a wire, not just cycles. A phone on a
/// cellular link is the case that makes that visible: the hum is a bright band
/// crawling across the full width, so every row it touches is a row that has to
/// be re-sent, and the warp field changes somewhere in almost every column of
/// every frame. Locally they are free. Over SSH they are the whole bill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quality {
    pub warp: bool,
    pub hum: bool,
    pub fringe: bool,
    pub vignette: bool,
    pub bloom: bool,
    pub scanlines: bool,
    /// Speak xterm-256 on the wire instead of truecolor. A palette pair is a
    /// third of the bytes and parses in a fraction of the time on a phone
    /// terminal; the picture quantises to the 6x6x6 cube, which the posterize
    /// pass has already half-done anyway.
    pub palette: bool,
    /// Colour-match tolerance, in 8-bit levels. The dial that actually governs
    /// what a frame costs.
    pub tol: i32,
    /// Frames a second. The other half of the bill — halving it halves the
    /// bytes, and neither of these games needs sixty.
    pub fps: u32,
    /// Beats a full-screen reaction is allowed. Every one of them inverts or
    /// washes the entire picture, which is the most expensive frame there is —
    /// a full repaint, forty bytes a cell — so on a metered link a pattern
    /// keeps its opening hit and loses its tail.
    ///
    /// Suppressing the inverts and keeping only the washes was tried, on the
    /// theory that a wash on a black screen falls under a loose tolerance and
    /// is nearly free. Measured, it saved five per cent and cost most of the
    /// punch, so it is not done: the length of the pattern is the dial that
    /// works, and the flip is what the pattern is for.
    pub strobe_cap: usize,
}

impl Quality {
    /// Everything on. What a local terminal gets.
    ///
    /// The hum and the vignette are off by default now, not for bytes but for
    /// the look: both are smooth, filmic effects, and on a chunky picture
    /// anything smooth reads as airbrush. The scanlines and the fringe carry
    /// the tube on their own.
    pub const fn full() -> Quality {
        Quality {
            warp: true,
            hum: false,
            fringe: true,
            vignette: false,
            bloom: true,
            scanlines: true,
            palette: false,
            tol: TOL_RICH,
            fps: 60,
            strobe_cap: usize::MAX,
        }
    }

    /// What survives when bytes are expensive.
    ///
    /// Measured rather than guessed: at the strict tolerance the warp field is
    /// ninety-three per cent of the bytes leaving this process, because its
    /// faint light spread by bloom nudges almost every cell of every frame past
    /// a two-level threshold. Raising the threshold to fourteen throws away
    /// changes nobody can see and keeps the field, which is the whole identity
    /// of the screen — a fifth of the cost for none of the look.
    ///
    /// The hum and the fringe go because they dirty cells nothing asked to
    /// change: the hum sweeps the full width every frame, and the fringe
    /// rewrites every column with contrast in it. Bloom stays; it only
    /// changes when the picture under it does.
    pub const fn lean() -> Quality {
        Quality {
            warp: true,
            hum: false,
            fringe: false,
            vignette: false,
            bloom: true,
            scanlines: true,
            palette: true,
            tol: TOL_LEAN,
            fps: 30,
            strobe_cap: 4,
        }
    }
}

/// One render frame of a full-screen reaction.
///
/// Written as data rather than as code because the only way to tune a strobe is
/// to read it as a rhythm — `on, off, on, off` — and a pattern spread across
/// branches cannot be read that way.
#[derive(Clone, Copy, Debug, Default)]
pub struct Beat {
    /// How far toward the negative the whole picture goes.
    pub invert: f32,
    /// A flat wash of light over everything.
    pub wash: f32,
    /// Sub-pixels of lost horizontal hold.
    pub tear: f32,
}

const fn beat(invert: f32, wash: f32, tear: f32) -> Beat {
    Beat { invert, wash, tear }
}
const REST: Beat = beat(0.0, 0.0, 0.0);

/// The named reactions. Every one is short: a strobe that outlasts the thing it
/// is reacting to stops being an impact and becomes a fault.
pub mod strobe {
    use super::{beat, Beat, REST};

    /// A single or a double. A pulse, not a blowout: this fires every few
    /// seconds of decent play, and a screen that whites out that often is a
    /// screen that hurts to watch.
    pub const CLEAR: &[Beat] = &[beat(0.0, 0.30, 0.0), beat(0.0, 0.10, 0.0), REST];

    /// A triple, or a spin that cleared. One flip of the picture.
    pub const BIG: &[Beat] = &[
        beat(0.0, 0.55, 3.0),
        beat(0.9, 0.0, 0.0),
        beat(0.0, 0.25, 0.0),
        REST,
    ];

    /// A Tetris or a perfect clear. Three flips and the hold going with them —
    /// the loudest thing that happens inside a game.
    pub const HUGE: &[Beat] = &[
        beat(1.0, 0.3, 0.0),
        beat(0.0, 0.9, 6.0),
        beat(1.0, 0.0, 4.0),
        beat(0.0, 0.6, 8.0),
        beat(0.0, 0.35, 0.0),
        beat(0.0, 0.15, 0.0),
        REST,
    ];

    /// A bonus taken. Bright and warm rather than inverted: the picture is not
    /// being interrupted, it is being paid.
    pub const BONUS: &[Beat] = &[
        beat(0.0, 0.7, 0.0),
        beat(0.0, 0.30, 0.0),
        beat(0.0, 0.5, 0.0),
        beat(0.0, 0.2, 0.0),
        REST,
    ];

    /// The run ending. Inverts hard, then the hold goes and stays gone for
    /// longer than anything else — the machine losing its grip.
    pub const DEATH: &[Beat] = &[
        beat(1.0, 0.0, 0.0),
        beat(1.0, 0.0, 9.0),
        beat(0.0, 0.5, 11.0),
        beat(0.8, 0.0, 8.0),
        beat(0.0, 0.2, 6.0),
        beat(0.0, 0.0, 4.0),
        beat(0.0, 0.0, 2.0),
        REST,
    ];

    /// The wheel stopping. It hits, flips, hits again and rings down — a detent
    /// catching rather than a light coming on.
    pub const LAND: &[Beat] = &[
        beat(0.0, 0.9, 0.0),
        beat(1.0, 0.0, 6.0),
        beat(0.0, 0.6, 4.0),
        REST,
        beat(0.0, 0.4, 3.0),
        REST,
        beat(0.0, 0.25, 0.0),
        REST,
        beat(0.0, 0.12, 0.0),
        REST,
    ];

    /// A coin going in. Two hard hits and a long fall-off — the weight is in
    /// the tail, because a credit is the machine waking up rather than
    /// something happening to it.
    pub const COIN: &[Beat] = &[
        beat(0.0, 0.9, 0.0),
        beat(0.0, 0.5, 5.0),
        beat(1.0, 0.0, 0.0),
        beat(0.0, 0.7, 3.0),
        beat(0.0, 0.4, 0.0),
        beat(0.0, 0.25, 0.0),
        beat(0.0, 0.12, 0.0),
        REST,
    ];

    /// A record. Long, alternating, and unapologetic — it is the one moment the
    /// machine is allowed to keep going after the point has been made.
    pub const RECORD: &[Beat] = &[
        beat(0.0, 1.0, 0.0),
        beat(1.0, 0.0, 0.0),
        beat(0.0, 0.8, 0.0),
        beat(1.0, 0.0, 0.0),
        beat(0.0, 0.6, 0.0),
        REST,
        beat(0.0, 0.5, 0.0),
        REST,
        beat(0.0, 0.3, 0.0),
        REST,
    ];
}

/// The permanent misconvergence, in sub-pixels at the frame edge. Under one
/// sub-pixel: not seen, only felt.
// Zero at rest: a permanent misconvergence doubles every edge on screen,
// which is exactly the thing that makes eyes ache after a minute. The guns
// now only pull apart when something hits.
const FRINGE_REST: f32 = 0.0;
/// How strong the hum bar is, and how long it takes to cross the picture.
const HUM_STRENGTH: f32 = 0.055;
const HUM_SECS: f32 = 7.5;

impl Stage {
    pub fn new(kind: Kind, field: (usize, usize), w: usize, h: usize) -> Self {
        Stage {
            w,
            h,
            kind,
            layout: Layout::for_field(w, h, field.0, field.1),
            warp: Warp::new(w, 2 * h, seed(w, h)),
            progress: None,
            flash: 0.0,
            curtain: 0.0,
            warmup: 0.0,
            fringe: 0.0,
            jolt: (0, 0),
            tear: 0.0,
            invert: 0.0,
            torn: 0x9e3779b9,
            fired: None,
            fired_hue: c(WHITE),
            rip: (0.0, 0.0),
            slam: 0.0,
            shown_score: 0,
            score_hot: 0.0,
            hum: 0.0,
            quality: Quality::full(),
            buf: Buf::new(w, h),
            px: Vec::new(),
            scratch: Vec::new(),
            blur_a: Vec::new(),
            blur_b: Vec::new(),
            cells: Vec::new(),
        }
    }

    /// Re-cut the screen for a different game. Cheap enough to call on every
    /// spin, and the only way the arena is ever allowed to change shape. The
    /// warp field survives it, so the background never stutters at a cut.
    /// The field comes from the game rather than from its kind: a run keeps the
    /// arena it was spawned with, so a resize can never move the walls of a
    /// snake that is already inside them.
    pub fn retarget(&mut self, kind: Kind, field: (usize, usize)) {
        self.kind = kind;
        self.layout = Layout::for_field(self.w, self.h, field.0, field.1);
    }

    /// Advance the background. Separate from drawing because the field has to
    /// keep flying on frames where the sim did nothing.
    pub fn animate(&mut self, dt: f32, heat: f32) {
        self.warp.step(dt, heat);
        self.hum = (self.hum + dt / HUM_SECS).fract();
    }

    /// Fire a full-screen reaction. A louder one displaces a quieter one that
    /// is already running rather than queueing behind it — by the time a
    /// queued strobe played, whatever asked for it would be long over.
    pub fn fire(&mut self, pattern: &'static [Beat], hue: Rgb) {
        let pattern = &pattern[..pattern.len().min(self.quality.strobe_cap.max(1))];
        let running = self.fired.map(|(p, _)| p.len()).unwrap_or(0);
        if pattern.len() >= running {
            self.fired = Some((pattern, 0));
            self.fired_hue = hue;
            // The rip starts at the top and crosses the picture over the
            // pattern, so a long reaction is one sweep rather than a flicker.
            self.rip = (0.0, 1.0);
        }
    }

    /// Take this frame's beat, if a reaction is playing.
    fn beat(&mut self) -> Beat {
        let Some((pattern, i)) = self.fired else {
            self.rip = (0.0, 0.0);
            return Beat::default();
        };
        let b = pattern[i];
        let next = i + 1;
        self.fired = (next < pattern.len()).then_some((pattern, next));
        self.rip = (next as f32 / pattern.len() as f32, self.rip.1);
        b
    }

    // ------------------------------------------------------------- screens

    /// Attract: the marquee over the machine playing itself. A cabinet was
    /// never idle — it demoed, and the demo is what got the coin out of you.
    pub fn attract(&mut self, demo: &dyn Game, best: u32, t: f32, tick: &Tick) {
        self.open(demo.heat() * 0.5);
        // The marquee's lamp chase: discrete bulbs around the frame edge,
        // on or off, stepping — never fading. It is the one thing every
        // machine in the room did, and it is what "on" looks like from
        // across that room.
        self.lamps(t);
        // The demo gets its own arena — it may not be the game the stage is cut
        // for — and no side columns: a NEXT queue nobody can use is clutter
        // around the marquee.
        let mut bare = Layout::for_field(self.w, self.h, demo.field().0, demo.field().1);
        bare.left_col = None;
        bare.right_col = None;
        // Behind the marquee at a fifth brightness: legible as motion, never
        // mistaken for a game you are in control of.
        demo.paint(&mut self.buf, &bare);
        self.dim_all(0.22);

        let l_w = self.layout.w as f32;
        let cx = self.layout.w as i32 / 2;
        let mid = self.layout.h as f32;
        let scale = hero_scale(TITLE, self.layout.w, 4);
        text_center(
            &mut self.buf,
            "TERMINAL",
            cx,
            mid as i32 - (7 * scale) as i32 - 14,
            1,
            c(CYAN),
            0.6,
        );
        // The marquee breathes. A wordmark that sits perfectly still on an
        // otherwise moving screen is the one thing that reads as a screenshot.
        let breathe = (t * 1.9).sin() * 1.5;
        chrome_word(&mut self.buf, TITLE, scale, l_w * 0.5, mid - 10.0 + breathe);

        // The one blinking thing on the screen, at the rate every cabinet used.
        if (t * 1.6) as i32 % 2 == 0 {
            text_center(
                &mut self.buf,
                "INSERT COIN",
                cx,
                mid as i32 + 8,
                2,
                c(YELLOW),
                1.2,
            );
        }
        // The record lives here and on the board, and nowhere else. It is not
        // something you look at while playing.
        if best > 0 {
            text_center(
                &mut self.buf,
                &format!("BEST {best}"),
                cx,
                mid as i32 + 22,
                1,
                c(YELLOW),
                0.4,
            );
        }
        self.close(tick);
    }

    /// A credit going in. The marquee is still up and nothing has been decided
    /// yet — this is the half-second of the machine noticing, which every
    /// cabinet had and which is most of why putting a coin in felt like
    /// something.
    pub fn coin(&mut self, demo: &dyn Game, age: f32, tick: &Tick) {
        self.open(0.4);
        let mut bare = Layout::for_field(self.w, self.h, demo.field().0, demo.field().1);
        bare.left_col = None;
        bare.right_col = None;
        demo.paint(&mut self.buf, &bare);
        self.dim_all(0.18);

        let cx = self.layout.w as i32 / 2;
        let mid = self.layout.h as f32;
        let scale = hero_scale("CREDIT", self.layout.w, 3);
        chrome_word(
            &mut self.buf,
            "CREDIT",
            scale,
            self.layout.w as f32 * 0.5,
            mid - 6.0,
        );
        // The count lands on the frame the coin does and sits there. One
        // credit, because there is only ever one game about to happen.
        text_center(
            &mut self.buf,
            "01",
            cx,
            mid as i32 + 5 * scale as i32 - 2,
            2,
            c(YELLOW),
            1.2,
        );
        // A ring going out from the middle, once, on the first third.
        if age < 0.2 {
            let r = age / 0.2;
            ring(
                &mut self.buf,
                self.layout.w as f32 * 0.5,
                mid,
                r * self.layout.w as f32 * 0.55,
                5.0,
                c(WHITE).mul(1.0 - r),
            );
        }
        self.close(tick);
    }

    /// The roulette: names flying through the warp toward the player and
    /// slamming to a stop on the one that won.
    pub fn spin(&mut self, reel: &[Kind], travel: f32, tick: &Tick) {
        // The wheel drives the field: it is the fastest thing this machine
        // does, and the background should be the first to say so.
        self.warp.punch(0.5 * (1.0 - travel));
        self.open(1.0);

        // A slot machine's reel, not a fly-past: names on a vertical strip
        // turning behind a lit window, sliced mid-letter at the lips, with
        // detent marks pinching the payline. The strip overshoots its detent
        // on landing and rocks back — a thing stopped by a pawl was visibly
        // moving under its own weight.
        let l_w = self.layout.w as f32;
        let mid_y = self.layout.h as i32; // sub-row centre of the frame
        let scale = hero_scale("BREAKOUT", self.layout.w, 2);
        let name_h = (7 * scale) as i32;
        let pitch = name_h + name_h / 2 + 4;
        let widest = reel
            .iter()
            .map(|k| chrome_word_w(k.name(), scale))
            .fold(0, usize::max) as i32;

        let win_w = (widest + 14).min(self.layout.w as i32 - 6);
        let win_h = pitch + name_h;
        let wx0 = (self.layout.w as i32 - win_w) / 2;
        let wy0 = mid_y - win_h / 2;
        let (ix0, ix1) = (wx0 + 2, wx0 + win_w - 3);
        let (iy0, iy1) = (wy0 + 2, wy0 + win_h - 3);

        // Strip position in slots, with the landing bounce: past the detent
        // and back, spent as the slam decays.
        let bounce = self.slam * self.slam * 0.22;
        let pos = travel * (reel.len() - 1) as f32 + if travel >= 1.0 { bounce } else { 0.0 };

        for (i, k) in reel.iter().enumerate() {
            let dy = ((i as f32 - pos) * pitch as f32) as i32;
            if dy.abs() > win_h {
                continue;
            }
            let cy = mid_y + dy;
            chrome_word(&mut self.buf, k.name(), scale, l_w * 0.5, cy as f32);
            // On the frames right after it stops, the winner is drawn again
            // a step off itself — two frames of double image is what a thing
            // arriving hard looks like when it cannot actually be scaled.
            if travel >= 1.0 && dy.abs() < pitch / 2 && self.slam > 0.6 {
                chrome_word(
                    &mut self.buf,
                    k.name(),
                    scale,
                    l_w * 0.5 + self.slam * 3.0,
                    cy as f32,
                );
            }
        }

        // Cut the strip back to the window: everything above and below the
        // glass goes, inside the window columns only, so the names are
        // sliced mid-letter as they enter and leave — the slicing is what
        // sells the turn. The warp outside the columns is untouched.
        let void = c(VOID);
        for y in 0..self.buf.sh as i32 {
            if y >= iy0 && y <= iy1 {
                continue;
            }
            if (y - mid_y).abs() > win_h + name_h {
                continue;
            }
            for x in wx0..wx0 + win_w {
                put_base(&mut self.buf, x, y, void);
            }
        }
        // The drum's curve, spent in the only currency a lit window has:
        // rows near the lips fall away.
        for (row, k) in [(0i32, 0.30f32), (1, 0.55), (2, 0.8)] {
            dim_rows(&mut self.buf, ix0, ix1, iy0 + row, k);
            dim_rows(&mut self.buf, ix0, ix1, iy1 - row, k);
        }

        // The window itself: a flat rail, lit toward the winner's hue as the
        // strip slows, with detent marks pinching the payline.
        let arriving = reel[((pos.round() as i64).clamp(0, reel.len() as i64 - 1)) as usize];
        let rail = c(STEEL).lerp(arriving.hue(), travel * travel);
        for t in 0..2 {
            let (x0, y0, x1, y1) = (wx0 + t, wy0 + t, wx0 + win_w - 1 - t, wy0 + win_h - 1 - t);
            for x in x0..=x1 {
                put_base(&mut self.buf, x, y0, rail);
                put_base(&mut self.buf, x, y1, rail);
            }
            for y in y0..=y1 {
                put_base(&mut self.buf, x0, y, rail);
                put_base(&mut self.buf, x1, y, rail);
            }
        }
        let hot = c(MAGENTA);
        for dy in -1..=1i32 {
            for ox in 0..3 {
                lit(&mut self.buf, wx0 - 1 - ox, mid_y + dy, hot, hot.mul(0.5));
                lit(
                    &mut self.buf,
                    wx0 + win_w + ox,
                    mid_y + dy,
                    hot,
                    hot.mul(0.5),
                );
            }
        }
        // The payout flash: the glass fires inverse for the first beats of
        // the landing, which is the pixel half of the slam the strobe is
        // already shouting.
        if travel >= 1.0 && self.slam > 0.75 {
            for y in iy0..=iy1 {
                for x in ix0..=ix1 {
                    let i = y as usize * self.buf.w + x as usize;
                    let p = self.buf.base[i];
                    self.buf.base[i] = Rgb::new(
                        (1.0 - p.r).max(0.0),
                        (1.0 - p.g).max(0.0),
                        (1.0 - p.b).max(0.0),
                    );
                }
            }
        }
        self.close(tick);
    }

    /// A game, running.
    pub fn game(&mut self, g: &dyn Game, tick: &Tick) {
        self.open_bare();
        g.paint(&mut self.buf, &self.layout);
        self.feedback(g);
        self.close_playing(g, tick);
    }

    /// The score markers, drawn here rather than in either game so a Tetris and
    /// a golden apple pay out in the same face at the same rate. The shout goes
    /// to the status strip, which [`Stage::close`] owns.
    fn feedback(&mut self, g: &dyn Game) {
        for p in g.pops() {
            pop(&mut self.buf, &self.layout, p, g.shake());
        }
    }

    /// The settle after a crash: the frozen game pulled down, a hero word over
    /// it, and the run's score counting up underneath.
    pub fn over(&mut self, g: &dyn Game, s: &Settle, tick: &Tick) {
        self.open(g.heat());
        g.paint(&mut self.buf, &self.layout);
        // The picture goes down so the words can come up. It happens to the
        // scene, before bloom, so the hero laid over it afterwards keeps its
        // colour and still blooms.
        self.dim_all(1.0 - 0.78 * s.fade);

        let l = &self.layout;
        let mid = l.h as f32;
        let word = if s.record { "NEW RECORD" } else { "GAME OVER" };
        let scale = hero_scale(word, l.w, 3);
        chrome_word(&mut self.buf, word, scale, l.w as f32 * 0.5, mid - 9.0);

        let hue = if s.record { c(YELLOW) } else { c(CYAN) };
        let cx = l.w as i32 / 2;
        text_center(
            &mut self.buf,
            &format!("{:06}", s.shown),
            cx,
            mid as i32 + 6,
            2,
            hue,
            1.2,
        );

        // The run, in the two numbers the game keeps. Only once the counter has
        // finished climbing, so there is one thing to read at a time.
        if s.shown >= 1 || s.tally.iter().any(|&(_, v)| v > 0) {
            let line = s
                .tally
                .iter()
                .map(|(name, v)| format!("{name} {v}"))
                .collect::<Vec<_>>()
                .join("   ");
            text_center(&mut self.buf, &line, cx, mid as i32 + 20, 1, c(STEEL), 0.0);
        }
        self.close(tick);
    }

    /// The clock stopped. The game stays on screen behind a half-collapsed
    /// raster — the picture is visibly held rather than replaced, so it is
    /// obvious nothing has been lost.
    pub fn paused(&mut self, g: &dyn Game, tick: &Tick) {
        self.open_bare();
        g.paint(&mut self.buf, &self.layout);
        self.dim_all(0.35);
        let cx = self.layout.w as i32 / 2;
        let mid = self.layout.h as f32;
        let scale = hero_scale("PAUSED", self.layout.w, 3);
        chrome_word(
            &mut self.buf,
            "PAUSED",
            scale,
            self.layout.w as f32 * 0.5,
            mid,
        );
        {
            text_center(
                &mut self.buf,
                "P TO RESUME",
                cx,
                mid as i32 + 5 * scale as i32 + 4,
                1,
                c(STEEL),
                0.0,
            );
        }
        self.close(tick);
    }

    /// The controls, on their own screen. Every machine had this printed on the
    /// bezel; this one has nowhere to print it.
    pub fn help(&mut self, kind: Kind, tick: &Tick) {
        self.open(0.1);
        let cx = self.layout.w as i32 / 2;
        // The title gives up its double height before the table gives up a
        // row: on a short frame the words are the point.
        let scale = if self.layout.h * 2 < 88 {
            1
        } else {
            hero_scale("CONTROLS", self.layout.w, 2)
        };
        let th = 7 * scale as i32;
        // Hung off the rule, or the title's top band is drawn through the
        // status strip — the same rule the board follows.
        let title_cy = self.layout.rule_sub as i32 + 3 + th / 2;
        chrome_word(
            &mut self.buf,
            "CONTROLS",
            scale,
            self.layout.w as f32 * 0.5,
            title_cy as f32,
        );

        // Long forms when the frame can seat them, short when it cannot — a
        // clipped table teaches nothing. Ordered so the rows a short frame
        // drops are the ones a player can live without.
        let rows: [(&str, &str); 7] = if self.layout.w >= 150 {
            [
                ("WASD  ARROWS  HJKL", "MOVE AND STEER"),
                ("X  UP", "ROTATE"),
                ("SPACE", "HARD DROP"),
                ("ESC", "LEAVE, AGAIN TO QUIT"),
                ("C", "HOLD"),
                ("Z", "ROTATE BACK"),
                ("P", "PAUSE"),
            ]
        } else {
            [
                ("WASD", "STEER"),
                ("X  UP", "ROTATE"),
                ("SPACE", "DROP"),
                ("ESC", "LEAVE"),
                ("C", "HOLD"),
                ("Z", "UNROTATE"),
                ("P", "PAUSE"),
            ]
        };
        // Two columns on one gutter: keys right-aligned into it, meanings
        // left-aligned out of it, which is how every manual of the period set
        // a control table. The table is measured and centred as a whole
        // rather than hung off the frame's midline, because its two halves
        // are nowhere near the same width — and it keeps only the rows that
        // fit above the ticker.
        let wk = rows.iter().map(|(k, _)| text_w(k, 1)).max().unwrap_or(0);
        let ww = rows.iter().map(|(_, w)| text_w(w, 1)).max().unwrap_or(0);
        let gutter = (self.layout.w as i32 - (wk + 8 + ww)) / 2 + wk + 4;
        let mut y = title_cy + th / 2 + 5;
        let floor = self.layout.ticker_sub.unwrap_or(2 * self.layout.h - 6) as i32;
        let n = (((floor - y - 2) / 8).max(3) as usize).min(rows.len());
        for (key, what) in rows.iter().take(n) {
            text(
                &mut self.buf,
                key,
                gutter - 4 - text_w(key, 1),
                y,
                1,
                c(YELLOW),
                0.3,
            );
            text(&mut self.buf, what, gutter + 4, y, 1, c(STEEL), 0.0);
            y += 8;
        }
        let _ = (cx, kind);
        self.close(tick);
    }

    /// The frame is smaller than the machine can draw. Says so, in the only
    /// thing that still fits.
    pub fn too_small(&mut self) {
        ground(&mut self.buf);
        let cx = self.layout.w as i32 / 2;
        let mid = self.layout.h as i32;
        text_center(
            &mut self.buf,
            "SCREEN TOO SMALL",
            cx,
            mid - 6,
            1,
            c(YELLOW),
            0.6,
        );
        text_center(
            &mut self.buf,
            &format!(
                "NEEDS {}x{}",
                crate::term::MIN_SIZE.0,
                crate::term::MIN_SIZE.1
            ),
            cx,
            mid + 2,
            1,
            c(STEEL),
            0.0,
        );
        self.post();
        self.resolve();
    }

    /// The board for one game, best first, with the run that just landed lit.
    pub fn board(
        &mut self,
        kind: Kind,
        rows: &[Entry],
        hilite: Option<usize>,
        age: f32,
        tick: &Tick,
    ) {
        self.open(0.15);

        let cx = self.layout.w as i32 / 2;
        let scale = hero_scale("HIGH SCORES", self.layout.w, 2);
        // Hung off the rule rather than off the frame edge, or the title's top
        // band is drawn through the status strip.
        let th = 7 * scale as i32;
        let title_cy = self.layout.rule_sub as i32 + 3 + th / 2;
        chrome_word(
            &mut self.buf,
            "HIGH SCORES",
            scale,
            self.layout.w as f32 * 0.5,
            title_cy as f32,
        );
        let top = title_cy + th / 2 + 3;
        text_center(&mut self.buf, kind.name(), cx, top, 1, c(CYAN), 0.5);

        // The table grows down from the title and must not reach the ticker; on
        // a short frame that means showing fewer places, never smaller ones.
        let first = top + 9;
        let bottom = self.layout.ticker_sub.unwrap_or(2 * self.layout.h) as i32 - 6;
        const PITCH: i32 = 8;
        let room = ((bottom - first) / PITCH).max(1) as usize;

        if rows.is_empty() {
            text_center(&mut self.buf, "NO RUNS YET", cx, first, 1, c(STEEL), 0.0);
        }
        for (i, e) in rows.iter().take(room).enumerate() {
            let y = first + i as i32 * PITCH;
            let new = hilite == Some(i);
            // The new entry blinks rather than merely being a different colour:
            // on a board of yellow numbers, colour alone does not find the eye.
            let on = !new || (age * 7.0) as i32 % 2 == 0;
            let hue = match (new, on, i) {
                (true, true, _) => c(WHITE),
                (true, false, _) => c(ORANGE),
                (_, _, 0) => c(YELLOW),
                _ => c(STEEL),
            };
            let line = format!("{:>3}  {:06}", place(i), e.score);
            let x = cx - text_w(&line, 1) / 2;
            text(
                &mut self.buf,
                &line,
                x,
                y,
                1,
                hue,
                if new { 1.2 } else { 0.2 },
            );
        }
        self.close(tick);
    }

    // ------------------------------------------------------------- plumbing

    /// Black ground and the warp behind everything.
    /// Discrete bulbs chasing the frame edge, three lit then one dark,
    /// stepping four times a second.
    fn lamps(&mut self, t: f32) {
        let w = self.buf.w as i32;
        let sh = self.buf.sh as i32;
        let step = 5;
        let phase = (t * 4.0) as i32;
        let on = c(YELLOW);
        let off = c(IRON);
        let mut n = 0;
        let bulb = |b: &mut Buf, x: i32, y: i32, idx: i32| {
            let lit_now = (idx + phase) % 4 != 3;
            let colv = if lit_now { on } else { off };
            for dy in 0..2 {
                for dx in 0..2 {
                    put_base(b, x + dx, y + dy, colv);
                }
            }
        };
        let mut x = 2;
        while x < w - 3 {
            bulb(&mut self.buf, x, 1, n);
            n += 1;
            x += step;
        }
        let mut y = 1;
        while y < sh - 8 {
            bulb(&mut self.buf, w - 4, y, n);
            n += 1;
            y += step;
        }
        let mut x = w - 4;
        while x > 2 {
            bulb(&mut self.buf, x, sh - 9, n);
            n += 1;
            x -= step;
        }
        let mut y = sh - 9;
        while y > 1 {
            bulb(&mut self.buf, 2, y, n);
            n += 1;
            y -= step;
        }
    }

    fn open(&mut self, heat: f32) {
        ground(&mut self.buf);
        if self.quality.warp {
            self.warp.draw(&mut self.buf, heat);
        }
    }

    /// Ground only. The play screens run on clean black: a field flying
    /// behind a game being *read* taxes exactly the attention the game
    /// needs, and it was the last moving thing left under the playfield.
    /// The warp keeps the screens with nothing to protect — the marquee,
    /// the reel, the settle — where spectacle is the whole job.
    fn open_bare(&mut self) {
        ground(&mut self.buf);
    }

    /// Strip, rule, ticker, post, resolve — the same tail on every screen, so
    /// the chrome never flickers between two of them.
    /// The one row of chrome, then post and resolve. Every screen ends here.
    fn close(&mut self, tick: &Tick) {
        self.close_with(0, None, tick);
    }

    /// [`Stage::close`] with a game's score and whatever it has to shout.
    fn close_playing(&mut self, g: &dyn Game, tick: &Tick) {
        let shout = g.shout();
        self.close_with(g.score(), shout, tick);
    }

    fn close_with(&mut self, score: u32, shout: Option<(&str, f32)>, tick: &Tick) {
        if score != self.shown_score {
            self.shown_score = score;
            self.score_hot = 1.0;
        }
        let hot = self.score_hot;
        self.score_hot = (self.score_hot - 0.14).max(0.0);
        rule(&mut self.buf, &self.layout, self.progress);
        strip(
            &mut self.buf,
            &self.layout,
            &Strip {
                score,
                shout,
                left: &tick.left,
                right: &tick.right,
                hot,
            },
        );
        self.post();
        self.resolve();
    }

    /// Bloom and scanlines, or the plain sum of the two planes when bloom is
    /// off — the picture still has to be resolved either way.
    fn post(&mut self) {
        if self.quality.bloom {
            bloom(
                &mut self.buf,
                BLOOM.0,
                BLOOM.1,
                BLOOM.2,
                &mut self.blur_a,
                &mut self.blur_b,
                &mut self.px,
            );
        } else {
            crate::world::resolve_no_bloom(&self.buf, &mut self.px);
        }
        // The step that makes it a machine's picture: every channel snapped
        // to a short ladder, so halos ring, fades step, and nothing is a
        // colour the palette does not have.
        crate::world::posterize(&mut self.px, POSTER_LEVELS);
        if self.quality.scanlines {
            scanlines(&mut self.px, self.buf.w, self.buf.sh, SCANLINE);
        }
    }

    /// Scale the whole scene, both planes, before bloom.
    fn dim_all(&mut self, k: f32) {
        for i in 0..self.buf.base.len() {
            self.buf.base[i] = self.buf.base[i].mul(k);
            self.buf.emis[i] = self.buf.emis[i].mul(k);
        }
    }

    /// The monitor, applied to the finished picture. Order is not arbitrary: the
    /// flash is light arriving at the glass, so it comes first; the guns are
    /// misaligned behind the glass, so the fringe comes next; the hum and the
    /// vignette are properties of the tube itself; the shake moves the whole
    /// chassis; and the collapse is the power going, which happens to whatever
    /// the screen was showing.
    fn resolve(&mut self) {
        let (w, sh) = (self.buf.w, self.buf.sh);
        // Whatever reaction is playing folds into this frame's one-off values,
        // so a caller that sets `flash` directly and a fired pattern cannot
        // fight over the same pixel.
        let beat = self.beat();
        let hue = self.fired_hue;
        // Consumed rather than cleared by the caller: a flash that outlives the
        // frame that asked for it is a white screen nobody can explain.
        crt::invert(
            &mut self.px,
            std::mem::take(&mut self.invert).max(beat.invert),
        );
        crt::wash(&mut self.px, c(WHITE), std::mem::take(&mut self.flash));
        crt::wash(&mut self.px, hue, beat.wash);
        if self.rip.1 > 0.0 && beat.wash + beat.invert > 0.0 {
            crt::rip(&mut self.px, w, sh, self.rip.0, hue, self.rip.1);
        }
        let extra = std::mem::take(&mut self.fringe);
        if self.quality.fringe || extra > 0.0 {
            let rest = if self.quality.fringe {
                FRINGE_REST
            } else {
                0.0
            };
            crt::fringe(&mut self.px, &mut self.scratch, w, sh, rest + extra);
        }
        if self.quality.hum {
            crt::hum(&mut self.px, w, sh, self.hum, HUM_STRENGTH);
        }
        if self.quality.vignette {
            crt::vignette(&mut self.px, w, sh);
        }
        let (dx, dy) = std::mem::take(&mut self.jolt);
        crt::shake(&mut self.px, &mut self.scratch, w, sh, dx, dy);
        self.torn = self.torn.wrapping_mul(6364136223846793005).wrapping_add(1);
        crt::tear(
            &mut self.px,
            &mut self.scratch,
            w,
            sh,
            std::mem::take(&mut self.tear).max(beat.tear),
            self.torn,
        );
        // A tube coming back does not simply appear: it bends and settles.
        crt::wobble(
            &mut self.px,
            &mut self.scratch,
            w,
            sh,
            self.warmup * 7.0,
            self.hum * 6.0,
        );
        crt::collapse(&mut self.px, &mut self.scratch, w, sh, self.curtain);
        // No dithering: dither exists to hide banding, and the bands are now
        // the look.
        resolve_d(&self.px, w, sh, self.quality.tol, false, &mut self.cells);
    }

    pub fn sub_pixels(&self) -> (&[Rgb], usize, usize) {
        (&self.px, self.buf.w, self.buf.sh)
    }
}

/// The two ends of the bottom row. Passed as one value because every screen
/// carries both and neither is ever meaningful alone.
pub struct Tick {
    pub left: String,
    pub right: String,
}

/// The state of a game-over settle, as the loop sees it.
pub struct Settle {
    /// `0.0..=1.0` — how far the picture has gone down.
    pub fade: f32,
    /// What the counter is showing, which climbs to the real score rather than
    /// appearing at it.
    pub shown: u32,
    pub record: bool,
    /// The run in two numbers, whatever the game calls them. This is where they
    /// belong — they are what you read after a run, not what you play with, and
    /// keeping them out of the column is most of what makes the game screen
    /// quiet enough to play on.
    pub tally: [(&'static str, u32); 2],
}

/// `1ST`, `2ND`, `3RD`, then `4TH` upward — the arcade board's own ordinals.
fn place(i: usize) -> String {
    let n = i + 1;
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "TH",
        (1, _) => "ST",
        (2, _) => "ND",
        (3, _) => "RD",
        _ => "TH",
    };
    format!("{n}{suffix}")
}

/// The largest chrome scale at or below `want` that still leaves the word clear
/// of the frame edges.
/// Attenuate one sub-row of the picture between two columns — the reel
/// window's lip shading, where the drum curves away from the glass.
fn dim_rows(b: &mut Buf, x0: i32, x1: i32, y: i32, k: f32) {
    if y < 0 || y as usize >= b.sh {
        return;
    }
    for x in x0.max(0)..=x1.min(b.w as i32 - 1) {
        let i = y as usize * b.w + x as usize;
        b.base[i] = b.base[i].mul(k);
        b.emis[i] = b.emis[i].mul(k);
    }
}

fn hero_scale(word: &str, w: usize, want: usize) -> usize {
    (1..=want)
        .rev()
        .find(|&s| chrome_word_w(word, s) + 6 <= w)
        .unwrap_or(1)
}

/// A per-size seed, so the warp field is the same on the same terminal from one
/// run to the next and a `--shot` frame is reproducible.
fn seed(w: usize, h: usize) -> u64 {
    ((w as u64) << 32) | h as u64 | 1
}

/// Binary P6 at native sub-pixel resolution. The dump is how a frame is
/// reviewed without a terminal in the way — and how a rendering change is
/// compared against the last one it is supposed to improve on.
pub fn write_ppm(path: &str, px: &[Rgb], w: usize, sh: usize) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(w * sh * 3 + 32);
    buf.extend_from_slice(format!("P6\n{w} {sh}\n255\n").as_bytes());
    for y in 0..sh {
        for x in 0..w {
            buf.extend_from_slice(&crate::world::to_srgb8(
                px[y * w + x],
                crate::world::bayer4(x, y),
            ));
        }
    }
    std::fs::write(path, buf)
}

/// `M:SS`, the only clock format the ticker has room for.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(stage: &mut Stage, pattern: &'static [Beat]) -> Vec<Beat> {
        stage.fire(pattern, c(WHITE));
        (0..pattern.len() + 2).map(|_| stage.beat()).collect()
    }

    #[test]
    fn a_reaction_plays_once_and_stops() {
        let mut s = Stage::new(Kind::Tetris, (10, 20), 80, 26);
        let beats = play(&mut s, strobe::CLEAR);
        assert_eq!(beats.len(), strobe::CLEAR.len() + 2);
        for (got, want) in beats.iter().zip(strobe::CLEAR) {
            assert_eq!(got.wash, want.wash);
        }
        // And nothing afterwards: a strobe that outlives its cause is a fault.
        let tail = &beats[strobe::CLEAR.len()..];
        assert!(tail.iter().all(|b| b.wash == 0.0 && b.invert == 0.0));
    }

    #[test]
    fn a_louder_reaction_displaces_a_quieter_one() {
        let mut s = Stage::new(Kind::Tetris, (10, 20), 80, 26);
        s.fire(strobe::CLEAR, c(WHITE));
        s.beat();
        s.fire(strobe::HUGE, c(WHITE));
        // By the time a queued strobe played, whatever asked for it would be
        // long over — so the big one takes the screen now.
        assert_eq!(
            s.fired.map(|(p, i)| (p.len(), i)),
            Some((strobe::HUGE.len(), 0))
        );
    }

    #[test]
    fn a_quieter_reaction_does_not_cut_a_louder_one_short() {
        let mut s = Stage::new(Kind::Tetris, (10, 20), 80, 26);
        s.fire(strobe::HUGE, c(WHITE));
        s.beat();
        s.fire(strobe::CLEAR, c(WHITE));
        assert_eq!(s.fired.map(|(p, _)| p.len()), Some(strobe::HUGE.len()));
    }

    #[test]
    fn every_pattern_ends_dark() {
        // The last beat of every reaction has to be nothing, or the frame after
        // it inherits whatever the pattern was doing.
        for (name, p) in [
            ("clear", strobe::CLEAR),
            ("big", strobe::BIG),
            ("huge", strobe::HUGE),
            ("bonus", strobe::BONUS),
            ("death", strobe::DEATH),
            ("land", strobe::LAND),
            ("record", strobe::RECORD),
        ] {
            let last = p.last().expect("a pattern with no beats");
            assert_eq!(last.wash, 0.0, "{name} ends lit");
            assert_eq!(last.invert, 0.0, "{name} ends inverted");
            assert_eq!(last.tear, 0.0, "{name} ends torn");
            assert!(p.len() <= 12, "{name} outlasts what it is reacting to");
        }
    }

    #[test]
    fn a_metered_link_gets_a_shorter_reaction_that_still_lets_go() {
        let mut s = Stage::new(Kind::Tetris, (10, 20), 80, 26);
        s.quality = Quality::lean();
        s.fire(strobe::HUGE, c(WHITE));
        let played: Vec<Beat> = (0..strobe::HUGE.len() + 2).map(|_| s.beat()).collect();
        let lit = played.iter().filter(|b| b.wash + b.invert > 0.0).count();
        assert!(lit <= Quality::lean().strobe_cap, "the cap did not hold");
        // Cut short or not, the picture has to come back on its own.
        assert!(
            played[Quality::lean().strobe_cap..]
                .iter()
                .all(|b| b.wash == 0.0 && b.invert == 0.0 && b.tear == 0.0),
            "a truncated reaction left the screen holding something"
        );
    }

    #[test]
    fn places_read_as_a_board() {
        let got: Vec<String> = (0..5).map(place).collect();
        assert_eq!(got, ["1ST", "2ND", "3RD", "4TH", "5TH"]);
        assert_eq!(place(10), "11TH");
    }

    #[test]
    fn a_hero_word_shrinks_to_fit_rather_than_running_off() {
        assert_eq!(hero_scale("HIGH SCORES", 40, 3), 1);
        assert!(hero_scale("ADHD", 400, 4) >= 3);
    }

    #[test]
    fn the_warp_seed_is_stable_for_a_size() {
        assert_eq!(seed(120, 30), seed(120, 30));
        assert_ne!(seed(120, 30), seed(121, 30));
    }
}
