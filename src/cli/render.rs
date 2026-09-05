//! `render`: play a score through the sample bank of a decomposition.

use std::io::Write;
use std::path::PathBuf;

use audio_decomposer::archive;
use audio_decomposer::audio;
use audio_decomposer::error::Result;
use audio_decomposer::formats::midi;
use audio_decomposer::recompose::arrange::{self, ArrangeOptions, Fallback};
use lino_arguments::Args;

use super::report;

/// Arguments of the `render` command.
#[derive(Args, Debug)]
pub struct Render {
    /// Decomposition directory holding the bank to play.
    pub archive: PathBuf,

    /// MIDI file to play through it; the decomposition's own notes by default.
    #[arg(long)]
    pub score: Option<PathBuf>,

    /// File to write the render to.
    #[arg(short, long, default_value = "render.wav")]
    pub output: PathBuf,

    /// Leave a note the bank cannot voice silent instead of synthesising it.
    #[arg(long)]
    pub silence_unvoiceable: bool,

    /// Cut every sample at the length of the note that plays it.
    #[arg(long)]
    pub trim: bool,

    /// Seconds of room left after the last note.
    #[arg(long)]
    pub tail_seconds: Option<f64>,
}

impl Render {
    /// Runs the command.
    pub fn run(&self, out: &mut impl Write) -> Result<()> {
        let decomposition = archive::read_dir(&self.archive)?;
        let options = self.options();
        let audio = if let Some(path) = &self.score {
            let file = midi::read_file(path)?;
            writeln!(out, "score: {}", path.display())?;
            arrange::from_midi(&decomposition, &file, &options)?
        } else {
            writeln!(
                out,
                "score: the {} recognised notes",
                decomposition.notes.len()
            )?;
            arrange::arrange(&decomposition, &decomposition.notes, &options)?
        };
        audio::write_file(&self.output, &audio)?;
        report::describe_audio(&self.output, &audio, out)
    }

    /// The rendering settings the switches add up to.
    #[must_use]
    pub fn options(&self) -> ArrangeOptions {
        let mut options = ArrangeOptions {
            trim: self.trim,
            ..ArrangeOptions::default()
        };
        if self.silence_unvoiceable {
            options.fallback = Fallback::Silence;
        }
        if let Some(seconds) = self.tail_seconds {
            options.tail_seconds = seconds.max(0.0);
        }
        options
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Cli, Command};
    use super::*;
    use lino_arguments::Parser;

    fn parsed(arguments: &[&str]) -> Render {
        let cli = Cli::try_parse_from(
            std::iter::once("audio-decomposer").chain(arguments.iter().copied()),
        )
        .unwrap();
        match cli.command {
            Command::Render(command) => command,
            other => panic!("expected render, got {other:?}"),
        }
    }

    #[test]
    fn a_render_writes_to_render_wav_unless_told_otherwise() {
        assert_eq!(
            parsed(&["render", "archive"]).output,
            PathBuf::from("render.wav")
        );
        assert_eq!(
            parsed(&["render", "archive", "-o", "take.wav"]).output,
            PathBuf::from("take.wav")
        );
    }

    #[test]
    fn the_decompositions_own_notes_are_the_default_score() {
        assert!(parsed(&["render", "archive"]).score.is_none());
        assert_eq!(
            parsed(&["render", "archive", "--score", "part.mid"]).score,
            Some(PathBuf::from("part.mid"))
        );
    }

    #[test]
    fn every_switch_reaches_the_arrangement_it_names() {
        let options = parsed(&[
            "render",
            "archive",
            "--trim",
            "--silence-unvoiceable",
            "--tail-seconds",
            "1.5",
        ])
        .options();
        assert!(options.trim);
        assert_eq!(options.fallback, Fallback::Silence);
        assert!((options.tail_seconds - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn a_negative_tail_is_read_as_no_tail() {
        let options = parsed(&["render", "archive", "--tail-seconds=-2"]).options();
        assert!(options.tail_seconds.abs() < f64::EPSILON);
        assert!(!options.trim);
        assert_eq!(options.fallback, ArrangeOptions::default().fallback);
    }
}
