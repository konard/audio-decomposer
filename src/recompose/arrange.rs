//! Playing a sample bank through a list of notes.
//!
//! Once a recording has been taken apart, the bank is an instrument and the
//! notes are a score. Editing the score — in a DAW, through the exported MIDI
//! file — and playing the bank through it again is what turns the
//! decomposition into something you can work with rather than only inspect.
//!
//! A note is played by the bank sample recognised as that pitch. When no
//! sample carries the pitch, the closest one is resampled by the interval
//! between them, exactly the way a hardware sampler transposes; when the bank
//! holds no pitched sample at all, [`Fallback`] decides whether the note is
//! silent or is voiced by a synthesised tone.

use std::f64::consts::TAU;

use super::voice_for;
use crate::audio::{Audio, SampleFormat};
use crate::decompose::model::{Decomposition, Note};
use crate::dsp::envelope::amplitude_from_velocity;
use crate::dsp::pitch::midi_to_frequency;
use crate::error::Result;
use crate::formats::midi::{self, MidiFile};

/// What to do with a note the bank cannot voice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fallback {
    /// Leave the note silent.
    Silence,
    /// Play a synthesised tone, so an arrangement is always audible.
    #[default]
    Sine,
}

/// How an arrangement is rendered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrangeOptions {
    /// How notes the bank cannot voice are handled.
    pub fallback: Fallback,
    /// Whether note velocity scales the sample to the amplitude it stands for.
    pub follow_velocity: bool,
    /// Whether a sample is cut at the length of the note that plays it.
    pub trim: bool,
    /// Frames of fade applied where a sample is cut, to stop the click.
    pub fade: usize,
    /// Seconds of room left after the last note, for tails that ring on.
    pub tail_seconds: f64,
    /// Format the render is delivered in; the source format when `None`.
    pub format: Option<SampleFormat>,
    /// Whether a render louder than full scale is scaled down instead of
    /// clipping when it is quantized.
    pub normalize: bool,
}

impl Default for ArrangeOptions {
    fn default() -> Self {
        Self {
            fallback: Fallback::default(),
            follow_velocity: true,
            trim: false,
            fade: 64,
            tail_seconds: 1.0,
            format: None,
            normalize: true,
        }
    }
}

impl ArrangeOptions {
    /// Options that voice only what the bank actually holds.
    #[must_use]
    pub const fn bank_only(mut self) -> Self {
        self.fallback = Fallback::Silence;
        self
    }

    /// Options that deliver the render in `format`.
    #[must_use]
    pub const fn in_format(mut self, format: SampleFormat) -> Self {
        self.format = Some(format);
        self
    }
}

/// Renders the notes of a MIDI file through the bank of a decomposition.
pub fn from_midi(
    decomposition: &Decomposition,
    file: &MidiFile,
    options: &ArrangeOptions,
) -> Result<Audio> {
    let notes = midi::to_notes(file, decomposition.source.sample_rate);
    arrange(decomposition, &notes, options)
}

/// Renders notes through the bank of a decomposition.
pub fn arrange(
    decomposition: &Decomposition,
    notes: &[Note],
    options: &ArrangeOptions,
) -> Result<Audio> {
    let source = &decomposition.source;
    let sample_rate = source.sample_rate;
    let voices: Vec<Voice> = notes
        .iter()
        .map(|note| Voice::of(decomposition, note, options))
        .collect();

    let tail = (options.tail_seconds * f64::from(sample_rate)).max(0.0) as usize;
    let end = voices
        .iter()
        .map(|voice| voice.start + voice.samples.len())
        .max()
        .map_or(0, |end| end + tail);
    let channels = notes
        .iter()
        .map(|note| note.channel + 1)
        .max()
        .unwrap_or(0)
        .max(source.channels.max(1));

    let mut buffers = vec![vec![0.0_f64; end.max(source.frames)]; channels];
    for (voice, note) in voices.iter().zip(notes) {
        let buffer = &mut buffers[note.channel % channels];
        for (offset, value) in voice.samples.iter().enumerate() {
            buffer[voice.start + offset] += value;
        }
    }

    let format = options.format.unwrap_or(source.format);
    let mut audio = Audio::from_channels(sample_rate, format, buffers)?;
    if options.normalize && !audio.is_exact_in_format() {
        let peak = audio.peak();
        if peak > 1.0 {
            scale(&mut audio, 1.0 / peak);
        }
    }
    audio.quantize();
    Ok(audio)
}

/// One note, already turned into the samples it contributes.
struct Voice {
    start: usize,
    samples: Vec<f64>,
}

