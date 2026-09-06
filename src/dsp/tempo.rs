//! Tempo and beat-phase estimation from an onset detection function.
//!
//! The estimator is the classic autocorrelation design. A detection function
//! that rises whenever energy appears repeats itself at the beat period, so the
//! autocorrelation of that function peaks at the lag corresponding to one beat.
//! Two corrections make the raw peak usable as a tempo:
//!
//! - a *harmonic sum* adds the autocorrelation at two, three and four times the
//!   candidate lag, so a candidate that also explains the bar-level repetition
//!   outscores the half-tempo candidate that only explains every other beat;
//! - a *log-normal prior* around a preferred tempo breaks the remaining
//!   octave ties the way a listener does, by preferring a tapping rate near
//!   two beats a second.
//!
//! Once the period is known the phase is found by sliding a pulse train across
//! the detection function and keeping the offset that collects the most energy.
//! The result is what [`crate::formats::session`] needs to place a clip on a
//! bar grid instead of assuming the default 120 BPM.

use crate::dsp::onset::{spectral_flux, subtract_baseline};
use crate::dsp::stft::Spectrogram;

/// Half-width in seconds of the kernel the detection function is widened with
/// before the autocorrelation, so that a beat period that falls between two
/// frames still correlates with itself.
pub const SMOOTHING_SECONDS: f64 = 0.02;

/// Sampling interval of the grid the onset times are rasterised onto.
pub const ONSET_GRID_SECONDS: f64 = 0.005;

/// Parameters of the estimator.
#[derive(Clone, Copy, Debug)]
pub struct TempoOptions {
    /// Slowest tempo considered, in beats per minute.
    pub minimum: f64,
    /// Fastest tempo considered, in beats per minute.
    pub maximum: f64,
    /// Tempo the prior is centred on, in beats per minute.
    pub preferred: f64,
    /// Width of the prior in octaves; larger values trust the signal more.
    pub spread: f64,
    /// How many multiples of the candidate lag the harmonic sum adds.
    pub harmonics: usize,
    /// How much support half the winning lag needs, as a fraction of the
    /// winner's own support, before the tempo is doubled. Onsets land on whole
    /// frames, which gives the slower grid a small unearned advantage; this is
    /// the margin that advantage has to overcome.
    pub octave_tolerance: f64,
}

impl Default for TempoOptions {
    fn default() -> Self {
        Self {
            minimum: 60.0,
            maximum: 200.0,
            preferred: 120.0,
            spread: 0.9,
            harmonics: 4,
            octave_tolerance: 0.7,
        }
    }
}

impl TempoOptions {
    /// The same options restricted to a tempo range.
    #[must_use]
    pub const fn between(mut self, minimum: f64, maximum: f64) -> Self {
        self.minimum = minimum;
        self.maximum = maximum;
        self
    }

    /// The same options with a different centre for the prior.
    #[must_use]
    pub const fn preferring(mut self, preferred: f64) -> Self {
        self.preferred = preferred;
        self
    }
}

/// An estimated tempo and the beat grid it implies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tempo {
    /// Tempo in beats per minute.
    pub bpm: f64,
    /// Length of one beat in seconds.
    pub period: f64,
    /// Time of the first beat, in seconds from the start of the signal.
    pub offset: f64,
    /// How far the winning candidate stood above the average candidate, in
    /// the range 0 (no periodicity at all) to 1 (a single perfect peak).
    pub confidence: f64,
}

impl Tempo {
    /// Position of a time in beats from the first beat.
    #[must_use]
    pub fn beats(&self, seconds: f64) -> f64 {
        if self.period <= 0.0 {
            return 0.0;
        }
        (seconds - self.offset) / self.period
    }

    /// Time of a beat, counting the first beat as zero.
    #[must_use]
    pub fn beat_time(&self, beat: f64) -> f64 {
        self.offset + beat * self.period
    }
}

/// Estimates the tempo of a spectrogram.
#[must_use]
pub fn detect(spectrogram: &Spectrogram, options: TempoOptions) -> Option<Tempo> {
    let flux = spectral_flux(spectrogram);
    let hop = spectrogram.frame_time(1) - spectrogram.frame_time(0);
    from_flux(&flux, hop, options)
}

