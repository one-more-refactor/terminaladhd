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
use crate::world::cabinet::{ground, pop, rule, strip, ticker};
use crate::world::draw::{lit, text, text_center, text_w};
use crate::world::layout::Layout;
use crate::world::scene::palette::*;
use crate::world::crt;
use crate::world::{bloom, chrome_word, chrome_word_w, hex, resolve_d, scanlines, Buf, Cell, Rgb};
use crate::world::Warp;

/// Bloom radius, sample step and weight. Wider and hotter than a lit picture
/// would want: on a black ground the bloom *is* the light, and a phosphor
/// monitor bled a long way.
const BLOOM: (usize, usize, f32) = (4, 2, 0.60);
/// How much darker every second sub-row is. A CRT hint, not a mask.
const SCANLINE: f32 = 0.82;
/// Colour-match tolerance when resolving sub-pixels to cells. 2/255 is below
/// the perceptual floor and cuts the diff substantially.
const TOL: i32 = 2;

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
    /// Where the supply hum currently is, `0.0..=1.0` down the picture.
    hum: f32,
    buf: Buf,
    px: Vec<Rgb>,
    scratch: Vec<Rgb>,
    pub cells: Vec<Cell>,
}

/// The permanent misconvergence, in sub-pixels at the frame edge. Under one
/// sub-pixel: not seen, only felt.
const FRINGE_REST: f32 = 0.45;
/// How strong the hum bar is, and how long it takes to cross the picture.
const HUM_STRENGTH: f32 = 0.055;
const HUM_SECS: f32 = 7.5;

impl Stage {
    pub fn new(kind: Kind, w: usize, h: usize) -> Self {
        Stage {
            w,
            h,
            kind,
            layout: kind.layout(w, h),
            warp: Warp::new(w, 2 * h, seed(w, h)),
            progress: None,
            flash: 0.0,
            curtain: 0.0,
            fringe: 0.0,
            jolt: (0, 0),
            tear: 0.0,
            invert: 0.0,
            torn: 0x9e3779b9,
            hum: 0.0,
            buf: Buf::new(w, h),
            px: Vec::new(),
            scratch: Vec::new(),
            cells: Vec::new(),
        }
    }

    /// Re-cut the screen for a different game. Cheap enough to call on every
    /// spin, and the only way the arena is ever allowed to change shape. The
    /// warp field survives it, so the background never stutters at a cut.
    pub fn retarget(&mut self, kind: Kind) {
        if kind == self.kind {
            return;
        }
        self.kind = kind;
        self.layout = kind.layout(self.w, self.h);
    }

    /// Advance the background. Separate from drawing because the field has to
    /// keep flying on frames where the sim did nothing.
    pub fn animate(&mut self, dt: f32, heat: f32) {
        self.warp.step(dt, heat);
        self.hum = (self.hum + dt / HUM_SECS).fract();
    }

    // ------------------------------------------------------------- screens

    /// Attract: the marquee over the machine playing itself. A cabinet was
    /// never idle — it demoed, and the demo is what got the coin out of you.
    pub fn attract(&mut self, demo: &dyn Game, best: u32, blink: bool, t: f32, tick: &Tick) {
        self.open(demo.heat() * 0.5);
        // The demo gets its own arena — it may not be the game the stage is cut
        // for — and no side columns: a NEXT queue nobody can use is clutter
        // around the marquee.
        let mut bare = demo.kind().layout(self.w, self.h);
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
        chrome_word(&mut self.buf, TITLE, scale, l_w * 0.5, mid - 10.0);

        // The one blinking thing on the screen, at the rate every cabinet used.
        if (t * 1.6) as i32 % 2 == 0 {
            text_center(&mut self.buf, "PRESS ENTER", cx, mid as i32 + 8, 2, c(YELLOW), 1.2);
        }
        text_center(&mut self.buf, "ESC TO QUIT", cx, mid as i32 + 22, 1, c(STEEL), 0.0);
        self.close(0, best, demo.kind().name(), blink, tick);
    }

