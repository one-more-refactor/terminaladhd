//! The cabinet: everything on screen that is not the game and not the warp.
//!
//! It is deliberately almost nothing — a status strip, a frame around the
//! arena, a shout, a board. A machine from 1983 had no room for decoration and
//! neither does this: if an element is not the game, not a number the player is
//! chasing, or not feedback for something they just did, it is not here.
//!
//! Nothing needs a backing plate any more. The ground is black, so ink is
//! legible everywhere, which is most of why removing the scenery removed half
//! the code with it.

use super::draw::{add_emis, lit, put_base, text, text_center, text_w};
use super::layout::{Layout, FOLDED_SUB, HEAD_SUB, READOUT_SUB};
use super::scene::palette::*;
use super::scene::Buf;
use crate::world::color::{hex, Rgb};

fn c(v: u32) -> Rgb {
    hex(v)
}

/// Lay the ground. Flat, constant, and the same on every screen — the black an
/// arcade monitor is at rest.
pub fn ground(b: &mut Buf) {
    b.clear();
    let void = c(VOID);
    for p in b.base.iter_mut() {
        *p = void;
    }
}

// ------------------------------------------------------------------- arena

/// The arena's edge, and the lights running round it.
///
/// It used to be a hairline with two sub-pixels of overshoot poking out of each
/// corner. The overshoot was meant to read as a bezel and read as whiskers, and
/// a single sub-pixel under this much bloom is a fuzzy line rather than an
/// edge. So: a solid two-deep rule, no overshoot, and the corners are made of
/// the same rule run brighter and longer rather than of anything sticking out
/// of them.
///
/// A cabinet's marquee had lights chasing its border, and a static rectangle is
/// the one thing on this screen that would otherwise never move. `phase` runs
/// `0..1` round the perimeter; the game drives it faster the better the player
/// is doing, so the frame is a second readout of the same thing the warp field
/// is showing.
///
/// `ignite` in `0.0..=1.0` drives the whole border toward white — a frame that
/// flares is the cheapest and loudest way to say a thing just landed.
pub fn frame(b: &mut Buf, l: &Layout, shake: i32, hue: Rgb, ignite: f32, phase: f32) {
    let (ax0, ay0, ax1, ay1) = l.arena_sub(shake);
    let ignite = ignite.clamp(0.0, 1.0);
    let col = hue.lerp(c(WHITE), ignite);

    // Two rules: the inner one is the edge of the field, the outer one is dim
    // and half a step away. One line reads as a hairline someone forgot to
    // finish; two read as a rail with a shadow.
    let inner = (ax0 - 1, ay0 - 1, ax1 + 1, ay1 + 1);
    let outer = (ax0 - 2, ay0 - 2, ax1 + 2, ay1 + 2);
    // No glow at rest. A lit rail feeds the bloom pass, and bloom over a black
    // field turns a one-sub-pixel line into a soft grey band hanging above and
    // below it — which reads as haze rather than as an edge, and is the ugliest
    // thing a frame can do. It gets its light back only when it flares.
    edge(b, outer, col.mul(0.18), col.mul(0.3 * ignite));
    edge(b, inner, col.mul(0.85), col.mul(1.1 * ignite));

    // Corners are the same rule run a little way along both edges. Nothing
    // sticks out of the rectangle, and nothing is brighter than the game
    // inside it — a frame that outshines the playfield is a picture frame.
    let reach = (l.mino_px as i32 * 3 / 2).clamp(3, 9);
    for (cx, cy, sx, sy) in [
        (ax0 - 1, ay0 - 1, 1, 1),
        (ax1 + 1, ay0 - 1, -1, 1),
        (ax0 - 1, ay1 + 1, 1, -1),
        (ax1 + 1, ay1 + 1, -1, -1),
    ] {
        for k in 0..reach {
            let fade = 1.0 - k as f32 / reach as f32;
            let bright = col.lerp(c(WHITE), 0.35 * fade);
            put_base(b, cx + sx * k, cy, bright);
            put_base(b, cx, cy + sy * k, bright);
        }
    }

    bulbs(b, inner, col, phase);
}

/// One rectangle of rule, one sub-pixel thick.
fn edge(b: &mut Buf, rect: (i32, i32, i32, i32), col: Rgb, glow: Rgb) {
    let (x0, y0, x1, y1) = rect;
    for y in y0..=y1 {
        lit(b, x0, y, col, glow);
        lit(b, x1, y, col, glow);
    }
    for x in x0..=x1 {
        lit(b, x, y0, col, glow);
        lit(b, x, y1, col, glow);
    }
}

