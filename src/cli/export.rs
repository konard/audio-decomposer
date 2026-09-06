//! `export`: write DAW projects beside a decomposition that already exists.

use std::io::Write;
use std::path::PathBuf;

use audio_decomposer::error::Result;
use audio_decomposer::formats::project;
use lino_arguments::Args;

use super::report;

/// Arguments of the `export` command.
#[derive(Args, Debug)]
pub struct Export {
    /// Decomposition directory to export.
    pub archive: PathBuf,

    /// Project formats to export, `all` or `none`.
    #[arg(long, default_value = report::ALL)]
    pub formats: Vec<String>,

    /// Tempo of the exported projects; measured from the music when absent.
    #[arg(long)]
    pub tempo: Option<f64>,

    /// Leave the recognised notes out of the projects.
    #[arg(long)]
    pub no_notes: bool,
}

impl Export {
    /// Runs the command.
    ///
    /// The projects go into the archive itself: every path a project holds is
    /// relative to the directory the audio is in, so an export somewhere else
    /// would open with every clip missing.
    pub fn run(&self, out: &mut impl Write) -> Result<()> {
        let manifest = report::manifest(&self.archive)?;
        let formats = report::formats(&self.formats)?;
        let options = report::session_options(self.tempo, !self.no_notes);
        let session = project::session_of(&self.archive, &manifest, &options);
        writeln!(out, "tempo: {:.3} BPM", session.tempo)?;
        let written = project::write_these(&self.archive, &session, &formats)?;
        report::describe_written(&self.archive, &written, out)
    }
}
