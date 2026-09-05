//! Multi-pitch estimation for chords and overlapping voices.
//!
//! Each candidate note is scored by summing the spectrum at its harmonics
//! (the classic harmonic-sum salience). The strongest candidate is accepted,
//! its harmonics are removed from the working spectrum so that its overtones
//! cannot be mistaken for separate notes, and the search repeats until the
//! remaining salience falls below a fraction of the first one. Frame-level
//! decisions are then joined into notes with a start and an end.

use crate::dsp::pitch::midi_to_frequency;
use crate::dsp::stft::Spectrogram;

/// Parameters of the multi-pitch estimator.
#[derive(Clone, Copy, Debug)]
pub struct PolyphonicOptions {
    /// Lowest MIDI note considered.
    pub lowest_note: i32,
    /// Highest MIDI note considered.
    pub highest_note: i32,
    /// Number of harmonics summed per candidate.
    pub harmonics: usize,
    /// Maximum number of simultaneous notes reported per frame.
    pub max_voices: usize,
    /// A candidate is kept while its salience stays above this fraction of the
    /// strongest salience in the frame.
    pub salience_ratio: f64,
    /// Frames quieter than this fraction of the loudest frame report nothing.
    pub silence_ratio: f64,
    /// Shortest note kept by [`track`], in frames.
    pub minimum_frames: usize,
}

impl Default for PolyphonicOptions {
    fn default() -> Self {
        Self {
            lowest_note: 21,
            highest_note: 108,
            harmonics: 8,
            max_voices: 6,
            salience_ratio: 0.45,
            silence_ratio: 0.01,
            minimum_frames: 2,
        }
    }
}

/// One note detected in one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramePitch {
    /// MIDI note number.
    pub note: i32,
    /// Harmonic-sum salience that selected it.
    pub salience: f64,
}

/// A note held across a run of frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackedNote {
    /// MIDI note number.
    pub note: i32,
    /// First frame the note is active in.
    pub start_frame: usize,
    /// One past the last frame the note is active in.
    pub end_frame: usize,
    /// Mean salience over the note's lifetime.
    pub salience: f64,
}

/// Reads a magnitude spectrum at an arbitrary frequency by linear
/// interpolation between the two neighbouring bins.
#[must_use]
pub fn magnitude_at(spectrum: &[f64], frequency: f64, sample_rate: u32, fft_size: usize) -> f64 {
    if spectrum.is_empty() || fft_size == 0 || frequency <= 0.0 {
        return 0.0;
    }
    let position = frequency * fft_size as f64 / f64::from(sample_rate);
    if position >= (spectrum.len() - 1) as f64 {
        return 0.0;
    }
    let lower = position.floor() as usize;
    let fraction = position - lower as f64;
    spectrum[lower] * (1.0 - fraction) + spectrum[lower + 1] * fraction
}

/// Harmonic-sum salience of one candidate note.
#[must_use]
pub fn salience(
    spectrum: &[f64],
    note: i32,
    sample_rate: u32,
    fft_size: usize,
    harmonics: usize,
) -> f64 {
    let fundamental = midi_to_frequency(f64::from(note));
    (1..=harmonics.max(1))
        .map(|harmonic| {
            let frequency = fundamental * harmonic as f64;
            magnitude_at(spectrum, frequency, sample_rate, fft_size) / harmonic as f64
        })
        .sum()
}

/// Estimates the notes present in a single magnitude spectrum.
#[must_use]
pub fn frame_pitches(
    spectrum: &[f64],
    sample_rate: u32,
    fft_size: usize,
    options: PolyphonicOptions,
) -> Vec<FramePitch> {
    if spectrum.is_empty() || options.highest_note < options.lowest_note {
        return Vec::new();
    }
    let mut working = spectrum.to_vec();
    let mut found: Vec<FramePitch> = Vec::new();
    let mut strongest = 0.0_f64;

    for _ in 0..options.max_voices.max(1) {
        let best = (options.lowest_note..=options.highest_note)
            .filter(|note| found.iter().all(|pitch| pitch.note != *note))
            .map(|note| {
                (
                    note,
                    salience(&working, note, sample_rate, fft_size, options.harmonics),
                )
            })
            .max_by(|left, right| left.1.total_cmp(&right.1));
        let Some((note, value)) = best else { break };
        if value <= 0.0 {
            break;
        }
        if found.is_empty() {
            strongest = value;
        } else if value < strongest * options.salience_ratio {
            break;
        }
        found.push(FramePitch {
            note,
            salience: value,
        });
        remove_harmonics(&mut working, note, sample_rate, fft_size, options.harmonics);
    }
    found
}

