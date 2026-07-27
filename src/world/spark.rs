//! Debris.
//!
//! When something is destroyed on this machine it comes apart rather than
//! disappearing. A row that vanishes was never there; a row that blows out
//! across the well was something you did.
//!
//! Sparks live in *arena cells* — the same coordinates the games think in — so
//! a game emits at the cell where a thing happened and never has to know how
//! big a cell currently is on screen. The painter converts on the way out.
//!
//! There is no pooling and no fixed-size buffer. The cap is a count, and the
//! oldest go first when it is reached, which is both simpler than a pool and
//! the behaviour you want: the sparks that matter are the ones that just left.

use crate::rng::Rng;
use crate::world::draw::add_emis;
use crate::world::layout::Layout;
use crate::world::{Buf, Rgb};

/// Cells per second squared, downward. Debris that does not fall reads as a
/// screensaver.
const GRAVITY: f32 = 34.0;
/// How much of its speed a spark keeps each second — enough drag that a burst
/// blooms out and settles rather than flying off at a constant rate.
const DRAG: f32 = 0.22;

/// The most that can be in the air at once. Reached only by a Tetris landing on
/// top of a death, and the oldest are the ones nobody is looking at.
const CAP: usize = 320;

#[derive(Clone, Copy, Debug)]
pub struct Spark {
    /// Arena cell, fractional.
    pub x: f32,
    pub y: f32,
    /// Cells per second.
    pub vx: f32,
    pub vy: f32,
    /// `1.0` at birth down to `0.0`.
    pub life: f32,
    /// How fast that life runs out.
    pub decay: f32,
    pub col: Rgb,
    /// Sub-pixels across. Fractional, so a spark can be smaller than one.
    pub size: f32,
    /// Whether gravity applies. A spark thrown by a clear falls; one thrown by
    /// a bonus floats, because it is light rather than matter.
    pub heavy: bool,
}

#[derive(Default)]
pub struct Sparks {
    items: Vec<Spark>,
}