/// How many lights run the border at once, and how long each one's tail is.
/// Long tails rather than dots: at this resolution a single bright sub-pixel
/// travelling a perimeter is a flicker, and a comet is a light.
const BULBS: usize = 4;
const BULB_TAIL: i32 = 16;

/// The sub-pixel `t` steps clockwise round a rectangle's perimeter from its
/// top-left corner.
fn perimeter(rect: (i32, i32, i32, i32), t: i32) -> (i32, i32) {
    let (x0, y0, x1, y1) = rect;
    let (w, h) = (x1 - x0, y1 - y0);
    let per = 2 * (w + h);
    let t = t.rem_euclid(per.max(1));
    if t < w {
        (x0 + t, y0)
    } else if t < w + h {
        (x1, y0 + (t - w))
    } else if t < 2 * w + h {
        (x1 - (t - w - h), y1)
    } else {
        (x0, y1 - (t - 2 * w - h))
    }
}

fn bulbs(b: &mut Buf, rect: (i32, i32, i32, i32), hue: Rgb, phase: f32) {
    let (x0, y0, x1, y1) = rect;
    let per = 2 * ((x1 - x0) + (y1 - y0));
    if per <= 0 {
        return;
    }
    let head = (phase.rem_euclid(1.0) * per as f32) as i32;
    for i in 0..BULBS {
        let start = head + (per * i as i32) / BULBS as i32;
        for k in 0..BULB_TAIL {
            // Bright at the head and gone at the tail, so each light reads as
            // travelling rather than as four dots blinking in sequence.
            let fade = 1.0 - k as f32 / BULB_TAIL as f32;
            let (x, y) = perimeter(rect, start - k);
            let lit_col = hue.lerp(c(WHITE), fade * fade);
            lit(b, x, y, lit_col, lit_col.mul(fade * fade * 1.5));
        }
    }
}

/// How the empty cells of an arena are ruled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// A pip at every cell's top-left. Reads as a lattice, which is what a game
    /// that moves in both axes wants.
    Lattice,
    /// Nothing at all. A falling game does not need help seeing its columns,
    /// and an empty well should be empty.
    None,
}

/// Black the arena out and rule it. This is what stops the warp field showing
/// through the playfield — inside the frame, the game is the only thing there
/// is.
pub fn floor(b: &mut Buf, l: &Layout, shake: i32, rule: Rule, filled: &dyn Fn(i32, i32) -> bool) {
    let (x0, y0, x1, y1) = l.arena_sub(shake);
    let void = c(VOID);
    for y in y0..=y1 {
        for x in x0..=x1 {
            put_base(b, x, y, void);
        }
    }
    if rule == Rule::None {
        return;
    }
    let tick = c(IRON);
    for mr in 0..l.rows as i32 {
        for mc in 0..l.cols as i32 {
            if filled(mc, mr) {
                continue;
            }
            let (cx, cy) = l.cell_origin(mc, mr, shake);
            put_base(b, cx, cy, tick);
        }
    }
}

/// Flip the inside of the arena to its negative for a frame or two. The border
/// and the strip hold still, which is what makes the field itself look like it
/// fired rather than the monitor glitching.
pub fn flash_arena(b: &mut Buf, l: &Layout, shake: i32) {
    let (x0, y0, x1, y1) = l.arena_sub(shake);
    for y in y0..=y1 {
        for x in x0..=x1 {
            if x < 0 || y < 0 || x as usize >= b.w || y as usize >= b.sh {
                continue;
            }
            let i = y as usize * b.w + x as usize;
            let p = b.base[i];
            b.base[i] = Rgb::new(
                (1.0 - p.r).max(0.0),
                (1.0 - p.g).max(0.0),
                (1.0 - p.b).max(0.0),
            );
            b.emis[i] = Rgb::ZERO;
        }
    }
}

// ------------------------------------------------------------------- strip

