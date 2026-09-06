//! What a decomposition is: stems, a deduplicated sample bank, the placements
//! that put the bank back on the timeline, the notes that describe it
//! musically, and the residual that makes the sum exact.
//!
//! The whole design follows one identity. For every channel and every frame,
//!
//! ```text
//! original = render(bank, placements) + residual (+ correction)
//! ```
//!
//! where [`Decomposition::render`] is a single deterministic function used both
//! when the decomposition is produced and when it is played back. Nothing about
//! the reconstruction depends on the analysis being *right*: a bad match simply
//! leaves more signal in the residual. That is what lets the round trip be
//! exact while the musical interpretation stays approximate.

use crate::audio::{Audio, SampleFormat};
use crate::error::Result;

/// Denominator of the fixed-point gain, chosen as a power of two so that a gain
/// is an exact `f64` and survives being written to text and read back.
pub const GAIN_SCALE: i64 = 1 << 16;

/// A gain factor stored as an exact binary rational.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Gain(i64);

impl Gain {
    /// The unit gain.
    pub const UNIT: Self = Self(GAIN_SCALE);

    /// A gain from its numerator over [`GAIN_SCALE`].
    #[must_use]
    pub const fn from_numerator(numerator: i64) -> Self {
        Self(numerator)
    }

    /// The nearest representable gain to a factor.
    #[must_use]
    pub fn from_factor(factor: f64) -> Self {
        Self(crate::audio::round_half_away_from_zero(
            factor * GAIN_SCALE as f64,
        ))
    }

    /// The numerator, which is what manifests store.
    #[must_use]
    pub const fn numerator(self) -> i64 {
        self.0
    }

    /// The factor as an exact `f64`.
    #[must_use]
    pub fn factor(self) -> f64 {
        self.0 as f64 / GAIN_SCALE as f64
    }

    /// Whether the gain is exactly zero.
    #[must_use]
    pub const fn is_silent(self) -> bool {
        self.0 == 0
    }
}

impl Default for Gain {
    fn default() -> Self {
        Self::UNIT
    }
}

/// Where a decomposition came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    /// Name of the recording, used for exported file names.
    pub name: String,
    /// Sample rate of every buffer in the decomposition.
    pub sample_rate: u32,
    /// Sample format the recording was decoded from.
    pub format: SampleFormat,
    /// Number of channels.
    pub channels: usize,
    /// Number of frames per channel.
    pub frames: usize,
}

impl SourceInfo {
    /// The description of an existing buffer.
    #[must_use]
    pub fn of(name: &str, audio: &Audio) -> Self {
        Self {
            name: name.to_string(),
            sample_rate: audio.sample_rate(),
            format: audio.format(),
            channels: audio.channel_count(),
            frames: audio.frames(),
        }
    }

    /// Length of the recording in seconds.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frames as f64 / f64::from(self.sample_rate)
    }

    /// An empty buffer with this shape.
    #[must_use]
    pub fn silence(&self) -> Audio {
        Audio::silence(self.sample_rate, self.format, self.channels, self.frames)
    }
}

/// A named part of the recording.
///
/// Every stem of a decomposition has the same shape as the source, and the
/// stems sum back to it sample for sample.
#[derive(Clone, Debug, PartialEq)]
pub struct Stem {
    /// Name used for the exported file and the manifest.
    pub name: String,
    /// The audio of this part.
    pub audio: Audio,
}

impl Stem {
    /// A stem from its parts.
    #[must_use]
    pub const fn new(name: String, audio: Audio) -> Self {
        Self { name, audio }
    }
}

/// One distinct waveform in the deduplicated bank.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// Position of the sample in the bank, and its address in exports.
    pub id: usize,
    /// Name used for the exported file and the manifest.
    pub name: String,
    /// The mono waveform, on the grid of the source format.
    pub waveform: Vec<f64>,
    /// MIDI note this waveform was recognised as, when it was.
    pub note: Option<u8>,
    /// Fundamental frequency in hertz, when one was found.
    pub frequency: Option<f64>,
    /// Peak amplitude, kept so exporters can normalise without rescanning.
    pub peak: f64,
}

