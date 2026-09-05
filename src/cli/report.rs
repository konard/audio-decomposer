//! What the commands print, and the argument shapes they share.

use std::io::Write;
use std::path::Path;

use audio_decomposer::archive;
use audio_decomposer::associative::schema::Manifest;
use audio_decomposer::audio::Audio;
use audio_decomposer::error::Result;
use audio_decomposer::formats::project::Format;
use audio_decomposer::formats::session::SessionOptions;
use audio_decomposer::invalid_argument_error;
use audio_decomposer::recompose::Report;

/// Word that asks for every project format.
pub const ALL: &str = "all";
/// Word that asks for no project at all.
pub const NONE: &str = "none";

/// The formats a `--formats` list names.
///
/// The list is taken literally: an unknown name is an error rather than a
/// silently skipped export, because a missing project file is exactly the kind
/// of thing nobody notices until the session is opened.
pub fn formats(list: &[String]) -> Result<Vec<Format>> {
    let mut chosen: Vec<Format> = Vec::new();
    for entry in list.iter().flat_map(|item| item.split(',')) {
        let name = entry.trim();
        if name.is_empty() {
            continue;
        }
        if name.eq_ignore_ascii_case(NONE) {
            return Ok(Vec::new());
        }
        if name.eq_ignore_ascii_case(ALL) {
            for format in Format::ALL {
                if !chosen.contains(&format) {
                    chosen.push(format);
                }
            }
            continue;
        }
        let format = Format::parse(name).ok_or_else(|| {
            invalid_argument_error!(
                "{name} is not a project format; known formats are {}",
                known()
            )
        })?;
        if !chosen.contains(&format) {
            chosen.push(format);
        }
    }
    Ok(chosen)
}

