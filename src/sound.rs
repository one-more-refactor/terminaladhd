//! The soundtrack: a hand-rolled chip synth, epic and a little dirty.
//!
//! No audio crate. The machine hand-rolls its pixels, so it hand-rolls its
//! samples: square waves, one triangle, white noise, an envelope apiece —
//! the voices a 1983 sound chip had — mixed, driven through a soft clip and
//! a touch of bit-crush, and piped as raw PCM into whatever player the
//! system already has (`pw-play`, `paplay` or `aplay`). No player, no sound,
//! no complaint.
//!
//! The music speaks the machine's own feel vocabulary. The shell hands it
//! the same two things it hands the screen: the scene and the player's heat.
//! Heat drives the tempo and the hat density; a [`Kick`] lands as a stinger
//! over the loop the same instant it lands as a strobe on the glass. The
//! score is procedural — an endless E-minor loop that is deliberately not
//! quite tidy: the second oscillator drifts against the first, the arp drops
//! notes, and every so often a bar comes back crushed.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::thread::JoinHandle;

use crate::games::Kick;
use crate::rng::Rng;

pub const RATE: u32 = 44_100;
/// Samples per render block: ~46 ms. The pipe to the player is the pacing —
/// a blocking write returns exactly as fast as the player drinks.
const BLOCK: usize = 2_048;

/// What the machine is showing, as far as the music cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    Attract,
    Spin,
    Play,
    Paused,
    Over,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mood {
    pub scene: Scene,
    pub heat: f32,
}

enum Msg {
    Mood(Mood),
    Kick(Kick),
}

// ---------------------------------------------------------------- the score

/// The loop: four bars of E minor, roots as MIDI notes. i–VI–III–VII, the
/// epic progression, with a v-minor bar swapped in on every other pass so
/// the loop never quite settles.
const BARS: [(i32, bool); 4] = [(40, false), (36, true), (43, true), (38, true)];
const ALT_LAST: (i32, bool) = (35, false);

/// E minor pentatonic, two octaves — the lead never has to think.
const PENTA: [i32; 10] = [64, 67, 69, 71, 74, 76, 79, 81, 83, 86];

fn midi_hz(n: f32) -> f32 {
    440.0 * ((n - 69.0) / 12.0).exp2()
}

/// Chord tones over a root: minor or major triad plus the octave.
fn chord(root: i32, major: bool) -> [i32; 4] {
    let third = if major { 4 } else { 3 };
    [root, root + third, root + 7, root + 12]
}

// ---------------------------------------------------------------- the chip

#[derive(Clone, Copy, Default)]
struct Voice {
    phase: f32,
    hz: f32,
    /// Where the pitch is going; portamento chases it.
    aim: f32,
    /// Falls from 1.0 at note-on.
    env: f32,
    decay: f32,
    gain: f32,
    /// 0 square, 1 triangle.
    tri: bool,
}

impl Voice {
    fn on(&mut self, hz: f32, gain: f32, decay: f32) {
        self.hz = if self.hz > 0.0 { self.hz } else { hz };
        self.aim = hz;
        self.env = 1.0;
        self.gain = gain;
        self.decay = decay;
    }

    fn sample(&mut self, detune: f32, slide: f32) -> f32 {
        if self.env <= 0.001 {
            return 0.0;
        }
        self.hz += (self.aim - self.hz) * slide;
        self.phase = (self.phase + self.hz * detune / RATE as f32).fract();
        self.env *= self.decay;
        let wave = if self.tri {
            4.0 * (self.phase - 0.5).abs() - 1.0
        } else if self.phase < 0.5 {
            1.0
        } else {
            -1.0
        };
        wave * self.env * self.gain
    }
}

/// The whole band. Pure logic: feed it moods and kicks, ask it for samples —
/// the tests do exactly that and never open a pipe.
pub struct Synth {
    rng: Rng,
    mood: Mood,
    /// Song position in samples within the current sixteenth.
    tick: f32,
    step: u32,
    /// Which pass over the four bars this is; odd passes take the alt bar.
    pass: u32,

