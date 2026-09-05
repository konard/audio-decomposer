//! Turns a recording into projects a DAW can open.
//!
//! The decomposition is written to a directory, and every project format the
//! crate knows is written beside it. The projects point at the stems and
//! samples that are already there, so opening one shows the recording laid out
//! as the parts it is made of.
//!
//! ```text
//! cargo run --example to_daw_project -- song.wav song-decomposition
//! ```

use audio_decomposer::decompose::{decompose, DecomposeOptions};
use audio_decomposer::formats::project;
use audio_decomposer::formats::session::SessionOptions;
use audio_decomposer::{archive, audio, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: to_daw_project <input.wav> <output-directory>");
        return Ok(());
    };

    let recording = audio::read_file(&input)?;
    let decomposition = decompose(&recording, &DecomposeOptions::default())?;
    archive::write_dir(&output, &decomposition)?;

    // Notes travel with the projects, so an instrument track carries the music
    // as notes and not only as audio clips.
    let options = SessionOptions {
        notes: true,
        ..SessionOptions::default()
    };
    for path in project::export(&output, &options)? {
        println!("{}", path.display());
    }

    Ok(())
}
