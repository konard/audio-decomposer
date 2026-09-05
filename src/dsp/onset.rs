//! Onset detection: where does one sample end and the next begin.
//!
//! The detector is the classic spectral-flux design: measure how much energy
//! appeared since the previous frame, subtract a moving baseline so that a
//! quiet passage is judged against quiet neighbours, and keep the local maxima
//! that stay above the baseline.

use crate::dsp::stft::Spectrogram;

/// Parameters of the detector.
#[derive(Clone, Copy, Debug)]
pub struct OnsetOptions {
    /// Length in frames of the moving baseline.
    pub baseline_span: usize,
    /// How far above the baseline a peak must rise, as a multiple of the
    /// baseline's own mean deviation.
    pub sensitivity: f64,
    /// Minimum distance between two onsets, in seconds.
    pub minimum_gap_seconds: f64,
    /// Absolute floor on the flux, relative to the largest flux in the signal.
    pub relative_floor: f64,
}

impl Default for OnsetOptions {
    fn default() -> Self {
        Self {
            baseline_span: 15,
            sensitivity: 1.5,
            minimum_gap_seconds: 0.03,
            relative_floor: 0.02,
        }
    }
}

/// A detected onset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Onset {
    /// Index of the spectrogram frame the onset was found in.
    pub frame: usize,
    /// Sample index in the original signal.
    pub sample: usize,
    /// Time in seconds from the start of the signal.
    pub time: f64,
    /// Detection-function value at the peak.
    pub strength: f64,
}

/// Half-wave rectified spectral flux, one value per frame.
#[must_use]
pub fn spectral_flux(spectrogram: &Spectrogram) -> Vec<f64> {
    let magnitudes = spectrogram.magnitudes();
    let mut flux = Vec::with_capacity(magnitudes.len());
    let mut previous: Option<&Vec<f64>> = None;
    for frame in &magnitudes {
        let value = previous.map_or(0.0, |last| {
            frame
                .iter()
                .zip(last.iter())
                .map(|(current, earlier)| (current - earlier).max(0.0))
                .sum()
        });
        flux.push(value);
        previous = Some(frame);
    }
    flux
}

/// Subtracts a moving average from a detection function.
#[must_use]
pub fn subtract_baseline(function: &[f64], span: usize) -> Vec<f64> {
    let half = span.max(1) / 2;
    function
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let start = index.saturating_sub(half);
            let end = (index + half + 1).min(function.len());
            let window = &function[start..end];
            let mean = window.iter().sum::<f64>() / window.len() as f64;
            (value - mean).max(0.0)
        })
        .collect()
}

/// Detects onsets in a spectrogram.
#[must_use]
pub fn detect(spectrogram: &Spectrogram, options: OnsetOptions) -> Vec<Onset> {
    let flux = spectral_flux(spectrogram);
    let detection = subtract_baseline(&flux, options.baseline_span);
    let peak = detection.iter().copied().fold(0.0_f64, f64::max);
    if peak <= 0.0 {
        return Vec::new();
    }
    let mean = detection.iter().sum::<f64>() / detection.len() as f64;
    let threshold = (mean * options.sensitivity).max(peak * options.relative_floor);
    let minimum_gap = (options.minimum_gap_seconds * f64::from(spectrogram.sample_rate())) as usize;

    let mut onsets: Vec<Onset> = Vec::new();
    for frame in 1..detection.len().saturating_sub(1) {
        let value = detection[frame];
        if value < threshold || value < detection[frame - 1] || value < detection[frame + 1] {
            continue;
        }
        let time = spectrogram.frame_time(frame);
        let sample = frame_start_sample(frame, spectrogram);
        if let Some(previous) = onsets.last() {
            if sample < previous.sample + minimum_gap {
                if value > previous.strength {
                    onsets.pop();
                } else {
                    continue;
                }
            }
        }
        onsets.push(Onset {
            frame,
            sample,
            time,
            strength: value,
        });
    }
    onsets
}