    bass: Voice,
    arp_a: Voice,
    arp_b: Voice,
    lead: Voice,
    pad: [Voice; 3],
    sting: [Voice; 3],
    /// Pending stinger notes: (samples until on, midi, gain, decay, voice).
    queue: Vec<(i32, f32, f32, f32, usize)>,

    /// Drum one-shots render straight from counters.
    kick_left: i32,
    snare_left: i32,
    hat_left: i32,
    hat_gain: f32,
    /// Pawl clicks for the wheel, as sample countdowns — scheduled on the
    /// same fourth-power decel the strip follows, so the ear and the eye
    /// agree about where the reel is.
    ratchet: Vec<i32>,
    /// Noise sweep for the spin riser and the death crash.
    sweep_left: i32,
    sweep_len: i32,
    sweep_up: bool,

    /// The mix ducks when a stinger lands and breathes back.
    duck: f32,
    /// One-pole low-pass state; engaged when paused or over.
    lp: f32,
    /// The detune LFO phase — the deliberate mess.
    drift: f32,
    noise: u32,
}

impl Synth {
    pub fn new(seed: u64) -> Synth {
        Synth {
            rng: Rng::from_seed(seed),
            mood: Mood {
                scene: Scene::Attract,
                heat: 0.0,
            },
            tick: 0.0,
            step: 0,
            pass: 0,
            bass: Voice::default(),
            arp_a: Voice::default(),
            arp_b: Voice::default(),
            lead: Voice {
                tri: true,
                ..Voice::default()
            },
            pad: [Voice {
                tri: true,
                ..Voice::default()
            }; 3],
            sting: [Voice::default(); 3],
            queue: Vec::new(),
            kick_left: 0,
            snare_left: 0,
            hat_left: 0,
            hat_gain: 0.0,
            ratchet: Vec::new(),
            sweep_left: 0,
            sweep_len: 1,
            sweep_up: false,
            duck: 1.0,
            lp: 0.0,
            drift: 0.0,
            noise: 0x2F6E_2B1D,
        }
    }

    pub fn mood(&mut self, mood: Mood) {
        if self.mood.scene != mood.scene && mood.scene == Scene::Spin {
            // The riser: a climbing noise sweep, and the pawl clicking past
            // each name. The strip's position is 1-(1-t)^4, so a notch lands
            // where that curve crosses each whole slot — the inverse curve,
            // sampled per notch.
            self.sweep_len = (RATE as f32 * 0.8) as i32;
            self.sweep_left = self.sweep_len;
            self.sweep_up = true;
            self.ratchet.clear();
            let spin = 0.95 * RATE as f32;
            let slots = 8.0;
            for i in 1..=8 {
                let t = 1.0 - (1.0 - i as f32 / slots).powf(0.25);
                self.ratchet.push((t * spin) as i32);
            }
        }
        self.mood = mood;
    }

