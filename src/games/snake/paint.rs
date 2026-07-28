//! How snake is drawn on the cabinet screen.
//!
//! The body is a lit tube: cyan at the head, magenta at the tail, so length is
//! something you read off the colour without counting. Everything else here
//! exists to make one of two facts unmissable at a glance — where the head is
//! about to be, and how much the next apple is worth.

use crate::games::{Game, Kind};
use crate::world::cabinet::{burst, flash_arena, floor, frame, readouts, Rule, Stat};
use crate::world::draw::{add_emis, pill, put_base};
use crate::world::layout::Layout;
use crate::world::scene::palette::*;
use crate::world::{hex, Buf, Rgb};

use super::{Dir, Snake};

fn c(v: u32) -> Rgb {
    hex(v)
}

/// How close the head has to get before the wall ahead of it lights up.
const WARN_CELLS: i32 = 3;

/// Head hue to tail hue, through violet rather than through white. A straight
/// cyan-to-magenta lerp desaturates through the middle, and a snake with a pale
/// waist reads as a bug rather than as a gradient.
fn body_color(t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        c(CYAN).lerp(c(VIOLET), t * 2.0)
    } else {
        c(VIOLET).lerp(c(MAGENTA), (t - 0.5) * 2.0)
    }
}

/// Two dark pupils on the head, looking where it is going. Skipped below a
/// four-sub-pixel mino, where they would eat the whole face.
fn eyes(b: &mut Buf, x: i32, y: i32, size: i32, dir: Dir) {
    if size < 4 {
        return;
    }
    let ink = c(VOID);
    let near = size - 2;
    let (a, d) = match dir {
        Dir::Up => ((1, 1), (near, 1)),
        Dir::Down => ((1, near), (near, near)),
        Dir::Left => ((1, 1), (1, near)),
        Dir::Right => ((near, 1), (near, near)),
    };
    for (ex, ey) in [a, d] {
        put_base(b, x + ex, y + ey, ink);
    }
}

/// The wall the head is running at, lit in proportion to how little room is
/// left. This is the only warning the player gets, so it has to arrive before
/// the mistake rather than with it.
///
/// Measured against the *live* arena, not the compile-time one: the field is
/// width-adaptive, and against the constants the right-wall warning never
/// fired at all on narrow frames and burned falsely mid-field on wide ones —
/// the one warning the player gets, dead at exactly the sizes people play.
fn wall_warning(b: &mut Buf, l: &Layout, g: &Snake, shake: i32) {
    let (hx, hy) = g.head();
    let gap = match g.dir() {
        Dir::Up => hy,
        Dir::Down => g.rows() - 1 - hy,
        Dir::Left => hx,
        Dir::Right => g.cols() - 1 - hx,
    };
    if !(0..=WARN_CELLS).contains(&gap) {
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
    let (cx, cy) = (x as f32 + p as f32 / 2.0, y as f32 + p as f32 / 2.0);

    let (hue, pulse) = match a.ttl {
        // Urgency is carried by the blink rate, not by dimming: a golden apple
        // must never be hard to see in the moment it is worth the most. The
        // rate normalises against this arena's own clock, which stretches
        // with the field.
        Some(ttl) => {
            let rate = 6.0 + 18.0 * (1.0 - (ttl / g.gold_life()).clamp(0.0, 1.0));
            (c(YELLOW), 0.55 + 0.45 * (t * rate).sin())
        }
        None => (c(GREEN), 0.75 + 0.25 * (t * 4.0).sin()),
    };

    pill(b, x, y, p, hue.mul(0.85 + 0.15 * pulse), 1.0 * pulse);
    if a.gold {
        burst(b, cx, cy, p as f32 * 2.2, 0.35 + 0.4 * pulse, hue.mul(0.8));
    }
}

/// The body, head first. After a crash it burns off from the tail, and the cell
/// currently going flashes white — the run reads as consumed rather than as
/// simply switched off.
fn body(b: &mut Buf, l: &Layout, g: &Snake, shake: i32) {
    let p = l.mino_px as i32;
    let n = g.len().max(1) as f32;
    let death = g.death();
    // Every segment is drawn part of the way between where it was and where it
    // is. Whole-cell steps read as a cursor; the same steps interpolated read
    // as an animal.
    let slide = g.glide();
    for i in 0..g.len() {
        // Tail-first: the last cell has the largest burn position, so it goes
        // as soon as the dissolve starts.
        let burn = (n - 1.0 - i as f32) / n;
        if death > 0.0 && death >= burn + 1.0 / n {
            continue;
        }
        let t = i as f32 / n;
        let (fc, fr) = g.segment_at(i, slide);
        let (ox, oy) = l.cell_origin(0, 0, shake);
        let x = ox + (fc * p as f32).round() as i32;
        let y = oy + (fr * p as f32).round() as i32;
        let igniting = death > 0.0 && death >= burn;
        let col = if igniting { c(WHITE) } else { body_color(t) };
        // The head carries the light; the tail is nearly matte, which keeps a
        // long snake from blooming into one pale rope. On top of that a pulse
        // runs head to tail — the body is the one thing on screen that is
        // always there, and a rope that only moves when the snake does is a
        // rope nobody looks at twice.
        let wave = ((g.elapsed.as_secs_f32() * 5.0 - t * 6.0).sin() * 0.5 + 0.5).powi(3);
        let halo = if i == 0 {
            0.9
        } else {
            0.30 * (1.0 - t) + 0.5 * wave
        };
        pill(b, x, y, p, col, if igniting { 1.5 } else { halo });
        if i == 0 && !igniting {
            eyes(b, x, y, p, g.dir());
        }
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

    // A bitmap, not a list: the lattice asks about every cell and a linear
    // scan of the body per cell made an empty-looking pass O(body x cells) —
    // half a million comparisons a frame on a long snake.
    let (cols, rows) = (g.cols(), g.rows());
    let mut occupied = vec![false; (cols * rows) as usize];
    for &(x, y) in g.body() {
        if (0..cols).contains(&x) && (0..rows).contains(&y) {
            occupied[(y * cols + x) as usize] = true;
        }
    }
    floor(b, l, shake, Rule::Lattice, &|mc, mr| {
        (0..cols).contains(&mc) && (0..rows).contains(&mr) && occupied[(mr * cols + mc) as usize]
    });
    // The frame flares when an apple lands and when the run ends. It is the
    // only time it is allowed to be brighter than the field.
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