impl Sample {
    /// A bank sample from a waveform.
    #[must_use]
    pub fn new(id: usize, name: String, waveform: Vec<f64>) -> Self {
        let peak = waveform
            .iter()
            .fold(0.0_f64, |peak, value| peak.max(value.abs()));
        Self {
            id,
            name,
            waveform,
            note: None,
            frequency: None,
            peak,
        }
    }

    /// Number of frames in the waveform.
    #[must_use]
    pub fn len(&self) -> usize {
        self.waveform.len()
    }

    /// Whether the waveform is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.waveform.is_empty()
    }

    /// Energy of the waveform, used when matching.
    #[must_use]
    pub fn energy(&self) -> f64 {
        self.waveform.iter().map(|value| value * value).sum()
    }
}

/// One use of a bank sample on the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Index into the bank.
    pub sample: usize,
    /// Channel the sample is added to.
    pub channel: usize,
    /// First frame of the sample, which may be negative when the best match
    /// starts before the recording does.
    pub start: i64,
    /// Factor the waveform is scaled by.
    pub gain: Gain,
}

impl Placement {
    /// A placement from its parts.
    #[must_use]
    pub const fn new(sample: usize, channel: usize, start: i64, gain: Gain) -> Self {
        Self {
            sample,
            channel,
            start,
            gain,
        }
    }
}

/// A recognised musical event, the layer MIDI and notation are exported from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    /// Channel the note was heard in.
    pub channel: usize,
    /// First frame of the note.
    pub start: usize,
    /// Length of the note in frames.
    pub length: usize,
    /// MIDI note number.
    pub note: u8,
    /// MIDI velocity, derived from the amplitude envelope.
    pub velocity: u8,
    /// Detected fundamental in hertz.
    pub frequency: f64,
    /// How strongly the analysis supports the note, in `0.0..=1.0`.
    pub confidence: f64,
}

impl Note {
    /// One past the last frame of the note.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.start + self.length
    }

    /// Start of the note in seconds.
    #[must_use]
    pub fn start_seconds(&self, sample_rate: u32) -> f64 {
        if sample_rate == 0 {
            return 0.0;
        }
        self.start as f64 / f64::from(sample_rate)
    }

    /// Length of the note in seconds.
    #[must_use]
    pub fn length_seconds(&self, sample_rate: u32) -> f64 {
        if sample_rate == 0 {
            return 0.0;
        }
        self.length as f64 / f64::from(sample_rate)
    }
}

/// A complete decomposition of one recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Decomposition {
    /// Shape and provenance of the recording.
    pub source: SourceInfo,
    /// Parts that sum back to the recording.
    pub stems: Vec<Stem>,
    /// The deduplicated sample bank.
    pub samples: Vec<Sample>,
    /// Where the bank samples are used.
    pub placements: Vec<Placement>,
    /// The musical reading of the recording.
    pub notes: Vec<Note>,
    /// `original - render(bank, placements)`, always stored in full precision.
    pub residual: Audio,
    /// `original - (render + residual)`, needed only for float sources where
    /// the sum can round.
    pub correction: Option<Audio>,
}

impl Decomposition {
    /// Renders the bank through its placements.
    ///
    /// This is the deterministic half of the round trip: given the same bank,
    /// the same placements and the same source description, it produces exactly
    /// the same samples on every platform, which is what lets the residual be
    /// an exact difference. Accumulation happens in placement order, and the
    /// result is snapped to the source grid for integer formats.
    #[must_use]
    pub fn render(&self) -> Audio {
        render(&self.source, &self.samples, &self.placements)
    }

    /// Rebuilds the recording.
    ///
    /// For integer PCM sources the result is bit-exact; for float sources it is
    /// exact as well whenever a [`Decomposition::correction`] was stored.
    #[must_use]
    pub fn reconstruct(&self) -> Audio {
        let mut audio = self.render();
        add_in_place(&mut audio, &self.residual);
        if let Some(correction) = &self.correction {
            add_in_place(&mut audio, correction);
        }
        audio.set_format(self.source.format);
        audio
    }

    /// The largest absolute difference between a recording and this
    /// decomposition's reconstruction of it.
    pub fn deviation_from(&self, original: &Audio) -> Result<f64> {
        original.max_abs_difference(&self.reconstruct())
    }

