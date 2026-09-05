//! The binary itself: what a person at a shell gets.
//!
//! These run the compiled command rather than the library behind it, so the
//! argument parsing, the exit statuses and the messages on standard error are
//! all part of what is under test.

use std::fs;

use audio_decomposer::audio::{self, SampleFormat};

use super::fixtures::{cli, repetitive_loop, stderr, stdout, succeeds, Scratch};

/// Writes a fixture into a scratch directory and returns where it went.
fn recording(scratch: &Scratch, name: &str) -> String {
    let path = scratch.join(name);
    audio::write_file(&path, &repetitive_loop(8_000, SampleFormat::PcmI16, 1)).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn the_whole_pipeline_runs_from_the_shell_and_gives_the_file_back() {
    let scratch = Scratch::new("cli-pipeline");
    let source = recording(&scratch, "loop.wav");
    let archive = scratch.join("out");
    let archive = archive.to_str().unwrap();

    let report = succeeds(
        &cli(&[
            "decompose",
            &source,
            "--output",
            archive,
            "--formats",
            "all",
        ]),
        "decompose",
    );
    assert!(report.contains("recording: loop"), "{report}");
    assert!(report.contains("reconstruction: exact"), "{report}");
    assert!(report.contains("loop.mid"), "{report}");
    assert!(report.contains("loop.als"), "{report}");
    assert!(report.contains("loop.flp"), "{report}");

    succeeds(&cli(&["verify", archive, &source, "--strict"]), "verify");

    let rebuilt = scratch.join("rebuilt.wav");
    succeeds(
        &cli(&["compose", archive, "-o", rebuilt.to_str().unwrap()]),
        "compose",
    );
    assert_eq!(
        fs::read(&source).unwrap(),
        fs::read(&rebuilt).unwrap(),
        "the file the shell gave back is not the file it was given"
    );
}

#[test]
fn a_decomposition_lands_beside_its_recording_when_no_output_is_named() {
    let scratch = Scratch::new("cli-default-output");
    let source = recording(&scratch, "take.wav");
    succeeds(
        &cli(&["decompose", &source, "--formats", "none"]),
        "decompose",
    );
    let expected = scratch.join("take-decomposition");
    assert!(expected.join("decomposition.lino").exists());
    assert!(
        !expected.join("take.mid").exists(),
        "--formats none exported anyway"
    );
}

#[test]
fn asking_for_no_projects_leaves_the_archive_alone() {
    let scratch = Scratch::new("cli-no-projects");
    let source = recording(&scratch, "loop.wav");
    let archive = scratch.join("out");
    let report = succeeds(
        &cli(&[
            "decompose",
            &source,
            "--output",
            archive.to_str().unwrap(),
            "--formats",
            "none",
            "--no-notes",
        ]),
        "decompose",
    );
    assert!(!report.contains("wrote"), "{report}");
    assert!(report.contains("notes: 0"), "{report}");
}

#[test]
fn a_tempo_that_is_given_is_the_tempo_the_projects_carry() {
    let scratch = Scratch::new("cli-tempo");
    let source = recording(&scratch, "loop.wav");
    let archive = scratch.join("out");
    let archive = archive.to_str().unwrap();
    succeeds(
        &cli(&[
            "decompose",
            &source,
            "--output",
            archive,
            "--formats",
            "none",
        ]),
        "decompose",
    );

    let exported = succeeds(
        &cli(&["export", archive, "--formats", "midi", "--tempo", "93.5"]),
        "export",
    );
    assert!(exported.contains("93.500 BPM"), "{exported}");

    let measured = succeeds(&cli(&["inspect", archive]), "inspect");
    assert!(measured.contains("BPM"), "{measured}");
    assert!(!measured.contains("93.500 BPM"), "{measured}");
}

#[test]
fn a_score_is_played_back_through_the_bank() {
    let scratch = Scratch::new("cli-render");
    let source = recording(&scratch, "loop.wav");
    let archive = scratch.join("out");
    let archive = archive.to_str().unwrap();
    succeeds(
        &cli(&[
            "decompose",
            &source,
            "--output",
            archive,
            "--formats",
            "midi",
        ]),
        "decompose",
    );

    let render = scratch.join("render.wav");
    let played = succeeds(
        &cli(&[
            "render",
            archive,
            "--score",
            &format!("{archive}/loop.mid"),
            "-o",
            render.to_str().unwrap(),
            "--trim",
        ]),
        "render",
    );
    assert!(played.contains("loop.mid"), "{played}");
    assert!(render.exists());
    assert_eq!(audio::read_file(&render).unwrap().sample_rate(), 8_000);
}

#[test]
fn a_missing_file_is_reported_on_standard_error_with_a_failing_status() {
    let output = cli(&["decompose", "/nowhere/at/all.wav"]);
    assert!(!output.status.success());
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.starts_with("audio-decomposer: "), "{message}");
    assert!(message.contains("all.wav"), "{message}");
}

#[test]
fn an_unknown_project_format_is_refused_before_anything_is_written() {
    let scratch = Scratch::new("cli-bad-format");
    let source = recording(&scratch, "loop.wav");
    let archive = scratch.join("out");
    let output = cli(&[
        "decompose",
        &source,
        "--output",
        archive.to_str().unwrap(),
        "--formats",
        "cubase",
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("cubase"), "{}", stderr(&output));
    assert!(
        !archive.exists(),
        "the archive was written even though the export could never have run"
    );
}

#[test]
fn a_decomposition_that_does_not_match_is_refused_under_strict() {
    let scratch = Scratch::new("cli-strict");
    let source = recording(&scratch, "loop.wav");
    let archive = scratch.join("out");
    let archive = archive.to_str().unwrap();
    succeeds(
        &cli(&[
            "decompose",
            &source,
            "--output",
            archive,
            "--formats",
            "none",
        ]),
        "decompose",
    );

    // The same recording with one sample moved: the same shape, so the check
    // runs, but not the same music, so it cannot come out exact.
    let mut changed = repetitive_loop(8_000, SampleFormat::PcmI16, 1);
    changed.channels_mut()[0][1_000] += 0.25;
    changed.quantize();
    let other = scratch.join("other.wav");
    audio::write_file(&other, &changed).unwrap();

    let output = cli(&["verify", archive, other.to_str().unwrap(), "--strict"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("does not reproduce"),
        "{}",
        stderr(&output)
    );

    // Without `--strict` the same check reports and succeeds.
    let lenient = cli(&["verify", archive, other.to_str().unwrap()]);
    assert!(lenient.status.success(), "{}", stderr(&lenient));
    assert!(stdout(&lenient).contains("off by"), "{}", stdout(&lenient));
}

#[test]
fn the_help_names_every_command() {
    let help = succeeds(&cli(&["--help"]), "--help");
    for command in [
        "decompose",
        "compose",
        "verify",
        "inspect",
        "export",
        "render",
    ] {
        assert!(help.contains(command), "--help does not mention {command}");
    }
    let version = succeeds(&cli(&["--version"]), "--version");
    assert!(version.contains(env!("CARGO_PKG_VERSION")), "{version}");
}
