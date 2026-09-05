//! `inspect`: describe a recording or a decomposition without changing it.

use std::io::Write;
use std::path::PathBuf;

use audio_decomposer::audio;
use audio_decomposer::error::Result;
use audio_decomposer::formats::project::Format;
use audio_decomposer::formats::session::Session;
use lino_arguments::Args;

use super::report;

/// Arguments of the `inspect` command.
#[derive(Args, Debug)]
pub struct Inspect {
    /// Audio file or decomposition directory to describe.
    pub path: PathBuf,

    /// Also list the tracks the projects would hold.
    #[arg(long)]
    pub tracks: bool,

    /// Also list the project formats the exporter writes.
    #[arg(long)]
    pub formats: bool,
}

impl Inspect {
    /// Runs the command.
    pub fn run(&self, out: &mut impl Write) -> Result<()> {
        if self.path.is_dir() {
            self.describe_archive(out)?;
        } else {
            let audio = audio::read_file(&self.path)?;
            report::describe_audio(&self.path, &audio, out)?;
        }
        if self.formats {
            writeln!(out, "formats:")?;
            for format in Format::ALL {
                writeln!(out, "  {:<11} {}", format.extension(), format.title())?;
            }
        }
        Ok(())
    }

    /// Describes a decomposition directory.
    fn describe_archive(&self, out: &mut impl Write) -> Result<()> {
        let manifest = report::manifest(&self.path)?;
        report::describe_manifest(&manifest, out)?;
        let session = Session::from_manifest(&manifest, &report::session_options(None, true));
        writeln!(out, "tempo: {:.3} BPM", session.tempo)?;
        if self.tracks {
            writeln!(out, "tracks:")?;
            for track in &session.tracks {
                writeln!(
                    out,
                    "  {:<24} {} clip(s), {} note(s)",
                    track.name,
                    track.clips.len(),
                    track.notes.len()
                )?;
            }
        }
        Ok(())
    }
}
