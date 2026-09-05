//! Note recognition: the musical reading of a recording.
//!
//! Where [`crate::decompose::dedup`] answers "which piece of audio is this",
//! this module answers "which note is this", so that the decomposition can be
//! written out as MIDI and as notation. The two readings are independent: a
//! wrong note never damages the reconstruction, it only makes the exported
//! score less faithful.
//!
//! Polyphonic material is tracked with harmonic-sum salience over the
//! spectrogram, and each isolated waveform is additionally checked with YIN,
//! which is more precise when only one note is sounding.

use crate::audio::Audio;
use crate::decompose::model::Note;
use crate::dsp::envelope;
use crate::dsp::pitch::{self, PitchOptions};
use crate::dsp::polyphonic::{self, PolyphonicOptions};
use crate::dsp::stft::{self, StftOptions};
use crate::error::Result;

/// How notes are recognised.
#[derive(Clone, Copy, Debug)]
pub struct NoteOptions {
    /// Transform used for the analysis.
    pub stft: StftOptions,
    /// Polyphonic tracker settings.
    pub polyphonic: PolyphonicOptions,
    /// Monophonic estimator settings, used to label bank samples.
    pub pitch: PitchOptions,
    /// Shortest note kept, in seconds.
    pub minimum_seconds: f64,
}

impl Default for NoteOptions {
    fn default() -> Self {
        Self {
            stft: StftOptions::default(),
            polyphonic: PolyphonicOptions::default(),
            pitch: PitchOptions::default(),
            minimum_seconds: 0.03,
        }
    }
}

/// Recognises the notes of every channel of a recording.
pub fn detect(audio: &Audio, options: &NoteOptions) -> Result<Vec<Note>> {
    let sample_rate = audio.sample_rate();
    if audio.frames() == 0 || sample_rate == 0 {
        return Ok(Vec::new());
    }
    let minimum = (options.minimum_seconds * f64::from(sample_rate)).round() as usize;
    let mut notes = Vec::new();
    for (channel, samples) in audio.channels().iter().enumerate() {
        let spectrogram = stft::forward(samples, sample_rate, options.stft)?;
        let tracked = polyphonic::track(&spectrogram, options.polyphonic);
        let loudest = tracked
            .iter()
            .map(|note| note.salience)
            .fold(0.0_f64, f64::max);
        for note in tracked {
            let start = frame_to_sample(&spectrogram, note.start_frame);
            let end = frame_to_sample(&spectrogram, note.end_frame).min(samples.len());
            if end <= start || end - start < minimum {
                continue;
            }
            let amplitude = envelope::peak(&samples[start..end]);
            let confidence = if loudest > 0.0 {
                (note.salience / loudest).clamp(0.0, 1.0)
            } else {
                0.0
            };
            notes.push(Note {
                channel,
                start,
                length: end - start,
                note: note.note.clamp(0, 127) as u8,
                velocity: envelope::velocity_from_amplitude(amplitude),
                frequency: pitch::midi_to_frequency(f64::from(note.note)),
                confidence,
            });
        }
    }
    notes.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then(left.channel.cmp(&right.channel))
            .then(left.note.cmp(&right.note))
    });
    Ok(notes)
}

/// Labels an isolated waveform with the note it sounds.
///
/// Returns the MIDI note, the measured frequency and the confidence of the
/// estimate, or `None` when the waveform is not periodic enough to name.
#[must_use]
pub fn identify(
    waveform: &[f64],
    sample_rate: u32,
    options: PitchOptions,
) -> Option<(u8, f64, f64)> {
    let estimate = pitch::estimate(waveform, sample_rate, options)?;
    if estimate.confidence < 0.5 {
        return None;
    }
    let note = estimate.nearest_note().clamp(0, 127) as u8;
    Some((note, estimate.frequency, estimate.confidence))
}

