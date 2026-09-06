//! Measures how close the events of a real recording actually come to each
//! other, so the reuse figure the pipeline reports can be explained rather
//! than guessed at.
//!
//! `decompose` reuses a stored waveform when subtracting it leaves less than
//! `tolerance` of the window behind. On synthesised fixtures that happens
//! constantly. On the public-domain corpus it almost never happens, and the
//! question this probe answers is which of the two possible reasons is true:
//! the threshold is too tight, or live acoustic music simply has no two events
//! that are the same waveform.
//!
//! ```text
//! cargo run --release -- ../../corpus/*.flac
//! ```

use audio_decomposer::decompose::dedup::{best_alignment, MatchOptions};
use audio_decomposer::decompose::events::{detect, EventOptions};
use audio_decomposer::{audio, Result};

/// Seconds of each recording to look at.
const SECONDS: f64 = 20.0;

fn main() -> Result<()> {
    for path in std::env::args().skip(1) {
        let whole = audio::read_file(&path)?;
        let frames = (SECONDS * f64::from(whole.sample_rate())) as usize;
        let audio = if frames < whole.frames() {
            whole.segment(0, frames)
        } else {
            whole
        };

        let events = detect(&audio, &EventOptions::default())?;
        let channel = audio.channel(0).to_vec();
        // Every event as its own waveform, which is what the bank would store.
        let waveforms: Vec<Vec<f64>> = events
            .iter()
            .map(|event| channel[event.start..event.end()].to_vec())
            .collect();

        // Wide open, so the measurement is the remainder itself rather than
        // whether it passed a threshold.
        let options = MatchOptions {
            tolerance: 1.0,
            search_radius: (0.05 * f64::from(audio.sample_rate())) as usize,
            minimum_gain: 1e-6,
            maximum_gain: 1e6,
            allow_inversion: true,
        };

        let mut best = Vec::new();
        for (index, event) in events.iter().enumerate() {
            let closest = waveforms[..index]
                .iter()
                .filter(|stored| !stored.is_empty())
                .filter_map(|stored| {
                    best_alignment(&channel, event.start as i64, stored, &options)
                })
                .map(|alignment| alignment.remainder)
                .fold(f64::INFINITY, f64::min);
            if closest.is_finite() {
                best.push(closest);
            }
        }
        best.sort_by(f64::total_cmp);

        println!("{path}");
        println!("  {} events, {} comparable", events.len(), best.len());
        if best.is_empty() {
            continue;
        }
        for cut in [0.05, 0.12, 0.25, 0.40, 0.60, 0.80] {
            let under = best.iter().filter(|value| **value <= cut).count();
            println!(
                "  remainder <= {cut:.2}: {under:4} ({:5.1}%)",
                100.0 * under as f64 / best.len() as f64
            );
        }
        let median = best[best.len() / 2];
        println!("  best {:.3}, median {median:.3}", best[0]);
    }
    Ok(())
}
