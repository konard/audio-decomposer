//! The command line: everything the library does, from a shell.
//!
//! The six commands follow the shape of the pipeline rather than the shape of
//! the code:
//!
//! - `decompose` takes a recording apart and writes the archive and projects;
//! - `compose` puts an archive back together into audio;
//! - `verify` checks a decomposition against the recording it came from;
//! - `inspect` describes a recording or an archive without changing anything;
//! - `export` writes DAW projects beside an archive that already exists; and
//! - `render` plays a score through the sample bank of an archive.
//!
//! Every command writes its report to a writer the caller provides, so the
//! output is testable rather than only printable.

pub mod compose;
pub mod decompose;
pub mod export;
pub mod inspect;
pub mod render;
pub mod report;
pub mod verify;

use std::io::Write;

use audio_decomposer::error::Result;
use lino_arguments::{Parser, Subcommand};

/// Decompose audio into deduplicated samples, stems, notes and DAW projects,
/// and compose it back losslessly.
#[derive(Parser, Debug)]
#[command(name = "audio-decomposer", version, about, long_about = None)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The commands the tool offers.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Take a recording apart into stems, a sample bank, notes and projects.
    Decompose(decompose::Decompose),
    /// Put a decomposition back together into an audio file.
    Compose(compose::Compose),
    /// Check that a decomposition reproduces the recording it came from.
    Verify(verify::Verify),
    /// Describe a recording or a decomposition without changing anything.
    Inspect(inspect::Inspect),
    /// Write DAW projects beside a decomposition that already exists.
    Export(export::Export),
    /// Play a score through the sample bank of a decomposition.
    Render(render::Render),
}

/// Runs a parsed command line.
pub fn run(cli: &Cli, out: &mut impl Write) -> Result<()> {
    match &cli.command {
        Command::Decompose(command) => command.run(out),
        Command::Compose(command) => command.run(out),
        Command::Verify(command) => command.run(out),
        Command::Inspect(command) => command.run(out),
        Command::Export(command) => command.run(out),
        Command::Render(command) => command.run(out),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use audio_decomposer::audio::{self, Audio, SampleFormat};
    use lino_arguments::Parser;

    use super::{run, Cli};

    /// A directory that lasts as long as the test that made it.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "audio-decomposer-cli-{label}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Two bars of a plucked note repeated on every beat: something with a
    /// pitch to recognise and a pattern to deduplicate, over in a moment.
    fn loop_recording() -> Audio {
        let rate: u32 = 8_000;
        let beat = rate as usize / 2;
        let mut samples = vec![0.0; beat * 8];
        for beat_index in 0..8 {
            let frequency = if beat_index % 2 == 0 { 220.0 } else { 330.0 };
            for offset in 0..beat / 2 {
                #[allow(clippy::cast_precision_loss)]
                let time = offset as f64 / f64::from(rate);
                let envelope = (-12.0 * time).exp();
                samples[beat_index * beat + offset] +=
                    0.4 * envelope * (std::f64::consts::TAU * frequency * time).sin();
            }
        }
        let mut audio = Audio::from_mono(rate, SampleFormat::PcmI16, samples).unwrap();
        audio.quantize();
        audio
    }

    fn go(arguments: &[&str]) -> String {
        let cli = Cli::try_parse_from(
            std::iter::once("audio-decomposer").chain(arguments.iter().copied()),
        )
        .unwrap_or_else(|error| panic!("{arguments:?}: {error}"));
        let mut out = Vec::new();
        run(&cli, &mut out).unwrap_or_else(|error| panic!("{arguments:?}: {error}"));
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_recording_goes_through_every_command_and_comes_back_unchanged() {
        let scratch = Scratch::new("round-trip");
        let source = scratch.join("loop.wav");
        audio::write_file(&source, &loop_recording()).unwrap();
        let archive = scratch.join("out");

        let report = go(&[
            "decompose",
            source.to_str().unwrap(),
            "--output",
            archive.to_str().unwrap(),
            "--formats",
            "midi,sfz",
        ]);
        assert!(report.contains("recording: loop"), "{report}");
        assert!(report.contains("reconstruction: exact"), "{report}");
        assert!(report.contains("loop.mid"), "{report}");
        assert!(report.contains("loop.sfz"), "{report}");
        assert!(archive.join("decomposition.lino").exists());

        let checked = go(&[
            "verify",
            archive.to_str().unwrap(),
            source.to_str().unwrap(),
            "--strict",
        ]);
        assert!(checked.contains("reconstruction: exact"), "{checked}");

        let rebuilt = scratch.join("rebuilt.wav");
        go(&[
            "compose",
            archive.to_str().unwrap(),
            "--output",
            rebuilt.to_str().unwrap(),
        ]);
        assert_eq!(
            fs::read(&source).unwrap(),
            fs::read(&rebuilt).unwrap(),
            "the reconstruction is not the file we started from"
        );

        let seen = go(&["inspect", archive.to_str().unwrap(), "--tracks"]);
        assert!(seen.contains("tracks:"), "{seen}");
        assert!(seen.contains("BPM"), "{seen}");

        let exported = go(&["export", archive.to_str().unwrap(), "--formats", "musicxml"]);
        assert!(exported.contains("loop.musicxml"), "{exported}");
        assert!(archive.join("loop.musicxml").exists());

        let render = scratch.join("render.wav");
        let played = go(&[
            "render",
            archive.to_str().unwrap(),
            "--output",
            render.to_str().unwrap(),
        ]);
        assert!(played.contains("score: the"), "{played}");
        assert!(audio::read_file(&render).unwrap().peak() > 0.0);
    }

    #[test]
    fn inspecting_an_audio_file_reads_the_file_itself() {
        let scratch = Scratch::new("inspect-audio");
        let source = scratch.join("loop.wav");
        audio::write_file(&source, &loop_recording()).unwrap();

        let seen = go(&["inspect", source.to_str().unwrap(), "--formats"]);
        assert!(seen.contains("8000 Hz, 1 channel(s)"), "{seen}");
        assert!(seen.contains("0:04.000"), "{seen}");
        assert!(seen.contains("formats:"), "{seen}");
        assert!(seen.contains("mid         MIDI"), "{seen}");
    }

    #[test]
    fn a_directory_that_is_not_a_decomposition_says_so() {
        let scratch = Scratch::new("not-an-archive");
        let cli = Cli::try_parse_from(["audio-decomposer", "inspect", scratch.0.to_str().unwrap()])
            .unwrap();
        let error = run(&cli, &mut Vec::new()).unwrap_err().to_string();
        assert!(
            error.contains("does not look like a decomposition"),
            "{error}"
        );
    }
}