    /// The roulette: names flying through the warp toward the player and
    /// slamming to a stop on the one that won.
    pub fn spin(&mut self, reel: &[Kind], travel: f32, best: u32, blink: bool, tick: &Tick) {
        // The wheel drives the field: it is the fastest thing this machine
        // does, and the background should be the first to say so.
        self.warp.punch(0.5 * (1.0 - travel));
        self.open(1.0);

        let l_w = self.layout.w as f32;
        let mid = self.layout.h as f32;
        let pitch = l_w * 0.62;
        let pos = travel * (reel.len() - 1) as f32;
        for (i, k) in reel.iter().enumerate() {
            let dx = (i as f32 - pos) * pitch;
            let near = 1.0 - (dx.abs() / l_w).min(1.0);
            if near < 0.2 {
                continue;
            }
            // The one at the centre is big and arriving; the ones either side
            // are small and already gone.
            let scale = if near > 0.82 {
                hero_scale(k.name(), self.layout.w, 3)
            } else {
                1
            };
            chrome_word(&mut self.buf, k.name(), scale, l_w * 0.5 + dx, mid);
        }

        // Two hard bars marking the slot, clear of the longest name on the reel
        // so they never sit on top of the word they are pointing at.
        let widest = reel
            .iter()
            .map(|k| chrome_word_w(k.name(), 3) as f32)
            .fold(0.0f32, f32::max);
        let reach = (widest * 0.5 + 6.0).min(l_w * 0.5 - 3.0);
        let hot = c(MAGENTA);
        for side in [-1.0f32, 1.0] {
            let x = (l_w * 0.5 + side * reach) as i32;
            for dy in -12..=12i32 {
                for ox in 0..2 {
                    lit(&mut self.buf, x + ox, mid as i32 + dy, hot, hot);
                }
            }
        }
        self.close(0, best, self.kind.name(), blink, tick);
    }

