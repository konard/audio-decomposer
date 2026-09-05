//! Putting a decomposition back together.
//!
//! Decomposing is only half a claim; the other half is that the parts add up.
//! This module is the other half:
//!
//! - [`reconstruct`] renders the bank through its placements and adds the
//!   residual back, which returns the original recording bit for bit.
//! - [`verify`] does that and measures the difference against a recording, so a
//!   caller can prove the round trip instead of trusting it.
//! - [`from_archive`] does the same starting from a directory on disk.
//! - [`arrange`] goes the other way: it takes notes — typically edited in a
//!   DAW and read back from a MIDI file — and plays the bank through them, so
//!   the decomposition works as an instrument and not only as a recording.

pub mod arrange;

use std::path::Path;

use crate::archive;
use crate::audio::Audio;
use crate::decompose::model::{Decomposition, Sample};
use crate::error::Result;

pub use arrange::{arrange, from_midi, ArrangeOptions, Fallback};

/// Rebuilds the recording a decomposition was made from.
#[must_use]
pub fn reconstruct(decomposition: &Decomposition) -> Audio {
    decomposition.reconstruct()
}

/// Rebuilds the recording an archive directory holds.
pub fn from_archive(path: impl AsRef<Path>) -> Result<Audio> {
    Ok(reconstruct(&archive::read_dir(path)?))
}

/// What a reconstruction was worth, in numbers a caller can assert on.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// Name the decomposition carries.
    pub name: String,
    /// Sample rate of the recording.
    pub sample_rate: u32,
    /// Channels in the recording.
    pub channels: usize,
    /// Frames in the recording.
    pub frames: usize,
    /// Largest absolute difference between the original and the reconstruction.
    pub deviation: f64,
    /// Largest absolute difference between the original and the sum of stems.
    pub stem_deviation: f64,
    /// Bank samples stored.
    pub samples: usize,
    /// Placements of those samples on the timeline.
    pub placements: usize,
    /// Frames the bank stores.
    pub stored_frames: usize,
    /// Frames those stored frames cover once placed.
    pub covered_frames: usize,
    /// Fraction of the energy the bank could not explain.
    pub residual_share: f64,
    /// Notes recognised.
    pub notes: usize,
}

impl Report {
    /// Whether the reconstruction is the original, to the last bit.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        self.deviation == 0.0
    }

    /// Whether the stems sum back to the original, to the last bit.
    #[must_use]
    pub fn stems_are_exact(&self) -> bool {
        self.stem_deviation == 0.0
    }

    /// How many frames of timeline each stored frame accounts for.
    ///
    /// One means the bank stores every event separately and deduplication
    /// found nothing; ten means each stored waveform is played ten times.
    #[must_use]
    pub fn reuse(&self) -> f64 {
        if self.stored_frames == 0 {
            return 0.0;
        }
        self.covered_frames as f64 / self.stored_frames as f64
    }

    /// Duration of the recording in seconds.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frames as f64 / f64::from(self.sample_rate)
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{}: {:.3} s, {} Hz, {} channel(s), {} frames",
            self.name,
            self.duration_seconds(),
            self.sample_rate,
            self.channels,
            self.frames
        )?;
        writeln!(
            f,
            "bank: {} sample(s) in {} frames, {} placement(s) covering {} frames, reuse {:.2}x",
            self.samples,
            self.stored_frames,
            self.placements,
            self.covered_frames,
            self.reuse()
        )?;
        writeln!(
            f,
            "notes: {}, residual energy {:.4}%",
            self.notes,
            self.residual_share * 100.0
        )?;
        write!(
            f,
            "reconstruction: {}, stems: {}",
            deviation(self.deviation),
            deviation(self.stem_deviation)
        )
    }
}

/// Describes a deviation the way a listener would want to hear it described.
fn deviation(value: f64) -> String {
    if value == 0.0 {
        "exact".to_string()
    } else {
        format!("off by {value:e}")
    }
}

/// Measures a decomposition against the recording it came from.
pub fn verify(decomposition: &Decomposition, original: &Audio) -> Result<Report> {
    let (stored_frames, covered_frames) = decomposition.bank_frames();
    let stem_deviation = if decomposition.stems.is_empty() {
        0.0
    } else {
        original.max_abs_difference(&decomposition.stem_sum())?
    };
    Ok(Report {
        name: decomposition.source.name.clone(),
        sample_rate: decomposition.source.sample_rate,
        channels: decomposition.source.channels,
        frames: decomposition.source.frames,
        deviation: decomposition.deviation_from(original)?,
        stem_deviation,
        samples: decomposition.samples.len(),
        placements: decomposition.placements.len(),
        stored_frames,
        covered_frames,
        residual_share: decomposition.residual_share(),
        notes: decomposition.notes.len(),
    })
}

/// Measures an archive against the recording it was made from.
pub fn verify_archive(path: impl AsRef<Path>, original: &Audio) -> Result<Report> {
    verify(&archive::read_dir(path)?, original)
}