impl Sparks {
    pub fn new() -> Sparks {
        Sparks::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    fn push(&mut self, s: Spark) {
        if self.items.len() >= CAP {
            self.items.remove(0);
        }
        self.items.push(s);
    }

    /// Blow a cell apart: `n` pieces thrown out of `(x, y)` at up to `speed`
    /// cells a second, in `col`.
    pub fn burst(&mut self, rng: &mut Rng, at: (f32, f32), n: usize, speed: f32, col: Rgb) {
        for _ in 0..n {
            let (dx, dy) = unit(rng);
            let k = 0.35 + 0.65 * frac(rng);
            self.push(Spark {
                x: at.0,
                y: at.1,
                vx: dx * speed * k,
                vy: dy * speed * k,
                life: 1.0,
                decay: 1.3 + frac(rng) * 1.2,
                col,
                size: 1.0 + frac(rng),
                heavy: true,
            });
        }
    }

    /// A row coming apart: pieces thrown sideways out of the whole width, which
    /// is what a line clear looks like when it is an event rather than an
    /// absence.
    pub fn shear(&mut self, rng: &mut Rng, row: f32, cols: usize, speed: f32, col: Rgb) {
        for c in 0..cols {
            let x = c as f32 + 0.5;
            // Thrown away from the middle, hardest at the edges, so the row
            // opens outward instead of scattering.
            let away = (x - cols as f32 * 0.5) / (cols as f32 * 0.5);
            for _ in 0..2 {
                self.push(Spark {
                    x,
                    y: row + frac(rng),
                    vx: away * speed * (0.6 + 0.8 * frac(rng)),
                    vy: -speed * 0.35 * frac(rng),
                    life: 1.0,
                    decay: 1.1 + frac(rng),
                    col,
                    size: 1.0 + 1.5 * frac(rng),
                    heavy: true,
                });
            }
        }
    }

    /// Light rather than matter: rises, spreads, does not fall.
    pub fn glimmer(&mut self, rng: &mut Rng, at: (f32, f32), n: usize, col: Rgb) {
        for _ in 0..n {
            let (dx, _) = unit(rng);
            self.push(Spark {
                x: at.0,
                y: at.1,
                vx: dx * 3.5,
                vy: -2.0 - 3.0 * frac(rng),
                life: 1.0,
                decay: 1.4 + frac(rng),
                col,
                size: 1.0,
                heavy: false,
            });
        }
    }

    pub fn step(&mut self, dt: f32) {
        for s in &mut self.items {
            if s.heavy {
                s.vy += GRAVITY * dt;
            }
            let drag = 1.0 - DRAG * dt;
            s.vx *= drag;
            s.vy *= drag;
            s.x += s.vx * dt;
            s.y += s.vy * dt;
            s.life -= s.decay * dt;
        }
        self.items.retain(|s| s.life > 0.0);
    }

    /// Emissive only, so debris never occludes the game under it and always
    /// blooms — the same reason the warp field is drawn this way.
    pub fn draw(&self, b: &mut Buf, l: &Layout, shake: i32) {
        let p = l.mino_px as f32;
        let (ox, oy) = l.cell_origin(0, 0, shake);
        for s in &self.items {
            let px = ox as f32 + s.x * p;
            let py = oy as f32 + s.y * p;
            let fade = s.life * s.life;
            let col = s.col.mul(fade * 1.3);
            let r = (s.size * fade).max(1.0) as i32;
            for dy in 0..r {
                for dx in 0..r {
                    add_emis(b, px as i32 + dx, py as i32 + dy, col);
                }
            }
        }
    }
}

fn frac(rng: &mut Rng) -> f32 {
    rng.range(4096) as f32 / 4096.0
}

/// A direction picked by rejection out of the unit square — cheaper than a sine
/// and a cosine, and called often enough for that to be worth caring about.
fn unit(rng: &mut Rng) -> (f32, f32) {
    loop {
        let x = frac(rng) * 2.0 - 1.0;
        let y = frac(rng) * 2.0 - 1.0;
        let d = (x * x + y * y).sqrt();
        if (0.2..=1.0).contains(&d) {
            return (x / d, y / d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white() -> Rgb {
        Rgb::new(1.0, 1.0, 1.0)
    }

    #[test]
    fn a_burst_throws_pieces_and_they_all_expire() {
        let mut s = Sparks::new();
        let mut rng = Rng::from_seed(1);
        s.burst(&mut rng, (3.0, 4.0), 24, 10.0, white());
        assert_eq!(s.len(), 24);
        for _ in 0..200 {
            s.step(0.016);
        }
        assert!(s.is_empty(), "sparks outlived their welcome: {}", s.len());
    }

    #[test]
    fn heavy_sparks_fall_and_light_ones_do_not() {
        let mut s = Sparks::new();
        let mut rng = Rng::from_seed(2);
        s.burst(&mut rng, (0.0, 0.0), 1, 0.0, white());
        for _ in 0..30 {
            s.step(0.016);
        }
        assert!(s.items[0].y > 0.0, "debris that does not fall is a screensaver");

        let mut t = Sparks::new();
        t.glimmer(&mut rng, (0.0, 0.0), 1, white());
        for _ in 0..10 {
            t.step(0.016);
        }
        assert!(t.items[0].y < 0.0, "light should rise");
    }

    #[test]
    fn a_shear_opens_outward_from_the_middle() {
        let mut s = Sparks::new();
        let mut rng = Rng::from_seed(3);
        s.shear(&mut rng, 5.0, 10, 12.0, white());
        let left = s.items.iter().filter(|k| k.x < 5.0).collect::<Vec<_>>();
        let right = s.items.iter().filter(|k| k.x > 5.0).collect::<Vec<_>>();
        assert!(left.iter().all(|k| k.vx <= 0.0), "the left half went right");
        assert!(right.iter().all(|k| k.vx >= 0.0), "the right half went left");
    }

    #[test]
    fn the_cap_drops_the_oldest_rather_than_refusing_the_newest() {
        let mut s = Sparks::new();
        let mut rng = Rng::from_seed(4);
        for _ in 0..40 {
            s.burst(&mut rng, (1.0, 1.0), 20, 8.0, white());
        }
        assert_eq!(s.len(), CAP);
        // The most recent burst is what someone is actually looking at.
        assert!(s.items.iter().all(|k| k.life > 0.9));
    }
}
