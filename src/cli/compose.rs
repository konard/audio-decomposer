//! `compose`: put a decomposition back together into audio.

use std::io::Write;
use std::path::PathBuf;

use audio_decomposer::audio;
use audio_decomposer::error::Result;
use audio_decomposer::recompose;

use lino_arguments::Args;

use super::report;

/// Arguments of the `compose` command.
#[derive(Args, Debug)]
pub struct Compose {
    /// Decomposition directory to rebuild.
    pub archive: PathBuf,

    /// File to write the reconstruction to; `<archive>.wav` by default.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

impl Compose {
    /// Runs the command.
    pub fn run(&self, out: &mut impl Write) -> Result<()> {
        let manifest = report::manifest(&self.archive)?;
        let audio = recompose::from_archive(&self.archive)?;
        let destination = self.destination();
        audio::write_file(&destination, &audio)?;

        report::describe_source(&manifest, out)?;
        report::describe_audio(&destination, &audio, out)?;
        Ok(())
    }

    /// Where the reconstruction is written.
    #[must_use]
    pub fn destination(&self) -> PathBuf {
        self.output.clone().unwrap_or_else(|| {
            let mut path = self.archive.clone();
            path.set_extension("wav");
            path
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Cli, Command};
    use super::*;
    use lino_arguments::Parser;

    fn parsed(arguments: &[&str]) -> Compose {
        let cli = Cli::try_parse_from(
            std::iter::once("audio-decomposer").chain(arguments.iter().copied()),
        )
        .unwrap();
        match cli.command {
            Command::Compose(command) => command,
            other => panic!("expected compose, got {other:?}"),
        }
    }

    #[test]
    fn the_reconstruction_lands_beside_the_archive_by_default() {
        let command = parsed(&["compose", "/music/song-decomposition"]);
        assert_eq!(
            command.destination(),
            PathBuf::from("/music/song-decomposition.wav")
        );
    }

    #[test]
    fn a_trailing_separator_does_not_confuse_the_default() {
        let command = parsed(&["compose", "/music/song-decomposition/"]);
        assert_eq!(
            command.destination().extension().and_then(|it| it.to_str()),
            Some("wav")
        );
    }

    #[test]
    fn a_given_output_is_used_as_it_stands() {
        let command = parsed(&["compose", "archive", "-o", "/tmp/out.aiff"]);
        assert_eq!(command.destination(), PathBuf::from("/tmp/out.aiff"));
    }
}