    /// A game, running.
    pub fn game(&mut self, g: &dyn Game, best: u32, blink: bool, tick: &Tick) {
        self.open(g.heat());
        g.paint(&mut self.buf, &self.layout);
        self.feedback(g);
        let shout = g.shout();
        self.close_shouting(g.score(), best, self.kind.name(), blink, tick, shout);
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
    pub fn over(&mut self, g: &dyn Game, s: &Settle, best: u32, blink: bool, tick: &Tick) {
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
        text_center(
            &mut self.buf,
            &format!("{:06}", s.shown),
            l.w as i32 / 2,
            mid as i32 + 6,
            2,
            hue,
            1.2,
        );
        self.close(s.shown, best, self.kind.name(), blink, tick);
    }

    /// The clock stopped. The game stays on screen behind a half-collapsed
    /// raster — the picture is visibly held rather than replaced, so it is
    /// obvious nothing has been lost.
    pub fn paused(&mut self, g: &dyn Game, best: u32, blink: bool, tick: &Tick) {
        self.open(0.0);
        g.paint(&mut self.buf, &self.layout);
        self.dim_all(0.35);
        let cx = self.layout.w as i32 / 2;
        let mid = self.layout.h as f32;
        let scale = hero_scale("PAUSED", self.layout.w, 3);
        chrome_word(&mut self.buf, "PAUSED", scale, self.layout.w as f32 * 0.5, mid);
        if blink {
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
        self.close(g.score(), best, self.kind.name(), blink, tick);
    }

    /// The controls, on their own screen. Every machine had this printed on the
    /// bezel; this one has nowhere to print it.
    pub fn help(&mut self, kind: Kind, best: u32, tick: &Tick) {
        self.open(0.1);
        let cx = self.layout.w as i32 / 2;
        let scale = hero_scale("CONTROLS", self.layout.w, 2);
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

        let rows: [(&str, &str); 7] = [
            ("ARROWS  HJKL", "MOVE AND STEER"),
            ("X  UP", "ROTATE"),
            ("Z", "ROTATE BACK"),
            ("SPACE", "HARD DROP"),
            ("C", "HOLD"),
            ("P", "PAUSE"),
            ("ESC", "LEAVE, AGAIN TO QUIT"),
        ];
        // Two columns on one gutter: keys right-aligned into the middle,
        // meanings left-aligned out of it, which is how every manual of the
        // period set a control table.
        let gutter = cx + 2;
        let mut y = title_cy + th / 2 + 5;
        for (key, what) in rows {
            text(&mut self.buf, key, gutter - 4 - text_w(key, 1), y, 1, c(YELLOW), 0.3);
            text(&mut self.buf, what, gutter + 4, y, 1, c(STEEL), 0.0);
            y += 8;
        }
        text_center(
            &mut self.buf,
            kind.hint(),
            cx,
            y + 6,
            1,
            c(CYAN),
            0.4,
        );
        self.close(0, best, kind.name(), true, tick);
    }

    /// The frame is smaller than the machine can draw. Says so, in the only
    /// thing that still fits.
    pub fn too_small(&mut self) {
        ground(&mut self.buf);
        let cx = self.layout.w as i32 / 2;
        let mid = self.layout.h as i32;
        text_center(&mut self.buf, "SCREEN TOO SMALL", cx, mid - 6, 1, c(YELLOW), 0.6);
        text_center(
            &mut self.buf,
            &format!("NEEDS {}x{}", crate::term::MIN_SIZE.0, crate::term::MIN_SIZE.1),
            cx,
            mid + 2,
            1,
            c(STEEL),
            0.0,
        );
        bloom(&mut self.buf, BLOOM.0, BLOOM.1, BLOOM.2, &mut self.px);
        scanlines(&mut self.px, self.buf.w, self.buf.sh, SCANLINE);
        self.resolve();
    }

    /// The board for one game, best first, with the run that just landed lit.
    pub fn board(
        &mut self,
        kind: Kind,
        rows: &[Entry],
        hilite: Option<usize>,
        age: f32,
        best: u32,
        tick: &Tick,
    ) {
        self.open(0.15);

        let cx = self.layout.w as i32 / 2;
        let scale = hero_scale("HIGH SCORES", self.layout.w, 2);
        // Hung off the rule rather than off the frame edge, or the title's top
        // band is drawn through the status strip.
        let th = 7 * scale as i32;
        let title_cy = self.layout.rule_sub as i32 + 3 + th / 2;
        chrome_word(&mut self.buf, "HIGH SCORES", scale, self.layout.w as f32 * 0.5, title_cy as f32);
        let top = title_cy + th / 2 + 3;
        text_center(&mut self.buf, kind.name(), cx, top, 1, c(CYAN), 0.5);

        // The table grows down from the title and must not reach the ticker; on
        // a short frame that means showing fewer places, never smaller ones.
        let first = top + 9;
        let bottom = self.layout.ticker_sub as i32 - 6;
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
            text(&mut self.buf, &line, x, y, 1, hue, if new { 1.2 } else { 0.2 });
        }
        self.close(0, best, kind.name(), true, tick);
    }

    // ------------------------------------------------------------- plumbing

    /// Black ground and the warp behind everything.
    fn open(&mut self, heat: f32) {
        ground(&mut self.buf);
        self.warp.draw(&mut self.buf, heat);
    }

    /// Strip, rule, ticker, post, resolve — the same tail on every screen, so
    /// the chrome never flickers between two of them.
    fn close(&mut self, score: u32, best: u32, name: &str, blink: bool, tick: &Tick) {
        self.close_shouting(score, best, name, blink, tick, None);
    }

    fn close_shouting(
        &mut self,
        score: u32,
        best: u32,
        name: &str,
        blink: bool,
        tick: &Tick,
        shout: Option<(&str, f32)>,
    ) {
        strip(&mut self.buf, &self.layout, score, name, best, blink, shout);
        rule(&mut self.buf, &self.layout, self.progress);
        ticker(&mut self.buf, &self.layout, &tick.left, &tick.right);
        bloom(&mut self.buf, BLOOM.0, BLOOM.1, BLOOM.2, &mut self.px);
        scanlines(&mut self.px, self.buf.w, self.buf.sh, SCANLINE);
        self.resolve();
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
        // Consumed rather than cleared by the caller: a flash that outlives the
        // frame that asked for it is a white screen nobody can explain.
        crt::invert(&mut self.px, std::mem::take(&mut self.invert));
        crt::flash(&mut self.px, std::mem::take(&mut self.flash));
        crt::fringe(
            &mut self.px,
            &mut self.scratch,
            w,
            sh,
            FRINGE_REST + std::mem::take(&mut self.fringe),
        );
        crt::hum(&mut self.px, w, sh, self.hum, HUM_STRENGTH);
        crt::vignette(&mut self.px, w, sh);
        let (dx, dy) = std::mem::take(&mut self.jolt);
        crt::shake(&mut self.px, &mut self.scratch, w, sh, dx, dy);
        self.torn = self.torn.wrapping_mul(6364136223846793005).wrapping_add(1);
        crt::tear(
            &mut self.px,
            &mut self.scratch,
            w,
            sh,
            std::mem::take(&mut self.tear),
            self.torn,
        );
        crt::collapse(&mut self.px, &mut self.scratch, w, sh, self.curtain);
        resolve_d(&self.px, w, sh, TOL, true, &mut self.cells);
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
