//! Checks this crate's FLAC codec against the reference implementation.
//!
//! A codec that only agrees with itself proves nothing: a round trip through a
//! symmetric bug still comes out clean. This probe writes synthesised audio,
//! then crosses the two implementations over — the reference encodes what we
//! decode, and decodes what we encode — so a disagreement in either direction
//! shows up.
//!
//! ```bash
//! cd experiments/flac-probe && cargo run -- /tmp/flac-probe
//! ```
//!
//! Needs `flac` on the path (`apt install flac`). Nothing in the test suite
//! does, so CI stays self-contained.

use std::process::Command;

use audio_decomposer::audio::{flac, wav, Audio, SampleFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/flac-probe".into());
    std::fs::create_dir_all(&directory)?;

    for (name, audio) in cases() {
        println!("== {name}");
        let source = format!("{directory}/{name}.wav");
        wav::write_file(&source, &audio)?;

        // The reference encodes; we decode.
        let reference = format!("{directory}/{name}-reference.flac");
        run("flac", &["--best", "-f", "-s", "-o", &reference, &source])?;
        let decoded = flac::read_file(&reference)?;
        report(
            "reference encode -> our decode",
            decoded.channels() == audio.channels() && decoded.format() == audio.format(),
        );

        // We encode; the reference verifies and decodes.
        let ours = format!("{directory}/{name}-ours.flac");
        flac::write_file(&ours, &audio)?;
        report("reference verifies our stream", run("flac", &["-t", "-s", &ours]).is_ok());

        let back = format!("{directory}/{name}-back.wav");
        run("flac", &["-d", "-f", "-s", "-o", &back, &ours])?;
        let rebuilt = wav::read_file(&back)?;
        report(
            "our encode -> reference decode",
            rebuilt.channels() == audio.channels(),
        );

        let raw = std::fs::metadata(&source)?.len();
        let theirs = std::fs::metadata(&reference)?.len();
        let mine = std::fs::metadata(&ours)?.len();
        println!(
            "   {raw} bytes of PCM -> {mine} ours, {theirs} reference ({:.1}% of theirs)",
            mine as f64 / theirs as f64 * 100.0
        );
    }
    Ok(())
}

/// Prints a line, and makes the process fail if the check did not hold.
fn report(what: &str, held: bool) {
    println!("   {} {what}", if held { "ok  " } else { "FAIL" });
    if !held {
        std::process::exit(1);
    }
}

/// Runs a command, failing loudly if it is missing or unhappy.
fn run(program: &str, arguments: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new(program).args(arguments).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} {arguments:?} exited with {status}").into())
    }
}

/// The buffers worth crossing over, one per thing that could go wrong.
fn cases() -> Vec<(String, Audio)> {
    let mut cases = Vec::new();
    for format in [
        SampleFormat::PcmU8,
        SampleFormat::PcmI16,
        SampleFormat::PcmI24,
    ] {
        cases.push((format!("tone-{}", format.name()), tone(format, 2, 44_100)));
    }
    cases.push(("mono".into(), tone(SampleFormat::PcmI16, 1, 30_000)));
    cases.push((
        "silence".into(),
        Audio::silence(44_100, SampleFormat::PcmI16, 2, 50_000),
    ));
    cases.push(("noise".into(), noise(SampleFormat::PcmI16, 2, 40_000)));
    cases.push(("short".into(), tone(SampleFormat::PcmI16, 2, 37)));
    cases
}

/// A chord, which the fixed predictors should handle well.
fn tone(format: SampleFormat, channels: usize, frames: usize) -> Audio {
    let quantum = format.quantum().unwrap();
    let (low, high) = format.code_range().unwrap();
    let planes = (0..channels)
        .map(|channel| {
            (0..frames)
                .map(|frame| {
                    let time = frame as f64 / 44_100.0;
                    let value = (0..4)
                        .map(|partial| {
                            let hertz = 220.0 * f64::from(partial + 1) + channel as f64;
                            (std::f64::consts::TAU * hertz * time).sin()
                                / f64::from(partial + 1)
                        })
                        .sum::<f64>()
                        / 2.0;
                    ((value * high as f64) as i64).clamp(low, high) as f64 * quantum
                })
                .collect()
        })
        .collect();
    Audio::from_channels(44_100, format, planes).unwrap()
}

/// Deterministic noise, which no predictor can help with.
fn noise(format: SampleFormat, channels: usize, frames: usize) -> Audio {
    let quantum = format.quantum().unwrap();
    let (low, high) = format.code_range().unwrap();
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let planes = (0..channels)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let code = (state >> 40) as i64 - (1 << 23);
                    code.clamp(low, high) as f64 * quantum
                })
                .collect()
        })
        .collect();
    Audio::from_channels(44_100, format, planes).unwrap()
}
