//! Short-time Fourier transform with an exactly invertible resynthesis.
//!
//! The analysis pads the signal so that every original sample sits in the
//! steady-state region of the overlap, then the inverse divides by the summed
//! squared window instead of assuming a constant. That makes
//! `inverse(forward(x)) == x` to within floating-point rounding for any window
//! that satisfies [`window::satisfies_overlap_add`], including at the very
//! first and last sample.

use crate::dsp::fft::{transform, Complex, Direction};
use crate::dsp::window::{self, WindowKind};
use crate::error::{Error, Result};
use crate::invalid_argument_error;

/// Analysis parameters shared by every spectral stage.
#[derive(Clone, Copy, Debug)]
pub struct StftOptions {
    /// Transform size in samples; must be a power of two.
    pub fft_size: usize,
    /// Distance between consecutive frames in samples.
    pub hop: usize,
    /// Window shape.
    pub window: WindowKind,
}

impl Default for StftOptions {
    fn default() -> Self {
        Self {
            fft_size: 2048,
            hop: 512,
            window: WindowKind::Hann,
        }
    }
}

impl StftOptions {
    /// Options with the given transform size and a quarter-length hop.
    #[must_use]
    pub const fn with_size(fft_size: usize) -> Self {
        Self {
            fft_size,
            hop: fft_size / 4,
            window: WindowKind::Hann,
        }
    }

    /// Validates the combination and builds the window.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidArgument`] when the transform size is not a
    /// power of two, when the hop is zero or larger than the transform, or
    /// when the window cannot be overlap-added at that hop.
    pub fn validate(self) -> Result<Vec<f64>> {
        if self.fft_size == 0 || !self.fft_size.is_power_of_two() {
            return Err(invalid_argument_error!(
                "FFT size {} must be a positive power of two",
                self.fft_size
            ));
        }
        if self.hop == 0 || self.hop > self.fft_size {
            return Err(invalid_argument_error!(
                "hop {} must be within 1..={}",
                self.hop,
                self.fft_size
            ));
        }
        let shape = self.window.build(self.fft_size);
        if !window::satisfies_overlap_add(&shape, self.hop) {
            return Err(invalid_argument_error!(
                "window {} cannot be overlap-added at hop {}",
                self.window.name(),
                self.hop
            ));
        }
        Ok(shape)
    }

    /// Number of unique frequency bins produced per frame.
    #[must_use]
    pub const fn bins(self) -> usize {
        self.fft_size / 2 + 1
    }
}

/// A complex spectrogram of one channel, plus everything needed to invert it.
#[derive(Clone, Debug)]
pub struct Spectrogram {
    options: StftOptions,
    sample_rate: u32,
    signal_length: usize,
    padding: usize,
    frames: Vec<Vec<Complex>>,
}

impl Spectrogram {
    /// Analysis options used to produce this spectrogram.
    #[must_use]
    pub const fn options(&self) -> StftOptions {
        self.options
    }

    /// Sample rate of the analyzed signal.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Length in samples of the signal this spectrogram was built from.
    #[must_use]
    pub const fn signal_length(&self) -> usize {
        self.signal_length
    }

    /// Number of frames.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Number of unique bins per frame.
    #[must_use]
    pub const fn bins(&self) -> usize {
        self.options.bins()
    }

    /// The frames as complex bins.
    #[must_use]
    pub fn frames(&self) -> &[Vec<Complex>] {
        &self.frames
    }

    /// Mutable access to the frames, for masking stages such as
    /// [`crate::dsp::hpss`].
    pub fn frames_mut(&mut self) -> &mut [Vec<Complex>] {
        &mut self.frames
    }

    /// Centre time in seconds of the given frame.
    #[must_use]
    pub fn frame_time(&self, frame: usize) -> f64 {
        let centre = (frame * self.options.hop) as f64 - self.padding as f64
            + self.options.fft_size as f64 / 2.0;
        centre / f64::from(self.sample_rate)
    }

    /// Centre frequency in hertz of the given bin.
    #[must_use]
    pub fn bin_frequency(&self, bin: usize) -> f64 {
        f64::from(self.sample_rate) * bin as f64 / self.options.fft_size as f64
    }

    /// Magnitudes of every frame, in the same layout as [`Self::frames`].
    #[must_use]
    pub fn magnitudes(&self) -> Vec<Vec<f64>> {
        self.frames
            .iter()
            .map(|frame| frame.iter().map(|bin| bin.magnitude()).collect())
            .collect()
    }

