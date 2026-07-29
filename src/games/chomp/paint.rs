//! How chomp is drawn on the cabinet screen.
//!
//! The maze is the classic look spoken in the machine's tones: wall blocks
//! with a lit rim on every side that faces a corridor, dark mass inside — the
//! outline IS the wall, the way it was on the original glass. Dots are pale
//! pips, pellets are the gold diamond the snake's apple taught the screen,
//! the muncher is the one yellow thing alive, and each ghost wears its
//! persona's colour until a pellet turns the whole pack blue.

use crate::games::{Game, Kind};
use crate::world::cabinet::{flash_arena, floor, frame, readouts, Edge, Rule, Stat};
use crate::world::draw::{add_emis, put_base};
use crate::world::layout::Layout;
use crate::world::scene::palette::*;
use crate::world::{hex, Buf, Rgb};

use super::{persona_hue, Cell, Chomp, Ghost, FRIGHT_BLINK};

fn c(v: u32) -> Rgb {
    hex(v)
}

/// The maze's two tones: a rim bright enough to survive the posterize ladder,
/// a body that deliberately does not — walls read as outline, not as mass.
const WALL_RIM: u32 = 0x3E6CFF;
const WALL_BODY: u32 = 0x101E4A;

/// A frightened ghost's body, and the pale meat of a dot.
const FRIGHT_BODY: u32 = 0x2430FF;
const DOT_MEAT: u32 = 0xFFCFA0;

pub fn paint(b: &mut Buf, l: &Layout, g: &Chomp) {
    let shake = g.shake();
    let p = l.mino_px as i32;
    let t = g.elapsed.as_secs_f32();

    floor(b, l, shake, Rule::None, &|_, _| false);

    // The walls of the boundary are furniture — it is the pack that kills.
    // The frame flares when a hunt starts and when the run ends.
    let ignite = if g.death() > 0.0 {
        (1.0 - g.death()).max(0.0)
    } else if g.flashing() {
        0.8
    } else {
        0.0
    };
    frame(b, l, shake, Kind::Chomp.hue(), ignite, [Edge::Wall; 4]);

    // The maze. The outer ring is the frame's job; the tunnel mouths punch
    // visibly through it so the wrap reads as a doorway, not a glitch.
    for my in 0..g.rows() {
        for mx in 0..g.cols() {
            match g.cell(mx, my) {
                Cell::Wall => wall(b, l, g, mx, my, shake, p),
                Cell::Dot => dot(b, l, mx, my, shake, p),
                Cell::Pellet => pellet(b, l, mx, my, shake, p, t),
                Cell::Empty => {}
            }
        }
    }
    tunnel_mouths(b, l, g, shake, p);

    for gh in g.ghosts() {
        ghost(b, l, g, gh, shake, p, t);
    }
    muncher(b, l, g, shake, p);

    if g.flashing() {
        flash_arena(b, l, shake);
    }
    g.sparks.draw(b, l, shake);

    // The right column: the level and what is left of it.
    if let Some((cx, cy)) = l.right_col {
        let stats = [
            Stat {
                label: "LEVEL",
                short: "LV",
                value: g.level(),
                hue: Kind::Chomp.hue(),
            },
            Stat {
                label: "DOTS",
                short: "DT",
                value: g.dots_left(),
                hue: c(DOT_MEAT),
            },
        ];
        readouts(b, l, cx as i32, 2 * cy as i32, &stats);
    }
}

