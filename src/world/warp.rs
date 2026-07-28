//! The warp field: the black behind the game, moving.
//!
//! Streaks fly outward from a vanishing point behind the arena and accelerate
//! as they near the edge of the screen, the way everything did on a cabinet
//! that wanted to look fast. It is the only thing on screen that is not the
//! game, and it exists for one reason: to be a readout of how well the player
//! is doing. At rest it is a slow drift. Under heat it stretches. On a clear,
//! an apple or a death it punches into hyperspace for a fifth of a second and
//! falls back.
//!
//! It is drawn additively, into the emissive plane only, so it never occludes
//! anything and always blooms — which is what a vector monitor looked like.

use crate::rng::Rng;
use crate::world::draw::add_emis;
use crate::world::scene::palette::*;
use crate::world::{hex, Buf};

/// Sub-pixels of screen per streak. Sparse enough that the field reads as
/// motion rather than as static, dense enough that it never looks empty.
const DENSITY: usize = 300;
const MAX_STREAKS: usize = 320;

/// Radius a streak is born at, as a fraction of the field's reach. Starting at
/// zero would pile every streak into one bright dot at the vanishing point.
const BIRTH: f32 = 0.06;

/// Sub-pixels per second at the birth radius, and the multiple of that a streak
/// reaches at the edge. The acceleration is what sells the perspective.
const SPEED: f32 = 34.0;
const EDGE_GAIN: f32 = 6.0;

/// What heat and a punch add to the speed, as multiples of the resting rate.
const HEAT_GAIN: f32 = 1.9;
const PUNCH_GAIN: f32 = 5.0;

/// How long a punch takes to fall back. Short: it is a hit, not a mood.
const PUNCH_DECAY: f32 = 0.22;

struct Streak {
    /// Unit direction from the vanishing point. Stored rather than an angle so
    /// the hot loop never calls a trig function.
    dx: f32,
    dy: f32,
    r: f32,
    /// `0..1` — how bright and how long this one runs. Varying it is what stops
    /// the field reading as a machine-generated pattern.
    weight: f32,
}

pub struct Warp {
    streaks: Vec<Streak>,
    rng: Rng,
    /// Half-diagonal of the frame: the radius at which a streak has left.
    reach: f32,
    cx: f32,
    cy: f32,
    punch: f32,
}

impl Warp {
    pub fn new(w: usize, sh: usize, seed: u64) -> Warp {
        let mut rng = Rng::from_seed(seed);
        let cx = w as f32 * 0.5;
        let cy = sh as f32 * 0.5;
        let reach = (cx * cx + cy * cy).sqrt();
        let n = (w * sh / DENSITY).clamp(24, MAX_STREAKS);
        let streaks = (0..n)
            .map(|_| {
                let mut s = spawn(&mut rng);
                // Scatter the first generation across the whole field, or every
                // streak arrives at the edge together on the opening frame.
                s.r = BIRTH + (1.0 - BIRTH) * frac(&mut rng);
                s
            })
            .collect();
        Warp {
            streaks,
            rng,
            reach,
            cx,
            cy,
            punch: 0.0,
        }
    }

    /// Hit the field. `amount` in `0.0..=1.0`; the largest recent hit wins
    /// rather than accumulating, so a combo does not saturate it.
    pub fn punch(&mut self, amount: f32) {
        self.punch = self.punch.max(amount.clamp(0.0, 1.0));
    }

    pub fn step(&mut self, dt: f32, heat: f32) {
        self.punch = (self.punch - dt / PUNCH_DECAY).max(0.0);
        let rate = SPEED * (1.0 + HEAT_GAIN * heat.clamp(0.0, 1.0) + PUNCH_GAIN * self.punch);
        for s in &mut self.streaks {
            // Speed rises with radius: the same world-space velocity subtends
            // more of the screen the closer it gets.
            s.r += dt * rate * (0.15 + EDGE_GAIN * s.r) / self.reach;
            if s.r >= 1.0 {
                *s = spawn(&mut self.rng);
            }
        }
    }

    /// How stretched the field currently is, `0..`. The painter uses it for the
    /// streak length; the shell uses it for nothing, which is the point — this
    /// is scenery that only the game drives.
    fn stretch(&self, heat: f32) -> f32 {
        1.0 + 2.2 * heat.clamp(0.0, 1.0) + 7.0 * self.punch
    }