/// The extensions the exporter knows, for error messages and `--help`.
#[must_use]
pub fn known() -> String {
    Format::ALL
        .iter()
        .map(|format| format.extension())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Session options built from the switches the commands share.
#[must_use]
pub fn session_options(tempo: Option<f64>, notes: bool) -> SessionOptions {
    let options = SessionOptions {
        notes,
        ..SessionOptions::default()
    };
    match tempo {
        Some(tempo) if tempo > 0.0 => options.at_tempo(tempo),
        _ => options,
    }
}

/// A duration in seconds, written the way a transport display would.
#[must_use]
pub fn duration(seconds: f64) -> String {
    let seconds = if seconds > 0.0 { seconds } else { 0.0 };
    let minutes = seconds as u64 / 60;
    let rest = seconds - (minutes * 60) as f64;
    format!("{minutes}:{rest:06.3}")
}

/// Describes the recording an archive came from.
pub fn describe_source(manifest: &Manifest, out: &mut impl Write) -> Result<()> {
    let source = &manifest.source;
    let seconds = if source.sample_rate == 0 {
        0.0
    } else {
        source.frames as f64 / f64::from(source.sample_rate)
    };
    writeln!(out, "recording: {}", source.name)?;
    writeln!(
        out,
        "  {} Hz, {} channel(s), {} frames, {} ({})",
        source.sample_rate,
        source.channels,
        source.frames,
        duration(seconds),
        source.format.name()
    )?;
    Ok(())
}

/// Describes what a decomposition holds.
pub fn describe_manifest(manifest: &Manifest, out: &mut impl Write) -> Result<()> {
    describe_source(manifest, out)?;
    let stems: Vec<&str> = manifest
        .stems
        .iter()
        .map(|stem| stem.name.as_str())
        .collect();
    writeln!(
        out,
        "stems: {}",
        if stems.is_empty() {
            "none".to_string()
        } else {
            stems.join(", ")
        }
    )?;
    let pitched = manifest
        .samples
        .iter()
        .filter(|sample| sample.note.is_some())
        .count();
    writeln!(
        out,
        "bank: {} sample(s), {} pitched, {} placement(s)",
        manifest.samples.len(),
        pitched,
        manifest.placements.len()
    )?;
    writeln!(out, "notes: {}", manifest.notes.len())?;
    if let Some(correction) = &manifest.correction {
        writeln!(out, "correction: {correction}")?;
    }
    Ok(())
}

/// Describes what a reconstruction was worth.
pub fn describe_report(report: &Report, out: &mut impl Write) -> Result<()> {
    writeln!(
        out,
        "reconstruction: {}",
        if report.is_exact() {
            "exact, to the last bit".to_string()
        } else {
            format!("off by at most {:.3e}", report.deviation)
        }
    )?;
    writeln!(
        out,
        "  stems: {}",
        if report.stems_are_exact() {
            "sum back exactly".to_string()
        } else {
            format!("sum back to within {:.3e}", report.stem_deviation)
        }
    )?;
    writeln!(
        out,
        "  bank: {} sample(s) over {} placement(s), {:.1}x reuse",
        report.samples,
        report.placements,
        report.reuse()
    )?;
    writeln!(
        out,
        "  residual holds {:.2}% of the energy",
        report.residual_share * 100.0
    )?;
    Ok(())
}

/// Describes an audio file the way `inspect` reports one.
pub fn describe_audio(path: &Path, audio: &Audio, out: &mut impl Write) -> Result<()> {
    writeln!(out, "file: {}", path.display())?;
    writeln!(
        out,
        "  {} Hz, {} channel(s), {} frames, {} ({})",
        audio.sample_rate(),
        audio.channel_count(),
        audio.frames(),
        duration(audio.duration_seconds()),
        audio.format().name()
    )?;
    writeln!(
        out,
        "  peak {:.6}, rms {:.6}, {}",
        audio.peak(),
        audio.rms(),
        if audio.is_exact_in_format() {
            "already on its format's grid"
        } else {
            "not on its format's grid"
        }
    )?;
    Ok(())
}

/// Lists files that were written, relative to where they were written.
pub fn describe_written(
    root: &Path,
    written: &[std::path::PathBuf],
    out: &mut impl Write,
) -> Result<()> {
    writeln!(
        out,
        "wrote {} project file(s) in {}",
        written.len(),
        root.display()
    )?;
    for path in written {
        writeln!(
            out,
            "  {}",
            path.strip_prefix(root).unwrap_or(path).display()
        )?;
    }
    Ok(())
}

/// Reads the manifest of an archive, saying which directory failed when it is
/// not one.
pub fn manifest(root: &Path) -> Result<Manifest> {
    archive::read_manifest(root).map_err(|error| {
        invalid_argument_error!(
            "{} does not look like a decomposition directory ({error})",
            root.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn a_list_of_formats_is_read_in_the_order_it_was_written() {
        let chosen = formats(&names(&["sfz,midi"])).unwrap();
        assert_eq!(chosen, vec![Format::Sfz, Format::Midi]);
    }

    #[test]
    fn a_format_named_twice_is_exported_once() {
        let chosen = formats(&names(&["midi", "midi,all,midi"])).unwrap();
        assert_eq!(chosen.len(), Format::ALL.len());
        assert_eq!(chosen[0], Format::Midi);
    }

    #[test]
    fn the_word_none_cancels_everything_before_it() {
        assert!(formats(&names(&["all,none"])).unwrap().is_empty());
    }

    #[test]
    fn an_unknown_format_is_refused_rather_than_skipped() {
        let error = formats(&names(&["midi,protools"])).unwrap_err().to_string();
        assert!(error.contains("protools"), "{error}");
        assert!(error.contains("mid"), "{error}");
    }

    #[test]
    fn blank_entries_are_ignored() {
        assert!(formats(&names(&["", " , "])).unwrap().is_empty());
    }

    #[test]
    fn a_tempo_that_was_given_is_the_tempo_that_is_used() {
        let options = session_options(Some(96.0), true);
        assert!((options.tempo - 96.0).abs() < f64::EPSILON);
        assert!(!options.estimate_tempo);
        assert!(options.notes);
    }

    #[test]
    fn an_absent_or_impossible_tempo_leaves_the_music_to_say() {
        for given in [None, Some(0.0), Some(-8.0)] {
            let options = session_options(given, false);
            assert!(options.estimate_tempo, "{given:?}");
            assert!(!options.notes);
        }
    }

    #[test]
    fn durations_are_written_the_way_a_transport_shows_them() {
        assert_eq!(duration(0.0), "0:00.000");
        assert_eq!(duration(8.0), "0:08.000");
        assert_eq!(duration(59.9995), "0:59.999");
        assert_eq!(duration(90.5), "1:30.500");
        assert_eq!(duration(3_601.25), "60:01.250");
        assert_eq!(duration(-1.0), "0:00.000");
        assert_eq!(duration(f64::NAN), "0:00.000");
    }
}
