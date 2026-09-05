//! In-memory audio representation and container codecs.
//!
//! Everything in this crate works on [`Audio`]: planar `f64` channels scaled to
//! the nominal `[-1.0, 1.0]` range, plus the [`SampleFormat`] the samples came
//! from. Integer PCM formats are converted with power-of-two factors, which are
//! exact in binary floating point, so decoding and re-encoding an integer PCM
//! file is bit-exact and every stage downstream can reason about an exact
//! integer grid (see [`SampleFormat::quantum`]).

pub mod aiff;
pub mod wav;

use crate::error::{Error, Result};

/// Sample encodings understood by the readers and writers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SampleFormat {
    /// Unsigned 8-bit PCM, the WAV/AIFF byte encoding centred on 128.
    PcmU8,
    /// Signed 16-bit PCM, the CD-audio encoding.
    PcmI16,
    /// Signed 24-bit PCM packed into three bytes per sample.
    PcmI24,
    /// Signed 32-bit PCM.
    PcmI32,
    /// IEEE 754 single-precision floating point.
    F32,
    /// IEEE 754 double-precision floating point.
    F64,
}

impl SampleFormat {
    /// Bits stored per sample.
    #[must_use]
    pub const fn bits(self) -> u16 {
        match self {
            Self::PcmU8 => 8,
            Self::PcmI16 => 16,
            Self::PcmI24 => 24,
            Self::PcmI32 | Self::F32 => 32,
            Self::F64 => 64,
        }
    }

    /// Bytes stored per sample.
    #[must_use]
    pub const fn bytes(self) -> usize {
        self.bits() as usize / 8
    }

    /// Whether the format stores floating-point samples.
    #[must_use]
    pub const fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }

    /// Distance between two neighbouring representable values, for the integer
    /// formats that have a uniform grid.
    ///
    /// The value is always a negative power of two, so multiplying or dividing
    /// an `f64` by it never rounds.
    #[must_use]
    pub const fn quantum(self) -> Option<f64> {
        match self {
            Self::PcmU8 => Some(1.0 / 128.0),
            Self::PcmI16 => Some(1.0 / 32_768.0),
            Self::PcmI24 => Some(1.0 / 8_388_608.0),
            Self::PcmI32 => Some(1.0 / 2_147_483_648.0),
            Self::F32 | Self::F64 => None,
        }
    }

    /// Inclusive range of integer codes for the integer formats.
    #[must_use]
    pub const fn code_range(self) -> Option<(i64, i64)> {
        match self {
            Self::PcmU8 => Some((-128, 127)),
            Self::PcmI16 => Some((-32_768, 32_767)),
            Self::PcmI24 => Some((-8_388_608, 8_388_607)),
            Self::PcmI32 => Some((-2_147_483_648, 2_147_483_647)),
            Self::F32 | Self::F64 => None,
        }
    }

    /// Short lowercase name used by the CLI and by manifest files.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PcmU8 => "pcm_u8",
            Self::PcmI16 => "pcm_i16",
            Self::PcmI24 => "pcm_i24",
            Self::PcmI32 => "pcm_i32",
            Self::F32 => "f32",
            Self::F64 => "f64",
        }
    }

    /// Parses the name produced by [`SampleFormat::name`].
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "pcm_u8" | "u8" | "8" => Ok(Self::PcmU8),
            "pcm_i16" | "i16" | "16" => Ok(Self::PcmI16),
            "pcm_i24" | "i24" | "24" => Ok(Self::PcmI24),
            "pcm_i32" | "i32" | "32" => Ok(Self::PcmI32),
            "f32" | "float32" => Ok(Self::F32),
            "f64" | "float64" => Ok(Self::F64),
            other => Err(Error::InvalidArgument(format!(
                "unknown sample format `{other}`"
            ))),
        }
    }
}

/// Planar audio: one `Vec<f64>` per channel, all of the same length.
#[derive(Clone, Debug, PartialEq)]
pub struct Audio {
    sample_rate: u32,
    format: SampleFormat,
    channels: Vec<Vec<f64>>,
}