/// A tempo candidate and how well the detection function supports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// Tempo of the candidate, in beats per minute.
    pub bpm: f64,
    /// Lag in frames the candidate was measured at.
    pub lag: usize,
    /// Harmonic sum of the autocorrelation at that lag, before the prior.
    pub support: f64,
    /// The same support after the prior has been applied.
    pub score: f64,
}

/// Scores every tempo the options allow, slowest candidate first.
///
/// [`from_flux`] keeps the best of these; the whole list is what a diagnostic
/// tool needs to show why one tempo won over its octave.
#[must_use]
pub fn candidates(detection: &[f64], hop: f64, options: TempoOptions) -> Vec<Candidate> {
    if hop <= 0.0 || !hop.is_finite() {
        return Vec::new();
    }
    let function = emphasise(detection, &options, hop);
    let Some((minimum_lag, maximum_lag)) = lag_range(function.len(), hop, &options) else {
        return Vec::new();
    };
    let correlation = autocorrelation(&function, maximum_lag * options.harmonics.max(1));
    (minimum_lag..=maximum_lag)
        .map(|lag| {
            let support = support_at(&correlation, lag as f64, options.harmonics);
            Candidate {
                bpm: 60.0 / (lag as f64 * hop),
                lag,
                support,
                score: support * prior(lag as f64, hop, &options),
            }
        })
        .collect()
}

/// Estimates the tempo of a detection function sampled every `hop` seconds.
#[must_use]
pub fn from_flux(detection: &[f64], hop: f64, options: TempoOptions) -> Option<Tempo> {
    let scored = candidates(detection, hop, options);
    let best = scored.iter().copied().reduce(|best, candidate| {
        if candidate.score > best.score {
            candidate
        } else {
            best
        }
    })?;
    if best.score <= 0.0 {
        return None;
    }
    let function = emphasise(detection, &options, hop);
    let correlation = autocorrelation(&function, best.lag * options.harmonics.max(1));
    let lag = double_while_the_beat_divides(&correlation, best.lag as f64, hop, &options);
    let period_frames = interpolate(&correlation, lag.round() as usize);
    let period = period_frames * hop;
    let average = scored.iter().map(|candidate| candidate.score).sum::<f64>() / scored.len() as f64;
    let confidence = (1.0 - average / best.score).clamp(0.0, 1.0);
    let offset = phase(&function, period_frames) * hop;
    Some(Tempo {
        bpm: 60.0 / period,
        period,
        offset,
        confidence,
    })
}

/// Estimates the tempo of a list of onset times, in seconds.
#[must_use]
pub fn from_onsets(times: &[f64], options: TempoOptions) -> Option<Tempo> {
    let last = times.iter().copied().fold(f64::NAN, f64::max);
    if !last.is_finite() {
        return None;
    }
    let length = (last / ONSET_GRID_SECONDS).ceil() as usize + 2;
    let mut function = vec![0.0; length];
    for &time in times {
        if time < 0.0 {
            continue;
        }
        // Split each onset between the two neighbouring grid points so that a
        // time between them does not jump a whole grid step.
        let position = time / ONSET_GRID_SECONDS;
        let index = position as usize;
        let fraction = position - index as f64;
        if let Some(slot) = function.get_mut(index) {
            *slot += 1.0 - fraction;
        }
        if let Some(slot) = function.get_mut(index + 1) {
            *slot += fraction;
        }
    }
    from_flux(&function, ONSET_GRID_SECONDS, options)
}

/// The detection function with its baseline removed, its peaks widened and
/// its mean subtracted, which is what the autocorrelation needs to see zero
/// where nothing happens.
///
/// The widening matters more than it looks: a beat period is rarely a whole
/// number of frames, so a needle-thin peak only correlates with itself on the
/// multiples of the lag that happen to land back on the frame grid, and the
/// estimator would answer with whichever multiple of the true tempo is best
/// aligned to the hop size.
fn emphasise(detection: &[f64], options: &TempoOptions, hop: f64) -> Vec<f64> {
    let span = ((60.0 / options.minimum.max(1.0)) / hop).ceil() as usize;
    let rectified = subtract_baseline(detection, span.max(3));
    let widened = smooth(&rectified, (SMOOTHING_SECONDS / hop).round() as usize);
    let mean = if widened.is_empty() {
        0.0
    } else {
        widened.iter().sum::<f64>() / widened.len() as f64
    };
    widened.iter().map(|value| value - mean).collect()
}

