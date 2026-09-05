//! Deduplication by subtraction at different positions.
//!
//! This is the operation the whole tool is named after. Given a stretch of the
//! recording that has not been explained yet and a waveform that was stored
//! earlier, it asks: is there a position and a gain at which subtracting that
//! waveform removes this stretch? When there is, the stretch costs one
//! reference instead of one more copy of the audio.
//!
//! Two mechanisms are used, cheapest first.
//!
//! * A **fingerprint** over the exact bits of the samples finds content that
//!   repeats verbatim — a looped bar, a duplicated channel, a copy-pasted note.
//!   The match is exact, so the subtraction leaves nothing at all.
//! * A **correlation search** finds content that repeats with a different
//!   amplitude or at a slightly different offset. The best position comes from
//!   an FFT cross-correlation, the best gain is the least-squares solution at
//!   that position, and the match is accepted only when what remains after the
//!   subtraction is small enough relative to what was there.
//!
//! Whatever is not accepted stays in the signal and ends up in the residual, so
//! the tolerance decides how large the bank gets, never whether the
//! reconstruction is correct.

use crate::decompose::model::Gain;
use crate::dsp::fft;

/// How willing the matcher is to reuse a waveform.
#[derive(Clone, Copy, Debug)]
pub struct MatchOptions {
    /// How much of the window may remain after the subtraction, as a fraction
    /// of its root-mean-square amplitude. `0.0` accepts only exact matches.
    pub tolerance: f64,
    /// How far, in frames, the match may be shifted from where the stretch
    /// starts.
    pub search_radius: usize,
    /// Smallest gain magnitude worth placing.
    pub minimum_gain: f64,
    /// Largest gain magnitude that may be placed.
    pub maximum_gain: f64,
    /// Whether a waveform may be subtracted with its phase inverted.
    pub allow_inversion: bool,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self {
            tolerance: 0.12,
            search_radius: 0,
            minimum_gain: 0.05,
            maximum_gain: 8.0,
            allow_inversion: true,
        }
    }
}

/// Where and how strongly a waveform should be subtracted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    /// Frame the waveform starts at, which may lie before the signal.
    pub start: i64,
    /// Gain the waveform is scaled by.
    pub gain: Gain,
    /// Root-mean-square amplitude left in the window after the subtraction,
    /// relative to what was there before it. `0.0` is an exact match.
    pub remainder: f64,
}

impl Alignment {
    /// How well the waveform explains the window, in `0.0..=1.0`.
    #[must_use]
    pub fn similarity(&self) -> f64 {
        (1.0 - self.remainder).clamp(0.0, 1.0)
    }
}