/// The bank sample a note number should be played with, and the speed factor
/// that brings it to that pitch.
///
/// A sample recognised as the note itself plays untouched; otherwise the
/// closest pitched sample is sped up or slowed down by the interval between
/// them, the way a sampler does it.
#[must_use]
pub fn voice_for(samples: &[Sample], note: u8) -> Option<(usize, f64)> {
    let mut best: Option<(usize, i32)> = None;
    for (index, sample) in samples.iter().enumerate() {
        let Some(pitch) = sample.note else { continue };
        let distance = i32::from(note).abs_diff(i32::from(pitch)) as i32;
        if distance == 0 {
            return Some((index, 1.0));
        }
        if best.is_none_or(|(_, previous)| distance < previous) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| {
        let pitch = samples[index].note.unwrap_or(note);
        let interval = f64::from(i32::from(note) - i32::from(pitch));
        (index, (interval / 12.0).exp2())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleFormat;
    use crate::decompose::model::{Gain, Note, Placement, SourceInfo, Stem};

    fn audio(values: &[f64]) -> Audio {
        Audio::from_mono(8_000, SampleFormat::PcmI16, values.to_vec()).unwrap()
    }

    fn decomposition() -> Decomposition {
        let original = audio(&[0.5, -0.5, 0.25, 0.0]);
        let mut samples = vec![Sample::new(0, "one".to_string(), vec![0.5, -0.5])];
        samples[0].note = Some(60);
        Decomposition {
            source: SourceInfo::of("song", &original),
            stems: vec![
                Stem::new("harmonic".to_string(), audio(&[0.25, -0.25, 0.25, 0.0])),
                Stem::new("percussive".to_string(), audio(&[0.25, -0.25, 0.0, 0.0])),
            ],
            samples,
            placements: vec![Placement::new(0, 0, 0, Gain::UNIT)],
            notes: vec![Note {
                channel: 0,
                start: 0,
                length: 2,
                note: 60,
                velocity: 100,
                frequency: 261.625_565_300_598_6,
                confidence: 1.0,
            }],
            residual: audio(&[0.0, 0.0, 0.25, 0.0]),
            correction: None,
        }
    }

    #[test]
    fn a_decomposition_reconstructs_its_recording_exactly() {
        let decomposition = decomposition();
        let original = audio(&[0.5, -0.5, 0.25, 0.0]);

        assert_eq!(reconstruct(&decomposition), original);

        let report = verify(&decomposition, &original).unwrap();
        assert!(report.is_exact(), "{report}");
        assert!(report.stems_are_exact(), "{report}");
        assert_eq!(report.samples, 1);
        assert_eq!(report.placements, 1);
        assert_eq!(report.stored_frames, 2);
        assert_eq!(report.covered_frames, 2);
        assert_eq!(report.notes, 1);
        assert!((report.duration_seconds() - 0.0005).abs() < 1e-12);
        assert!(report.to_string().contains("exact"));
    }

    #[test]
    fn a_report_says_when_something_does_not_add_up() {
        let mut decomposition = decomposition();
        decomposition.residual = audio(&[0.0; 4]);
        decomposition.stems.pop();
        let original = audio(&[0.5, -0.5, 0.25, 0.0]);

        let report = verify(&decomposition, &original).unwrap();
        assert!(!report.is_exact());
        assert!(!report.stems_are_exact());
        assert_eq!(report.deviation, 0.25);
        assert!(report.to_string().contains("off by"));
    }

    #[test]
    fn reuse_counts_how_often_the_bank_is_played() {
        let mut decomposition = decomposition();
        decomposition
            .placements
            .push(Placement::new(0, 0, 2, Gain::UNIT));

        let report = verify(&decomposition, &audio(&[0.5, -0.5, 0.25, 0.0])).unwrap();
        assert!((report.reuse() - 2.0).abs() < 1e-12);

        decomposition.samples.clear();
        decomposition.placements.clear();
        let empty = verify(&decomposition, &audio(&[0.5, -0.5, 0.25, 0.0])).unwrap();
        assert_eq!(empty.reuse(), 0.0);
    }

    #[test]
    fn an_archive_reconstructs_its_recording_exactly() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-recompose-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);

        let decomposition = decomposition();
        let original = audio(&[0.5, -0.5, 0.25, 0.0]);
        archive::write_dir(&path, &decomposition).unwrap();

        assert_eq!(from_archive(&path).unwrap(), original);
        assert!(verify_archive(&path, &original).unwrap().is_exact());

        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn a_note_picks_the_sample_that_plays_it() {
        let mut samples = vec![
            Sample::new(0, "c".to_string(), vec![0.5]),
            Sample::new(1, "a".to_string(), vec![0.5]),
            Sample::new(2, "noise".to_string(), vec![0.5]),
        ];
        samples[0].note = Some(60);
        samples[1].note = Some(69);

        assert_eq!(voice_for(&samples, 60), Some((0, 1.0)));
        assert_eq!(voice_for(&samples, 69), Some((1, 1.0)));

        let (index, speed) = voice_for(&samples, 72).unwrap();
        assert_eq!(index, 1);
        assert!((speed - 2.0_f64.powf(3.0 / 12.0)).abs() < 1e-12);

        let (index, speed) = voice_for(&samples, 48).unwrap();
        assert_eq!(index, 0);
        assert!((speed - 0.5).abs() < 1e-12);

        assert_eq!(voice_for(&samples[2..], 60), None);
        assert_eq!(voice_for(&[], 60), None);
    }
}