/// First sample of an analysis frame, clamped to the start of the signal.
fn frame_to_sample(spectrogram: &stft::Spectrogram, frame: usize) -> usize {
    let centre = spectrogram.frame_time(frame) * f64::from(spectrogram.sample_rate());
    let start = centre - spectrogram.options().fft_size as f64 / 2.0;
    if start <= 0.0 {
        0
    } else {
        start.round() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleFormat;
    use crate::dsp::WindowKind;

    fn tone(frequency: f64, sample_rate: u32, length: usize, amplitude: f64) -> Vec<f64> {
        (0..length)
            .map(|index| {
                let time = index as f64 / f64::from(sample_rate);
                amplitude
                    * ((std::f64::consts::TAU * frequency * time).sin()
                        + 0.5 * (std::f64::consts::TAU * 2.0 * frequency * time).sin()
                        + 0.25 * (std::f64::consts::TAU * 3.0 * frequency * time).sin())
                    / 1.75
            })
            .collect()
    }

    fn options() -> NoteOptions {
        NoteOptions {
            stft: StftOptions {
                fft_size: 2048,
                hop: 512,
                window: WindowKind::Hann,
            },
            ..NoteOptions::default()
        }
    }

    #[test]
    fn a_melody_is_recognised_note_by_note() {
        let sample_rate = 22_050;
        let played = [60, 64, 67];
        let note_length = 11_025;
        let mut signal = Vec::new();
        for note in played {
            let frequency = pitch::midi_to_frequency(f64::from(note));
            signal.extend(tone(frequency, sample_rate, note_length, 0.5));
        }
        let audio = Audio::from_mono(sample_rate, SampleFormat::PcmI16, signal).unwrap();

        let notes = detect(&audio, &options()).unwrap();

        for (index, expected) in played.iter().enumerate() {
            let window = index * note_length;
            let found = notes
                .iter()
                .find(|note| note.note == *expected && note.start.abs_diff(window) < 4096);
            assert!(found.is_some(), "note {expected} not found in {notes:?}");
        }
        assert!(notes.iter().all(|note| note.channel == 0));
        assert!(notes.iter().all(|note| note.velocity > 1));
        assert!(notes.iter().all(|note| note.confidence > 0.0));
        assert!(notes.iter().all(|note| note.length > 0));
    }

    #[test]
    fn a_chord_is_recognised_as_several_notes_at_once() {
        let sample_rate = 22_050;
        let length = 22_050;
        let mut signal = vec![0.0; length];
        for note in [60, 64, 67] {
            let frequency = pitch::midi_to_frequency(f64::from(note));
            for (index, value) in tone(frequency, sample_rate, length, 0.3).iter().enumerate() {
                signal[index] += value;
            }
        }
        let audio = Audio::from_mono(sample_rate, SampleFormat::PcmI16, signal).unwrap();

        let notes = detect(&audio, &options()).unwrap();

        for expected in [60, 64, 67] {
            assert!(
                notes.iter().any(|note| note.note == expected),
                "missing {expected} in {:?}",
                notes.iter().map(|note| note.note).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn notes_carry_the_channel_they_were_heard_in() {
        let sample_rate = 22_050;
        let length = 22_050;
        let left = tone(pitch::midi_to_frequency(60.0), sample_rate, length, 0.5);
        let right = tone(pitch::midi_to_frequency(72.0), sample_rate, length, 0.5);
        let audio =
            Audio::from_channels(sample_rate, SampleFormat::PcmI16, vec![left, right]).unwrap();

        let notes = detect(&audio, &options()).unwrap();

        assert!(notes
            .iter()
            .any(|note| note.channel == 0 && note.note == 60));
        assert!(notes
            .iter()
            .any(|note| note.channel == 1 && note.note == 72));
    }

    #[test]
    fn an_isolated_waveform_can_be_named() {
        let sample_rate = 22_050;
        let waveform = tone(pitch::midi_to_frequency(69.0), sample_rate, 8192, 0.8);

        let (note, frequency, confidence) =
            identify(&waveform, sample_rate, PitchOptions::default()).unwrap();

        assert_eq!(note, 69);
        assert!((frequency - 440.0).abs() < 5.0);
        assert!(confidence > 0.5);
    }

    #[test]
    fn noise_and_silence_are_not_named() {
        let sample_rate = 22_050;
        assert!(identify(&vec![0.0; 4096], sample_rate, PitchOptions::default()).is_none());
        assert!(identify(&[], sample_rate, PitchOptions::default()).is_none());

        let silence = Audio::silence(sample_rate, SampleFormat::PcmI16, 1, 8192);
        assert!(detect(&silence, &options()).unwrap().is_empty());

        let empty = Audio::silence(sample_rate, SampleFormat::PcmI16, 1, 0);
        assert!(detect(&empty, &options()).unwrap().is_empty());
    }
}
