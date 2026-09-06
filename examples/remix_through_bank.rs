//! Plays a different score through the sounds of a recording.
//!
//! Once a recording has been reduced to a bank of distinct waveforms with a
//! note recognised for each, the bank is an instrument: any melody can be
//! played with the sounds the recording was made of.
//!
//! ```text
//! cargo run --example remix_through_bank -- song-decomposition melody.mid remix.wav
//! ```

use audio_decomposer::formats::midi;
use audio_decomposer::recompose::arrange::{self, ArrangeOptions};
use audio_decomposer::{archive, audio, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(score), Some(output)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: remix_through_bank <decomposition> <score.mid> <output.wav>");
        return Ok(());
    };

    let decomposition = archive::read_dir(&input)?;
    let score = midi::read_file(&score)?;

    // Only what the bank really holds: a note it cannot voice stays silent
    // rather than being covered by a synthesised tone.
    let options = ArrangeOptions::default().bank_only();
    let render = arrange::from_midi(&decomposition, &score, &options)?;

    audio::write_file(&output, &render)?;
    println!(
        "{output}: {} frames from {} bank sample(s)",
        render.frames(),
        decomposition.samples.len()
    );

    Ok(())
}