/// Attenuates the bins around a note's harmonics.
fn remove_harmonics(
    spectrum: &mut [f64],
    note: i32,
    sample_rate: u32,
    fft_size: usize,
    harmonics: usize,
) {
    let fundamental = midi_to_frequency(f64::from(note));
    let bin_width = f64::from(sample_rate) / fft_size as f64;
    for harmonic in 1..=harmonics.max(1) {
        let frequency = fundamental * harmonic as f64;
        let centre = frequency / bin_width;
        // A semitone either side, but never narrower than a single bin.
        let half_width = (centre * 0.06).max(1.5);
        let start = (centre - half_width).max(0.0) as usize;
        let end = ((centre + half_width).ceil() as usize + 1).min(spectrum.len());
        for bin in &mut spectrum[start..end] {
            *bin = 0.0;
        }
    }
}

/// Tracks notes across a whole spectrogram.
#[must_use]
pub fn track(spectrogram: &Spectrogram, options: PolyphonicOptions) -> Vec<TrackedNote> {
    let magnitudes = spectrogram.magnitudes();
    let energies: Vec<f64> = magnitudes
        .iter()
        .map(|frame| frame.iter().sum::<f64>())
        .collect();
    let loudest = energies.iter().copied().fold(0.0_f64, f64::max);
    let floor = loudest * options.silence_ratio;
    let fft_size = spectrogram.options().fft_size;

    let mut active: Vec<(i32, usize, f64, usize)> = Vec::new();
    let mut finished: Vec<TrackedNote> = Vec::new();
    for (frame, magnitude) in magnitudes.iter().enumerate() {
        let pitches = if energies[frame] > floor {
            frame_pitches(magnitude, spectrogram.sample_rate(), fft_size, options)
        } else {
            Vec::new()
        };
        let mut still_active = Vec::with_capacity(active.len());
        for (note, start, total, count) in std::mem::take(&mut active) {
            if let Some(pitch) = pitches.iter().find(|pitch| pitch.note == note) {
                still_active.push((note, start, total + pitch.salience, count + 1));
            } else {
                push_note(&mut finished, note, start, frame, total, count, options);
            }
        }
        for pitch in &pitches {
            if !still_active.iter().any(|(note, ..)| *note == pitch.note) {
                still_active.push((pitch.note, frame, pitch.salience, 1));
            }
        }
        active = still_active;
    }
    let end = magnitudes.len();
    for (note, start, total, count) in active {
        push_note(&mut finished, note, start, end, total, count, options);
    }
    finished.sort_by(|left, right| {
        left.start_frame
            .cmp(&right.start_frame)
            .then(left.note.cmp(&right.note))
    });
    finished
}