impl Voice {
    /// Renders one note.
    fn of(decomposition: &Decomposition, note: &Note, options: &ArrangeOptions) -> Self {
        let amplitude = if options.follow_velocity {
            amplitude_from_velocity(note.velocity)
        } else {
            1.0
        };
        let mut samples = match voice_for(&decomposition.samples, note.note) {
            Some((index, speed)) => {
                let sample = &decomposition.samples[index];
                let mut played = resample(&sample.waveform, speed);
                if options.follow_velocity && sample.peak > 0.0 {
                    let gain = amplitude / sample.peak;
                    for value in &mut played {
                        *value *= gain;
                    }
                }
                played
            }
            None => match options.fallback {
                Fallback::Silence => Vec::new(),
                Fallback::Sine => tone(
                    midi_to_frequency(f64::from(note.note)),
                    amplitude,
                    note.length.max(1),
                    decomposition.source.sample_rate,
                ),
            },
        };
        if options.trim && note.length > 0 && samples.len() > note.length {
            samples.truncate(note.length);
            fade_out(&mut samples, options.fade);
        }
        Self {
            start: note.start,
            samples,
        }
    }
}

/// Resamples a waveform by a speed factor, with linear interpolation.
///
/// A factor above one raises the pitch and shortens the sample, below one
/// lowers and lengthens it, which is the transposition a sampler performs.
#[must_use]
pub fn resample(waveform: &[f64], speed: f64) -> Vec<f64> {
    if waveform.is_empty() || !speed.is_finite() || speed <= 0.0 {
        return Vec::new();
    }
    if speed == 1.0 {
        return waveform.to_vec();
    }
    let length = ((waveform.len() as f64) / speed).floor() as usize;
    (0..length)
        .map(|index| {
            let position = index as f64 * speed;
            let left = position.floor() as usize;
            let fraction = position - position.floor();
            let first = waveform.get(left).copied().unwrap_or(0.0);
            let second = waveform.get(left + 1).copied().unwrap_or(0.0);
            fraction.mul_add(second - first, first)
        })
        .collect()
}

/// A decaying tone, used when the bank cannot voice a note.
#[must_use]
pub fn tone(frequency: f64, amplitude: f64, frames: usize, sample_rate: u32) -> Vec<f64> {
    if sample_rate == 0 {
        return Vec::new();
    }
    let rate = f64::from(sample_rate);
    let mut samples: Vec<f64> = (0..frames)
        .map(|index| {
            let time = index as f64 / rate;
            let decay = (-3.0 * time).exp();
            amplitude * decay * (TAU * frequency * time).sin()
        })
        .collect();
    fade_out(&mut samples, 64);
    samples
}

/// Fades the last `length` frames out linearly.
fn fade_out(samples: &mut [f64], length: usize) {
    let usable = length.min(samples.len());
    let total = samples.len();
    for index in 0..usable {
        let gain = index as f64 / usable as f64;
        samples[total - 1 - index] *= gain;
    }
}