    pub fn kick(&mut self, k: Kick) {
        let q = |d: f32, n: i32, g: f32, dur: f32, v: usize| {
            ((d * RATE as f32) as i32, n as f32, g, env_decay(dur), v)
        };
        match k {
            // Two quick fifths — acknowledged, not interrupted.
            Kick::Small => {
                self.queue.push(q(0.0, 76, 0.30, 0.09, 0));
                self.queue.push(q(0.09, 83, 0.30, 0.12, 1));
            }
            // Three octave stabs.
            Kick::Big => {
                for (i, n) in [64, 76, 88].into_iter().enumerate() {
                    self.queue.push(q(i as f32 * 0.07, n, 0.34, 0.10, i % 3));
                }
                self.duck = self.duck.min(0.6);
            }
            // The fanfare: root, fifth and octave held in unison, then the
            // octave again a step up the loop's own scale. Loud on purpose.
            Kick::Huge => {
                for (i, n) in [52, 59, 64].into_iter().enumerate() {
                    self.queue.push(q(0.0, n, 0.30, 0.50, i));
                }
                self.queue.push(q(0.28, 76, 0.36, 0.45, 0));
                self.queue.push(q(0.42, 79, 0.36, 0.60, 1));
                self.duck = self.duck.min(0.35);
            }
            // A major arp sparkling upward — paid, warm.
            Kick::Bonus => {
                for (i, n) in [67, 71, 74, 79].into_iter().enumerate() {
                    self.queue.push(q(i as f32 * 0.05, n, 0.28, 0.14, i % 3));
                }
            }
            // The bottom falls out: a long slide down and a noise crash.
            Kick::Death => {
                self.queue.push(q(0.0, 52, 0.40, 0.9, 0));
                self.sting[0].aim = midi_hz(28.0);
                self.sweep_len = (RATE as f32 * 0.7) as i32;
                self.sweep_left = self.sweep_len;
                self.sweep_up = false;
                self.duck = self.duck.min(0.3);
            }
        }
    }

    fn bpm(&self) -> f32 {
        match self.mood.scene {
            Scene::Over => 54.0,
            _ => 108.0 * (1.0 + 0.30 * self.mood.heat.clamp(0.0, 1.0)),
        }
    }

