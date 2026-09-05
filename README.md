# audio-decomposer

Take a recording apart into the pieces it is made of, and put it back together
without losing a single sample.

[![CI/CD Pipeline](https://github.com/konard/audio-decomposer/workflows/CI%2FCD%20Pipeline/badge.svg)](https://github.com/konard/audio-decomposer/actions?workflow=CI%2FCD+Pipeline)
[![Crates.io](https://img.shields.io/crates/v/audio-decomposer?label=crates.io&style=flat)](https://crates.io/crates/audio-decomposer)
[![Docs.rs](https://img.shields.io/docsrs/audio-decomposer?label=docs.rs&style=flat)](https://docs.rs/audio-decomposer)
[![Rust Version](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org/)
[![License: Unlicense](https://img.shields.io/badge/license-Unlicense-blue.svg)](https://unlicense.org/)

A song is mostly repetition. The same kick, the same snare, the same plucked
string, played again at a different moment with a different loudness.
`audio-decomposer` finds those repetitions, stores each distinct waveform once,
and describes the recording as a list of places where the stored waveforms are
used. What is left over after every match is subtracted is kept as a residual,
so the reconstruction is not an approximation: for integer PCM input it is the
same file, byte for byte.

The result is then written out in the formats music software already reads —
MIDI, MusicXML, DAWproject, Ableton Live, FL Studio, REAPER, Ardour, LMMS and
SFZ — so the decomposition can be opened, edited and played somewhere else.

## What it does

```
recording ─► stems ─► events ─► sample bank + placements + notes ─► projects
     ▲                                                                  │
     └───────────────── exact reconstruction ◄──────────────────────────┘
```

1. **Decode** a WAV or AIFF container into planar `f64` channels that stay on
   their exact integer grid, so nothing is lost before analysis begins.
2. **Split** the recording into a harmonic and a percussive stem, plus a
   correction stem that makes the three sum back to the input exactly.
3. **Cut** each stem into events at detected onsets, snapped to the attack the
   onset belongs to.
4. **Deduplicate** the events into a sample bank: every event is matched against
   the waveforms already stored, at every plausible shift and gain, and the best
   match is subtracted. Only what does not match anything becomes a new sample.
5. **Recognise** the note of every placement, monophonic and polyphonic, so the
   music can be written as notes and not only as audio.
6. **Store** the whole result as a doublet link network in Links Notation, where
   every repeated value — a number, a name, a pair — exists exactly once.
7. **Export** projects, stems and samples, and **reconstruct** the original.

## Install

```bash
cargo install audio-decomposer
```

Or build from a clone:

```bash
git clone https://github.com/konard/audio-decomposer.git
cd audio-decomposer
cargo build --release
```

## Command line

```bash
# Take a recording apart. Writes song-decomposition/ beside the file.
audio-decomposer decompose song.wav

# Put it back together and check it against the original.
audio-decomposer compose song-decomposition -o rebuilt.wav
audio-decomposer verify song-decomposition song.wav --strict

# Look at what is inside, without changing anything.
audio-decomposer inspect song-decomposition

# Write DAW projects beside a decomposition that already exists.
audio-decomposer export song-decomposition --formats midi,musicxml,ableton

# Play a score through the sample bank of a decomposition.
audio-decomposer render song-decomposition --score melody.mid -o melody.wav
```

| Command     | What it does                                                        |
| ----------- | ------------------------------------------------------------------- |
| `decompose` | Takes a recording apart into stems, a sample bank, notes and projects |
| `compose`   | Puts a decomposition back together into an audio file                 |
| `verify`    | Checks that a decomposition reproduces the recording it came from     |
| `inspect`   | Describes a recording or a decomposition                              |
| `export`    | Writes DAW projects beside an existing decomposition                  |
| `render`    | Plays a score through the sample bank                                 |

Every command takes `--help`. The options that matter most for `decompose`:

| Option              | Meaning                                                        |
| ------------------- | -------------------------------------------------------------- |
| `--formats`         | Project formats to export, `all`, `none` or a list              |
| `--tempo`           | Tempo the projects carry; measured from the music when absent   |
| `--exact-repeats`   | Reuse a waveform only where it repeats verbatim: faster         |
| `--no-stems`        | Keep the recording whole instead of splitting it                |
| `--no-notes`        | Skip note recognition                                           |
| `--search-seconds`  | How far a match may be shifted                                  |
| `--candidates`      | How many stored waveforms are tried per event                   |

## Library

```rust,no_run
use audio_decomposer::decompose::{decompose, DecomposeOptions};
use audio_decomposer::{audio, recompose};

let original = audio::read_file("song.wav")?;
let decomposition = decompose(&original, &DecomposeOptions::default())?;
let rebuilt = recompose::reconstruct(&decomposition);

assert_eq!(original.channels(), rebuilt.channels());
# Ok::<(), audio_decomposer::Error>(())
```

The modules mirror the pipeline:

| Module        | Responsibility                                                    |
| ------------- | ----------------------------------------------------------------- |
| `audio`       | WAV and AIFF codecs, planar buffers on an exact integer grid       |
| `dsp`         | FFT, STFT, windows, onsets, pitch detection, stem separation       |
| `decompose`   | Stems, events, the sample bank, matching pursuit, note recognition |
| `associative` | The doublet link network and its Links Notation projection         |
| `archive`     | Writing a decomposition to a directory and reading it back         |
| `formats`     | MIDI, MusicXML, DAWproject and the DAW project writers             |
| `recompose`   | Rebuilding the audio, sample-exactly for integer PCM               |

See [docs/lib-overview.md](docs/lib-overview.md) and the
[API documentation](https://docs.rs/audio-decomposer).

## The archive

A decomposition is a directory, not an opaque blob:

```
song-decomposition/
├── decomposition.lino   # the whole structure as a doublet link network
├── samples/             # every distinct waveform, once
├── stems/               # harmonic, percussive and the correction that closes the sum
├── residual.wav         # what no sample explains
├── song.mid             # the projects, one per format
├── song.musicxml
├── song.dawproject
├── song.als
├── song.flp
├── song.rpp
├── song.ardour
├── song.mmp
├── song.sfz
└── interchange/         # the MIDI files an Ardour session keeps outside itself
```

`decomposition.lino` is the source of truth. Everything else is written from it,
and reading the directory back gives the same decomposition it was written from.

## Formats

| Format     | Extension    | Written | Read back |
| ---------- | ------------ | ------- | --------- |
| MIDI       | `.mid`       | yes     | yes       |
| MusicXML   | `.musicxml`  | yes     | yes       |
| DAWproject | `.dawproject`| yes     | yes       |
| Ableton    | `.als`       | yes     | yes       |
| FL Studio  | `.flp`       | yes     | yes       |
| REAPER     | `.rpp`       | yes     | yes       |
| Ardour     | `.ardour`    | yes     | yes       |
| LMMS       | `.mmp`       | yes     | yes       |
| SFZ        | `.sfz`       | yes     | yes       |
| WAV, AIFF  | `.wav`, `.aiff` | yes  | yes       |

Every project format has a reader as well as a writer, and the integration
suite asserts that a session written by the exporter reads back as the session
it was written from.

## Testing

The tests never use copyrighted audio. Every fixture is synthesised inside the
test process — plucked strings, noise hits, chord progressions, wide stereo
images, silence — so nothing is committed to the repository and nothing is
downloaded when the suite runs.

The round trip is checked in memory, through an archive on disk, across all
four PCM sample formats, through both containers and through the command line.

```bash
cargo test                            # everything
cargo test --test integration         # the round trip suite
cargo test --test unit                # unit tests
```

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features
rust-script scripts/check-file-size.rs   # cargo install rust-script
cargo test
```

Add a changelog fragment for every change that should appear in a release:

```bash
touch changelog.d/$(date +%Y%m%d_%H%M%S)_my_change.md
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full workflow, and
[docs/ci-cd](docs/ci-cd) for how the pipeline is put together.

## Project layout

```
.
├── src/
│   ├── audio/         # containers, codecs, planar buffers
│   ├── dsp/           # FFT, STFT, onsets, pitch, stem separation
│   ├── decompose/     # stems, events, sample bank, notes
│   ├── associative/   # doublet link network, Links Notation
│   ├── archive/       # decomposition directories
│   ├── formats/       # MIDI, MusicXML, DAW projects, SFZ
│   ├── recompose/     # exact reconstruction
│   ├── cli/           # the six commands
│   ├── error.rs
│   ├── lib.rs
│   └── main.rs
├── tests/
│   ├── unit/          # unit tests, including the CI/CD scripts
│   └── integration/   # round trip, projects, command line
├── examples/          # runnable use cases
├── experiments/       # probes kept from debugging sessions
├── scripts/           # rust-script CI/CD utilities
├── docs/              # library overview, CI/CD notes, case studies
└── changelog.d/       # changelog fragments
```

## License

[Unlicense](LICENSE) — public domain. This is free and unencumbered software
released into the public domain.

## Acknowledgments

- [lino-arguments](https://github.com/link-foundation/lino-arguments) for
  argument parsing with `.lenv` support.
- The associative tech stack described by
  [formal-ai](https://github.com/link-assistant/formal-ai), which is why a
  decomposition is stored as doublets rather than as a bespoke file format.
- [rust-ai-driven-development-pipeline-template](https://github.com/link-foundation/rust-ai-driven-development-pipeline-template)
  for the CI/CD pipeline this repository is built on.
