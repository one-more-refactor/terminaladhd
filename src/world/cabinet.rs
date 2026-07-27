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

/// The arena's edge: a hard vector rectangle, one sub-pixel thick, in the
/// game's own hue. `ignite` in `0.0..=1.0` drives it toward white — a frame
/// that flares is the cheapest and loudest way to say a thing just landed.
pub fn frame(b: &mut Buf, l: &Layout, shake: i32, hue: Rgb, ignite: f32) {
    let (x0, y0, x1, y1) = l.arena_sub(shake);
    let col = hue.lerp(c(WHITE), ignite.clamp(0.0, 1.0));
    let glow = col.mul(0.26 + 0.9 * ignite);
    for y in (y0 - 1)..=(y1 + 1) {
        lit(b, x0 - 1, y, col, glow);
        lit(b, x1 + 1, y, col, glow);
    }
    for x in (x0 - 1)..=(x1 + 1) {
        lit(b, x, y0 - 1, col, glow);
        lit(b, x, y1 + 1, col, glow);
    }
    // Corner nubs: two sub-pixels of overshoot on each diagonal. A plain
    // rectangle reads as a text-mode box; the overshoot reads as a bezel.
    for (cx, cy, sx, sy) in [
        (x0 - 1, y0 - 1, -1, -1),
        (x1 + 1, y0 - 1, 1, -1),
        (x0 - 1, y1 + 1, -1, 1),
        (x1 + 1, y1 + 1, 1, 1),
    ] {
        for k in 1..=2 {
            lit(b, cx + sx * k, cy, col, glow);
            lit(b, cx, cy + sy * k, col, glow);
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

// ------------------------------------------------------------------- strip

/// The status strip: `1UP` and the live score at the left, the game's name in
/// the middle, `HI` and the record at the right. The arcade convention, near
/// enough verbatim, because it is the one piece of chrome every player already
/// knows how to read.
///
/// `blink` alternates the `1UP` marker, which is the only part of the strip
/// that moves — and the reason a still frame of an arcade screen always looks
/// like it is running.
pub fn strip(
    b: &mut Buf,
    l: &Layout,
    score: u32,
    name: &str,
    best: u32,
    blink: bool,
    shout: Option<(&str, f32)>,
) {
    let y = l.strip_sub as i32;
    let left = format!("1UP {score:06}");
    let right = format!("HI {best:06}");

    if blink {
        text(b, "1UP", 2, y, 1, c(RED), 0.6);
    }
    text(b, &left[3..], 2 + text_w("1UP", 1), y, 1, c(WHITE), 0.35);

    let rx = l.w as i32 - text_w(&right, 1) - 2;
    text(b, "HI", rx, y, 1, c(CYAN), 0.5);
    text(
        b,
        &right[2..],
        rx + text_w("HI", 1),
        y,
        1,
        c(YELLOW),
        0.35,
    );

    // The middle slot carries the game's name, and gives it up whenever the
    // game has something to shout. A machine talks to you from its status
    // strip; a banner floating over the playfield is a modern habit, and at
    // most sizes there is nowhere to put one that does not collide.
    let mid = l.w as i32 / 2;
    let free = rx - (2 + text_w(&left, 1)) - 6;
    match shout {
        Some((word, life)) if text_w(word, 1) <= free => {
            let fade = (life * 1.8).min(1.0);
            let col = c(WHITE).lerp(c(YELLOW), 1.0 - life).mul(fade);
            text(b, word, mid - text_w(word, 1) / 2, y, 1, col, 1.0 * fade);
        }
        _ if text_w(name, 1) <= free => {
            text(b, name, mid - text_w(name, 1) / 2, y, 1, c(STEEL), 0.0);
        }
        _ => {}
    }
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

/// Row H-1: the wrapped command's last line at the left, the clock at the
/// right. Dim, never bloomed — it is the one thing on screen that is not
/// asking to be looked at.
pub fn ticker(b: &mut Buf, l: &Layout, left: &str, right: &str) {
    let y = l.ticker_sub as i32;
    let dim = c(STEEL).mul(0.75);
    let budget = (((l.w as i32 - text_w(right, 1) - 8) / 4).max(0)) as usize;
    let s: String = left.chars().take(budget).collect();
    text(b, &s, 2, y, 1, dim, 0.0);
    text(b, right, l.w as i32 - text_w(right, 1) - 2, y, 1, dim, 0.0);
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
    let mut y = y;
    for s in stats {
        if l.compact_readouts {
            // Label and value on one line, in the reading's own colour: a short
            // column would rather lose the heading than lose the queue above it.
            text(b, &format!("{} {}", s.short, s.value), x, y, 1, s.hue, 0.45);
            y += FOLDED_SUB as i32;
            continue;
        }
        let inner = heading(b, l, x, y, s.label, s.short);
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
