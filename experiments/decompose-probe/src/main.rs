//! Prints how synthetic recordings of repeated hits are cut into events, how
//! each event is matched, and what the driver ends up storing, so the
//! deduplication can be judged step by step.

use audio_decomposer::audio::{Audio, SampleFormat};
use audio_decomposer::decompose::{dedup, events, DecomposeOptions, EventOptions};
use audio_decomposer::dsp::envelope;

fn hit(sample_rate: u32, frequency: f64, length: usize) -> Vec<f64> {
    (0..length)
        .map(|index| {
            let time = index as f64 / f64::from(sample_rate);
            let decay = (-8.0 * time).exp();
            0.6 * decay
                * ((std::f64::consts::TAU * frequency * time).sin()
                    + 0.4 * (std::f64::consts::TAU * 2.0 * frequency * time).sin())
                / 1.4
        })
        .collect()
}

/// The mono fixture used by the tests: `times` identical strikes followed by
/// one spacing of silence.
fn repeated(sample_rate: u32, times: usize, spacing: usize) -> Audio {
    let mut signal = vec![0.0; spacing * (times + 1)];
    let sound = hit(sample_rate, 220.0, spacing);
    for time in 0..times {
        for (index, value) in sound.iter().enumerate() {
            signal[time * spacing + index] += value;
        }
    }
    let mut audio = Audio::from_mono(sample_rate, SampleFormat::PcmI16, signal).unwrap();
    audio.quantize();
    audio
}

fn report(title: &str, audio: &Audio, expected: &[usize]) {
    println!("== {title}");
    println!("   expected onsets {expected:?}");
    let found = events::detect(audio, &EventOptions::default()).unwrap();
    for event in &found {
        println!(
            "   event channel {} start {:6} length {:6} rms {:.6}",
            event.channel,
            event.start,
            event.length,
            envelope::rms(event.slice(audio.channels()))
        );
    }

    let options = DecomposeOptions::default().without_analysis();
    let decomposition = audio_decomposer::decompose::decompose(audio, &options).unwrap();
    println!("   bank: {} samples", decomposition.samples.len());
    for sample in &decomposition.samples {
        println!("     {} length {}", sample.name, sample.len());
    }
    for placement in &decomposition.placements {
        println!(
            "     placement sample {} channel {} start {:6} gain {:.4}",
            placement.sample,
            placement.channel,
            placement.start,
            placement.gain.factor()
        );
    }
    for (index, channel) in decomposition.residual.channels().iter().enumerate() {
        let peak = channel.iter().fold(0.0_f64, |best, value| best.max(value.abs()));
        let where_peak = channel
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(position, _)| position)
            .unwrap_or(0);
        println!("   residual channel {index}: peak {peak:.6} at frame {where_peak}");
    }
    println!(
        "   reconstruction deviation {}",
        decomposition.deviation_from(audio).unwrap()
    );
}

/// How well the first event explains the later ones, at several radii.
fn alignments(audio: &Audio) {
    let found = events::detect(audio, &EventOptions::default()).unwrap();
    let pattern = found[0].slice(audio.channels()).to_vec();
    for event in &found[1..] {
        for radius in [0_usize, 64, 256] {
            let matching = dedup::MatchOptions {
                search_radius: radius,
                tolerance: 1.0,
                ..dedup::MatchOptions::default()
            };
            let alignment = dedup::best_alignment(
                audio.channel(event.channel),
                event.start as i64,
                &pattern,
                &matching,
            );
            match alignment {
                Some(alignment) => println!(
                    "   event channel {} at {:6} radius {:4}: start {:6} (shift {:5}) gain {:.4} remainder {:.4}",
                    event.channel,
                    event.start,
                    radius,
                    alignment.start,
                    alignment.start - event.start as i64,
                    alignment.gain.factor(),
                    alignment.remainder
                ),
                None => println!(
                    "   event channel {} at {:6} radius {:4}: no alignment",
                    event.channel, event.start, radius
                ),
            }
        }
    }
}

fn main() {
    let sample_rate = 22_050_u32;

    let mono = repeated(sample_rate, 4, 5512);
    report("four strikes, mono", &mono, &[0, 5512, 11024, 16536]);
    alignments(&mono);

    let short = repeated(sample_rate, 2, 4096);
    report("two strikes, mono", &short, &[0, 4096]);

    let channel = short.channel(0).to_vec();
    let stereo = Audio::from_channels(
        sample_rate,
        SampleFormat::PcmI16,
        vec![channel.clone(), channel],
    )
    .unwrap();
    report("two strikes, duplicated channels", &stereo, &[0, 4096]);
    alignments(&stereo);
}