/// The sample the given frame starts at, clamped into the signal.
fn frame_start_sample(frame: usize, spectrogram: &Spectrogram) -> usize {
    let centre = spectrogram.frame_time(frame) * f64::from(spectrogram.sample_rate());
    let start = centre - (spectrogram.options().fft_size as f64) / 2.0;
    if start <= 0.0 {
        0
    } else {
        (start as usize).min(spectrogram.signal_length())
    }
}

/// Turns onsets into half-open sample ranges that tile the whole signal.
#[must_use]
pub fn segments(onsets: &[Onset], signal_length: usize) -> Vec<(usize, usize)> {
    if signal_length == 0 {
        return Vec::new();
    }
    let mut boundaries: Vec<usize> = std::iter::once(0)
        .chain(onsets.iter().map(|onset| onset.sample.min(signal_length)))
        .collect();
    boundaries.push(signal_length);
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
        .windows(2)
        .filter(|pair| pair[1] > pair[0])
        .map(|pair| (pair[0], pair[1]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::stft::{self, StftOptions};
    use crate::dsp::WindowKind;

    fn pulse_train(sample_rate: u32, positions: &[usize], length: usize) -> Vec<f64> {
        let mut signal = vec![0.0; length];
        for &position in positions {
            for index in 0..2000 {
                let sample = position + index;
                if sample >= length {
                    break;
                }
                let time = index as f64 / f64::from(sample_rate);
                let decay = (-time * 30.0).exp();
                signal[sample] += decay * (std::f64::consts::TAU * 440.0 * time).sin() * 0.7;
            }
        }
        signal
    }

    fn analyze(signal: &[f64], sample_rate: u32) -> Spectrogram {
        stft::forward(
            signal,
            sample_rate,
            StftOptions {
                fft_size: 512,
                hop: 128,
                window: WindowKind::Hann,
            },
        )
        .unwrap()
    }

    #[test]
    fn flux_is_zero_for_the_first_frame_and_for_silence() {
        let spectrogram = analyze(&vec![0.0; 4000], 16000);
        let flux = spectral_flux(&spectrogram);

        assert_eq!(flux[0], 0.0);
        assert!(flux.iter().all(|value| value.abs() < 1e-12));
        assert!(detect(&spectrogram, OnsetOptions::default()).is_empty());
    }

    #[test]
    fn baseline_subtraction_keeps_only_what_rises_above_the_neighbourhood() {
        let function = vec![0.0, 0.0, 5.0, 0.0, 0.0];
        let detection = subtract_baseline(&function, 5);

        assert!(detection[2] > 0.0);
        assert_eq!(detection[0], 0.0);
        assert_eq!(subtract_baseline(&function, 0).len(), function.len());
    }

    #[test]
    fn three_pulses_produce_three_onsets_near_their_starts() {
        let sample_rate = 16000;
        let positions = [1000, 6000, 11000];
        let signal = pulse_train(sample_rate, &positions, 16000);
        let spectrogram = analyze(&signal, sample_rate);
        let onsets = detect(&spectrogram, OnsetOptions::default());

        assert_eq!(onsets.len(), positions.len(), "{onsets:?}");
        for (onset, expected) in onsets.iter().zip(positions.iter()) {
            let error = onset.sample.abs_diff(*expected);
            assert!(error < 400, "onset at {} vs {expected}", onset.sample);
            assert!(onset.time > 0.0);
            assert!(onset.strength > 0.0);
        }
    }

    #[test]
    fn onsets_tile_the_signal_into_segments() {
        let onsets = vec![
            Onset {
                frame: 4,
                sample: 500,
                time: 0.03,
                strength: 1.0,
            },
            Onset {
                frame: 9,
                sample: 1200,
                time: 0.07,
                strength: 1.0,
            },
        ];
        let ranges = segments(&onsets, 2000);

        assert_eq!(ranges, vec![(0, 500), (500, 1200), (1200, 2000)]);
        assert!(segments(&[], 0).is_empty());
        assert_eq!(segments(&onsets, 400), vec![(0, 400)]);
    }
}