/// A hash over the exact bits of a waveform.
///
/// Two waveforms with the same fingerprint are equal sample for sample with
/// overwhelming probability, and the caller confirms it before reusing one, so
/// a collision costs a comparison rather than correctness.
#[must_use]
pub fn fingerprint(samples: &[f64]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash ^= samples.len() as u64;
    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    for sample in samples {
        // Normalising negative zero keeps the fingerprint agnostic to a
        // difference the arithmetic itself ignores.
        let bits = if *sample == 0.0 { 0 } else { sample.to_bits() };
        hash ^= bits;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Reads a signal at a position that may lie outside it.
#[must_use]
pub fn sample_at(signal: &[f64], position: i64) -> f64 {
    if position < 0 {
        return 0.0;
    }
    signal.get(position as usize).copied().unwrap_or(0.0)
}

/// Energy of a window of the signal, counting samples outside it as silence.
#[must_use]
pub fn window_energy(signal: &[f64], start: i64, length: usize) -> f64 {
    (0..length as i64)
        .map(|offset| {
            let value = sample_at(signal, start + offset);
            value * value
        })
        .sum()
}

/// Finds the best position and gain for subtracting `pattern` from `signal`
/// around `anchor`.
///
/// Returns `None` when the pattern is silent, when the window is silent, or
/// when the best match is worse than [`MatchOptions::tolerance`].
#[must_use]
pub fn best_alignment(
    signal: &[f64],
    anchor: i64,
    pattern: &[f64],
    options: &MatchOptions,
) -> Option<Alignment> {
    let length = pattern.len();
    let pattern_energy: f64 = pattern.iter().map(|value| value * value).sum();
    if length == 0 || pattern_energy <= 0.0 {
        return None;
    }

    let radius = options.search_radius as i64;
    let region_start = anchor - radius;
    let region_length = (2 * radius) as usize + length;
    let region: Vec<f64> = (0..region_length as i64)
        .map(|offset| sample_at(signal, region_start + offset))
        .collect();

    let correlation = correlate(&region, pattern);
    let mut squares = Vec::with_capacity(region.len() + 1);
    squares.push(0.0);
    for value in &region {
        let last = *squares.last().unwrap_or(&0.0);
        squares.push(last + value * value);
    }

    let mut best: Option<Alignment> = None;
    let mut best_distance = i64::MAX;
    for shift in 0..=(2 * radius) as usize {
        let window_energy = squares[shift + length] - squares[shift];
        if window_energy <= 0.0 {
            continue;
        }
        let dot = correlation.get(shift).copied().unwrap_or(0.0);
        let gain = Gain::from_factor(dot / pattern_energy);
        let factor = gain.factor();
        if factor.abs() < options.minimum_gain || factor.abs() > options.maximum_gain {
            continue;
        }
        if factor < 0.0 && !options.allow_inversion {
            continue;
        }
        let left = window_energy - 2.0 * factor * dot + factor * factor * pattern_energy;
        let remainder = (left.max(0.0) / window_energy).sqrt();
        let distance = (shift as i64 - radius).abs();
        let better = best.as_ref().is_none_or(|current| {
            remainder < current.remainder - 1e-15
                || ((remainder - current.remainder).abs() <= 1e-15 && distance < best_distance)
        });
        if better {
            best_distance = distance;
            best = Some(Alignment {
                start: region_start + shift as i64,
                gain,
                remainder,
            });
        }
    }

    best.filter(|alignment| alignment.remainder <= options.tolerance)
}

/// Cross-correlation of a region with a pattern, with the direct sum used for
/// the short inputs where it is cheaper than a transform.
fn correlate(region: &[f64], pattern: &[f64]) -> Vec<f64> {
    let shifts = region.len().saturating_sub(pattern.len()) + 1;
    if shifts * pattern.len() <= 4096 {
        return (0..shifts)
            .map(|shift| {
                pattern
                    .iter()
                    .enumerate()
                    .map(|(index, value)| region[shift + index] * value)
                    .sum()
            })
            .collect();
    }
    fft::cross_correlate(region, pattern)
}

/// Subtracts a scaled waveform from a signal, ignoring the part that falls
/// outside it.
pub fn subtract(signal: &mut [f64], start: i64, pattern: &[f64], gain: Gain) {
    if gain.is_silent() {
        return;
    }
    let factor = gain.factor();
    let first = (-start).max(0);
    let last = (signal.len() as i64 - start).min(pattern.len() as i64);
    for offset in first..last {
        signal[(start + offset) as usize] -= factor * pattern[offset as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(length: usize, frequency: f64) -> Vec<f64> {
        (0..length)
            .map(|index| {
                let time = index as f64 / 8000.0;
                (-6.0 * time).exp() * (std::f64::consts::TAU * frequency * time).sin()
            })
            .collect()
    }

    #[test]
    fn identical_waveforms_share_a_fingerprint() {
        let first = note(256, 440.0);
        let second = note(256, 440.0);

        assert_eq!(fingerprint(&first), fingerprint(&second));
        assert_ne!(fingerprint(&first), fingerprint(&note(256, 441.0)));
        assert_ne!(fingerprint(&first), fingerprint(&first[..255]));
        assert_eq!(
            fingerprint(&[0.0, 1.0]),
            fingerprint(&[-0.0, 1.0]),
            "negative zero is the same sample"
        );
        assert_eq!(fingerprint(&[]), fingerprint(&[]));
    }

    #[test]
    fn an_exact_repeat_is_found_and_removed_completely() {
        let pattern = note(256, 330.0);
        let mut signal = vec![0.0; 1024];
        signal[500..756].copy_from_slice(&pattern);

        let alignment = best_alignment(&signal, 500, &pattern, &MatchOptions::default()).unwrap();

        assert_eq!(alignment.start, 500);
        assert_eq!(alignment.gain, Gain::UNIT);
        assert!(alignment.remainder < 1e-12);
        assert_eq!(alignment.similarity(), 1.0);

        subtract(&mut signal, alignment.start, &pattern, alignment.gain);
        assert!(signal.iter().all(|value| value.abs() < 1e-12));
    }

    #[test]
    fn a_quieter_repeat_at_another_position_is_found() {
        let pattern = note(256, 330.0);
        let mut signal = vec![0.0; 1024];
        for (index, value) in pattern.iter().enumerate() {
            signal[517 + index] = value * 0.5;
        }

        let options = MatchOptions {
            search_radius: 32,
            ..MatchOptions::default()
        };
        let alignment = best_alignment(&signal, 500, &pattern, &options).unwrap();

        assert_eq!(alignment.start, 517, "the search found the shifted copy");
        assert_eq!(alignment.gain.factor(), 0.5);
        assert!(alignment.remainder < 1e-6);
    }

    #[test]
    fn an_inverted_repeat_is_found_only_when_inversion_is_allowed() {
        let pattern = note(128, 220.0);
        let signal: Vec<f64> = pattern.iter().map(|value| -value).collect();

        let inverted = best_alignment(&signal, 0, &pattern, &MatchOptions::default()).unwrap();
        assert_eq!(inverted.gain.factor(), -1.0);

        let options = MatchOptions {
            allow_inversion: false,
            ..MatchOptions::default()
        };
        assert!(best_alignment(&signal, 0, &pattern, &options).is_none());
    }

    #[test]
    fn unrelated_content_is_refused() {
        let pattern = note(256, 330.0);
        let signal = note(256, 517.0);

        assert!(best_alignment(&signal, 0, &pattern, &MatchOptions::default()).is_none());
    }

    #[test]
    fn a_gain_outside_the_allowed_range_is_refused() {
        let pattern = note(128, 220.0);
        let loud: Vec<f64> = pattern.iter().map(|value| value * 20.0).collect();
        let quiet: Vec<f64> = pattern.iter().map(|value| value * 0.001).collect();

        assert!(best_alignment(&loud, 0, &pattern, &MatchOptions::default()).is_none());
        assert!(best_alignment(&quiet, 0, &pattern, &MatchOptions::default()).is_none());
    }

    #[test]
    fn silence_on_either_side_produces_no_match() {
        let pattern = note(128, 220.0);

        assert!(best_alignment(&[0.0; 512], 0, &pattern, &MatchOptions::default()).is_none());
        assert!(best_alignment(&pattern, 0, &[0.0; 128], &MatchOptions::default()).is_none());
        assert!(best_alignment(&pattern, 0, &[], &MatchOptions::default()).is_none());
    }

    #[test]
    fn matches_may_start_before_the_signal() {
        // A bank sample that begins with silence can legitimately be placed so
        // that its silent head falls before the recording starts.
        let mut pattern = vec![0.0; 64];
        pattern.extend(note(64, 220.0));
        let signal = pattern[64..].to_vec();

        let options = MatchOptions {
            search_radius: 96,
            ..MatchOptions::default()
        };
        let alignment = best_alignment(&signal, 0, &pattern, &options).unwrap();

        assert_eq!(
            alignment.start, -64,
            "the silent head sits before frame zero"
        );
        assert_eq!(alignment.gain, Gain::UNIT);
        // The remainder is the square root of a cancelling difference, so an
        // exact match shows up as the square root of the rounding, not as zero.
        assert!(alignment.remainder < 1e-6, "{}", alignment.remainder);
    }

    #[test]
    fn a_pattern_that_would_ring_outside_the_recording_is_refused() {
        // Only the tail of a decaying note is left in the signal: subtracting
        // the whole note would write its loud head before frame zero, so the
        // match is judged against that and rejected.
        let pattern = note(128, 220.0);
        let signal = pattern[64..].to_vec();

        let options = MatchOptions {
            search_radius: 96,
            ..MatchOptions::default()
        };
        assert!(best_alignment(&signal, 0, &pattern, &options).is_none());
    }

    #[test]
    fn subtraction_stays_inside_the_signal() {
        let mut signal = vec![1.0; 4];

        subtract(&mut signal, -2, &[1.0; 4], Gain::UNIT);
        assert_eq!(signal, vec![0.0, 0.0, 1.0, 1.0]);

        subtract(&mut signal, 3, &[1.0; 4], Gain::UNIT);
        assert_eq!(signal, vec![0.0, 0.0, 1.0, 0.0]);

        subtract(&mut signal, 0, &[1.0; 4], Gain::from_factor(0.0));
        assert_eq!(
            signal,
            vec![0.0, 0.0, 1.0, 0.0],
            "a silent gain does nothing"
        );
    }

    #[test]
    fn the_transform_and_the_direct_sum_agree() {
        let pattern = note(512, 210.0);
        let mut signal = vec![0.0; 4096];
        signal[1000..1512].copy_from_slice(&pattern);

        let direct: Vec<f64> = (0..=200)
            .map(|shift| {
                pattern
                    .iter()
                    .enumerate()
                    .map(|(index, value)| sample_at(&signal, 900 + shift + index as i64) * value)
                    .sum()
            })
            .collect();
        let region: Vec<f64> = (0..(200 + 512))
            .map(|offset| sample_at(&signal, 900 + offset))
            .collect();
        let transformed = correlate(&region, &pattern);

        for (shift, expected) in direct.iter().enumerate() {
            assert!(
                (transformed[shift] - expected).abs() < 1e-6,
                "shift {shift}: {} vs {expected}",
                transformed[shift]
            );
        }
        assert_eq!(
            window_energy(&signal, 1000, 512),
            pattern.iter().map(|v| v * v).sum::<f64>()
        );
        assert_eq!(window_energy(&signal, -10, 5), 0.0);
    }
}
