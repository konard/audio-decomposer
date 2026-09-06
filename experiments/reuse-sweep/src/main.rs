//! Which knob, if any, makes deduplication find repeats in real music.
//!
//! `experiments/match-quality` measured pairs of events directly and found that
//! a third to two thirds of them have a near-perfect best match somewhere in
//! the recording, which does not square with the pipeline reporting `reuse
//! 1.00x` on the same files. So the loss is not in the matcher's arithmetic but
//! somewhere between it and the pursuit: the candidate list, the search radius,
//! the stem split, or the tolerance.
//!
//! This probe runs the real pipeline, changing one thing at a time, and prints
//! what each change does to the size of the bank.
//!
//! The answer turned out to be "none of them", and `match-quality` then found
//! why: its own first measurement was the artefact. Kept because a sweep that
//! moves nothing is the evidence that the defaults are not the problem.
//!
//! ```text
//! cargo run --release -- ../../corpus/*.flac
//! ```

use std::path::Path;

use audio_decomposer::audio;
use audio_decomposer::decompose::{decompose, DecomposeOptions, StemMode};
use audio_decomposer::{recompose, Result};

/// How much of each recording is analysed.
const SECONDS: f64 = 12.0;

fn main() -> Result<()> {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    for path in &paths {
        let whole = audio::read_file(path)?;
        let wanted = (SECONDS * f64::from(whole.sample_rate())) as usize;
        let original = if wanted >= whole.frames() {
            whole
        } else {
            whole.segment(0, wanted)
        };
        println!("{}", Path::new(path).file_name().unwrap().to_string_lossy());

        for (label, options) in variants() {
            let report = recompose::verify(&decompose(&original, &options)?, &original)?;
            println!(
                "  {label:<34} {:>4} samples, {:>4} placements, reuse {:.2}x, {}",
                report.samples,
                report.placements,
                report.reuse(),
                if report.is_exact() {
                    "exact"
                } else {
                    "INEXACT"
                }
            );
        }
    }
    Ok(())
}

/// The option sets worth comparing, each a single step from the one before it.
fn variants() -> Vec<(&'static str, DecomposeOptions)> {
    let wide = DecomposeOptions {
        candidates: 4096,
        ..DecomposeOptions::default()
    };
    let mut far = wide.clone();
    far.search_seconds = 0.05;
    let mut loose = far.clone();
    loose.matching.tolerance = 0.4;
    let mut looser = loose.clone();
    looser.matching.tolerance = 0.8;
    let mut whole = looser.clone();
    whole.stems = StemMode::None;
    let mut tiny_gain = whole.clone();
    tiny_gain.matching.minimum_gain = 1e-6;
    tiny_gain.matching.maximum_gain = 1e6;
    vec![
        ("default", DecomposeOptions::default()),
        ("candidates 4096", wide),
        ("+ search 0.05 s", far),
        ("+ tolerance 0.4", loose),
        ("+ tolerance 0.8", looser),
        ("+ no stems", whole),
        ("+ any gain", tiny_gain),
    ]
}
