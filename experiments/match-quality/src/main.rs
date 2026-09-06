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
//! There is a trap here that the first version of this probe fell into. The
//! matcher is given the original signal and allowed to slide the pattern by up
//! to the search radius. An event taken from that same signal can therefore be
//! slid back onto the stretch it was cut from, where it matches itself
//! perfectly. Every neighbour closer than the radius scores a remainder of
//! exactly zero, and the probe reports a third of the recording as repeating
//! when nothing repeats at all. The pipeline never sees this because it
//! subtracts each placement as it goes, so a waveform's own source is already
//! silence by the time anything is compared against it. The probe reproduces
//! that by refusing any pair whose source regions could touch, and reports
//! both figures so the size of the artefact is visible.
//!
//! ```text
//! cargo run --release -- ../../corpus/*.flac
//! ```

use audio_decomposer::decompose::dedup::{best_alignment, MatchOptions};
use audio_decomposer::decompose::events::{detect, EventOptions};
use audio_decomposer::{audio, Result};

/// Seconds of each recording to look at.
const SECONDS: f64 = 20.0;

/// The thresholds the distribution is reported at.
const CUTS: [f64; 6] = [0.05, 0.12, 0.25, 0.40, 0.60, 0.80];

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
        let radius = (0.05 * f64::from(audio.sample_rate())) as usize;
        let options = MatchOptions {
            tolerance: 1.0,
            search_radius: radius,
            minimum_gain: 1e-6,
            maximum_gain: 1e6,
            allow_inversion: true,
        };

        let mut naive = Vec::new();
        let mut honest = Vec::new();
        for (index, event) in events.iter().enumerate() {
            let anchor = event.start as i64;
            let mut closest = f64::INFINITY;
            let mut distant = f64::INFINITY;
            for (other, stored) in waveforms[..index].iter().enumerate() {
                if stored.is_empty() {
                    continue;
                }
                let Some(alignment) = best_alignment(&channel, anchor, stored, &options) else {
                    continue;
                };
                closest = closest.min(alignment.remainder);
                // The pattern came from `events[other]`. Anywhere the search
                // could put it that touches that stretch is the waveform
                // finding itself, not a repeat.
                let reachable = event.start + radius + stored.len();
                if events[other].end() + radius < event.start || events[other].start > reachable {
                    distant = distant.min(alignment.remainder);
                }
            }
            if closest.is_finite() {
                naive.push(closest);
            }
            if distant.is_finite() {
                honest.push(distant);
            }
        }

        println!("{path}");
        println!("  {} events", events.len());
        report("counting a waveform's own source", &mut naive);
        report("comparing distinct stretches only", &mut honest);
    }
    Ok(())
}

/// Prints the distribution of best remainders under a heading.
fn report(heading: &str, best: &mut [f64]) {
    best.sort_by(f64::total_cmp);
    println!("  {heading}: {} comparable", best.len());
    if best.is_empty() {
        return;
    }
    for cut in CUTS {
        let under = best.iter().filter(|value| **value <= cut).count();
        println!(
            "    remainder <= {cut:.2}: {under:4} ({:5.1}%)",
            100.0 * under as f64 / best.len() as f64
        );
    }
    println!(
        "    best {:.3}, median {:.3}",
        best[0],
        best[best.len() / 2]
    );
}
