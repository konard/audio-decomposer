//! Takes a recording apart and puts it back together, then proves the two are
//! the same file.
//!
//! This is the claim the whole crate rests on: a decomposition is not a lossy
//! summary of a recording, it is another way of writing it down.
//!
//! ```text
//! cargo run --example lossless_round_trip -- song.wav
//! ```

use audio_decomposer::decompose::{decompose, DecomposeOptions};
use audio_decomposer::{audio, recompose, Result};

fn main() -> Result<()> {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: lossless_round_trip <input.wav>");
        return Ok(());
    };

    let original = audio::read_file(&path)?;
    println!(
        "{path}: {} Hz, {} channel(s), {} frames",
        original.sample_rate(),
        original.channel_count(),
        original.frames()
    );

    let decomposition = decompose(&original, &DecomposeOptions::default())?;
    println!(
        "  {} stem(s), {} sample(s), {} placement(s), {} note(s)",
        decomposition.stems.len(),
        decomposition.samples.len(),
        decomposition.placements.len(),
        decomposition.notes.len()
    );

    let report = recompose::verify(&decomposition, &original)?;
    println!(
        "  reuse {:.2}x, reconstruction {}",
        report.reuse(),
        if report.is_exact() {
            "exact"
        } else {
            "inexact"
        }
    );

    // The rebuilt audio is the original, not something close to it.
    let rebuilt = recompose::reconstruct(&decomposition);
    assert_eq!(original.channels(), rebuilt.channels());
    println!("  every sample matches");

    Ok(())
}
