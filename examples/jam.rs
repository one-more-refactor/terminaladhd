//! Render the soundtrack to a WAV, no terminal and no player needed:
//! a tour through every scene and every stinger, for judging the band
//! without booting the machine.
//!
//!     cargo run --release --example jam -- /tmp/jam.wav

use terminaladhd::games::Kick;
use terminaladhd::sound::{Mood, Scene, Synth, RATE};

fn main() {
    let path = std::env::args().nth(1).unwrap_or("jam.wav".into());
    let mut synth = Synth::new(0xC0FFEE);
    let mut samples: Vec<i16> = Vec::new();
    let mut block = [0i16; 2048];

    let mut run = |synth: &mut Synth, secs: f32, out: &mut Vec<i16>| {
        for _ in 0..((secs * RATE as f32) as usize / 2048) {
            synth.render(&mut block);
            out.extend_from_slice(&block);
        }
    };

    // The attract loop, at rest.
    synth.mood(Mood {
        scene: Scene::Attract,
        heat: 0.0,
    });
    run(&mut synth, 6.0, &mut samples);

    // The wheel: the riser.
    synth.mood(Mood {
        scene: Scene::Spin,
        heat: 0.0,
    });
    run(&mut synth, 1.6, &mut samples);

    // Play, heating up, with the stingers landing as they would.
    for (heat, kick, secs) in [
        (0.15, None, 3.0),
        (0.35, Some(Kick::Small), 2.5),
        (0.55, Some(Kick::Bonus), 2.5),
        (0.75, Some(Kick::Big), 2.5),
        (1.0, Some(Kick::Huge), 4.0),
    ] {
        synth.mood(Mood {
            scene: Scene::Play,
            heat,
        });
        if let Some(k) = kick {
            synth.kick(k);
        }
        run(&mut synth, secs, &mut samples);
    }

    // The end of the run, and the board after it.
    synth.kick(Kick::Death);
    run(&mut synth, 1.2, &mut samples);
    synth.mood(Mood {
        scene: Scene::Over,
        heat: 0.0,
    });
    run(&mut synth, 4.0, &mut samples);

    write_wav(&path, &samples);
    println!(
        "{path}: {:.1}s of soundtrack",
        samples.len() as f32 / RATE as f32
    );
}

/// The 44-byte classic, by hand — the file format equivalent of a square
/// wave.
fn write_wav(path: &str, samples: &[i16]) {
    let data = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes()); // PCM
    out.extend(1u16.to_le_bytes()); // mono
    out.extend(RATE.to_le_bytes());
    out.extend((RATE * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data.to_le_bytes());
    for s in samples {
        out.extend(s.to_le_bytes());
    }
    std::fs::write(path, out).expect("could not write the wav");
}
