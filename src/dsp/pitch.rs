//! Monophonic pitch detection and the note arithmetic built on top of it.
//!
//! The estimator is YIN (de Cheveigné and Kawahara, 2002): a squared
//! difference function, a cumulative mean normalization that removes the
//! trivial zero-lag minimum, an absolute threshold, and parabolic
//! interpolation around the chosen lag. It is accurate enough that a
//! synthesized note is recovered to well under a semitone, which is what the
//! MIDI export needs.

/// Parameters of the estimator.
#[derive(Clone, Copy, Debug)]
pub struct PitchOptions {
    /// Lowest frequency considered, in hertz.
    pub minimum_hz: f64,
    /// Highest frequency considered, in hertz.
    pub maximum_hz: f64,
    /// YIN's absolute threshold; smaller values demand a more periodic signal.
    pub threshold: f64,
}

impl Default for PitchOptions {
    fn default() -> Self {
        Self {
            minimum_hz: 27.5,
            maximum_hz: 4186.0,
            threshold: 0.15,
        }
    }
}

/// A pitch estimate with the confidence that produced it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchEstimate {
    /// Estimated fundamental frequency in hertz.
    pub frequency: f64,
    /// `1 - d'(tau)`, where `d'` is YIN's normalized difference; `1.0` means a
    /// perfectly periodic frame.
    pub confidence: f64,
}

impl PitchEstimate {
    /// The estimate as a fractional MIDI note number.
    #[must_use]
    pub fn midi(self) -> f64 {
        frequency_to_midi(self.frequency)
    }

    /// The nearest MIDI note number.
    #[must_use]
    pub fn nearest_note(self) -> i32 {
        nearest_midi_note(self.frequency)
    }
}

/// Estimates the fundamental frequency of one frame of samples.
#[must_use]
pub fn estimate(samples: &[f64], sample_rate: u32, options: PitchOptions) -> Option<PitchEstimate> {
    if sample_rate == 0 || options.minimum_hz <= 0.0 || options.maximum_hz <= options.minimum_hz {
        return None;
    }
    let rate = f64::from(sample_rate);
    let minimum_lag = (rate / options.maximum_hz).floor().max(2.0) as usize;
    let maximum_lag = (rate / options.minimum_hz).ceil() as usize;
    let maximum_lag = maximum_lag.min(samples.len() / 2);
    if maximum_lag <= minimum_lag {
        return None;
    }

    let difference = difference_function(samples, maximum_lag);
    let normalized = cumulative_mean_normalized(&difference);

    let mut chosen = None;
    for lag in minimum_lag..maximum_lag {
        if normalized[lag] < options.threshold {
            let mut best = lag;
            while best + 1 < maximum_lag && normalized[best + 1] < normalized[best] {
                best += 1;
            }
            chosen = Some(best);
            break;
        }
    }
    let lag = chosen.or_else(|| {
        (minimum_lag..maximum_lag)
            .min_by(|a, b| normalized[*a].total_cmp(&normalized[*b]))
            .filter(|lag| normalized[*lag] < 1.0)
    })?;

    let refined = parabolic_minimum(&normalized, lag);
    if refined <= 0.0 {
        return None;
    }
    Some(PitchEstimate {
        frequency: rate / refined,
        confidence: (1.0 - normalized[lag]).clamp(0.0, 1.0),
    })
}

/// YIN's squared difference function for lags `0..=max_lag`.
#[must_use]
pub fn difference_function(samples: &[f64], max_lag: usize) -> Vec<f64> {
    let window = samples.len().saturating_sub(max_lag);
    (0..=max_lag)
        .map(|lag| {
            (0..window)
                .map(|index| {
                    let delta = samples[index] - samples[index + lag];
                    delta * delta
                })
                .sum()
        })
        .collect()
}

/// The cumulative mean normalized difference, with `d'(0) = 1` by definition.
#[must_use]
pub fn cumulative_mean_normalized(difference: &[f64]) -> Vec<f64> {
    let mut normalized = vec![1.0; difference.len()];
    let mut running = 0.0;
    for lag in 1..difference.len() {
        running += difference[lag];
        normalized[lag] = if running > 0.0 {
            difference[lag] * lag as f64 / running
        } else {
            1.0
        };
    }
    normalized
}

/// Refines a minimum to sub-sample resolution with a parabolic fit.
#[must_use]
pub fn parabolic_minimum(values: &[f64], index: usize) -> f64 {
    if index == 0 || index + 1 >= values.len() {
        return index as f64;
    }
    let left = values[index - 1];
    let centre = values[index];
    let right = values[index + 1];
    let denominator = 2.0 * (2.0f64.mul_add(centre, -left) - right);
    if denominator.abs() < f64::EPSILON {
        return index as f64;
    }
    index as f64 + (right - left) / denominator
}

/// Converts a frequency in hertz to a fractional MIDI note number.
#[must_use]
pub fn frequency_to_midi(frequency: f64) -> f64 {
    if frequency <= 0.0 {
        return 0.0;
    }
    69.0 + 12.0 * (frequency / 440.0).log2()
}

/// Converts a MIDI note number to its frequency in hertz.
#[must_use]
pub fn midi_to_frequency(note: f64) -> f64 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

/// The MIDI note nearest to a frequency, clamped to the MIDI range.
#[must_use]
pub fn nearest_midi_note(frequency: f64) -> i32 {
    frequency_to_midi(frequency).round().clamp(0.0, 127.0) as i32
}