/// Convolves a function with a triangular kernel of the given half-width.
fn smooth(function: &[f64], half_width: usize) -> Vec<f64> {
    if half_width == 0 {
        return function.to_vec();
    }
    let weights: Vec<f64> = (0..=2 * half_width)
        .map(|index| (half_width + 1) as f64 - (index as f64 - half_width as f64).abs())
        .collect();
    let total: f64 = weights.iter().sum();
    (0..function.len())
        .map(|index| {
            let mut sum = 0.0;
            for (step, weight) in weights.iter().enumerate() {
                let position = index + step;
                if position >= half_width {
                    if let Some(value) = function.get(position - half_width) {
                        sum += value * weight;
                    }
                }
            }
            sum / total
        })
        .collect()
}

/// The inclusive range of lags the tempo range allows, in frames.
fn lag_range(length: usize, hop: f64, options: &TempoOptions) -> Option<(usize, usize)> {
    let slowest = options.minimum.min(options.maximum).max(1.0);
    let fastest = options.maximum.max(options.minimum);
    let minimum = ((60.0 / fastest) / hop).round().max(1.0) as usize;
    let maximum = ((60.0 / slowest) / hop).round().max(1.0) as usize;
    // Two full periods have to fit, otherwise the lag is measured from a single
    // repetition and any number would do.
    let usable = maximum.min(length / 2);
    (minimum <= usable).then_some((minimum, usable))
}

/// Unbiased autocorrelation up to `maximum` lags.
fn autocorrelation(function: &[f64], maximum: usize) -> Vec<f64> {
    let limit = maximum.min(function.len().saturating_sub(1));
    let mut result = Vec::with_capacity(limit + 1);
    for lag in 0..=limit {
        let overlap = function.len() - lag;
        let sum: f64 = function
            .iter()
            .zip(function.iter().skip(lag))
            .map(|(current, later)| current * later)
            .sum();
        result.push(sum / overlap as f64);
    }
    result
}

/// The autocorrelation at a lag plus its attenuated multiples, so that a
/// candidate explaining the bar as well as the beat wins the octave.
///
/// The lag is a real number of frames: a beat period rarely divides the hop
/// size, and the octave correction asks about half a lag.
fn support_at(correlation: &[f64], lag: f64, harmonics: usize) -> f64 {
    if lag < 1.0 {
        return 0.0;
    }
    let mut sum = 0.0;
    for harmonic in 1..=harmonics.max(1) {
        let position = lag * harmonic as f64;
        if position + 1.0 >= correlation.len() as f64 {
            break;
        }
        sum += sample_at(correlation, position).max(0.0) / harmonic as f64;
    }
    sum
}

/// A log-normal preference for tempi near the preferred one.
fn prior(lag: f64, hop: f64, options: &TempoOptions) -> f64 {
    let bpm = 60.0 / (lag * hop);
    let distance = (bpm / options.preferred).log2() / options.spread.max(1e-6);
    (-0.5 * distance * distance).exp()
}

/// Halves the lag while the midpoint between two beats carries a beat of its
/// own, which is how a tempo heard at half speed is brought back.
fn double_while_the_beat_divides(
    correlation: &[f64],
    lag: f64,
    hop: f64,
    options: &TempoOptions,
) -> f64 {
    let fastest = options.maximum.max(options.minimum);
    let shortest = (60.0 / fastest) / hop;
    let mut current = lag;
    loop {
        let half = current / 2.0;
        if half < shortest - 0.5 || half < 1.0 {
            return current;
        }
        let support = support_at(correlation, current, options.harmonics);
        let divided = support_at(correlation, half, options.harmonics);
        if divided < support * options.octave_tolerance {
            return current;
        }
        current = half;
    }
}