    pub fn draw(&self, b: &mut Buf, heat: f32) {
        // A streak is a run of fat square dashes, not an airbrushed line: the
        // head at full strength, the tail at one flat step down, and nothing
        // in between. Two levels is all an old machine had, and it reads as
        // speed just the same — the taper was never the point, the length was.
        const T: i32 = 2;
        let near = hex(MAGENTA);
        let far = hex(CYAN);
        let stretch = self.stretch(heat);
        for s in &self.streaks {
            let r0 = s.r * self.reach;
            // Streaks are short far away and long near the edge, which is the
            // same perspective cue as the speed ramp, seen instead of felt.
            let len = (2.0 + 16.0 * s.r * s.r) * stretch * (0.5 + s.weight);
            let col = far.lerp(near, s.r);
            // Fade in out of the vanishing point and out again at the frame
            // edge, so nothing ever pops into or out of existence.
            let alpha = smooth_edges(s.r) * (0.14 + 0.44 * s.weight);
            let dashes = ((len / T as f32).ceil() as i32).clamp(1, 24);
            let mut last = (i32::MIN, i32::MIN);
            for i in 0..dashes {
                let r = r0 - (i * T) as f32;
                if r < 0.0 {
                    break;
                }
                // Snapped to the dash grid, and never the same block twice —
                // near the vanishing point the dashes land on top of each
                // other and would add up into a hot spot.
                let bx = ((self.cx + s.dx * r) as i32).div_euclid(T) * T;
                let by = ((self.cy + s.dy * r) as i32).div_euclid(T) * T;
                if (bx, by) == last {
                    continue;
                }
                last = (bx, by);
                let a = if i * 3 < dashes { alpha } else { alpha * 0.4 };
                for dy in 0..T {
                    for dx in 0..T {
                        add_emis(b, bx + dx, by + dy, col.mul(a));
                    }
                }
            }
        }
    }
}

/// Bright in the middle of the flight, dark at both ends.
fn smooth_edges(r: f32) -> f32 {
    let in_ = (r / 0.22).clamp(0.0, 1.0);
    let out = ((1.0 - r) / 0.18).clamp(0.0, 1.0);
    in_ * out
}

fn frac(rng: &mut Rng) -> f32 {
    rng.range(4096) as f32 / 4096.0
}

fn spawn(rng: &mut Rng) -> Streak {
    // A direction picked by rejection out of the unit square. Cheaper than a
    // sine and a cosine, and it is called once per streak per lap.
    let (dx, dy) = loop {
        let x = frac(rng) * 2.0 - 1.0;
        let y = frac(rng) * 2.0 - 1.0;
        let d = (x * x + y * y).sqrt();
        if (0.15..=1.0).contains(&d) {
            break (x / d, y / d);
        }
    };
    Streak {
        dx,
        dy,
        r: BIRTH,
        weight: frac(rng),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaks_lap_the_field_instead_of_leaving_it() {
        let mut w = Warp::new(120, 60, 3);
        for _ in 0..600 {
            w.step(0.016, 1.0);
        }
        assert!(w.streaks.iter().all(|s| (0.0..1.0).contains(&s.r)));
        // Directions stay unit length, or the field would drift off-centre.
        for s in &w.streaks {
            let d = (s.dx * s.dx + s.dy * s.dy).sqrt();
            assert!((d - 1.0).abs() < 1e-3, "direction not normalised: {d}");
        }
    }

    #[test]
    fn a_punch_decays_rather_than_latching() {
        let mut w = Warp::new(80, 48, 4);
        w.punch(1.0);
        assert!(w.stretch(0.0) > 5.0);
        for _ in 0..40 {
            w.step(0.016, 0.0);
        }
        assert!(w.stretch(0.0) < 1.2, "the field settles back to a drift");
    }

    #[test]
    fn heat_stretches_the_field_without_a_punch() {
        let w = Warp::new(80, 48, 5);
        assert!(w.stretch(1.0) > w.stretch(0.0));
    }

    #[test]
    fn the_field_scales_with_the_frame_and_stays_bounded() {
        for (w, h) in [(80usize, 48usize), (400, 200)] {
            let f = Warp::new(w, h, 1);
            assert!(f.streaks.len() <= MAX_STREAKS);
            assert!(!f.streaks.is_empty());
        }
    }
}