    /// Sum of the stems, which reproduces the recording exactly.
    #[must_use]
    pub fn stem_sum(&self) -> Audio {
        let mut audio = self.source.silence();
        for stem in &self.stems {
            add_in_place(&mut audio, &stem.audio);
        }
        audio.set_format(self.source.format);
        audio
    }

    /// A bank sample by its identifier.
    #[must_use]
    pub fn sample(&self, id: usize) -> Option<&Sample> {
        self.samples.get(id)
    }

    /// Every placement of one bank sample.
    #[must_use]
    pub fn placements_of(&self, sample: usize) -> Vec<&Placement> {
        self.placements
            .iter()
            .filter(|placement| placement.sample == sample)
            .collect()
    }

    /// How many frames the bank holds in total, against how many frames the
    /// placements cover. The ratio is what deduplication saved.
    #[must_use]
    pub fn bank_frames(&self) -> (usize, usize) {
        let stored: usize = self.samples.iter().map(Sample::len).sum();
        let covered: usize = self
            .placements
            .iter()
            .filter_map(|placement| self.samples.get(placement.sample))
            .map(Sample::len)
            .sum();
        (stored, covered)
    }

    /// Fraction of the recording's energy left in the residual.
    #[must_use]
    pub fn residual_share(&self) -> f64 {
        let residual: f64 = energy(&self.residual);
        let rendered: f64 = energy(&self.render());
        if residual + rendered <= 0.0 {
            return 0.0;
        }
        residual / (residual + rendered)
    }
}

/// Renders a bank through its placements into a buffer shaped like `source`.
#[must_use]
pub fn render(source: &SourceInfo, samples: &[Sample], placements: &[Placement]) -> Audio {
    let mut audio = source.silence();
    let frames = source.frames as i64;
    {
        let channels = audio.channels_mut();
        for placement in placements {
            let Some(sample) = samples.get(placement.sample) else {
                continue;
            };
            let Some(channel) = channels.get_mut(placement.channel) else {
                continue;
            };
            if placement.gain.is_silent() {
                continue;
            }
            let factor = placement.gain.factor();
            let first = (-placement.start).max(0);
            let last = (frames - placement.start).min(sample.len() as i64);
            for offset in first..last {
                let target = (placement.start + offset) as usize;
                channel[target] += factor * sample.waveform[offset as usize];
            }
        }
    }
    audio.quantize();
    audio
}

/// Adds `other` into `audio`, ignoring any part that does not overlap.
fn add_in_place(audio: &mut Audio, other: &Audio) {
    let frames = audio.frames();
    for (index, channel) in audio.channels_mut().iter_mut().enumerate() {
        let Some(source) = other.channels().get(index) else {
            continue;
        };
        for frame in 0..frames.min(source.len()) {
            channel[frame] += source[frame];
        }
    }
}

