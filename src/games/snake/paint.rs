//! How snake is drawn on the cabinet screen.
//!
//! The body is a lit tube: cyan at the head, magenta at the tail, so length is
//! something you read off the colour without counting. Everything else here
//! exists to make one of two facts unmissable at a glance — where the head is
//! about to be, and how much the next apple is worth.

use crate::games::{Game, Kind};
use crate::world::cabinet::{flash_arena, floor, frame, readouts, Rule, Stat};
use crate::world::draw::{add_emis, diamond, link, put_base};
use crate::world::layout::Layout;
use crate::world::scene::palette::*;
use crate::world::{hex, Buf, Rgb};

use super::{Dir, Snake, COLS, ROWS};

fn c(v: u32) -> Rgb {
    hex(v)
}

/// How close the head has to get before the wall ahead of it lights up.
const WARN_CELLS: i32 = 3;

/// One colour for the whole body.
///
/// It used to be a gradient from cyan to magenta with a pulse travelling down
/// it. On a screen this size a gradient under bloom is a smear, and the shape
/// was doing none of the work. A snake is legible because it is a chain of
/// parts with holes in them and one solid head — which is how every snake ever
/// drawn on a small screen has been drawn, and it survived for a reason.
const BODY: u32 = CYAN;

/// The wall the head is running at, lit in proportion to how little room is
/// left. This is the only warning the player gets, so it has to arrive before
/// the mistake rather than with it.
fn wall_warning(b: &mut Buf, l: &Layout, g: &Snake, shake: i32) {
    let (hx, hy) = g.head();
    let gap = match g.dir() {
        Dir::Up => hy,
        Dir::Down => ROWS - 1 - hy,
        Dir::Left => hx,
        Dir::Right => COLS - 1 - hx,
    };
    if gap > WARN_CELLS {
        return;
    }
    let heat = 1.0 - gap as f32 / (WARN_CELLS + 1) as f32;
    // The wall's own amber, driven toward white as the gap closes. A warning
    // that changes hue reads as a different object; one that only gets hotter
    // reads as the same wall, closer.
    let col = c(ORANGE).lerp(c(WHITE), heat * 0.6).mul(heat * 1.7);
    let (x0, y0, x1, y1) = l.arena_sub(shake);
    let span = 3 * l.mino_px as i32;
    match g.dir() {
        Dir::Left | Dir::Right => {
            let x = if g.dir() == Dir::Left { x0 - 1 } else { x1 + 1 };
            let cy = l.cell_origin(hx, hy, shake).1 + l.mino_px as i32 / 2;
            for y in (cy - span)..=(cy + span) {
                add_emis(b, x, y, col);
                add_emis(b, x + if g.dir() == Dir::Left { -1 } else { 1 }, y, col);
            }
        }
        Dir::Up | Dir::Down => {
            let y = if g.dir() == Dir::Up { y0 - 1 } else { y1 + 1 };
            let cx = l.cell_origin(hx, hy, shake).0 + l.mino_px as i32 / 2;
            for x in (cx - span)..=(cx + span) {
                add_emis(b, x, y, col);
                add_emis(b, x, y + if g.dir() == Dir::Up { -1 } else { 1 }, col);
            }
        }
    }
}

/// The apple. A plain one breathes; a golden one strobes faster the closer it
/// is to leaving, and throws a burst so it is spotted from the far corner.
fn apple(b: &mut Buf, l: &Layout, g: &Snake, shake: i32, t: f32) {
    let a = g.apple();
    let p = l.mino_px as i32;
    let (x, y) = l.cell_origin(a.at.0, a.at.1, shake);

    let Some(ttl) = a.ttl else {
        // An ordinary morsel: a diamond, because it has to read as not-snake at
        // a glance and a square cannot.
        let pulse = 0.8 + 0.2 * (t * 4.0).sin();
        diamond(b, x, y, p, c(GREEN).mul(pulse), 0.9);
        return;
    };

    // The bonus blinks between a solid block and the same diamond, so it never
    // disappears outright, and the blink doubles in rate over the last quarter
    // of its clock. Urgency is carried by the rate, never by dimming: it must
    // not be hard to see in the moment it is worth the most.
    let fast = ttl <= super::GOLD_LIFE / 4.0;
    let rate = if fast { 14.0 } else { 7.0 };
    if (t * rate).sin() > 0.0 {
        link(b, x, y, p, c(YELLOW), false, 1.2);
    } else {
        diamond(b, x, y, p, c(YELLOW), 1.0);
    }
}

