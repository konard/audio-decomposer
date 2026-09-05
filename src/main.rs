//! The `audio-decomposer` command line.
//!
//! Everything the tool can do lives in the library; this binary only parses
//! arguments, runs the command and reports what happened.

mod cli;

use std::io::{self, Write};
use std::process::ExitCode;

use audio_decomposer::error::Error;
use cli::{run, Cli};
use lino_arguments::Parser;

fn main() -> ExitCode {
    lino_arguments::init();
    let cli = Cli::parse();
    // The report is written as it is produced rather than collected first, so
    // a long analysis says what it has found so far.
    let mut stdout = io::stdout().lock();
    let result = run(&cli, &mut stdout).and_then(|()| stdout.flush().map_err(Error::from));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // A run piped into `head` is not a failed run.
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "audio-decomposer: {error}");
            ExitCode::FAILURE
        }
    }
}