impl Audio {
    /// Creates a silent buffer.
    #[must_use]
    pub fn silence(sample_rate: u32, format: SampleFormat, channels: usize, frames: usize) -> Self {
        Self {
            sample_rate,
            format,
            channels: vec![vec![0.0; frames]; channels],
        }
    }

    /// Creates a buffer from planar channel data.
    pub fn from_channels(
        sample_rate: u32,
        format: SampleFormat,
        channels: Vec<Vec<f64>>,
    ) -> Result<Self> {
        if sample_rate == 0 {
            return Err(Error::InvalidArgument("sample rate must be positive".into()));
        }
        if channels.is_empty() {
            return Err(Error::InvalidArgument("at least one channel required".into()));
        }
        let frames = channels[0].len();
        if channels.iter().any(|channel| channel.len() != frames) {
            return Err(Error::Mismatch("channels have different lengths".into()));
        }
        Ok(Self {
            sample_rate,
            format,
            channels,
        })
    }

    /// Creates a mono buffer from a single channel.
    pub fn from_mono(sample_rate: u32, format: SampleFormat, samples: Vec<f64>) -> Result<Self> {
        Self::from_channels(sample_rate, format, vec![samples])
    }

    /// Sample rate in hertz.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Encoding the samples were decoded from, and will be written back as.
    #[must_use]
    pub const fn format(&self) -> SampleFormat {
        self.format
    }

    /// Replaces the target encoding without touching the samples.
    pub const fn set_format(&mut self, format: SampleFormat) {
        self.format = format;
    }