/// How far a frequency sits from the nearest tempered note, in cents.
#[must_use]
pub fn cents_from_nearest_note(frequency: f64) -> f64 {
    let exact = frequency_to_midi(frequency);
    (exact - exact.round()) * 100.0
}

/// The scientific-pitch name of a MIDI note, such as `A4` or `C#3`.
#[must_use]
pub fn note_name(note: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let clamped = note.clamp(0, 127);
    let octave = clamped / 12 - 1;
    format!("{}{octave}", NAMES[(clamped % 12) as usize])
}

/// Parses a scientific-pitch name back into a MIDI note number.
#[must_use]
pub fn parse_note_name(name: &str) -> Option<i32> {
    let trimmed = name.trim();
    let mut characters = trimmed.chars();
    let letter = characters.next()?.to_ascii_uppercase();
    let base = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = characters.collect();
    let (accidental, octave_text) = match rest.as_bytes().first() {
        Some(b'#') => (1, &rest[1..]),
        Some(b'b') => (-1, &rest[1..]),
        _ => (0, rest.as_str()),
    };
    let octave: i32 = octave_text.trim().parse().ok()?;
    let note = (octave + 1) * 12 + base + accidental;
    (0..=127).contains(&note).then_some(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frequency: f64, sample_rate: u32, length: usize) -> Vec<f64> {
        (0..length)
            .map(|index| {
                let time = index as f64 / f64::from(sample_rate);
                (std::f64::consts::TAU * frequency * time).sin()
                    + 0.4 * (std::f64::consts::TAU * 2.0 * frequency * time).sin()
                    + 0.2 * (std::f64::consts::TAU * 3.0 * frequency * time).sin()
            })
            .collect()
    }

    #[test]
    fn midi_conversions_are_inverses() {
        assert!((frequency_to_midi(440.0) - 69.0).abs() < 1e-12);
        assert!((midi_to_frequency(69.0) - 440.0).abs() < 1e-12);
        assert!((midi_to_frequency(60.0) - 261.625_565_300_598_6).abs() < 1e-9);
        for note in 0..128 {
            let frequency = midi_to_frequency(f64::from(note));
            assert_eq!(nearest_midi_note(frequency), note);
            assert!(cents_from_nearest_note(frequency).abs() < 1e-9);
        }
        assert_eq!(frequency_to_midi(0.0), 0.0);
        assert_eq!(nearest_midi_note(0.0), 0);
    }

    #[test]
    fn note_names_round_trip() {
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(0), "C-1");
        assert_eq!(note_name(200), "G9");
        for note in 0..128 {
            assert_eq!(parse_note_name(&note_name(note)), Some(note));
        }
        assert_eq!(parse_note_name("Db4"), Some(61));
        assert_eq!(parse_note_name(" a4 "), Some(69));
        assert_eq!(parse_note_name("H4"), None);
        assert_eq!(parse_note_name("C"), None);
        assert_eq!(parse_note_name("C99"), None);
    }

    #[test]
    fn a_synthesized_note_is_recovered_within_a_few_cents() {
        let sample_rate = 44100;
        for note in [40, 52, 60, 69, 81] {
            let frequency = midi_to_frequency(f64::from(note));
            let samples = tone(frequency, sample_rate, 4096);
            let estimate = estimate(&samples, sample_rate, PitchOptions::default()).unwrap();

            assert_eq!(estimate.nearest_note(), note, "note {note}");
            let error = (estimate.frequency - frequency).abs() / frequency;
            assert!(
                error < 0.01,
                "note {note}: {} vs {frequency}",
                estimate.frequency
            );
            assert!(estimate.confidence > 0.7);
            assert!((estimate.midi() - f64::from(note)).abs() < 0.2);
        }
    }

    #[test]
    fn noise_and_degenerate_input_do_not_produce_confident_pitches() {
        let sample_rate = 44100;
        let mut state = 12_345_u64;
        let noise: Vec<f64> = (0..4096)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                ((state >> 33) as f64 / f64::from(u32::MAX)) - 0.5
            })
            .collect();
        let noisy = estimate(&noise, sample_rate, PitchOptions::default());
        assert!(noisy.is_none_or(|value| value.confidence < 0.9));

        assert!(estimate(&[], sample_rate, PitchOptions::default()).is_none());
        assert!(estimate(&[0.0; 64], 0, PitchOptions::default()).is_none());
        assert!(estimate(
            &[0.0; 4096],
            sample_rate,
            PitchOptions {
                minimum_hz: 500.0,
                maximum_hz: 100.0,
                threshold: 0.15,
            }
        )
        .is_none());
    }

    #[test]
    fn helper_functions_handle_their_edges() {
        assert_eq!(difference_function(&[1.0, 2.0], 0), vec![0.0]);
        assert_eq!(cumulative_mean_normalized(&[0.0, 0.0])[1], 1.0);
        assert_eq!(parabolic_minimum(&[1.0, 0.0, 1.0], 0), 0.0);
        assert_eq!(parabolic_minimum(&[1.0, 0.0, 1.0], 2), 2.0);
        assert_eq!(parabolic_minimum(&[1.0, 0.0, 1.0], 1), 1.0);
        assert_eq!(parabolic_minimum(&[0.0, 0.0, 0.0], 1), 1.0);
    }
}