/// Scales every sample of a buffer.
fn scale(audio: &mut Audio, factor: f64) {
    for channel in audio.channels_mut() {
        for sample in channel.iter_mut() {
            *sample *= factor;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decompose::model::{Decomposition, Sample, SourceInfo};

    fn decomposition(pitched: bool) -> Decomposition {
        let audio = Audio::silence(8_000, SampleFormat::PcmI16, 1, 8);
        let mut samples = vec![Sample::new(
            0,
            "one".to_string(),
            vec![0.5, -0.5, 0.25, 0.0],
        )];
        if pitched {
            samples[0].note = Some(60);
            samples[0].frequency = Some(midi_to_frequency(60.0));
        }
        Decomposition {
            source: SourceInfo::of("song", &audio),
            stems: Vec::new(),
            samples,
            placements: Vec::new(),
            notes: Vec::new(),
            residual: audio,
            correction: None,
        }
    }

    fn note(number: u8, start: usize, length: usize) -> Note {
        Note {
            channel: 0,
            start,
            length,
            note: number,
            velocity: 127,
            frequency: midi_to_frequency(f64::from(number)),
            confidence: 1.0,
        }
    }

    #[test]
    fn a_note_plays_the_sample_that_matches_it() {
        let decomposition = decomposition(true);
        let options = ArrangeOptions {
            tail_seconds: 0.0,
            ..ArrangeOptions::default()
        };

        let audio = arrange(&decomposition, &[note(60, 2, 4)], &options).unwrap();

        assert_eq!(audio.sample_rate(), 8_000);
        assert_eq!(audio.channel_count(), 1);
        assert_eq!(audio.frames(), 8);
        assert_eq!(&audio.channel(0)[0..2], &[0.0, 0.0]);
        // Velocity 127 means full scale, so the sample is played at twice the
        // amplitude it was captured with.
        assert!((audio.channel(0)[2] - 1.0).abs() < 1e-4);
        assert!((audio.channel(0)[3] + 1.0).abs() < 1e-4);
        assert!((audio.channel(0)[4] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn a_note_the_bank_does_not_hold_is_transposed_from_the_nearest() {
        let decomposition = decomposition(true);
        let options = ArrangeOptions {
            tail_seconds: 0.0,
            ..ArrangeOptions::default()
        };

        let up = arrange(&decomposition, &[note(72, 0, 4)], &options).unwrap();
        let down = arrange(&decomposition, &[note(48, 0, 4)], &options).unwrap();

        // An octave up plays twice as fast, an octave down twice as slow.
        assert!(up.channel(0)[0..2].iter().any(|value| *value != 0.0));
        assert!(down.channel(0)[..8].iter().filter(|v| **v != 0.0).count() > 3);
    }

    #[test]
    fn an_unvoiceable_note_follows_the_fallback() {
        let decomposition = decomposition(false);
        let options = ArrangeOptions {
            tail_seconds: 0.0,
            ..ArrangeOptions::default()
        };

        let sine = arrange(&decomposition, &[note(69, 0, 800)], &options).unwrap();
        assert!(sine.peak() > 0.0);

        let silent = arrange(&decomposition, &[note(69, 0, 800)], &options.bank_only()).unwrap();
        assert_eq!(silent.peak(), 0.0);
        assert_eq!(silent.frames(), 8);
    }

    #[test]
    fn velocity_sets_how_loud_a_sample_is_played() {
        let decomposition = decomposition(true);
        let options = ArrangeOptions {
            tail_seconds: 0.0,
            format: Some(SampleFormat::F64),
            ..ArrangeOptions::default()
        };

        let mut quiet = note(60, 0, 4);
        quiet.velocity = 64;
        let loud = arrange(&decomposition, &[note(60, 0, 4)], &options).unwrap();
        let quiet = arrange(&decomposition, &[quiet], &options).unwrap();

        assert!(quiet.peak() < loud.peak());
        assert!((loud.peak() - 1.0).abs() < 1e-9);
        assert!((quiet.peak() - amplitude_from_velocity(64)).abs() < 1e-9);

        let flat = ArrangeOptions {
            follow_velocity: false,
            ..options
        };
        let flat = arrange(&decomposition, &[note(60, 0, 4)], &flat).unwrap();
        assert!((flat.peak() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn a_trimmed_note_stops_when_it_is_over() {
        let decomposition = decomposition(true);
        let options = ArrangeOptions {
            trim: true,
            fade: 1,
            tail_seconds: 0.0,
            ..ArrangeOptions::default()
        };

        let audio = arrange(&decomposition, &[note(60, 0, 2)], &options).unwrap();

        assert_eq!(audio.channel(0)[2], 0.0);
        assert_eq!(audio.channel(0)[1], 0.0, "the cut is faded, not clicked");
    }

    #[test]
    fn overlapping_notes_are_scaled_instead_of_clipped() {
        let decomposition = decomposition(true);
        let options = ArrangeOptions {
            tail_seconds: 0.0,
            ..ArrangeOptions::default()
        };
        let notes = vec![note(60, 0, 4), note(60, 0, 4), note(60, 0, 4)];

        let audio = arrange(&decomposition, &notes, &options).unwrap();

        assert!(audio.peak() <= 1.0);
        assert!(audio.peak() > 0.9);
    }

    #[test]
    fn an_arrangement_can_be_read_from_a_midi_file() {
        let decomposition = decomposition(true);
        let notes = vec![note(60, 0, 4), note(62, 4, 4)];
        let file = midi::from_notes(&notes, 8_000, "song").unwrap();
        let options = ArrangeOptions {
            tail_seconds: 0.0,
            ..ArrangeOptions::default()
        };

        let direct = arrange(&decomposition, &notes, &options).unwrap();
        let through_midi = from_midi(&decomposition, &file, &options).unwrap();

        assert_eq!(through_midi, direct);
    }

    #[test]
    fn a_second_channel_appears_when_the_notes_ask_for_one() {
        let decomposition = decomposition(true);
        let mut right = note(60, 0, 4);
        right.channel = 1;
        let options = ArrangeOptions {
            tail_seconds: 0.0,
            ..ArrangeOptions::default()
        };

        let audio = arrange(&decomposition, &[right], &options).unwrap();

        assert_eq!(audio.channel_count(), 2);
        assert_eq!(audio.channel(0).iter().sum::<f64>(), 0.0);
        assert!(audio.channel(1).iter().any(|value| *value != 0.0));
    }

    #[test]
    fn resampling_handles_the_degenerate_cases() {
        assert!(resample(&[], 1.0).is_empty());
        assert!(resample(&[1.0], 0.0).is_empty());
        assert!(resample(&[1.0], f64::NAN).is_empty());
        assert_eq!(resample(&[1.0, 2.0], 1.0), vec![1.0, 2.0]);
        assert_eq!(resample(&[0.0, 1.0, 2.0, 3.0], 2.0), vec![0.0, 2.0]);
        // Half speed doubles the length, and the last step ramps out to the
        // silence that follows the waveform.
        assert_eq!(resample(&[0.0, 1.0], 0.5), vec![0.0, 0.5, 1.0, 0.5]);
        assert!(tone(440.0, 1.0, 10, 0).is_empty());
    }

    #[test]
    fn an_empty_score_renders_the_silence_the_source_had() {
        let decomposition = decomposition(true);

        let audio = arrange(&decomposition, &[], &ArrangeOptions::default()).unwrap();

        assert_eq!(audio.frames(), 8);
        assert_eq!(audio.peak(), 0.0);
    }
}
