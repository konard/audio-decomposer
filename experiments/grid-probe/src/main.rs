//! Prints, for each audio file named on the command line, which sample formats
//! could hold every one of its samples exactly, and how far the samples sit
//! from the recording's own grid.
//!
//! Written to answer why a 1.5 MB recording produced a 33 MB archive: the
//! stems, the residual and the bank samples were all tagged `f64` even though
//! every one of their samples was still on the source's 16-bit grid.

use audio_decomposer::audio;
use audio_decomposer::SampleFormat;

fn main() -> audio_decomposer::Result<()> {
    for path in std::env::args().skip(1) {
        let buffer = audio::read_file(&path)?;
        let exact: Vec<_> = SampleFormat::ALL
            .iter()
            .filter(|format| buffer.is_exact_in(**format))
            .map(|format| format!("{format:?}"))
            .collect();
        println!("{path}");
        println!("  tagged {:?}, exact in {exact:?}", buffer.format());
        println!(
            "  narrowest exact at its own floor: {:?}",
            buffer.narrowest_exact_format(buffer.format())
        );

        // How far off the 16-bit grid do the worst samples sit, and how loud
        // does the buffer get in units of that grid?
        let quantum = SampleFormat::PcmI16.quantum().expect("16-bit has a quantum");
        let mut worst_error = 0.0_f64;
        let mut biggest_code = 0.0_f64;
        for channel in buffer.channels() {
            for sample in channel {
                let code = (sample / quantum).round();
                worst_error = worst_error.max((code * quantum - sample).abs() / quantum);
                biggest_code = biggest_code.max(code.abs());
            }
        }
        println!("  worst 16-bit grid error {worst_error:e} quanta, loudest code {biggest_code}");
    }
    Ok(())
}
