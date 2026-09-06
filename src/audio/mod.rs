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
use std::path::Path;

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
    /// Every format, narrowest first, so a search for the smallest exact
    /// encoding can simply take the first that fits.
    pub const ALL: [Self; 6] = [
        Self::PcmU8,
        Self::PcmI16,
        Self::PcmI24,
        Self::PcmI32,
        Self::F32,
        Self::F64,
    ];

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
            return Err(Error::InvalidArgument(
                "sample rate must be positive".into(),
            ));
        }
        if channels.is_empty() {
            return Err(Error::InvalidArgument(
                "at least one channel required".into(),
            ));
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
                        if index < frames {
                            channel[index]
                        } else {
                            0.0
                        }
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

    /// Whether writing the buffer in its own format and reading it back yields
    /// exactly the same numbers.
    ///
    /// A buffer this crate decoded always says yes: decoding puts every sample
    /// on the grid of its format. A buffer that analysis produced may say no,
    /// and then it has to be stored as `f64` to stay lossless.
    #[must_use]
    pub fn is_exact_in_format(&self) -> bool {
        self.is_exact_in(self.format)
    }

    /// Whether every sample would survive being written in `format` and read
    /// back, unchanged.
    #[must_use]
    pub fn is_exact_in(&self, format: SampleFormat) -> bool {
        let mut samples = self.channels.iter().flat_map(|channel| channel.iter());
        match (format.quantum(), format.code_range()) {
            (Some(quantum), Some((low, high))) => samples.all(|sample| {
                let code = round_half_away_from_zero(sample / quantum);
                (low..=high).contains(&code) && code as f64 * quantum == *sample
            }),
            _ => {
                format == SampleFormat::F64
                    || samples.all(|sample| f64::from(*sample as f32) == *sample)
            }
        }
    }

    /// The smallest format that stores every sample of this buffer exactly and
    /// resolves at least as finely as `floor`.
    ///
    /// Reaching straight for `f64` whenever a buffer leaves its own grid is
    /// safe and usually four times larger than it needs to be. A residual is a
    /// difference of grid values, so it lands back on that same grid and 16
    /// bits still hold it; only arithmetic that lands *between* grid points
    /// buys anything with the extra width.
    ///
    /// Note that integer quanta are tied to full scale, so a value past ±1.0
    /// is off every integer grid no matter how wide, and only a float format
    /// can hold it.
    ///
    /// `floor` keeps the answer honest about where the audio came from. An
    /// all-zero correction is exact in 8 bits, but tagging it that way would
    /// arm [`Audio::quantize`] to crush anything later mixed into it, so a
    /// buffer derived from a 16-bit recording stays at 16 bits or wider.
    #[must_use]
    pub fn narrowest_exact_format(&self, floor: SampleFormat) -> SampleFormat {
        SampleFormat::ALL
            .iter()
            .copied()
            .filter(|format| format.bits() >= floor.bits())
            .find(|format| self.is_exact_in(*format))
            .unwrap_or(SampleFormat::F64)
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

/// A container an audio file can be stored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Container {
    /// Microsoft RIFF/WAVE.
    Wav,
    /// Apple AIFF, including the uncompressed AIFF-C layout.
    Aiff,
}

impl Container {
    /// Every container the crate reads and writes.
    pub const ALL: [Self; 2] = [Self::Wav, Self::Aiff];

    /// The extension the container is conventionally stored under.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Aiff => "aiff",
        }
    }

    /// The container an extension or name stands for.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().trim_start_matches('.').to_ascii_lowercase();
        match name.as_str() {
            "wav" | "wave" | "riff" => Some(Self::Wav),
            "aif" | "aiff" | "aifc" => Some(Self::Aiff),
            _ => None,
        }
    }

    /// The container a path names, by its extension.
    #[must_use]
    pub fn of_path(path: impl AsRef<Path>) -> Option<Self> {
        path.as_ref()
            .extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::parse)
    }

    /// The container a file's first bytes announce.
    ///
    /// Extensions lie; the four-character chunk identifiers at the head of a
    /// RIFF or IFF file do not.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Option<Self> {
        match bytes.get(..4)? {
            b"RIFF" | b"RIFX" => Some(Self::Wav),
            b"FORM" => Some(Self::Aiff),
            _ => None,
        }
    }
}