fn body(b: &mut Buf, l: &Layout, g: &Snake, shake: i32) {
    let p = l.mino_px as i32;
    let n = g.len().max(1) as f32;
    let death = g.death();
    let slide = g.glide();
    let (ox, oy) = l.cell_origin(0, 0, shake);
    for i in 0..g.len() {
        // Tail-first: the last cell has the largest burn position, so it goes
        // as soon as the dissolve starts.
        let burn = (n - 1.0 - i as f32) / n;
        if death > 0.0 && death >= burn + 1.0 / n {
            continue;
        }
        let (fc, fr) = g.segment_at(i, slide);
        let x = ox + (fc * p as f32).round() as i32;
        let y = oy + (fr * p as f32).round() as i32;
        let igniting = death > 0.0 && death >= burn;
        let head = i == 0;
        // The head is the only part that is filled in and the only part that is
        // near-white, so which end is which is never a question.
        let col = if igniting {
            c(WHITE)
        } else if head {
            c(WHITE).lerp(c(BODY), 0.35)
        } else {
            c(BODY)
        };
        let halo = if igniting {
            1.5
        } else if head {
            0.8
        } else {
            0.22
        };
        link(b, x, y, p, col, !head && !igniting, halo);
    }
}

/// The right column: the readings, with the streak's drain bar under the
/// multiplier it belongs to. The bar is the only element on screen that moves
/// on its own, which is what keeps the eye coming back to it.
fn right_column(b: &mut Buf, l: &Layout, g: &Snake) {
    let Some((cx, cy)) = l.right_col else { return };
    let x = cx as i32;
    let end = readouts(
        b,
        l,
        x,
        2 * cy as i32,
        // Only the multiplier. The apple count was the score by another name,
        // and the column is better for having one number in it that the player
        // is actually playing for.
        &[Stat {
            label: "MULT",
            short: "MU",
            value: g.mult(),
            hue: c(MAGENTA),
        }],
    );

    let left = g.streak_left();
    if left <= 0.0 {
        return;
    }
    let w = crate::world::cabinet::col_w(l);
    let lit_to = (w as f32 * left).round() as i32;
    for i in 0..w {
        let col = if i < lit_to { c(YELLOW) } else { c(IRON) };
        put_base(b, x + i, end + 1, col);
        if i < lit_to {
            add_emis(b, x + i, end + 1, col.mul(0.8));
        }
    }
}

pub fn paint(b: &mut Buf, l: &Layout, g: &Snake) {
    let shake = g.shake();
    // The animation clock is the game's own elapsed time, so a pulse never
    // stutters when the frame rate does.
    let t = g.elapsed.as_secs_f32();

    let occupied: Vec<(i32, i32)> = g.body().iter().copied().collect();
    floor(b, l, shake, Rule::Lattice, &|mc, mr| {
        occupied.contains(&(mc, mr))
    });
    // The frame flares when an apple lands and when the run ends.
    let ignite = if g.death() > 0.0 {
        (1.0 - g.death()).max(0.0)
    } else {
        (0.8 - g.since_eat() * 9.0).max(0.0)
    };
    let chase = t * (0.45 + 1.6 * g.heat());
    frame(b, l, shake, Kind::Snake.hue(), ignite, chase);
    if g.death() == 0.0 {
        wall_warning(b, l, g, shake);
    }

    apple(b, l, g, shake, t);
    body(b, l, g, shake);
    // The field fires, and nothing else moves. A border and a strip holding
    // still is what makes it read as the arena rather than the monitor.
    if g.flashing() {
        flash_arena(b, l, shake);
    }
    g.sparks.draw(b, l, shake);

    right_column(b, l, g);
}
