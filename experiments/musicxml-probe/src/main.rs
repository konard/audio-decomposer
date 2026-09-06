//! Prints the notes a score is written from and the notes it is read back as,
//! so a round-trip mismatch can be seen note by note.

use audio_decomposer::audio::SampleFormat;
use audio_decomposer::decompose::{decompose, DecomposeOptions};
use audio_decomposer::formats::musicxml;
use audio_decomposer::formats::session::{Session, SessionOptions};
use audio_decomposer::{archive, Audio};

fn main() {
    let rate = 8_000u32;
    let bar = rate as usize;
    let chords = [[57, 60, 64], [55, 59, 62], [53, 57, 60], [50, 53, 57]];
    let mut samples = vec![0.0; bar * chords.len()];
    for (index, chord) in chords.iter().enumerate() {
        for note in chord {
            let frequency = 440.0 * ((f64::from(*note) - 69.0) / 12.0).exp2();
            for offset in 0..bar {
                let time = offset as f64 / f64::from(rate);
                samples[index * bar + offset] +=
                    0.22 * (-2.5 * time).exp() * (std::f64::consts::TAU * frequency * time).sin();
            }
        }
    }
    let mut audio = Audio::from_mono(rate, SampleFormat::PcmI16, samples).unwrap();
    audio.quantize();

    let decomposition = decompose(
        &audio,
        &DecomposeOptions {
            name: "chords".to_string(),
            candidates: 8,
            ..DecomposeOptions::default()
        },
    )
    .unwrap();
    let manifest = archive::manifest_of(&decomposition);
    let session = Session::from_manifest(&manifest, &SessionOptions::default());
    let written: Vec<_> = session
        .tracks
        .iter()
        .flat_map(|track| track.notes.iter())
        .collect();
    println!("tempo {:.3}", session.tempo);
    println!("written {}", written.len());
    for note in &written {
        println!("  {:>3} {:>8}..{:<8}", note.note, note.start, note.end());
    }
    let text = musicxml::write(&session);
    let read = musicxml::parse(&text, session.sample_rate, Some(session.tempo)).unwrap();
    println!("read {}", read.len());
    for note in &read {
        println!("  {:>3} {:>8}..{:<8}", note.note, note.start, note.end());
    }
}