/// The status strip: `1UP` and the live score at the left, the game's name in
/// the middle, `HI` and the record at the right. The arcade convention, near
/// enough verbatim, because it is the one piece of chrome every player already
/// knows how to read.
///
/// `blink` alternates the `1UP` marker, which is the only part of the strip
/// that moves — and the reason a still frame of an arcade screen always looks
/// like it is running.
/// The one row of chrome.
///
/// Everything that is not the game lives here: the score at the left, whatever
/// the game has to shout after it, the wrapped command's last line, and the
/// clock at the right. It used to be a bar across the top carrying a `1UP`
/// marker, the game's name and the all-time best, plus a second row under the
/// well for the score, plus a third for the command.
///
/// The marker said nothing. The name is answered by looking at the game. The
/// record belongs on the screen where records are read. And three rows of
/// chrome around a game is a dashboard — one row is a cabinet.
pub struct Strip<'a> {
    pub score: u32,
    /// What the game has to shout, which takes the space beside the score.
    pub shout: Option<(&'a str, f32)>,
    /// The wrapped command's last line, or empty when nothing is running.
    pub left: &'a str,
    pub right: &'a str,
    /// How far the score is still lifting from its last change.
    pub hot: f32,
}

pub fn strip(b: &mut Buf, l: &Layout, s: &Strip) {
    let y = l.ticker_sub.unwrap_or(2 * l.h - 6) as i32;
    let dim = c(STEEL).mul(0.75);

    // The score is the only bright thing on this row, and it lifts whenever it
    // moves.
    let score = s.score.to_string();
    let live = c(WHITE).mul(1.0 + s.hot);
    text(b, &score, 2, y, 1, live, 0.25 + s.hot);
    let mut x = 2 + text_w(&score, 1) + 6;

    if let Some((word, life)) = s.shout.filter(|(_, life)| *life > 0.0) {
        let fade = (life * 1.8).min(1.0);
        let col = c(WHITE).lerp(c(YELLOW), 1.0 - life).mul(fade);
        text(b, word, x, y, 1, col, 1.0 * fade);
        x += text_w(word, 1) + 6;
    }

    let right_x = l.w as i32 - text_w(s.right, 1) - 2;
    if !s.left.is_empty() {
        let budget = (((right_x - x - 4) / 4).max(0)) as usize;
        let cut: String = s.left.chars().take(budget).collect();
        text(b, &cut, x, y, 1, dim, 0.0);
    }
    text(b, s.right, right_x, y, 1, dim, 0.0);
}

/// The rule under the strip, doubling as the wrapped command's progress bar.
/// `progress` of `None` leaves it inert — the machine is not waiting on
/// anything, so the bar has nothing to say.
pub fn rule(b: &mut Buf, l: &Layout, progress: Option<f32>) {
    // Nothing running means nothing to say. A full-width line drawn on every
    // screen forever is furniture, and furniture is what this design does not
    // have.
    let Some(progress) = progress else { return };
    let y = l.rule_sub as i32;
    let done = (progress.clamp(0.0, 1.0) * l.w as f32) as i32;
    for x in 0..l.w as i32 {
        if x < done {
            // Cyan at the start, magenta as it finishes: the bar says how far
            // along it is by colour as well as by length, which is readable in
            // peripheral vision while the eye is on the game.
            let t = x as f32 / l.w as f32;
            let col = c(CYAN).lerp(c(MAGENTA), t);
            lit(b, x, y, col, col.mul(0.5));
        } else {
            put_base(b, x, y, c(IRON));
        }
    }
}

// ------------------------------------------------------------ side columns
//
// One grammar, used by every game: a label, a hairline under it, then content —
// all on the same left edge, all the same width, stacked at the same pitch.
// Two games that lay their sides out differently read as two programs.

/// Sub-rows a heading claims: five for the face, one of air, one for the rule,
/// two of air under it.
pub const HEAD_ROWS: i32 = 9;

/// Columns a side column occupies: what is left of the flank once it has been
/// hung off the arena, and never wider than the frame it has to end inside.
pub fn col_w(l: &Layout) -> i32 {
    let start = l.right_col.map(|(x, _)| x).unwrap_or(0) as i32;
    (l.w as i32 - start - 1).clamp(4, 26)
}

/// A heading: the label, then a hairline the width of the column. Returns the
/// sub-row its content starts at, so a caller never counts rows itself.
///
/// `label` is the long form and `short` the one used when the column cannot
/// hold it. A clipped word is worse than an abbreviated one — `APPLE` reads as
/// a different reading, `AP` reads as the same one, smaller.
pub fn heading(b: &mut Buf, l: &Layout, x: i32, y: i32, label: &str, short: &str) -> i32 {
    let w = col_w(l);
    let label = if text_w(label, 1) <= w { label } else { short };
    text(b, label, x, y, 1, c(STEEL), 0.0);
    for i in 0..w {
        // Brightest under the label and fading out along its length: a rule
        // that stops dead reads as a border, one that fades reads as an
        // underline.
        let k = 1.0 - i as f32 / w as f32;
        put_base(b, x + i, y + 6, c(IRON).mul(0.35 + 0.65 * k));
    }
    y + HEAD_ROWS
}

