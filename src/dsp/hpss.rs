//! Harmonic/percussive source separation.
//!
//! Harmonic content is steady in time and sparse in frequency; percussive
//! content is the opposite. Median-filtering the magnitude spectrogram along
//! each axis produces two enhanced pictures, and the soft masks derived from
//! them are complementary by construction: the two output spectrograms sum
//! back to the input, so the two rendered signals sum back to the original
//! audio. That property is what keeps the decomposition lossless.

use crate::dsp::stft::Spectrogram;
use crate::error::Result;

/// Parameters of the separation.
#[derive(Clone, Copy, Debug)]
pub struct HpssOptions {
    /// Length in frames of the median filter that enhances harmonics.
    pub harmonic_span: usize,
    /// Length in bins of the median filter that enhances percussion.
    pub percussive_span: usize,
    /// Exponent applied to the enhanced magnitudes; `1.0` gives amplitude
    /// masks and `2.0` gives Wiener-style power masks.
    pub power: f64,
}

impl Default for HpssOptions {
    fn default() -> Self {
        Self {
            harmonic_span: 17,
            percussive_span: 17,
            power: 2.0,
        }
    }
}

impl HpssOptions {
    /// Forces both spans to be odd and at least one, so the filters stay
    /// centred on the sample they replace.
    #[must_use]
    pub const fn normalized(self) -> Self {
        Self {
            harmonic_span: odd_at_least_one(self.harmonic_span),
            percussive_span: odd_at_least_one(self.percussive_span),
            power: self.power,
        }
    }
}

const fn odd_at_least_one(span: usize) -> usize {
    if span < 1 {
        1
    } else if span % 2 == 0 {
        span + 1
    } else {
        span
    }
}

/// The two halves of a separation.
#[derive(Clone, Debug)]
pub struct Separation {
    /// Sustained, pitched content.
    pub harmonic: Spectrogram,
    /// Transient, broadband content.
    pub percussive: Spectrogram,
}

/// Splits a spectrogram into harmonic and percussive parts.
///
/// # Errors
///
/// Propagates mask application failures, which cannot occur for masks built
/// here but are surfaced rather than hidden.
pub fn separate(spectrogram: &Spectrogram, options: HpssOptions) -> Result<Separation> {
    let options = options.normalized();
    let magnitudes = spectrogram.magnitudes();
    let harmonic_enhanced = median_filter_time(&magnitudes, options.harmonic_span);
    let percussive_enhanced = median_filter_frequency(&magnitudes, options.percussive_span);

    let mut harmonic_mask = Vec::with_capacity(magnitudes.len());
    let mut percussive_mask = Vec::with_capacity(magnitudes.len());
    for (harmonic_frame, percussive_frame) in
        harmonic_enhanced.iter().zip(percussive_enhanced.iter())
    {
        let mut harmonic_row = Vec::with_capacity(harmonic_frame.len());
        let mut percussive_row = Vec::with_capacity(harmonic_frame.len());
        for (harmonic_value, percussive_value) in harmonic_frame.iter().zip(percussive_frame.iter())
        {
            let harmonic_weight = harmonic_value.powf(options.power);
            let percussive_weight = percussive_value.powf(options.power);
            let total = harmonic_weight + percussive_weight;
            let share = if total > 0.0 {
                harmonic_weight / total
            } else {
                0.5
            };
            harmonic_row.push(share);
            percussive_row.push(1.0 - share);
        }
        harmonic_mask.push(harmonic_row);
        percussive_mask.push(percussive_row);
    }

    let mut harmonic = spectrogram.clone();
    harmonic.apply_mask(&harmonic_mask)?;
    let mut percussive = spectrogram.clone();
    percussive.apply_mask(&percussive_mask)?;
    Ok(Separation {
        harmonic,
        percussive,
    })
}

/// Median of a slice, computed on a copy so the input is left untouched.
#[must_use]
pub fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        0.5 * (sorted[middle - 1] + sorted[middle])
    }
}

/// Median-filters each frequency bin along the time axis.
#[must_use]
pub fn median_filter_time(magnitudes: &[Vec<f64>], span: usize) -> Vec<Vec<f64>> {
    let frames = magnitudes.len();
    let half = span / 2;
    let mut window = Vec::with_capacity(span);
    magnitudes
        .iter()
        .enumerate()
        .map(|(frame, row)| {
            row.iter()
                .enumerate()
                .map(|(bin, _)| {
                    window.clear();
                    let start = frame.saturating_sub(half);
                    let end = (frame + half + 1).min(frames);
                    window.extend(
                        magnitudes[start..end]
                            .iter()
                            .filter_map(|neighbour| neighbour.get(bin).copied()),
                    );
                    median(&window)
                })
                .collect()
        })
        .collect()
}

