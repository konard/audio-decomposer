//! Amplitude envelopes and the note dynamics derived from them.

/// Root-mean-square envelope over overlapping blocks.
#[must_use]
pub fn rms_envelope(samples: &[f64], block: usize, hop: usize) -> Vec<f64> {
    if samples.is_empty() || block == 0 || hop == 0 {
        return Vec::new();
    }
    let mut envelope = Vec::new();
    let mut start = 0;
    while start < samples.len() {
        let end = (start + block).min(samples.len());
        let window = &samples[start..end];
        let mean = window.iter().map(|value| value * value).sum::<f64>() / window.len() as f64;
        envelope.push(mean.sqrt());
        start += hop;
    }
    envelope
}

/// Peak envelope over overlapping blocks.
#[must_use]
pub fn peak_envelope(samples: &[f64], block: usize, hop: usize) -> Vec<f64> {
    if samples.is_empty() || block == 0 || hop == 0 {
        return Vec::new();
    }
    let mut envelope = Vec::new();
    let mut start = 0;
    while start < samples.len() {
        let end = (start + block).min(samples.len());
        envelope.push(
            samples[start..end]
                .iter()
                .fold(0.0_f64, |peak, value| peak.max(value.abs())),
        );
        start += hop;
    }
    envelope
}

/// Peak absolute amplitude of a slice.
#[must_use]
pub fn peak(samples: &[f64]) -> f64 {
    samples
        .iter()
        .fold(0.0_f64, |current, value| current.max(value.abs()))
}

/// Root-mean-square amplitude of a slice.
#[must_use]
pub fn rms(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|value| value * value).sum::<f64>() / samples.len() as f64).sqrt()
}

/// Number of samples from the start of a slice to its loudest sample.
#[must_use]
pub fn attack_samples(samples: &[f64]) -> usize {
    samples
        .iter()
        .enumerate()
        .fold((0_usize, 0.0_f64), |(index, best), (position, value)| {
            if value.abs() > best {
                (position, value.abs())
            } else {
                (index, best)
            }
        })
        .0
}

/// Maps an amplitude in `0..=1` to a MIDI velocity in `1..=127`.
///
/// The curve is perceptual rather than linear: velocity tracks the cube root
/// of amplitude, which spreads quiet notes over a usable range instead of
/// crushing them into the bottom few values.
#[must_use]
pub fn velocity_from_amplitude(amplitude: f64) -> u8 {
    let clamped = amplitude.clamp(0.0, 1.0);
    if clamped <= 0.0 {
        return 1;
    }
    let curved = clamped.cbrt();
    let velocity = (curved * 126.0).round() as i32 + 1;
    velocity.clamp(1, 127) as u8
}

/// The amplitude a velocity stands for, inverting
/// [`velocity_from_amplitude`].
#[must_use]
pub fn amplitude_from_velocity(velocity: u8) -> f64 {
    let normalized = f64::from(velocity.clamp(1, 127) - 1) / 126.0;
    normalized * normalized * normalized
}

/// Applies linear fades to the first and last `length` samples in place, which
/// keeps extracted samples from clicking at their edges.
pub fn apply_fades(samples: &mut [f64], length: usize) {
    let usable = length.min(samples.len() / 2);
    if usable == 0 {
        return;
    }
    let total = samples.len();
    for index in 0..usable {
        let gain = (index + 1) as f64 / (usable + 1) as f64;
        samples[index] *= gain;
        samples[total - 1 - index] *= gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_follow_a_decaying_tone() {
        let samples: Vec<f64> = (0..1000)
            .map(|index| {
                let time = f64::from(index) / 1000.0;
                (-time * 5.0).exp() * (time * 300.0).sin()
            })
            .collect();
        let rms_values = rms_envelope(&samples, 100, 50);
        let peaks = peak_envelope(&samples, 100, 50);

        assert_eq!(rms_values.len(), peaks.len());
        assert!(rms_values[0] > *rms_values.last().unwrap());
        assert!(peaks[0] > *peaks.last().unwrap());
        for (rms_value, peak_value) in rms_values.iter().zip(peaks.iter()) {
            assert!(rms_value <= peak_value);
        }
    }

    #[test]
    fn summary_measures_match_their_definitions() {
        let samples = [0.0, 0.5, -1.0, 0.25];

        assert_eq!(peak(&samples), 1.0);
        assert!((rms(&samples) - (1.312_5_f64 / 4.0).sqrt()).abs() < 1e-12);
        assert_eq!(attack_samples(&samples), 2);
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(peak(&[]), 0.0);
        assert_eq!(attack_samples(&[]), 0);
    }

    #[test]
    fn empty_and_degenerate_envelope_requests_return_nothing() {
        assert!(rms_envelope(&[], 10, 5).is_empty());
        assert!(rms_envelope(&[1.0], 0, 5).is_empty());
        assert!(rms_envelope(&[1.0], 10, 0).is_empty());
        assert!(peak_envelope(&[], 10, 5).is_empty());
        assert!(peak_envelope(&[1.0], 0, 5).is_empty());
        assert!(peak_envelope(&[1.0], 10, 0).is_empty());
    }

    #[test]
    fn velocity_and_amplitude_are_inverse_within_a_step() {
        assert_eq!(velocity_from_amplitude(0.0), 1);
        assert_eq!(velocity_from_amplitude(-1.0), 1);
        assert_eq!(velocity_from_amplitude(1.0), 127);
        assert_eq!(velocity_from_amplitude(2.0), 127);
        assert_eq!(amplitude_from_velocity(1), 0.0);
        assert!((amplitude_from_velocity(127) - 1.0).abs() < 1e-12);
        for velocity in 1..=127_u8 {
            let amplitude = amplitude_from_velocity(velocity);
            let restored = velocity_from_amplitude(amplitude);
            assert!(restored.abs_diff(velocity) <= 1, "{velocity} -> {restored}");
        }
    }

    #[test]
    fn fades_taper_the_edges_without_touching_the_middle() {
        let mut samples = vec![1.0; 10];
        apply_fades(&mut samples, 3);

        assert!(samples[0] < samples[1]);
        assert!(samples[1] < samples[2]);
        assert_eq!(samples[5], 1.0);
        assert!((samples[0] - samples[9]).abs() < 1e-15);

        let mut short = vec![1.0, 1.0];
        apply_fades(&mut short, 5);
        assert!(short[0] < 1.0);

        let mut single = vec![1.0];
        apply_fades(&mut single, 5);
        assert_eq!(single, vec![1.0]);
    }
}