/// One reading under a heading of its own.
pub struct Stat {
    pub label: &'static str,
    /// What the label becomes when the column is too narrow for it.
    pub short: &'static str,
    pub value: u32,
    pub hue: Rgb,
}

/// A game's readings, stacked down the right column under whatever the game
/// hung there first. Returns the sub-row after the last one.
pub fn readouts(b: &mut Buf, l: &Layout, x: i32, y: i32, stats: &[Stat]) -> i32 {
    // One form for the whole stack. Shortening only the labels that do not fit
    // leaves APPLES abbreviated next to MULT written out, which reads as a
    // mistake rather than as a decision.
    let room = col_w(l);
    let long = stats.iter().all(|s| text_w(s.label, 1) <= room);
    let mut y = y;
    for s in stats {
        if l.compact_readouts {
            // Label and value on one line, in the reading's own colour: a short
            // column would rather lose the heading than lose the queue above it.
            text(b, &format!("{} {}", s.short, s.value), x, y, 1, s.hue, 0.45);
            y += FOLDED_SUB as i32;
            continue;
        }
        let label = if long { s.label } else { s.short };
        let inner = heading(b, l, x, y, label, s.short);
        text(b, &s.value.to_string(), x, inner, 1, s.hue, 0.45);
        y = inner + (READOUT_SUB - HEAD_SUB) as i32;
    }
    y
}

// ---------------------------------------------------------------- feedback

/// A `+N` rising off the cell that earned it. Both games emit these and this is
/// the only place they are drawn, which is what makes a Tetris and a golden
/// apple feel like the same machine paying out.
pub fn pop(b: &mut Buf, l: &Layout, p: &crate::games::Pop, shake: i32) {
    let (x, y) = l.cell_origin(0, 0, shake);
    let px = x as f32 + p.col * l.mino_px as f32 + l.mino_px as f32 * 0.5;
    let py = y as f32 + p.row * l.mino_px as f32;
    let label = format!("+{}", p.points);
    // Rises a few sub-rows over its life and fades out; the last third is
    // almost gone, so two in a row never read as a stack.
    let rise = ((1.0 - p.life) * 7.0) as i32;
    let fade = (p.life * 1.6).min(1.0);
    let col = c(WHITE).lerp(c(YELLOW), 1.0 - p.life).mul(fade);
    text_center(b, &label, px as i32, py as i32 - 2 - rise, 1, col, 0.9 * fade);
}

/// A radial burst out of a point, in emissive light only — what a cleared row
/// or a swallowed apple throws off. `age` in `0.0..=1.0`.
pub fn burst(b: &mut Buf, cx: f32, cy: f32, reach: f32, age: f32, col: Rgb) {
    if !(0.0..1.0).contains(&age) {
        return;
    }
    let r = reach * age;
    let fade = (1.0 - age) * (1.0 - age);
    // Sixteen spokes rather than a ring: a ring reads as a shockwave from a
    // modern engine, spokes read as a sprite explosion from a cabinet.
    const SPOKES: usize = 16;
    // Half a turn at 22.5-degree steps; the other half is these negated, which
    // is why only eight are listed for sixteen spokes.
    const OCT: f32 = std::f32::consts::FRAC_1_SQRT_2;
    const SIN22: f32 = 0.382_683_43;
    const COS22: f32 = 0.923_879_5;
    const DIRS: [(f32, f32); 8] = [
        (1.0, 0.0),
        (COS22, SIN22),
        (OCT, OCT),
        (SIN22, COS22),
        (0.0, 1.0),
        (-SIN22, COS22),
        (-OCT, OCT),
        (-COS22, SIN22),
    ];
    for i in 0..SPOKES {
        let (dx, dy) = DIRS[i % 8];
        let (dx, dy) = if i < 8 { (dx, dy) } else { (-dx, -dy) };
        for k in 0..3 {
            let rr = r - k as f32 * 1.6;
            if rr <= 0.0 {
                continue;
            }
            add_emis(
                b,
                (cx + dx * rr) as i32,
                (cy + dy * rr) as i32,
                col.mul(fade * (1.0 - k as f32 * 0.3)),
            );
        }
    }
}