    /// Number of channels.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Number of frames (samples per channel).
    #[must_use]
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// Duration in seconds.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate)
    }

    /// Planar channel data.
    #[must_use]
    pub fn channels(&self) -> &[Vec<f64>] {
        &self.channels
    }

    /// Mutable planar channel data.
    pub fn channels_mut(&mut self) -> &mut [Vec<f64>] {
        &mut self.channels
    }

    /// A single channel.
    #[must_use]
    pub fn channel(&self, index: usize) -> &[f64] {
        &self.channels[index]
    }

    /// Average of all channels, the signal most analysis stages run on.
    #[must_use]
    pub fn mono_mix(&self) -> Vec<f64> {
        let frames = self.frames();
        let count = self.channel_count() as f64;
        (0..frames)
            .map(|frame| {
                self.channels
                    .iter()
                    .map(|channel| channel[frame])
                    .sum::<f64>()
                    / count
            })
            .collect()
    }

    /// Largest absolute sample value.
    #[must_use]
    pub fn peak(&self) -> f64 {
        self.channels
            .iter()
            .flat_map(|channel| channel.iter())
            .fold(0.0_f64, |peak, value| peak.max(value.abs()))
    }

    /// Root-mean-square level over every channel.
    #[must_use]
    pub fn rms(&self) -> f64 {
        let total = self.channels.iter().map(Vec::len).sum::<usize>();
        if total == 0 {
            return 0.0;
        }
        let energy: f64 = self
            .channels
            .iter()
            .flat_map(|channel| channel.iter())
            .map(|value| value * value)
            .sum();
        (energy / total as f64).sqrt()
    }

    /// Checks that another buffer has the same rate, channel count and length.
    pub fn ensure_compatible(&self, other: &Self) -> Result<()> {
        if self.sample_rate != other.sample_rate {
            return Err(Error::Mismatch(format!(
                "sample rate {} != {}",
                self.sample_rate, other.sample_rate
            )));
        }
        if self.channel_count() != other.channel_count() {
            return Err(Error::Mismatch(format!(
                "channel count {} != {}",
                self.channel_count(),
                other.channel_count()
            )));
        }
        if self.frames() != other.frames() {
            return Err(Error::Mismatch(format!(
                "frame count {} != {}",
                self.frames(),
                other.frames()
            )));
        }
        Ok(())
    }

    /// Adds `other` into this buffer sample by sample.
    pub fn add_assign(&mut self, other: &Self) -> Result<()> {
        self.ensure_compatible(other)?;
        for (target, source) in self.channels.iter_mut().zip(other.channels.iter()) {
            for (target_sample, source_sample) in target.iter_mut().zip(source.iter()) {
                *target_sample += *source_sample;
            }
        }
        Ok(())
    }

    /// Returns `self - other`.
    pub fn difference(&self, other: &Self) -> Result<Self> {
        self.ensure_compatible(other)?;
        let channels = self
            .channels
            .iter()
            .zip(other.channels.iter())
            .map(|(left, right)| {
                left.iter()
                    .zip(right.iter())
                    .map(|(a, b)| a - b)
                    .collect::<Vec<f64>>()
            })
            .collect();
        Self::from_channels(self.sample_rate, self.format, channels)
    }

    /// Largest absolute difference against another buffer.
    pub fn max_abs_difference(&self, other: &Self) -> Result<f64> {
        self.ensure_compatible(other)?;
        Ok(self
            .channels
            .iter()
            .zip(other.channels.iter())
            .flat_map(|(left, right)| left.iter().zip(right.iter()))
            .fold(0.0_f64, |worst, (a, b)| worst.max((a - b).abs())))
    }

    /// Extracts `length` frames starting at `start`, zero-padding past the end.
    #[must_use]
    pub fn segment(&self, start: usize, length: usize) -> Self {
        let frames = self.frames();
        let channels = self
            .channels
            .iter()
            .map(|channel| {
                (0..length)
                    .map(|offset| {
                        let index = start + offset;
                        if index < frames { channel[index] } else { 0.0 }
                    })
                    .collect()
            })
            .collect();
        Self {
            sample_rate: self.sample_rate,
            format: self.format,
            channels,
        }
    }

    /// Snaps every sample to the integer grid of the current format.
    ///
    /// Float formats have no grid, so the buffer is left untouched.
    pub fn quantize(&mut self) {
        let (Some(quantum), Some((low, high))) = (self.format.quantum(), self.format.code_range())
        else {
            return;
        };
        for channel in &mut self.channels {
            for sample in channel.iter_mut() {
                let code = round_half_away_from_zero(*sample / quantum).clamp(low, high);
                *sample = code as f64 * quantum;
            }
        }
    }

    /// Integer codes for the current format, or `None` for float formats.
    #[must_use]
    pub fn codes(&self) -> Option<Vec<Vec<i64>>> {
        let quantum = self.format.quantum()?;
        let (low, high) = self.format.code_range()?;
        Some(
            self.channels
                .iter()
                .map(|channel| {
                    channel
                        .iter()
                        .map(|sample| round_half_away_from_zero(sample / quantum).clamp(low, high))
                        .collect()
                })
                .collect(),
        )
    }

    /// Rebuilds a buffer from integer codes of `format`.
    pub fn from_codes(sample_rate: u32, format: SampleFormat, codes: &[Vec<i64>]) -> Result<Self> {
        let quantum = format
            .quantum()
            .ok_or_else(|| Error::InvalidArgument("float formats have no code grid".into()))?;
        let channels = codes
            .iter()
            .map(|channel| channel.iter().map(|code| *code as f64 * quantum).collect())
            .collect();
        Self::from_channels(sample_rate, format, channels)
    }
}