    fn white(&mut self) -> f32 {
        // xorshift — the same noise a chip made, one bit at a time.
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        (self.noise as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Advance one sixteenth: trigger whatever this step plays.
    fn on_step(&mut self) {
        let step = self.step % 64;
        if step == 0 {
            self.pass = self.pass.wrapping_add(1);
        }
        let bar = (step / 16) as usize;
        let in_bar = step % 16;
        let (root, major) = if bar == 3 && self.pass % 2 == 1 {
            ALT_LAST
        } else {
            BARS[bar]
        };
        let tones = chord(root, major);
        let heat = self.mood.heat.clamp(0.0, 1.0);
        let playing = matches!(self.mood.scene, Scene::Attract | Scene::Play);

        if matches!(self.mood.scene, Scene::Spin) {
            // The wheel's rhythm is the ratchet, scheduled when the spin
            // began; the step clock stays out of its way.
            return;
        }
        if !playing && !matches!(self.mood.scene, Scene::Over) {
            return;
        }

        // Bass: driving eighths, sixteenths once the run is hot. Every so
        // often it leans on the seventh instead of the root — the mess.
        let bass_on = in_bar.is_multiple_of(2) || heat > 0.55;
        if bass_on && !matches!(self.mood.scene, Scene::Over) {
            let mut n = root - 12;
            if in_bar == 14 && self.rng.range(4) == 0 {
                n -= 2;
            }
            self.bass.on(midi_hz(n as f32), 0.30, env_decay(0.11));
        }
        // Over: a halftime toll on the root only.
        if matches!(self.mood.scene, Scene::Over) && in_bar.is_multiple_of(8) {
            self.bass
                .on(midi_hz((root - 12) as f32), 0.26, env_decay(0.5));
        }

        // The arp climbs the chord over two octaves, dropping a note now and
        // then so it never turns into a metronome.
        if playing && self.rng.range(10) != 0 {
            let t = tones[(in_bar % 4) as usize] + 12 * ((in_bar as i32 / 4) % 2);
            let hz = midi_hz((t + 12) as f32);
            self.arp_a.on(hz, 0.16, env_decay(0.10));
            self.arp_b.on(hz, 0.13, env_decay(0.10));
        }

        // The lead: sparse pentatonic calls, sliding into every note.
        if playing && in_bar % 4 == 2 && self.rng.range(3) != 0 {
            let n = PENTA[self.rng.range(PENTA.len() as u32) as usize];
            self.lead.on(midi_hz(n as f32), 0.20, env_decay(0.35));
        }

        // The pad holds the chord under everything — the epic part.
        if in_bar == 0 {
            for (i, v) in self.pad.iter_mut().enumerate() {
                v.on(midi_hz(tones[i] as f32), 0.10, env_decay(2.2));
            }
        }

        // Drums.
        if playing {
            if in_bar.is_multiple_of(4) {
                self.kick_left = (RATE / 12) as i32;
            }
            if in_bar == 4 || in_bar == 12 {
                self.snare_left = (RATE / 9) as i32;
            }
            let hats = if heat > 0.4 { 1 } else { 2 };
            if in_bar.is_multiple_of(hats) {
                self.hat_left = (RATE / 40) as i32;
                self.hat_gain = 0.05 + 0.07 * heat + 0.04 * self.rng.range(100) as f32 / 100.0;
            }
        }
    }

    /// Render a block. This is the whole mix: sequencer, voices, drums,
    /// stingers, duck, soft clip, crush.
    pub fn render(&mut self, out: &mut [i16]) {
        let paused = matches!(self.mood.scene, Scene::Paused);
        let spt = RATE as f32 * 60.0 / (self.bpm() * 4.0);
        let heat = self.mood.heat.clamp(0.0, 1.0);
        // The drift LFO detunes the twin oscillator by up to ~9 cents.
        let drift_rate = 0.13 / RATE as f32;

        for s in out.iter_mut() {
            // Sequencer clock.
            if !paused {
                self.tick += 1.0;
                if self.tick >= spt {
                    self.tick -= spt;
                    self.step = self.step.wrapping_add(1);
                    self.on_step();
                }
            }
            // Stinger queue.
            self.queue.retain_mut(|(delay, n, g, d, v)| {
                *delay -= 1;
                if *delay <= 0 {
                    self.sting[*v].on(midi_hz(*n), *g, *d);
                    false
                } else {
                    true
                }
            });

            self.drift = (self.drift + drift_rate).fract();
            let wob = 1.0 + 0.005 * (self.drift * std::f32::consts::TAU).sin();

            let mut mix = 0.0f32;
            if !paused {
                mix += self.bass.sample(1.0, 1.0);
                mix += self.arp_a.sample(1.0, 1.0);
                mix += self.arp_b.sample(wob, 1.0);
            }
            mix += self.lead.sample(1.0, 0.004);
            for v in &mut self.pad {
                mix += v.sample(1.0, 1.0);
            }
            mix *= self.duck;
            for v in &mut self.sting {
                mix += v.sample(1.0, 0.02);
            }

            // Drums, straight out of counters.
            if self.kick_left > 0 {
                self.kick_left -= 1;
                let t = 1.0 - self.kick_left as f32 / (RATE / 12) as f32;
                let hz = 95.0 - 55.0 * t;
                let ph = t * (RATE / 12) as f32 * hz / RATE as f32;
                mix += (ph * std::f32::consts::TAU).sin() * 0.5 * (1.0 - t);
            }
            if self.snare_left > 0 {
                self.snare_left -= 1;
                let t = self.snare_left as f32 / (RATE / 9) as f32;
                mix += self.white() * 0.22 * t + (self.drift * 1200.0).sin() * 0.05 * t;
            }
            if self.hat_left > 0 {
                self.hat_left -= 1;
                mix += self.white() * self.hat_gain * (self.hat_left as f32 / (RATE / 40) as f32);
            }
            // The pawl: each scheduled click lands as a bright tick and a
            // short thock — the tooth catching the next notch.
            let mut fired = false;
            self.ratchet.retain_mut(|left| {
                *left -= 1;
                if *left <= 0 {
                    fired = true;
                }
                *left > 0
            });
            if fired {
                self.hat_left = (RATE / 60) as i32;
                self.hat_gain = 0.30;
                self.kick_left = (RATE / 30) as i32;
            }
            if self.sweep_left > 0 {
                self.sweep_left -= 1;
                let t = 1.0 - self.sweep_left as f32 / self.sweep_len as f32;
                let a = if self.sweep_up { t } else { 1.0 - t };
                mix += self.white() * 0.24 * a * a;
            }

            self.duck += (1.0 - self.duck) * 0.00006;

            // The tube: darker when paused or over, open in play.
            let cut = match self.mood.scene {
                Scene::Paused | Scene::Over => 0.045,
                _ => 0.55,
            };
            self.lp += (mix - self.lp) * cut;
            let mut v = self.lp;

            // Soft clip, then a bit-crush that bites harder with heat —
            // the dirt is a dial, and the dial is the player.
            v = v.clamp(-1.5, 1.5);
            v = v - v * v * v / 6.75;
            let bits = 512.0 - 384.0 * heat * 0.5;
            v = (v * bits).round() / bits;

            *s = (v * 24_000.0) as i16;
        }
    }
}

fn env_decay(secs: f32) -> f32 {
    // Reach -60 dB after `secs`.
    (-6.9 / (secs * RATE as f32)).exp()
}

// ------------------------------------------------------------------ the IO

/// The player process the samples go to, found rather than linked.
fn find_player() -> Option<Command> {
    let candidates: [(&str, &[&str]); 3] = [
        (
            "pw-play",
            &["--format", "s16", "--rate", "44100", "--channels", "1", "-"],
        ),
        (
            "paplay",
            &["--raw", "--format=s16le", "--rate=44100", "--channels=1"],
        ),
        (
            "aplay",
            &[
                "-q", "-f", "S16_LE", "-r", "44100", "-c", "1", "-t", "raw", "-",
            ],
        ),
    ];
    for (bin, args) in candidates {
        let found = std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|p| p.join(bin).is_file()));
        if found {
            let mut cmd = Command::new(bin);
            cmd.args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            return Some(cmd);
        }
    }
    None
}