/// Median-filters each frame along the frequency axis.
#[must_use]
pub fn median_filter_frequency(magnitudes: &[Vec<f64>], span: usize) -> Vec<Vec<f64>> {
    let half = span / 2;
    let mut window = Vec::with_capacity(span);
    magnitudes
        .iter()
        .map(|row| {
            (0..row.len())
                .map(|bin| {
                    window.clear();
                    let start = bin.saturating_sub(half);
                    let end = (bin + half + 1).min(row.len());
                    window.extend_from_slice(&row[start..end]);
                    median(&window)
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::stft::{self, StftOptions};

    fn mixture(sample_rate: u32, length: usize) -> Vec<f64> {
        (0..length)
            .map(|index| {
                let time = index as f64 / f64::from(sample_rate);
                let tone = 0.4 * (std::f64::consts::TAU * 220.0 * time).sin();
                let click = if index % 4000 < 12 { 0.6 } else { 0.0 };
                tone + click
            })
            .collect()
    }

    #[test]
    fn median_handles_both_parities_and_empty_input() {
        assert_eq!(median(&[]), 0.0);
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), 2.5);
    }

    #[test]
    fn spans_are_forced_odd() {
        let options = HpssOptions {
            harmonic_span: 8,
            percussive_span: 0,
            power: 1.0,
        }
        .normalized();
        assert_eq!(options.harmonic_span, 9);
        assert_eq!(options.percussive_span, 1);
        assert_eq!(HpssOptions::default().normalized().harmonic_span, 17);
    }

    #[test]
    fn time_and_frequency_filters_smooth_along_their_own_axis() {
        // Bin 1 is a sustained ridge; frame 1 is a broadband burst.
        let magnitudes = vec![
            vec![0.0, 5.0, 0.0, 0.0, 0.0],
            vec![9.0, 9.0, 9.0, 9.0, 9.0],
            vec![0.0, 5.0, 0.0, 0.0, 0.0],
        ];

        let over_time = median_filter_time(&magnitudes, 3);
        assert_eq!(over_time[1][0], 0.0, "the burst is filtered out over time");
        assert_eq!(over_time[1][1], 5.0, "the ridge survives over time");

        let over_frequency = median_filter_frequency(&magnitudes, 3);
        assert_eq!(over_frequency[1][0], 9.0, "the burst survives over bins");
        assert_eq!(
            over_frequency[0][1], 0.0,
            "the ridge is filtered out over bins"
        );
    }

    #[test]
    fn the_two_parts_sum_back_to_the_original_signal() {
        let sample_rate = 16000;
        let signal = mixture(sample_rate, 16000);
        let options = StftOptions {
            fft_size: 512,
            hop: 128,
            window: crate::dsp::WindowKind::Hann,
        };
        let spectrogram = stft::forward(&signal, sample_rate, options).unwrap();
        let separation = separate(&spectrogram, HpssOptions::default()).unwrap();

        let harmonic = stft::inverse(&separation.harmonic).unwrap();
        let percussive = stft::inverse(&separation.percussive).unwrap();
        let worst = signal
            .iter()
            .zip(harmonic.iter().zip(percussive.iter()))
            .map(|(original, (left, right))| (original - (left + right)).abs())
            .fold(0.0_f64, f64::max);
        assert!(worst < 1e-10, "parts did not sum back, worst error {worst}");
    }

    #[test]
    fn a_click_lands_in_the_percussive_part_and_a_tone_in_the_harmonic_part() {
        let sample_rate = 16000;
        let signal = mixture(sample_rate, 16000);
        let options = StftOptions {
            fft_size: 512,
            hop: 128,
            window: crate::dsp::WindowKind::Hann,
        };
        let spectrogram = stft::forward(&signal, sample_rate, options).unwrap();
        let separation = separate(&spectrogram, HpssOptions::default()).unwrap();
        let harmonic = stft::inverse(&separation.harmonic).unwrap();
        let percussive = stft::inverse(&separation.percussive).unwrap();

        let click_energy: f64 = percussive[0..64].iter().map(|value| value * value).sum();
        let tone_energy: f64 = harmonic[8000..8256].iter().map(|value| value * value).sum();
        let click_leak: f64 = harmonic[0..64].iter().map(|value| value * value).sum();
        let tone_leak: f64 = percussive[8000..8256]
            .iter()
            .map(|value| value * value)
            .sum();

        assert!(click_energy > click_leak, "{click_energy} vs {click_leak}");
        assert!(tone_energy > tone_leak, "{tone_energy} vs {tone_leak}");
    }
}