/// Rounds halfway cases away from zero, the rule every PCM encoder here uses.
///
/// `f64::round` already rounds half away from zero; the wrapper documents the
/// intent and keeps the conversion in a single place.
#[must_use]
pub fn round_half_away_from_zero(value: f64) -> i64 {
    value.round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_quanta_are_powers_of_two() {
        for format in [
            SampleFormat::PcmU8,
            SampleFormat::PcmI16,
            SampleFormat::PcmI24,
            SampleFormat::PcmI32,
        ] {
            let quantum = format.quantum().unwrap();
            assert_eq!(quantum, 2.0_f64.powi(-(i32::from(format.bits()) - 1)));
            assert!(format.code_range().is_some());
        }
        assert!(SampleFormat::F32.quantum().is_none());
        assert!(SampleFormat::F64.code_range().is_none());
    }

    #[test]
    fn format_names_round_trip() {
        for format in [
            SampleFormat::PcmU8,
            SampleFormat::PcmI16,
            SampleFormat::PcmI24,
            SampleFormat::PcmI32,
            SampleFormat::F32,
            SampleFormat::F64,
        ] {
            assert_eq!(SampleFormat::parse(format.name()).unwrap(), format);
            assert_eq!(format.bytes(), format.bits() as usize / 8);
        }
        assert!(SampleFormat::parse("mp3").is_err());
    }

    #[test]
    fn codes_round_trip_without_loss() {
        let format = SampleFormat::PcmI16;
        let quantum = format.quantum().unwrap();
        let samples: Vec<f64> = (-5..5).map(|code| f64::from(code) * quantum).collect();
        let audio = Audio::from_mono(48_000, format, samples.clone()).unwrap();

        let codes = audio.codes().unwrap();
        let restored = Audio::from_codes(48_000, format, &codes).unwrap();

        assert_eq!(restored.channel(0), samples.as_slice());
        assert_eq!(codes[0], (-5..5).collect::<Vec<i64>>());
    }

    #[test]
    fn quantize_snaps_to_the_grid_and_clamps() {
        let format = SampleFormat::PcmI16;
        let mut audio = Audio::from_mono(8_000, format, vec![0.000_01, 2.0, -2.0]).unwrap();
        audio.quantize();

        assert_eq!(audio.channel(0)[0], 0.0);
        assert_eq!(audio.channel(0)[1], 32_767.0 / 32_768.0);
        assert_eq!(audio.channel(0)[2], -1.0);
    }

    #[test]
    fn arithmetic_helpers_agree_with_manual_math() {
        let a = Audio::from_channels(8_000, SampleFormat::F32, vec![vec![1.0, 2.0], vec![3.0, 4.0]])
            .unwrap();
        let b = Audio::from_channels(8_000, SampleFormat::F32, vec![vec![0.5, 0.5], vec![1.0, 1.0]])
            .unwrap();

        let difference = a.difference(&b).unwrap();
        assert_eq!(difference.channel(0), &[0.5, 1.5]);
        assert_eq!(a.max_abs_difference(&b).unwrap(), 3.0);

        let mut sum = difference;
        sum.add_assign(&b).unwrap();
        assert_eq!(sum, a);

        assert_eq!(a.mono_mix(), vec![2.0, 3.0]);
        assert_eq!(a.peak(), 4.0);
        assert!((a.rms() - (30.0_f64 / 4.0).sqrt()).abs() < 1e-12);
        assert_eq!(a.duration_seconds(), 2.0 / 8_000.0);
    }

    #[test]
    fn incompatible_buffers_are_rejected() {
        let a = Audio::silence(8_000, SampleFormat::PcmI16, 1, 4);
        let wrong_rate = Audio::silence(16_000, SampleFormat::PcmI16, 1, 4);
        let wrong_channels = Audio::silence(8_000, SampleFormat::PcmI16, 2, 4);
        let wrong_length = Audio::silence(8_000, SampleFormat::PcmI16, 1, 8);

        assert!(a.ensure_compatible(&wrong_rate).is_err());
        assert!(a.ensure_compatible(&wrong_channels).is_err());
        assert!(a.ensure_compatible(&wrong_length).is_err());
        assert!(a.ensure_compatible(&a).is_ok());
    }

    #[test]
    fn segment_zero_pads_past_the_end() {
        let audio = Audio::from_mono(8_000, SampleFormat::PcmI16, vec![1.0, 2.0, 3.0]).unwrap();
        let segment = audio.segment(2, 3);

        assert_eq!(segment.channel(0), &[3.0, 0.0, 0.0]);
        assert_eq!(segment.frames(), 3);
    }

    #[test]
    fn construction_validates_its_inputs() {
        assert!(Audio::from_channels(0, SampleFormat::F32, vec![vec![0.0]]).is_err());
        assert!(Audio::from_channels(8_000, SampleFormat::F32, vec![]).is_err());
        assert!(
            Audio::from_channels(8_000, SampleFormat::F32, vec![vec![0.0], vec![0.0, 1.0]])
                .is_err()
        );
        assert!(Audio::from_codes(8_000, SampleFormat::F32, &[vec![1]]).is_err());
    }
}