/// The shell's handle. All methods are non-blocking and infallible: a full
/// channel drops the message, a dead player ends the music, and neither is
/// ever allowed to cost a frame.
pub struct Jukebox {
    tx: Option<SyncSender<Msg>>,
    worker: Option<JoinHandle<()>>,
    child: Option<Child>,
    last: Option<Mood>,
}

impl Jukebox {
    /// Silent — the handle every method still works on.
    pub fn muted() -> Jukebox {
        Jukebox {
            tx: None,
            worker: None,
            child: None,
            last: None,
        }
    }

    pub fn start(enabled: bool) -> Jukebox {
        if !enabled {
            return Self::muted();
        }
        let Some(mut cmd) = find_player() else {
            return Self::muted();
        };
        let Ok(mut child) = cmd.spawn() else {
            return Self::muted();
        };
        let Some(mut pipe) = child.stdin.take() else {
            return Self::muted();
        };
        let (tx, rx): (SyncSender<Msg>, Receiver<Msg>) = sync_channel(64);
        let worker = std::thread::spawn(move || {
            let mut synth = Synth::new(0xC0FFEE);
            let mut block = [0i16; BLOCK];
            let mut bytes = [0u8; BLOCK * 2];
            loop {
                loop {
                    match rx.try_recv() {
                        Ok(Msg::Mood(m)) => synth.mood(m),
                        Ok(Msg::Kick(k)) => synth.kick(k),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                synth.render(&mut block);
                for (i, s) in block.iter().enumerate() {
                    let [a, b] = s.to_le_bytes();
                    bytes[2 * i] = a;
                    bytes[2 * i + 1] = b;
                }
                // The blocking write is the metronome: the player consumes
                // at 44.1 kHz and this thread idles against the pipe.
                if pipe.write_all(&bytes).is_err() {
                    return;
                }
            }
        });
        Jukebox {
            tx: Some(tx),
            worker: Some(worker),
            child: Some(child),
            last: None,
        }
    }

    /// Tell the band what is on screen. Deduplicated here so the caller can
    /// say it every frame.
    pub fn mood(&mut self, scene: Scene, heat: f32) {
        let mood = Mood {
            scene,
            // Quantized: the tempo should breathe with the run, not tremble
            // with every frame's decay.
            heat: (heat * 8.0).round() / 8.0,
        };
        if self.last == Some(mood) {
            return;
        }
        self.last = Some(mood);
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(Msg::Mood(mood));
        }
    }

    pub fn kick(&mut self, k: Kick) {
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(Msg::Kick(k));
        }
    }
}

