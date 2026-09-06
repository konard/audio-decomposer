//! `verify`: check a decomposition against the recording it came from.

use std::io::Write;
use std::path::PathBuf;

use audio_decomposer::audio;
use audio_decomposer::error::Result;
use audio_decomposer::recompose;
use lino_arguments::Args;

use super::report;

/// Arguments of the `verify` command.
#[derive(Args, Debug)]
pub struct Verify {
    /// Decomposition directory to check.
    pub archive: PathBuf,

    /// The recording the decomposition claims to reproduce.
    pub original: PathBuf,

    /// Fail with a non-zero exit status when the reconstruction is not exact.
    #[arg(long)]
    pub strict: bool,
}

impl Verify {
    /// Runs the command.
    pub fn run(&self, out: &mut impl Write) -> Result<()> {
        let original = audio::read_file(&self.original)?;
        let verified = recompose::verify_archive(&self.archive, &original)?;
        report::describe_report(&verified, out)?;
        if self.strict && !verified.is_exact() {
            return Err(audio_decomposer::error::Error::Mismatch(format!(
                "{} does not reproduce {} exactly; largest difference {:.3e}",
                self.archive.display(),
                self.original.display(),
                verified.deviation
            )));
        }
        Ok(())
    }
}