/// Total energy of a buffer.
fn energy(audio: &Audio) -> f64 {
    audio
        .channels()
        .iter()
        .flat_map(|channel| channel.iter())
        .map(|value| value * value)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(frames: usize) -> SourceInfo {
        SourceInfo {
            name: "test".into(),
            sample_rate: 8000,
            format: SampleFormat::PcmI16,
            channels: 1,
            frames,
        }
    }

    fn ramp(length: usize) -> Vec<f64> {
        (0..length)
            .map(|index| (index as f64 / 32_768.0).sin())
            .collect()
    }

    #[test]
    fn gains_are_exact_binary_rationals() {
        assert_eq!(Gain::default(), Gain::UNIT);
        assert_eq!(Gain::UNIT.factor(), 1.0);
        assert_eq!(Gain::from_factor(0.5).numerator(), GAIN_SCALE / 2);
        assert_eq!(Gain::from_factor(0.5).factor(), 0.5);
        assert_eq!(Gain::from_factor(-0.25).factor(), -0.25);
        assert!(Gain::from_factor(0.0).is_silent());
        assert_eq!(
            Gain::from_numerator(GAIN_SCALE * 3).factor(),
            3.0,
            "gains above one are allowed"
        );
    }

    #[test]
    fn a_sample_reports_its_shape() {
        let sample = Sample::new(0, "kick".into(), vec![0.5, -0.25, 0.0]);

        assert_eq!(sample.len(), 3);
        assert!(!sample.is_empty());
        assert_eq!(sample.peak, 0.5);
        assert_eq!(sample.energy(), 0.25 + 0.0625);
        assert!(Sample::new(1, "empty".into(), Vec::new()).is_empty());
    }

    #[test]
    fn rendering_places_a_sample_at_every_position() {
        let source = source(16);
        let samples = vec![Sample::new(0, "s".into(), vec![0.5, 0.25])];
        let placements = vec![
            Placement::new(0, 0, 0, Gain::UNIT),
            Placement::new(0, 0, 8, Gain::from_factor(0.5)),
        ];

        let audio = render(&source, &samples, &placements);

        assert_eq!(audio.channel(0)[0], 0.5);
        assert_eq!(audio.channel(0)[1], 0.25);
        assert_eq!(audio.channel(0)[8], 0.25);
        assert_eq!(audio.channel(0)[9], 0.125);
        assert_eq!(audio.channel(0)[4], 0.0);
    }

    #[test]
    fn rendering_clips_placements_to_the_timeline() {
        let source = source(4);
        let samples = vec![Sample::new(0, "s".into(), vec![0.5; 8])];
        let placements = vec![
            Placement::new(0, 0, -6, Gain::UNIT),
            Placement::new(0, 0, 2, Gain::UNIT),
            Placement::new(0, 3, 0, Gain::UNIT),
            Placement::new(7, 0, 0, Gain::UNIT),
            Placement::new(0, 0, 0, Gain::from_factor(0.0)),
        ];

        let audio = render(&source, &samples, &placements);

        assert_eq!(audio.channel(0)[0], 0.5, "tail of the early placement");
        assert_eq!(audio.channel(0)[1], 0.5, "its last frame");
        assert_eq!(audio.channel(0)[2], 0.5, "head of the later placement");
        assert_eq!(audio.channel(0)[3], 0.5);
        assert_eq!(
            audio.channel_count(),
            1,
            "channel 3 and sample 7 were skipped"
        );
    }

    #[test]
    fn a_decomposition_reconstructs_its_source_exactly() {
        let frames = 64;
        let source = source(frames);
        let waveform = ramp(frames);
        let mut original = Audio::from_mono(8000, SampleFormat::PcmI16, waveform.clone()).unwrap();
        original.quantize();

        let samples = vec![Sample::new(0, "whole".into(), waveform)];
        let placements = vec![Placement::new(0, 0, 0, Gain::UNIT)];
        let rendered = render(&source, &samples, &placements);
        let residual = original.difference(&rendered).unwrap();
        let decomposition = Decomposition {
            source,
            stems: vec![Stem::new("full".into(), original.clone())],
            samples,
            placements,
            notes: Vec::new(),
            residual,
            correction: None,
        };

        assert_eq!(decomposition.reconstruct(), original);
        assert_eq!(decomposition.deviation_from(&original).unwrap(), 0.0);
        assert_eq!(decomposition.stem_sum(), original);
        assert_eq!(decomposition.bank_frames(), (frames, frames));
        assert!(decomposition.sample(0).is_some());
        assert!(decomposition.sample(1).is_none());
        assert_eq!(decomposition.placements_of(0).len(), 1);
        assert!(decomposition.residual_share() < 1e-12);
    }

    #[test]
    fn notes_convert_frames_to_seconds() {
        let note = Note {
            channel: 0,
            start: 4000,
            length: 2000,
            note: 69,
            velocity: 100,
            frequency: 440.0,
            confidence: 0.9,
        };

        assert_eq!(note.end(), 6000);
        assert_eq!(note.start_seconds(8000), 0.5);
        assert_eq!(note.length_seconds(8000), 0.25);
        assert_eq!(note.start_seconds(0), 0.0);
        assert_eq!(note.length_seconds(0), 0.0);
    }

    #[test]
    fn source_information_describes_a_buffer() {
        let audio = Audio::silence(48_000, SampleFormat::PcmI24, 2, 96_000);
        let info = SourceInfo::of("song", &audio);

        assert_eq!(info.channels, 2);
        assert_eq!(info.frames, 96_000);
        assert_eq!(info.duration_seconds(), 2.0);
        assert_eq!(info.silence(), audio);
        assert_eq!(
            SourceInfo {
                sample_rate: 0,
                ..info
            }
            .duration_seconds(),
            0.0
        );
    }
}
