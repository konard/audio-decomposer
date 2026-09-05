//! `decompose`: take a recording apart and write everything it is made of.

use std::io::Write;
use std::path::{Path, PathBuf};

use audio_decomposer::archive;
use audio_decomposer::audio;
use audio_decomposer::decompose::{self, DecomposeOptions, StemMode};
use audio_decomposer::error::Result;
use audio_decomposer::formats::project;
use audio_decomposer::recompose;
use lino_arguments::Args;

use super::report;

/// Suffix of the directory a decomposition is written to when none is given.
pub const SUFFIX: &str = "-decomposition";

/// Arguments of the `decompose` command.
///
/// The switches are flags because that is what a command line offers; each one
/// turns off a stage of the pipeline that is otherwise on.
#[derive(Args, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct Decompose {
    /// Audio file to take apart.
    pub input: PathBuf,

    /// Directory to write the decomposition to.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Name the decomposition carries; the file name by default.
    #[arg(long)]
    pub name: Option<String>,

    /// Project formats to export, `all` or `none`.
    #[arg(long, default_value = report::ALL)]
    pub formats: Vec<String>,

    /// Tempo of the exported projects; measured from the music when absent.
    #[arg(long)]
    pub tempo: Option<f64>,

    /// Reuse a waveform only where it repeats verbatim, which is faster and
    /// keeps every placement exact.
    #[arg(long)]
    pub exact_repeats: bool,

    /// Keep the recording whole instead of splitting it into stems.
    #[arg(long)]
    pub no_stems: bool,

    /// Skip note recognition.
    #[arg(long)]
    pub no_notes: bool,

    /// How far a match may be shifted, in seconds.
    #[arg(long)]
    pub search_seconds: Option<f64>,

    /// How many stored waveforms are tried per event.
    #[arg(long)]
    pub candidates: Option<usize>,

    /// Skip the reconstruction check.
    #[arg(long)]
    pub no_verify: bool,
}

impl Decompose {
    /// Runs the command.
    pub fn run(&self, out: &mut impl Write) -> Result<()> {
        let audio = audio::read_file(&self.input)?;
        let root = self.destination();
        let formats = report::formats(&self.formats)?;

        let decomposition = decompose::decompose(&audio, &self.options())?;
        let manifest = archive::write_dir(&root, &decomposition)?;

        report::describe_manifest(&manifest, out)?;
        writeln!(out, "archive: {}", root.display())?;

        if !self.no_verify {
            let verified = recompose::verify(&decomposition, &audio)?;
            report::describe_report(&verified, out)?;
        }

        if !formats.is_empty() {
            let options = report::session_options(self.tempo, !self.no_notes);
            let session = project::session_of(&root, &manifest, &options);
            writeln!(out, "tempo: {:.3} BPM", session.tempo)?;
            let written = project::write_these(&root, &session, &formats)?;
            report::describe_written(&root, &written, out)?;
        }
        Ok(())
    }

    /// Where the decomposition is written.
    #[must_use]
    pub fn destination(&self) -> PathBuf {
        if let Some(output) = &self.output {
            return output.clone();
        }
        let parent = self.input.parent().unwrap_or_else(|| Path::new("."));
        parent.join(format!("{}{SUFFIX}", self.stem()))
    }

    /// The name the decomposition carries.
    #[must_use]
    pub fn stem(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        self.input
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("recording")
            .to_string()
    }

    /// The analysis settings the switches add up to.
    #[must_use]
    pub fn options(&self) -> DecomposeOptions {
        let mut options = DecomposeOptions {
            name: self.stem(),
            ..DecomposeOptions::default()
        };
        if self.exact_repeats {
            options = options.exact_repeats_only();
        }
        if self.no_stems {
            options.stems = StemMode::None;
        }
        if self.no_notes {
            options.recognise_notes = false;
        }
        if let Some(seconds) = self.search_seconds {
            options.search_seconds = seconds.max(0.0);
        }
        if let Some(candidates) = self.candidates {
            options.candidates = candidates;
        }
        options
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Cli, Command};
    use super::*;
    use lino_arguments::Parser;

    fn parsed(arguments: &[&str]) -> Decompose {
        let cli = Cli::try_parse_from(
            std::iter::once("audio-decomposer").chain(arguments.iter().copied()),
        )
        .unwrap_or_else(|error| panic!("{arguments:?}: {error}"));
        match cli.command {
            Command::Decompose(command) => command,
            other => panic!("expected decompose, got {other:?}"),
        }
    }

    #[test]
    fn the_decomposition_lands_beside_the_recording_by_default() {
        let command = parsed(&["decompose", "/music/take one.wav"]);
        assert_eq!(
            command.destination(),
            PathBuf::from("/music/take one-decomposition")
        );
        assert_eq!(command.stem(), "take one");
    }

    #[test]
    fn a_given_output_is_used_as_it_stands() {
        let command = parsed(&["decompose", "song.wav", "--output", "/tmp/here"]);
        assert_eq!(command.destination(), PathBuf::from("/tmp/here"));
    }

    #[test]
    fn a_given_name_replaces_the_file_stem_everywhere() {
        let command = parsed(&["decompose", "song.wav", "--name", "chorus"]);
        assert_eq!(command.stem(), "chorus");
        assert_eq!(command.options().name, "chorus");
        assert_eq!(command.destination(), PathBuf::from("chorus-decomposition"));
    }

    #[test]
    fn every_switch_reaches_the_analysis_it_names() {
        let command = parsed(&[
            "decompose",
            "song.wav",
            "--exact-repeats",
            "--no-stems",
            "--no-notes",
            "--search-seconds",
            "0.25",
            "--candidates",
            "12",
        ]);
        let options = command.options();
        assert_eq!(options.stems, StemMode::None);
        assert!(!options.recognise_notes);
        assert!((options.search_seconds - 0.25).abs() < f64::EPSILON);
        assert_eq!(options.candidates, 12);
        assert_eq!(options.matching.tolerance, 0.0);
    }

    #[test]
    fn a_negative_search_window_is_read_as_no_window() {
        let command = parsed(&["decompose", "song.wav", "--search-seconds=-1"]);
        assert!(command.options().search_seconds.abs() < f64::EPSILON);
    }

    #[test]
    fn every_project_format_is_exported_unless_asked_otherwise() {
        let command = parsed(&["decompose", "song.wav"]);
        assert_eq!(
            report::formats(&command.formats).unwrap().len(),
            audio_decomposer::formats::project::Format::ALL.len()
        );
        let quiet = parsed(&["decompose", "song.wav", "--formats", "none"]);
        assert!(report::formats(&quiet.formats).unwrap().is_empty());
    }
}