/// One wall cell: dark mass, with a lit rim on each side that faces open
/// maze. Neighbouring wall cells share no rim, so a run of them reads as one
/// solid block with a single outline — the classic maze wall.
fn wall(b: &mut Buf, l: &Layout, g: &Chomp, mx: i32, my: i32, shake: i32, p: i32) {
    // The outer ring belongs to the frame.
    if mx == 0 || my == 0 || mx == g.cols() - 1 || my == g.rows() - 1 {
        return;
    }
    let (x0, y0) = l.cell_origin(mx, my, shake);
    let body = c(WALL_BODY);
    let rim = c(WALL_RIM);
    for dy in 0..p {
        for dx in 0..p {
            put_base(b, x0 + dx, y0 + dy, body);
        }
    }
    let open = |dx: i32, dy: i32| g.cell(mx + dx, my + dy) != Cell::Wall;
    if open(0, -1) {
        for dx in 0..p {
            put_base(b, x0 + dx, y0, rim);
        }
    }
    if open(0, 1) {
        for dx in 0..p {
            put_base(b, x0 + dx, y0 + p - 1, rim.mul(0.55));
        }
    }
    if open(-1, 0) {
        for dy in 0..p {
            put_base(b, x0, y0 + dy, rim);
        }
    }
    if open(1, 0) {
        for dy in 0..p {
            put_base(b, x0 + p - 1, y0 + dy, rim.mul(0.55));
        }
    }
}

/// The tunnel mouths: where the maze's tunnel row meets the boundary, the
/// frame is cut open so the wrap has a visible door at both ends.
fn tunnel_mouths(b: &mut Buf, l: &Layout, g: &Chomp, shake: i32, p: i32) {
    let (ax0, ay0, ax1, _) = l.arena_sub(shake);
    for my in 0..g.rows() {
        if g.cell(0, my) == Cell::Wall {
            continue;
        }
        let (_, y0) = l.cell_origin(0, my, shake);
        for dy in 0..p {
            for dx in 1..=2 {
                put_base(b, ax0 - dx, y0 + dy, c(VOID));
                put_base(b, ax1 + dx, y0 + dy, c(VOID));
            }
        }
        let _ = ay0;
    }
}

/// A dot: one pale pip, dead centre. It is most of what is on screen, so it
/// stays quiet — a field of blinking anything is a field nobody can read.
fn dot(b: &mut Buf, l: &Layout, mx: i32, my: i32, shake: i32, p: i32) {
    let (x0, y0) = l.cell_origin(mx, my, shake);
    let s = (p / 4).max(1);
    let o = (p - s) / 2;
    let meat = c(DOT_MEAT).mul(0.8);
    for dy in 0..s {
        for dx in 0..s {
            put_base(b, x0 + o + dx, y0 + o + dy, meat);
        }
    }
}

/// A pellet: the gold diamond, pulsing in brightness and never in shape —
/// the same object language as the snake's golden apple, because it makes
/// the same promise: take this and something big happens.
fn pellet(b: &mut Buf, l: &Layout, mx: i32, my: i32, shake: i32, p: i32, t: f32) {
    let (x0, y0) = l.cell_origin(mx, my, shake);
    let pulse = 0.7 + 0.3 * ((t * 4.0).sin() * 0.5 + 0.5);
    let gold = c(YELLOW).mul(pulse);
    let half = (p - 1) / 2;
    for dy in 0..p {
        for dx in 0..p {
            // The diamond: skip the knocked corners.
            if (dx - half).abs() + (dy - half).abs() > half + 1 {
                continue;
            }
            let col = if dy == 0 || dx == 0 {
                gold.lerp(c(WHITE), 0.25)
            } else if dy == p - 1 || dx == p - 1 {
                gold.mul(0.6)
            } else {
                gold
            };
            put_base(b, x0 + dx, y0 + dy, col);
        }
    }
    add_emis(b, x0 + half, y0 + half, gold.mul(0.5));
}

