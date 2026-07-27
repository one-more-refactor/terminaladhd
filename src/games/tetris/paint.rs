//! How tetris is drawn on the cabinet screen.
//!
//! Everything here is the game's own: the blocks, the ghost, the HOLD and NEXT
//! columns, the white-out a clear throws. The ground, the warp field and the
//! status strip are the shell's and are already on the buffer.
//!
//! One rule governs the palette: on black, dim means invisible. Nothing is
//! greyed to say it is inactive — it is either lit or it is not drawn.

use crate::games::Kind;
use crate::world::cabinet::{burst, floor, frame, heading, readouts, Rule, Stat};
use crate::world::draw::{add_emis, capsule, lit};
use crate::world::layout::Layout;
use crate::world::scene::palette::*;
use crate::world::{hex, Buf, Rgb};

use super::{Mino, Tetris};

fn c(v: u32) -> Rgb {
    hex(v)
}

/// A dotted outline in the piece's own hue, no fill — the drop target has to be
/// unmistakably not a block.
fn ghost(b: &mut Buf, x0: i32, y0: i32, size: i32, mino: Mino) {
    let col = mino.color().mul(0.55);
    for dy in 0..size {
        for dx in 0..size {
            let edge = dx == 0 || dy == 0 || dx == size - 1 || dy == size - 1;
            if edge && (dx + dy) % 2 == 0 {
                lit(b, x0 + dx, y0 + dy, col, col.mul(0.5));
            }
        }
    }
}

fn hold_column(b: &mut Buf, l: &Layout, hold: Option<Mino>, ready: bool) {
    let Some((hx, hy)) = l.left_col else { return };
    let inner = heading(b, l, hx as i32, 2 * hy as i32, "HOLD", "HLD");
    if let Some(m) = hold {
        // A spent hold is drawn at a third: the slot is visibly not available
        // again until the next piece locks.
        let k = if ready { 1.0 } else { 0.33 };
        capsule(b, hx as i32, inner, l.mino_px as i32, m.color().mul(k), 0.8 * k);
    }
}

/// The right column, top to bottom: the queue, then the readings, on the same
/// left edge and the same rhythm. Nothing floats.
fn right_column(b: &mut Buf, l: &Layout, g: &Tetris) {
    let Some((nx, ny)) = l.right_col else { return };
    let x = nx as i32;
    let p = l.mino_px as i32;

    let mut y = heading(b, l, x, 2 * ny as i32, "NEXT", "NXT");
    let deep = l.next_deep.min(g.next().len());
    for (i, &m) in g.next().iter().take(deep).enumerate() {
        // The queue recedes: the piece you get next is lit, the ones behind it
        // fade back, so the eye lands on the right one without counting.
        let fade = 1.0 - i as f32 / (deep as f32 + 1.0);
        capsule(b, x, y, p, m.color().mul(0.35 + 0.65 * fade), 0.8 * fade);
        y += p + 2;
    }

    readouts(
        b,
        l,
        x,
        y + 5,
        &[
            Stat {
                label: "LINES",
                short: "LN",
                value: g.lines(),
                hue: c(GREEN),
            },
            Stat {
                label: "LEVEL",
                short: "LV",
                value: g.level(),
                hue: c(MAGENTA),
            },
        ],
    );
}

pub fn paint(b: &mut Buf, l: &Layout, g: &Tetris) {
    let shake = g.shake();
    let well = g.cells();
    let clearing = g.clearing_rows();
    let p = l.mino_px as i32;

    // No rule inside the well: a falling game does not need help seeing its
    // columns, and an empty well should look empty.
    floor(b, l, shake, Rule::None, &|_, _| false);
    // The frame ignites while rows are lit and cools as they collapse.
    let ignite = if clearing.is_empty() { 0.0 } else { 0.9 };
    frame(b, l, shake, Kind::Tetris.hue(), ignite);

    // The stack falls into a cleared gap over about a tenth of a second. A
    // block is drawn as many rows above where it now logically sits as there
    // were cleared rows under it, scaled by how far the collapse has to go.
    let (collapse, gone) = g.collapsing();
    let lift = |mr: i32| -> f32 {
        if collapse <= 0.0 {
            return 0.0;
        }
        let under = gone.iter().filter(|&&r| (r as i32) > mr).count();
        -(under as f32) * collapse
    };

    for mr in 0..l.rows as i32 {
        for mc in 0..l.cols as i32 {
            let Some(m) = well[mr as usize][mc as usize] else {
                continue;
            };
            let (x, y) = l.cell_origin(mc, mr, shake);
            let y = y + (lift(mr) * p as f32) as i32;
            if clearing.contains(&(mr as usize)) {
                capsule(b, x, y, p, c(WHITE), 1.4);
                continue;
            }
            capsule(b, x, y, p, m.color().mul(0.82), 0.32);
        }
    }

    // The streak a hard drop leaves: a column of the piece's own light from
    // where it started to where it stopped, fading out behind it.
    if let Some(t) = g.trail() {
        let col = t.mino.color().mul(t.life * t.life * 0.9);
        for &(mc, mr) in &t.cells {
            let (x, top) = l.cell_origin(mc, mr, shake);
            for dy in 0..(t.rows * p) {
                // Brightest at the top of the fall, where the piece was
                // longest ago, so the streak reads as a wake.
                let k = 1.0 - dy as f32 / (t.rows * p) as f32;
                for dx in 0..p {
                    add_emis(b, x + dx, top + dy, col.mul(k * 0.8));
                }
            }
        }
    }

    if let (Some((mino, cells)), gh) = (g.active(), g.ghost()) {
        if let Some(gh) = gh {
            for &(mc, mr) in &gh {
                let (x, y) = l.cell_origin(mc, mr, shake);
                ghost(b, x, y, p, mino);
            }
        }
        // A grounded piece brightens as its lock window runs out, so the last
        // moment to slide it is something you see rather than something you
        // have to count.
        let urgency = g.lock_phase();
        let fill = mino.color().lerp(c(WHITE), 0.5 * urgency);
        // The piece is drawn where it is going, not where it has got to: part
        // of a row down under gravity and part of a cell behind its own column
        // after a shift. Whole-cell steps are what make a falling game feel
        // like a spreadsheet.
        let (dx, dy) = g.drift();
        let (ox, oy) = ((dx * p as f32) as i32, (dy * p as f32) as i32);
        for &(mc, mr) in &cells {
            let (x, y) = l.cell_origin(mc, mr, shake);
            capsule(b, x + ox, y + oy, p, fill, 0.9 + 0.7 * urgency);
        }
    }

    // A burst out of every cleared row, so a Tetris throws four of them.
    for &mr in &clearing {
        let (_, y) = l.cell_origin(0, mr as i32, shake);
        burst(
            b,
            l.arena.x0 as f32 + l.arena.w() as f32 * 0.5,
            y as f32 + p as f32 * 0.5,
            l.arena.w() as f32 * 0.6,
            0.45,
            c(WHITE),
        );
    }

    hold_column(b, l, g.hold(), g.hold_ready());
    right_column(b, l, g);
}
