//! Reads a WAV file, prints what it contains, and writes it back out.

use audio_decomposer::audio::wav;
use audio_decomposer::Result;

fn main() -> Result<()> {
    let path = std::env::args().nth(1);
    let Some(path) = path else {
        eprintln!("usage: basic_usage <input.wav>");
        return Ok(());
    };

    let audio = wav::read_file(&path)?;
    println!(
        "{path}: {} Hz, {} channel(s), {} frames, {:.3} s, {}",
        audio.sample_rate(),
        audio.channel_count(),
        audio.frames(),
        audio.duration_seconds(),
        audio.format().name(),
    );
    Ok(())
}