/// The muncher: the one yellow thing alive. The mouth is a wedge carved out
/// of the facing side, chewing with the glide; the death is the same mouth
/// opening all the way and swallowing the body — the classic wilt, cheap.
fn muncher(b: &mut Buf, l: &Layout, g: &Chomp, shake: i32, p: i32) {
    if g.death() >= 1.0 {
        return;
    }
    let (fx, fy) = g.player_pos();
    let (ox, oy) = l.cell_origin(0, 0, shake);
    let x0 = ox + (fx * p as f32) as i32;
    let y0 = oy + (fy * p as f32) as i32;

    let body = c(YELLOW);
    let lit = body.lerp(c(WHITE), 0.25);
    for dy in 0..p {
        for dx in 0..p {
            // Knock the corners: a coin, not a brick.
            let corner = (dx == 0 || dx == p - 1) && (dy == 0 || dy == p - 1);
            if p >= 4 && corner {
                continue;
            }
            let col = if dy == 0 { lit } else { body };
            put_base(b, x0 + dx, y0 + dy, col);
        }
    }

    // The mouth: a wedge from the centre of the facing edge. Alive it chews
    // between shut and open with the walk; dead it opens all the way.
    let chew = ((g.glide() * std::f32::consts::PI).sin() * 0.5).abs();
    let frac = if g.death() > 0.0 {
        0.5 + g.death() * 0.5
    } else {
        0.15 + chew * 0.45
    };
    let reach = ((p as f32 * frac) as i32).min(p);
    let mid = p / 2;
    for k in 0..reach {
        // The wedge widens as it leaves the mouth's corner of the face.
        let spread = ((reach - k) as f32 / reach.max(1) as f32 * mid as f32) as i32;
        for s in -spread..=spread {
            let (dx, dy) = match g.dir() {
                super::Dir::Right => (p - 1 - k, mid + s),
                super::Dir::Left => (k, mid + s),
                super::Dir::Up => (mid + s, k),
                super::Dir::Down => (mid + s, p - 1 - k),
            };
            if (0..p).contains(&dx) && (0..p).contains(&dy) {
                put_base(b, x0 + dx, y0 + dy, c(VOID));
            }
        }
    }
    add_emis(b, x0 + mid, y0 + mid, body.mul(0.4));
}

/// A ghost: dome head, toothed skirt, and two eyes that look where it is
/// going. Frightened it turns the deep panic blue and blinks white when the
/// hunt is nearly over; eaten, only the eyes fly home.
fn ghost(b: &mut Buf, l: &Layout, g: &Chomp, gh: &Ghost, shake: i32, p: i32, t: f32) {
    if gh.wait > 0.0 {
        // Unreleased ghosts materialise at home: a blink, so the pack
        // arriving is legible before it is lethal.
        if ((t * 4.0) as u32).is_multiple_of(2) {
            return;
        }
    }
    let (fx, fy) = g.ghost_pos(gh);
    let (ox, oy) = l.cell_origin(0, 0, shake);
    let x0 = ox + (fx * p as f32) as i32;
    let y0 = oy + (fy * p as f32) as i32;
    let mid = p / 2;

    let (edx, edy) = gh.dir.delta();
    if !gh.eyes {
        let frightened = gh.fright > 0.0;
        let blink =
            frightened && gh.fright < FRIGHT_BLINK && ((gh.fright * 6.0) as u32).is_multiple_of(2);
        let body = if blink {
            c(WHITE)
        } else if frightened {
            c(FRIGHT_BODY)
        } else {
            persona_hue(gh.persona)
        };
        let lit = body.lerp(c(WHITE), 0.2);
        for dy in 0..p {
            for dx in 0..p {
                // The dome: knock the top corners. The skirt: every other
                // sub-pixel of the bottom row, so it reads as teeth.
                if dy == 0 && (dx == 0 || dx == p - 1) && p >= 3 {
                    continue;
                }
                if dy == p - 1 && p >= 3 && dx % 2 == 1 {
                    continue;
                }
                let col = if dy == 0 { lit } else { body };
                put_base(b, x0 + dx, y0 + dy, col);
            }
        }
        if frightened && !blink {
            add_emis(b, x0 + mid, y0 + mid, body.mul(0.5));
        }
    }

    // The eyes: white pips shifted the way it is walking. On an eaten ghost
    // they are all that is left.
    let eye = if gh.eyes || gh.fright > 0.0 {
        c(WHITE)
    } else {
        c(WHITE).mul(0.9)
    };
    if p >= 4 {
        let ey = p / 3 + edy.max(0);
        let off = (p / 4).max(1);
        put_base(b, x0 + mid - off + edx, y0 + ey, eye);
        put_base(b, x0 + mid + off + edx, y0 + ey, eye);
    } else {
        put_base(b, x0 + mid, y0 + (p / 3), eye);
    }
}