/// Sub-frame peak position by fitting a parabola through three samples.
fn interpolate(correlation: &[f64], lag: usize) -> f64 {
    let (Some(&before), Some(&at), Some(&after)) = (
        correlation.get(lag.wrapping_sub(1)),
        correlation.get(lag),
        correlation.get(lag + 1),
    ) else {
        return lag as f64;
    };
    let denominator = before - 2.0 * at + after;
    if denominator.abs() < f64::EPSILON {
        return lag as f64;
    }
    let shift = 0.5 * (before - after) / denominator;
    if shift.abs() > 1.0 {
        return lag as f64;
    }
    lag as f64 + shift
}

/// The offset in frames of the pulse train that collects the most energy.
fn phase(function: &[f64], period: f64) -> f64 {
    if period <= 0.0 || function.is_empty() {
        return 0.0;
    }
    let steps = period.ceil() as usize;
    let mut best = (0.0, f64::NEG_INFINITY);
    for step in 0..steps.max(1) {
        let start = step as f64;
        let mut sum = 0.0;
        let mut position = start;
        while (position as usize) < function.len() {
            sum += sample_at(function, position);
            position += period;
        }
        if sum > best.1 {
            best = (start, sum);
        }
    }
    best.0
}

/// Linear interpolation into a detection function.
fn sample_at(function: &[f64], position: f64) -> f64 {
    let index = position as usize;
    let fraction = position - index as f64;
    let current = function.get(index).copied().unwrap_or(0.0);
    let next = function.get(index + 1).copied().unwrap_or(0.0);
    current * (1.0 - fraction) + next * fraction
}

#[cfg(test)]
mod tests {
    use super::{candidates, detect, from_flux, from_onsets, Tempo, TempoOptions};
    use crate::dsp::stft::{self, StftOptions};

    /// A detection function with a decaying spike every `period` seconds.
    fn clicks(bpm: f64, seconds: f64, hop: f64) -> Vec<f64> {
        let period = 60.0 / bpm;
        let length = (seconds / hop) as usize;
        let mut function = vec![0.0; length];
        for click in 0..(seconds / period).ceil() as usize {
            let index = ((click as f64 * period) / hop) as usize;
            for (step, weight) in [1.0, 0.5, 0.25].into_iter().enumerate() {
                if let Some(slot) = function.get_mut(index + step) {
                    *slot += weight;
                }
            }
        }
        function
    }

    #[test]
    fn a_steady_click_train_reports_its_own_tempo() {
        let function = clicks(120.0, 12.0, 0.01);
        let tempo = from_flux(&function, 0.01, TempoOptions::default()).unwrap();
        assert!(
            (tempo.bpm - 120.0).abs() < 1.0,
            "expected 120 BPM, found {}",
            tempo.bpm
        );
        assert!(tempo.confidence > 0.5, "confidence {}", tempo.confidence);
    }

    #[test]
    fn a_tempo_away_from_the_prior_is_still_found() {
        for bpm in [75.0, 90.0, 100.0, 140.0, 160.0] {
            let function = clicks(bpm, 20.0, 0.01);
            let tempo = from_flux(&function, 0.01, TempoOptions::default()).unwrap();
            assert!(
                (tempo.bpm - bpm).abs() < 2.0,
                "expected {bpm} BPM, found {}",
                tempo.bpm
            );
        }
    }

    #[test]
    fn the_first_beat_is_where_the_clicks_start() {
        let hop = 0.01;
        let mut function = vec![0.0; 40];
        function.extend(clicks(120.0, 12.0, hop));
        let tempo = from_flux(&function, hop, TempoOptions::default()).unwrap();
        assert!(
            (tempo.offset - 0.4).abs() < 0.02,
            "expected the grid to start at 0.4 s, found {}",
            tempo.offset
        );
    }

