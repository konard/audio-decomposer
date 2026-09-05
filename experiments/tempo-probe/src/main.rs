//! Prints the tempo candidates a click train produces, so the octave choices
//! the estimator makes can be judged lag by lag.

use audio_decomposer::dsp::tempo::{candidates, from_flux, TempoOptions};

fn clicks(bpm: f64, seconds: f64, hop: f64) -> Vec<f64> {
    let period = 60.0 / bpm;
    let length = (seconds / hop) as usize;
    let mut function = vec![0.0; length];
    let mut time = 0.0;
    while time < seconds {
        let index = (time / hop) as usize;
        for (step, weight) in [1.0, 0.5, 0.25].into_iter().enumerate() {
            if let Some(slot) = function.get_mut(index + step) {
                *slot += weight;
            }
        }
        time += period;
    }
    function
}

fn main() {
    let hop = 0.01;
    for bpm in [80.0, 96.0, 140.0, 160.0] {
        let function = clicks(bpm, 20.0, hop);
        let options = TempoOptions::default();
        let found = from_flux(&function, hop, options);
        println!("\n== clicks at {bpm} BPM -> {found:?}");
        let mut scored = candidates(&function, hop, options);
        scored.sort_by(|left, right| right.score.total_cmp(&left.score));
        for candidate in scored.iter().take(8) {
            println!(
                "   {:7.2} BPM lag {:3} support {:8.4} score {:8.4}",
                candidate.bpm, candidate.lag, candidate.support, candidate.score
            );
        }
    }
}