fn push_note(
    notes: &mut Vec<TrackedNote>,
    note: i32,
    start: usize,
    end: usize,
    total: f64,
    count: usize,
    options: PolyphonicOptions,
) {
    if end.saturating_sub(start) < options.minimum_frames.max(1) {
        return;
    }
    notes.push(TrackedNote {
        note,
        start_frame: start,
        end_frame: end,
        salience: total / count.max(1) as f64,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::stft::{self, StftOptions};
    use crate::dsp::WindowKind;

    fn chord(notes: &[i32], sample_rate: u32, length: usize) -> Vec<f64> {
        (0..length)
            .map(|index| {
                let time = index as f64 / f64::from(sample_rate);
                notes
                    .iter()
                    .map(|note| {
                        let frequency = midi_to_frequency(f64::from(*note));
                        0.3 * (std::f64::consts::TAU * frequency * time).sin()
                            + 0.1 * (std::f64::consts::TAU * 2.0 * frequency * time).sin()
                    })
                    .sum()
            })
            .collect()
    }

    fn analyze(signal: &[f64], sample_rate: u32) -> Spectrogram {
        stft::forward(
            signal,
            sample_rate,
            StftOptions {
                fft_size: 4096,
                hop: 1024,
                window: WindowKind::Hann,
            },
        )
        .unwrap()
    }

    #[test]
    fn interpolated_magnitude_reads_between_bins() {
        let spectrum = vec![0.0, 2.0, 4.0, 0.0];
        let sample_rate = 8;
        let fft_size = 4;

        assert_eq!(magnitude_at(&spectrum, 2.0, sample_rate, fft_size), 2.0);
        assert_eq!(magnitude_at(&spectrum, 3.0, sample_rate, fft_size), 3.0);
        assert_eq!(magnitude_at(&spectrum, 0.0, sample_rate, fft_size), 0.0);
        assert_eq!(magnitude_at(&spectrum, 100.0, sample_rate, fft_size), 0.0);
        assert_eq!(magnitude_at(&[], 1.0, sample_rate, fft_size), 0.0);
        assert_eq!(magnitude_at(&spectrum, 1.0, sample_rate, 0), 0.0);
    }

    #[test]
    fn a_single_tone_is_found_by_the_frame_estimator() {
        let sample_rate = 22050;
        let signal = chord(&[60], sample_rate, 22050);
        let spectrogram = analyze(&signal, sample_rate);
        let magnitudes = spectrogram.magnitudes();
        let pitches = frame_pitches(
            &magnitudes[magnitudes.len() / 2],
            sample_rate,
            4096,
            PolyphonicOptions::default(),
        );

        assert_eq!(pitches.first().map(|pitch| pitch.note), Some(60));
        assert!(pitches[0].salience > 0.0);
    }

    #[test]
    fn a_major_triad_yields_its_three_notes() {
        let sample_rate = 22050;
        let expected = [60, 64, 67];
        let signal = chord(&expected, sample_rate, 22050);
        let spectrogram = analyze(&signal, sample_rate);
        let magnitudes = spectrogram.magnitudes();
        let pitches = frame_pitches(
            &magnitudes[magnitudes.len() / 2],
            sample_rate,
            4096,
            PolyphonicOptions::default(),
        );
        let mut notes: Vec<i32> = pitches.iter().map(|pitch| pitch.note).collect();
        notes.sort_unstable();

        for note in expected {
            assert!(notes.contains(&note), "missing {note} in {notes:?}");
        }
    }

    #[test]
    fn tracking_produces_notes_with_a_start_and_an_end() {
        let sample_rate = 22050;
        let mut signal = chord(&[60], sample_rate, 11025);
        signal.extend(chord(&[67], sample_rate, 11025));
        let spectrogram = analyze(&signal, sample_rate);
        let notes = track(&spectrogram, PolyphonicOptions::default());

        assert!(notes.iter().any(|note| note.note == 60));
        assert!(notes.iter().any(|note| note.note == 67));
        let first = notes.iter().find(|note| note.note == 60).unwrap();
        let second = notes.iter().find(|note| note.note == 67).unwrap();
        assert!(first.start_frame <= second.start_frame);
        assert!(first.end_frame > first.start_frame);
        assert!(second.salience > 0.0);
    }

    #[test]
    fn silence_and_impossible_ranges_produce_nothing() {
        let sample_rate = 22050;
        let spectrogram = analyze(&vec![0.0; 8192], sample_rate);
        assert!(track(&spectrogram, PolyphonicOptions::default()).is_empty());
        assert!(frame_pitches(&[], sample_rate, 4096, PolyphonicOptions::default()).is_empty());
        assert!(frame_pitches(
            &[1.0; 16],
            sample_rate,
            4096,
            PolyphonicOptions {
                lowest_note: 80,
                highest_note: 20,
                ..PolyphonicOptions::default()
            }
        )
        .is_empty());
    }
}