    #[test]
    fn onset_times_are_enough_to_find_the_tempo() {
        let period = 60.0 / 96.0;
        let times: Vec<f64> = (0..40)
            .map(|beat| 0.25 + f64::from(beat) * period)
            .collect();
        let tempo = from_onsets(&times, TempoOptions::default()).unwrap();
        assert!(
            (tempo.bpm - 96.0).abs() < 1.5,
            "expected 96 BPM, found {}",
            tempo.bpm
        );
        assert!(
            (tempo.offset - 0.25).abs() < 0.05,
            "expected the first beat at 0.25 s, found {}",
            tempo.offset
        );
    }

    #[test]
    fn a_tempo_outside_the_range_is_folded_into_it() {
        // 240 BPM is above the default maximum, so the estimator has to report
        // the half-tempo grid that shares every other beat with the signal.
        let function = clicks(240.0, 20.0, 0.01);
        let tempo = from_flux(&function, 0.01, TempoOptions::default()).unwrap();
        assert!(
            (tempo.bpm - 120.0).abs() < 2.0,
            "expected the 120 BPM grid, found {}",
            tempo.bpm
        );
    }

    #[test]
    fn silence_and_impossible_input_report_nothing() {
        assert!(from_flux(&[], 0.01, TempoOptions::default()).is_none());
        assert!(from_flux(&[0.0; 2000], 0.0, TempoOptions::default()).is_none());
        assert!(from_flux(&[1.0; 10], 0.01, TempoOptions::default()).is_none());
        assert!(from_onsets(&[], TempoOptions::default()).is_none());
    }

    #[test]
    fn the_grid_converts_between_seconds_and_beats() {
        let tempo = Tempo {
            bpm: 120.0,
            period: 0.5,
            offset: 0.25,
            confidence: 1.0,
        };
        assert!((tempo.beats(1.25) - 2.0).abs() < 1e-12);
        assert!((tempo.beat_time(2.0) - 1.25).abs() < 1e-12);
    }

    #[test]
    fn every_candidate_carries_the_tempo_it_stands_for() {
        let function = clicks(100.0, 15.0, 0.01);
        let scored = candidates(&function, 0.01, TempoOptions::default());
        assert!(!scored.is_empty());
        for candidate in &scored {
            assert!((60.0..=200.0).contains(&candidate.bpm), "{candidate:?}");
            assert!((candidate.bpm - 60.0 / (candidate.lag as f64 * 0.01)).abs() < 1e-9);
            assert!(candidate.score <= candidate.support + 1e-12);
        }
        let best = scored
            .iter()
            .copied()
            .reduce(|best, next| if next.score > best.score { next } else { best })
            .unwrap();
        assert!((best.bpm - 100.0).abs() < 2.0, "{best:?}");
    }

    #[test]
    fn a_recording_of_repeated_hits_reports_its_tempo() {
        let sample_rate = 22_050;
        let period = 0.5;
        let length = (f64::from(sample_rate) * 10.0) as usize;
        let mut samples = vec![0.0; length];
        for hit in 0..(10.0 / period) as usize {
            let first = (hit as f64 * period * f64::from(sample_rate)) as usize;
            for index in 0..sample_rate as usize / 4 {
                let time = index as f64 / f64::from(sample_rate);
                let decay = (-14.0 * time).exp();
                if let Some(slot) = samples.get_mut(first + index) {
                    *slot += 0.5 * decay * (std::f64::consts::TAU * 220.0 * time).sin();
                }
            }
        }
        let spectrogram = stft::forward(&samples, sample_rate, StftOptions::default()).unwrap();
        let tempo = detect(&spectrogram, TempoOptions::default()).unwrap();
        assert!(
            (tempo.bpm - 120.0).abs() < 3.0,
            "expected 120 BPM, found {}",
            tempo.bpm
        );
    }

    #[test]
    fn the_range_can_be_narrowed_to_what_a_style_allows() {
        let function = clicks(160.0, 20.0, 0.01);
        let options = TempoOptions::default()
            .between(60.0, 100.0)
            .preferring(80.0);
        let tempo = from_flux(&function, 0.01, options).unwrap();
        assert!(
            (tempo.bpm - 80.0).abs() < 2.0,
            "expected the 80 BPM grid, found {}",
            tempo.bpm
        );
    }
}