/// Decodes audio, choosing the codec by what the bytes say.
pub fn decode(bytes: &[u8]) -> Result<Audio> {
    match Container::of_bytes(bytes) {
        Some(Container::Wav) => wav::decode(bytes),
        Some(Container::Aiff) => aiff::decode(bytes),
        None => Err(crate::format_error!(
            "unrecognised audio container; expected a RIFF/WAVE or IFF/AIFF header"
        )),
    }
}

/// Encodes audio into a container.
pub fn encode(audio: &Audio, container: Container) -> Result<Vec<u8>> {
    match container {
        Container::Wav => wav::encode(audio),
        Container::Aiff => aiff::encode(audio),
    }
}

/// Reads an audio file, whatever container it is in.
pub fn read_file(path: impl AsRef<Path>) -> Result<Audio> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|error| crate::error::io_at(path, &error))?;
    match Container::of_bytes(&bytes).or_else(|| Container::of_path(path)) {
        Some(Container::Wav) => wav::decode(&bytes),
        Some(Container::Aiff) => aiff::decode(&bytes),
        None => Err(crate::format_error!(
            "{} is not a container this build reads; expected WAV or AIFF",
            path.display()
        )),
    }
}

/// Writes an audio file in the container its extension names.
pub fn write_file(path: impl AsRef<Path>, audio: &Audio) -> Result<()> {
    let path = path.as_ref();
    let container = Container::of_path(path).ok_or_else(|| {
        crate::invalid_argument_error!(
            "{} has no extension this build writes; expected .wav or .aiff",
            path.display()
        )
    })?;
    match container {
        Container::Wav => wav::write_file(path, audio),
        Container::Aiff => aiff::write_file(path, audio),
    }
    .map_err(|error| match error {
        crate::error::Error::Io(error) => crate::error::io_at(path, &error),
        other => other,
    })
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
    fn a_container_is_recognised_by_its_bytes_before_its_name() {
        assert_eq!(Container::of_bytes(b"RIFF....WAVE"), Some(Container::Wav));
        assert_eq!(Container::of_bytes(b"FORM....AIFF"), Some(Container::Aiff));
        assert_eq!(Container::of_bytes(b"OggS"), None);
        assert_eq!(Container::of_bytes(b"RIF"), None);
        assert_eq!(Container::of_path("song.AIF"), Some(Container::Aiff));
        assert_eq!(Container::of_path("song.flac"), None);
        assert_eq!(Container::parse(".Wav"), Some(Container::Wav));
    }

    #[test]
    fn every_container_round_trips_through_the_dispatching_codecs() {
        let audio = Audio::from_channels(
            8_000,
            SampleFormat::PcmI16,
            vec![vec![0.5, -0.25, 0.0, 0.125]],
        )
        .unwrap();
        for container in Container::ALL {
            let bytes = encode(&audio, container).unwrap();
            assert_eq!(Container::of_bytes(&bytes), Some(container));
            let read = decode(&bytes).unwrap();
            assert_eq!(read.channels(), audio.channels());
            assert_eq!(read.sample_rate(), audio.sample_rate());
        }
        assert!(decode(b"not audio at all").is_err());
    }

    #[test]
    fn files_are_read_and_written_by_the_container_their_name_asks_for() {
        let audio =
            Audio::from_channels(8_000, SampleFormat::PcmI16, vec![vec![0.5, -0.5]]).unwrap();
        let mut root = std::env::temp_dir();
        root.push(format!(
            "audio-decomposer-container-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        for container in Container::ALL {
            let path = root.join(format!("song.{}", container.extension()));
            write_file(&path, &audio).unwrap();
            let read = read_file(&path).unwrap();
            assert_eq!(read.channels(), audio.channels());
        }

        // An extension nobody writes is refused instead of guessed at.
        assert!(write_file(root.join("song.flac"), &audio).is_err());

        // A file named wrongly is still read by what its bytes say.
        let misnamed = root.join("actually.aiff");
        std::fs::write(&misnamed, wav::encode(&audio).unwrap()).unwrap();
        assert_eq!(read_file(&misnamed).unwrap().channels(), audio.channels());

        let _ = std::fs::remove_dir_all(&root);
    }

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
        let a = Audio::from_channels(
            8_000,
            SampleFormat::F32,
            vec![vec![1.0, 2.0], vec![3.0, 4.0]],
        )
        .unwrap();
        let b = Audio::from_channels(
            8_000,
            SampleFormat::F32,
            vec![vec![0.5, 0.5], vec![1.0, 1.0]],
        )
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

    #[test]
    fn exactness_in_a_format_is_reported_honestly() {
        let grid = Audio::from_mono(8_000, SampleFormat::PcmI16, vec![0.5, -0.25]).unwrap();
        assert!(grid.is_exact_in_format());

        let off_grid =
            Audio::from_mono(8_000, SampleFormat::PcmI16, vec![0.5 + 1.0 / 65_536.0]).unwrap();
        assert!(!off_grid.is_exact_in_format());

        let clipping = Audio::from_mono(8_000, SampleFormat::PcmI16, vec![2.0]).unwrap();
        assert!(!clipping.is_exact_in_format());

        let double = Audio::from_mono(8_000, SampleFormat::F64, vec![0.1]).unwrap();
        assert!(double.is_exact_in_format());

        let single = Audio::from_mono(8_000, SampleFormat::F32, vec![0.5]).unwrap();
        assert!(single.is_exact_in_format());

        let too_precise = Audio::from_mono(8_000, SampleFormat::F32, vec![0.1]).unwrap();
        assert!(!too_precise.is_exact_in_format());
    }

    #[test]
    fn the_narrowest_exact_format_is_preferred_over_reaching_for_double() {
        // Between two 16-bit grid points, but exactly on a 24-bit one: `f64`
        // would be four times the bytes for no extra fidelity.
        let finer = Audio::from_mono(8_000, SampleFormat::PcmI16, vec![1.0 / 8_388_608.0]).unwrap();
        assert!(!finer.is_exact_in_format());
        assert_eq!(
            finer.narrowest_exact_format(SampleFormat::PcmI16),
            SampleFormat::PcmI24
        );
    }

    #[test]
    fn a_value_past_full_scale_leaves_every_integer_grid() {
        // Integer quanta are tied to full scale, so widening never brings a
        // sample louder than ±1.0 back onto the grid: only a float format can
        // hold it, and single precision is enough for a value this simple.
        let loud = Audio::from_mono(8_000, SampleFormat::PcmI16, vec![1.5]).unwrap();
        assert_eq!(
            loud.narrowest_exact_format(SampleFormat::PcmU8),
            SampleFormat::F32
        );

        let awkward = Audio::from_mono(8_000, SampleFormat::PcmI16, vec![1.1]).unwrap();
        assert_eq!(
            awkward.narrowest_exact_format(SampleFormat::PcmU8),
            SampleFormat::F64
        );
    }

    #[test]
    fn the_floor_keeps_a_derived_buffer_on_the_grid_it_came_from() {
        // Silence fits in 8 bits, but a correction for a 16-bit recording that
        // claimed to be 8-bit would quantize anything later mixed into it.
        let silence = Audio::silence(8_000, SampleFormat::PcmI16, 1, 8);
        assert_eq!(
            silence.narrowest_exact_format(SampleFormat::PcmU8),
            SampleFormat::PcmU8
        );
        assert_eq!(
            silence.narrowest_exact_format(SampleFormat::PcmI16),
            SampleFormat::PcmI16
        );
    }

    #[test]
    fn a_buffer_no_integer_grid_can_hold_still_falls_back_to_double() {
        let precise = Audio::from_mono(8_000, SampleFormat::F32, vec![0.1]).unwrap();
        assert_eq!(
            precise.narrowest_exact_format(SampleFormat::PcmU8),
            SampleFormat::F64
        );
    }
}
