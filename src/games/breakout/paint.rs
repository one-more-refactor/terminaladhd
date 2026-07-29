//! How breakout is drawn on the cabinet screen.
//!
//! The wall carries the whole palette — six rows in the classic ladder, red
//! paying most at the top — the paddle is steel that heats with the player,
//! and the ball is the one white thing on the court, because white is
//! reserved for what matters and nothing matters more than where the ball is.

use crate::games::{Game, Kind};
use crate::world::cabinet::{floor, frame, readouts, Edge, Rule, Stat};
use crate::world::draw::{add_emis, slab};
use crate::world::layout::Layout;
use crate::world::scene::palette::*;
use crate::world::{hex, Buf, Rgb};

use super::{Breakout, WALL_ROWS};

fn c(v: u32) -> Rgb {
    hex(v)
}

/// The wall's ladder, top row first: the hot end of the palette where the
/// points are, the cool end where they are not. The same order every cabinet
/// used, because it is a bar chart you can read from across a room.
pub fn row_color(row: usize) -> Rgb {
    const LADDER: [u32; WALL_ROWS] = [RED, ORANGE, YELLOW, GREEN, CYAN, BLUE];
    c(LADDER[row.min(WALL_ROWS - 1)])
}

pub fn paint(b: &mut Buf, l: &Layout, g: &Breakout) {
    let shake = g.shake();
    let p = l.mino_px as i32;

    floor(b, l, shake, Rule::None, &|_, _| false);

    // Three quiet walls the ball plays off, and a floor that is not there:
    // the open bottom is dashed in the game's violet, because the one edge
    // that takes a ball must not be drawn as a wall.
    frame(
        b,
        l,
        shake,
        Kind::Breakout.hue(),
        0.0,
        [Edge::Wall, Edge::Wall, Edge::Lava, Edge::Wall],
    );

    // The wall.
    for brick in g.bricks() {
        let (x, y) = l.cell_origin(brick.at.0, brick.at.1, shake);
        slab(b, x, y, 2 * p, p, row_color(brick.row).mul(0.85), 0.25);
    }

    // The paddle: steel at rest, white-hot with the run. Blinks out over the
    // death rather than being cut — losing the last ball should read as the
    // machine's verdict, not a dropped frame.
    let (px, pw) = g.paddle();
    let show_paddle = !(g.death() > 0.0 && (g.death() * 6.0) as u32 % 2 == 1);
    if show_paddle {
        let (ox, oy) = l.cell_origin(0, g.paddle_row(), shake);
        let w = (pw * p as f32) as i32;
        let x = ox + ((px - pw * 0.5) * p as f32) as i32;
        let fill = c(STEEL).lerp(c(WHITE), 0.35 + 0.5 * g.heat());
        slab(b, x, oy + p / 4, w, (p - p / 4).max(2), fill, 0.3);
    }

    // The ball: a fat white square, snapped to the spark grid so it moves in
    // the same chunks everything else does. On the serve it blinks, which is
    // the cabinet's way of saying "yours in a moment".
    let blink = ((g.elapsed * 6.0) as u32).is_multiple_of(2);
    if (g.flying() || (g.serving() && blink)) && g.death() == 0.0 {
        let (bx, by) = g.ball();
        let (ox, oy) = l.cell_origin(0, 0, shake);
        let size = (p * 3 / 5).max(2);
        let x = ((ox as f32 + bx * p as f32) as i32 - size / 2).div_euclid(2) * 2;
        let y = ((oy as f32 + by * p as f32) as i32 - size / 2).div_euclid(2) * 2;
        for dy in 0..size {
            for dx in 0..size {
                crate::world::draw::put_base(b, x + dx, y + dy, c(WHITE));
            }
        }
        // A one-block wake behind a flying ball: speed you can see at the
        // edge of vision, cheap as one dash.
        if g.flying() {
            let (vx, vy) = g.vel_dir();
            let wx = ((x as f32 - vx * size as f32) as i32).div_euclid(2) * 2;
            let wy = ((y as f32 - vy * size as f32) as i32).div_euclid(2) * 2;
            for dy in 0..size {
                for dx in 0..size {
                    add_emis(b, wx + dx, wy + dy, c(WHITE).mul(0.22));
                }
            }
        }
    }

    g.sparks.draw(b, l, shake);

    // The right column: the balls in hand, and the wall count once the first
    // one has gone — the two numbers a run is remembered by.
    if let Some((cx, cy)) = l.right_col {
        let mut stats = vec![Stat {
            label: "BALLS",
            short: "BL",
            value: g.balls_left(),
            hue: c(WHITE),
        }];
        if g.walls() > 0 {
            stats.push(Stat {
                label: "WALLS",
                short: "WL",
                value: g.walls(),
                hue: Kind::Breakout.hue(),
            });
        }
        readouts(b, l, cx as i32, 2 * cy as i32, &stats);
    }
}