    /// Replaces every bin with the corresponding value scaled by `mask`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Mismatch`] when the mask does not have the same shape
    /// as the spectrogram.
    pub fn apply_mask(&mut self, mask: &[Vec<f64>]) -> Result<()> {
        if mask.len() != self.frames.len() {
            return Err(Error::Mismatch(format!(
                "mask has {} frames but the spectrogram has {}",
                mask.len(),
                self.frames.len()
            )));
        }
        for (frame, weights) in self.frames.iter_mut().zip(mask.iter()) {
            if weights.len() != frame.len() {
                return Err(Error::Mismatch(format!(
                    "mask frame has {} bins but the spectrogram has {}",
                    weights.len(),
                    frame.len()
                )));
            }
            for (bin, weight) in frame.iter_mut().zip(weights.iter()) {
                *bin = bin.scale(*weight);
            }
        }
        Ok(())
    }
}

/// Transforms one channel into a complex spectrogram.
///
/// # Errors
///
/// Propagates [`StftOptions::validate`] failures.
pub fn forward(samples: &[f64], sample_rate: u32, options: StftOptions) -> Result<Spectrogram> {
    let shape = options.validate()?;
    let padding = options.fft_size;
    let mut padded = vec![0.0; padding];
    padded.extend_from_slice(samples);
    padded.resize(padded.len() + padding, 0.0);
    while (padded.len() - options.fft_size) % options.hop != 0 {
        padded.push(0.0);
    }

    let frame_count = (padded.len() - options.fft_size) / options.hop + 1;
    let bins = options.bins();
    let mut frames = Vec::with_capacity(frame_count);
    let mut buffer = vec![Complex::default(); options.fft_size];
    for frame in 0..frame_count {
        let start = frame * options.hop;
        for (index, slot) in buffer.iter_mut().enumerate() {
            *slot = Complex::real(padded[start + index] * shape[index]);
        }
        transform(&mut buffer, Direction::Forward);
        frames.push(buffer[..bins].to_vec());
    }

    Ok(Spectrogram {
        options,
        sample_rate,
        signal_length: samples.len(),
        padding,
        frames,
    })
}

