//! Recordings made from scratch, and the scaffolding the suites share.
//!
//! Nothing here is sampled from anybody's music: every fixture is arithmetic,
//! which keeps the suite free of copyright and free of large binary files.

use std::f64::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use audio_decomposer::audio::{Audio, SampleFormat};

/// A directory that lasts as long as the test that made it.
pub struct Scratch(PathBuf);

impl Scratch {
    /// Makes an empty directory named after the test using it.
    pub fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "audio-decomposer-it-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }

    /// A path inside the scratch directory.
    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// The scratch directory itself.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs the installed binary and returns what it did.
pub fn cli(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_audio-decomposer"))
        .args(arguments)
        .output()
        .expect("the binary under test runs")
}

/// What the binary printed on standard output.
pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// What the binary printed on standard error.
pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Asserts the binary succeeded, showing both streams when it did not.
pub fn succeeds(output: &Output, what: &str) -> String {
    assert!(
        output.status.success(),
        "{what} failed:\n{}\n{}",
        stdout(output),
        stderr(output)
    );
    stdout(output)
}

/// One decaying sine, the shape of a plucked or struck note.
pub fn pluck(rate: u32, frequency: f64, length: usize, decay: f64, gain: f64) -> Vec<f64> {
    (0..length)
        .map(|index| {
            let time = index as f64 / f64::from(rate);
            gain * (-decay * time).exp() * (TAU * frequency * time).sin()
        })
        .collect()
}

/// A short burst of band-limited noise, the shape of a hat or a snare.
pub fn hit(rate: u32, length: usize, seed: u64, gain: f64) -> Vec<f64> {
    let mut state = seed | 1;
    let mut previous = 0.0;
    (0..length)
        .map(|index| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let white = ((state >> 33) as f64 / f64::from(u32::MAX)) * 2.0 - 1.0;
            previous = 0.5f64.mul_add(previous, 0.5 * white);
            let time = index as f64 / f64::from(rate);
            gain * (-40.0 * time).exp() * previous
        })
        .collect()
}

/// Mixes `part` into `into` starting at `at`, growing nothing.
pub fn mix(into: &mut [f64], at: usize, part: &[f64]) {
    for (offset, value) in part.iter().enumerate() {
        if let Some(slot) = into.get_mut(at + offset) {
            *slot += value;
        }
    }
}

/// A loop that repeats the same two waveforms on a grid: the case the sample
/// bank is built for, because almost every event is a copy of an earlier one.
#[must_use]
pub fn repetitive_loop(rate: u32, format: SampleFormat, bars: usize) -> Audio {
    let beat = rate as usize / 2;
    let low = pluck(rate, 110.0, beat, 9.0, 0.45);
    let high = hit(rate, beat / 3, 12_345, 0.3);
    let mut samples = vec![0.0; beat * 4 * bars];
    for beat_index in 0..4 * bars {
        mix(&mut samples, beat_index * beat, &low);
        mix(&mut samples, beat_index * beat + beat / 2, &high);
    }
    finish(rate, format, vec![samples])
}

/// A chord progression: overlapping pitched notes, the case note recognition
/// and the score exporters are built for.
#[must_use]
pub fn chord_progression(rate: u32, format: SampleFormat) -> Audio {
    let bar = rate as usize;
    let chords = [[57, 60, 64], [55, 59, 62], [53, 57, 60], [50, 53, 57]];
    let mut samples = vec![0.0; bar * chords.len()];
    for (index, chord) in chords.iter().enumerate() {
        for note in chord {
            let frequency = 440.0 * ((f64::from(*note) - 69.0) / 12.0).exp2();
            let voice = pluck(rate, frequency, bar, 2.5, 0.22);
            mix(&mut samples, index * bar, &voice);
        }
    }
    finish(rate, format, vec![samples])
}

/// A stereo recording whose two channels differ, so a decomposition that
/// quietly folded them together could not reproduce it.
#[must_use]
pub fn wide_stereo(rate: u32, format: SampleFormat) -> Audio {
    let beat = rate as usize / 2;
    let mut left = vec![0.0; beat * 8];
    let mut right = vec![0.0; beat * 8];
    for beat_index in 0..8 {
        mix(
            &mut left,
            beat_index * beat,
            &pluck(rate, 165.0, beat, 7.0, 0.4),
        );
        if beat_index % 2 == 1 {
            mix(
                &mut right,
                beat_index * beat,
                &pluck(rate, 247.0, beat, 7.0, 0.4),
            );
        }
    }
    finish(rate, format, vec![left, right])
}

/// Silence, which every stage still has to survive.
#[must_use]
pub fn silence(rate: u32, format: SampleFormat) -> Audio {
    Audio::silence(rate, format, 1, rate as usize)
}

/// Builds the audio and puts it on its format's grid, the way a decoded file
/// already is.
fn finish(rate: u32, format: SampleFormat, channels: Vec<Vec<f64>>) -> Audio {
    let mut audio = Audio::from_channels(rate, format, channels).expect("a well-formed recording");
    audio.quantize();
    audio
}

/// Every fixture, named, for suites that want to run over all of them.
#[must_use]
pub fn every(rate: u32, format: SampleFormat) -> Vec<(&'static str, Audio)> {
    vec![
        ("repetitive-loop", repetitive_loop(rate, format, 2)),
        ("chord-progression", chord_progression(rate, format)),
        ("wide-stereo", wide_stereo(rate, format)),
        ("silence", silence(rate, format)),
    ]
}