impl Drop for Jukebox {
    fn drop(&mut self) {
        // Sever the channel so the worker returns, kill the player, then
        // join — the terminal must get its prompt back with no orphan
        // holding the audio device.
        self.tx = None;
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(buf: &[i16]) -> f32 {
        (buf.iter().map(|&s| (s as f32).powi(2)).sum::<f32>() / buf.len() as f32).sqrt()
    }

    fn run(synth: &mut Synth, secs: f32) -> Vec<i16> {
        let mut all = Vec::new();
        let mut block = [0i16; BLOCK];
        for _ in 0..((secs * RATE as f32) as usize / BLOCK) {
            synth.render(&mut block);
            all.extend_from_slice(&block);
        }
        all
    }

    #[test]
    fn the_same_seed_plays_the_same_song() {
        let mut a = Synth::new(7);
        let mut b = Synth::new(7);
        assert_eq!(run(&mut a, 2.0), run(&mut b, 2.0));
    }

    #[test]
    fn the_loop_is_audible_and_bounded() {
        let mut s = Synth::new(1);
        s.mood(Mood {
            scene: Scene::Play,
            heat: 0.5,
        });
        let buf = run(&mut s, 4.0);
        let level = rms(&buf);
        assert!(level > 300.0, "the band is not playing: rms {level}");
        assert!(
            buf.iter().all(|&v| v > -32_500 && v < 32_500),
            "the mix clips the container"
        );
    }

    #[test]
    fn a_huge_lands_louder_than_the_loop() {
        let mut s = Synth::new(2);
        s.mood(Mood {
            scene: Scene::Play,
            heat: 0.3,
        });
        let _ = run(&mut s, 2.0);
        let before = rms(&run(&mut s, 0.4));
        s.kick(Kick::Huge);
        let after = rms(&run(&mut s, 0.4));
        assert!(
            after > before * 1.1,
            "the fanfare does not rise over the loop: {before} -> {after}"
        );
    }

    #[test]
    fn heat_raises_the_tempo() {
        let cold = Synth::new(3);
        let mut hot = Synth::new(3);
        hot.mood(Mood {
            scene: Scene::Play,
            heat: 1.0,
        });
        assert!(hot.bpm() > cold.bpm());
    }

    #[test]
    fn pausing_quiets_the_band_without_stopping_it() {
        let mut s = Synth::new(4);
        s.mood(Mood {
            scene: Scene::Play,
            heat: 0.5,
        });
        let _ = run(&mut s, 2.0);
        let loud = rms(&run(&mut s, 1.0));
        s.mood(Mood {
            scene: Scene::Paused,
            heat: 0.0,
        });
        let _ = run(&mut s, 0.5);
        let quiet = rms(&run(&mut s, 1.0));
        assert!(
            quiet < loud * 0.6,
            "the pause does not hush the band: {loud} -> {quiet}"
        );
    }

    #[test]
    fn rendering_outruns_realtime() {
        // The worker must synthesize far faster than the player drinks, or
        // the pipe starves and the music stutters.
        let mut s = Synth::new(5);
        s.mood(Mood {
            scene: Scene::Play,
            heat: 1.0,
        });
        let start = std::time::Instant::now();
        let _ = run(&mut s, 4.0);
        let took = start.elapsed().as_secs_f32();
        assert!(took < 2.0, "4 s of audio took {took} s to render");
    }

    #[test]
    fn a_muted_jukebox_swallows_everything() {
        let mut j = Jukebox::muted();
        j.mood(Scene::Play, 0.7);
        j.kick(Kick::Huge);
    }
}