/// Rebuilds the time-domain channel from a spectrogram.
///
/// # Errors
///
/// Propagates [`StftOptions::validate`] failures.
pub fn inverse(spectrogram: &Spectrogram) -> Result<Vec<f64>> {
    let options = spectrogram.options;
    let shape = options.validate()?;
    let total = spectrogram
        .frames
        .len()
        .saturating_sub(1)
        .saturating_mul(options.hop)
        + options.fft_size;
    let mut accumulated = vec![0.0; total];
    let mut normalization = vec![0.0; total];
    let mut buffer = vec![Complex::default(); options.fft_size];

    for (frame, bins) in spectrogram.frames.iter().enumerate() {
        for (index, slot) in buffer.iter_mut().enumerate() {
            *slot = if index < bins.len() {
                bins[index]
            } else {
                bins[options.fft_size - index].conjugate()
            };
        }
        transform(&mut buffer, Direction::Inverse);
        let start = frame * options.hop;
        for index in 0..options.fft_size {
            accumulated[start + index] += buffer[index].re * shape[index];
            normalization[start + index] += shape[index] * shape[index];
        }
    }

    let mut output = Vec::with_capacity(spectrogram.signal_length);
    for index in 0..spectrogram.signal_length {
        let position = spectrogram.padding + index;
        let divisor = normalization.get(position).copied().unwrap_or(0.0);
        let value = accumulated.get(position).copied().unwrap_or(0.0);
        output.push(if divisor > 1e-12 {
            value / divisor
        } else {
            0.0
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramped_signal(length: usize) -> Vec<f64> {
        (0..length)
            .map(|index| {
                let time = index as f64 / 400.0;
                0.6 * (time * 31.0).sin() + 0.3 * (time * 7.0).cos() - 0.1
            })
            .collect()
    }

    #[test]
    fn options_reject_impossible_combinations() {
        assert!(StftOptions {
            fft_size: 100,
            hop: 25,
            window: WindowKind::Hann
        }
        .validate()
        .is_err());
        assert!(StftOptions {
            fft_size: 64,
            hop: 0,
            window: WindowKind::Hann
        }
        .validate()
        .is_err());
        assert!(StftOptions {
            fft_size: 64,
            hop: 65,
            window: WindowKind::Hann
        }
        .validate()
        .is_err());
        assert!(StftOptions::with_size(1024).validate().is_ok());
        assert_eq!(StftOptions::default().bins(), 1025);
        assert_eq!(StftOptions::with_size(256).hop, 64);
    }

    #[test]
    fn round_trip_restores_the_signal_including_its_edges() {
        let signal = ramped_signal(1000);
        let options = StftOptions {
            fft_size: 256,
            hop: 64,
            window: WindowKind::Hann,
        };
        let spectrogram = forward(&signal, 8000, options).unwrap();
        let restored = inverse(&spectrogram).unwrap();

        assert_eq!(restored.len(), signal.len());
        let worst = signal
            .iter()
            .zip(restored.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f64, f64::max);
        assert!(worst < 1e-10, "worst reconstruction error was {worst}");
    }

    #[test]
    fn round_trip_holds_for_every_window_and_several_hops() {
        let signal = ramped_signal(600);
        for window in [
            WindowKind::Rectangular,
            WindowKind::Hann,
            WindowKind::Hamming,
            WindowKind::Blackman,
        ] {
            for hop in [16, 32, 64] {
                let options = StftOptions {
                    fft_size: 128,
                    hop,
                    window,
                };
                let spectrogram = forward(&signal, 8000, options).unwrap();
                let restored = inverse(&spectrogram).unwrap();
                let worst = signal
                    .iter()
                    .zip(restored.iter())
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f64, f64::max);
                assert!(worst < 1e-10, "{} hop {hop}: {worst}", window.name());
            }
        }
    }

    #[test]
    fn a_tone_lands_on_the_expected_bin_and_time() {
        let sample_rate = 8000;
        let frequency = 500.0;
        let signal: Vec<f64> = (0..4000)
            .map(|index| {
                (std::f64::consts::TAU * frequency * index as f64 / f64::from(sample_rate)).sin()
            })
            .collect();
        let options = StftOptions {
            fft_size: 512,
            hop: 128,
            window: WindowKind::Hann,
        };
        let spectrogram = forward(&signal, sample_rate, options).unwrap();
        let magnitudes = spectrogram.magnitudes();
        let middle = &magnitudes[magnitudes.len() / 2];
        let peak = middle
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index)
            .unwrap();

        assert!((spectrogram.bin_frequency(peak) - frequency).abs() < 20.0);
        assert_eq!(spectrogram.bins(), 257);
        assert_eq!(spectrogram.signal_length(), signal.len());
        assert_eq!(spectrogram.sample_rate(), sample_rate);
        assert_eq!(spectrogram.options().fft_size, 512);
        let expected = (spectrogram.frame_count() / 2 * 128) as f64 - 512.0 + 256.0;
        assert!(
            (spectrogram.frame_time(spectrogram.frame_count() / 2)
                - expected / f64::from(sample_rate))
            .abs()
                < 1e-12
        );
    }

    #[test]
    fn masking_scales_every_bin_and_checks_its_shape() {
        let signal = ramped_signal(400);
        let options = StftOptions {
            fft_size: 128,
            hop: 32,
            window: WindowKind::Hann,
        };
        let mut spectrogram = forward(&signal, 8000, options).unwrap();
        let mask = vec![vec![0.5; spectrogram.bins()]; spectrogram.frame_count()];
        spectrogram.apply_mask(&mask).unwrap();
        let halved = inverse(&spectrogram).unwrap();

        for (original, scaled) in signal.iter().zip(halved.iter()) {
            assert!((original * 0.5 - scaled).abs() < 1e-10);
        }
        assert!(spectrogram.apply_mask(&[vec![1.0; 3]]).is_err());
        assert!(spectrogram
            .apply_mask(&vec![vec![1.0; 3]; spectrogram.frame_count()])
            .is_err());
        assert!(!spectrogram.frames_mut().is_empty());
    }

    #[test]
    fn an_empty_signal_produces_an_empty_reconstruction() {
        let spectrogram = forward(&[], 8000, StftOptions::with_size(64)).unwrap();
        assert!(inverse(&spectrogram).unwrap().is_empty());
        assert!(spectrogram.frame_count() > 0);
    }
}
