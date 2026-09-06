//! Writing every project file beside the audio it points at.
//!
//! Each exporter in this module's siblings turns a [`Session`] into one
//! format. This module is the step after that: it puts the project files into
//! the directory the archive already holds, so that opening any one of them
//! finds `samples/`, `stems/` and `residual.wav` exactly where the clips say
//! they are. Nothing is copied and nothing is converted; the projects point at
//! the same audio the archive was written with.
//!
//! ```text
//! song.decomposition/
//!   decomposition.lino        the archive manifest
//!   samples/kick-01.wav       the audio the archive wrote
//!   song.mid                  the notes, for anything that reads MIDI
//!   song.musicxml             the notes, as a score
//!   song.dawproject           Bitwig, Studio One, and the audio inside it
//!   song.als                  Ableton Live
//!   song.flp                  FL Studio
//!   song.rpp                  REAPER
//!   song.ardour               Ardour, plus interchange/song/midifiles/*.mid
//!   song.mmp                  LMMS
//!   song.sfz                  the bank as one playable instrument
//! ```
//!
//! DAWproject is the one exception to nothing being copied: it is a single
//! archive rather than a directory, so the audio a session refers to is read
//! back out of the directory and packed inside it. That makes it the one file
//! to send someone who has nothing else.

use std::fs;
use std::path::{Path, PathBuf};

use crate::archive;
use crate::error::{Error, Result};
use crate::formats::session::{Session, SessionOptions};
use crate::formats::zip;
use crate::formats::{ableton, ardour, dawproject, flstudio, lmms, midi, musicxml, reaper, sfz};

/// Name used for a session that has none.
pub const FALLBACK_NAME: &str = "project";

/// One of the formats a session can be written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// A standard MIDI file.
    Midi,
    /// A MusicXML score.
    MusicXml,
    /// A DAWproject archive.
    DawProject,
    /// An Ableton Live set.
    Ableton,
    /// An FL Studio project.
    FlStudio,
    /// A REAPER project.
    Reaper,
    /// An Ardour session.
    Ardour,
    /// An LMMS project.
    Lmms,
    /// An SFZ instrument.
    Sfz,
}

impl Format {
    /// Every format, in the order [`write_all`] writes them.
    pub const ALL: [Self; 9] = [
        Self::Midi,
        Self::MusicXml,
        Self::DawProject,
        Self::Ableton,
        Self::FlStudio,
        Self::Reaper,
        Self::Ardour,
        Self::Lmms,
        Self::Sfz,
    ];

    /// The extension a file of this format carries.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Midi => "mid",
            Self::MusicXml => "musicxml",
            Self::DawProject => "dawproject",
            Self::Ableton => "als",
            Self::FlStudio => "flp",
            Self::Reaper => "rpp",
            Self::Ardour => "ardour",
            Self::Lmms => "mmp",
            Self::Sfz => "sfz",
        }
    }

    /// What to call the format when telling someone what was written.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Midi => "MIDI",
            Self::MusicXml => "MusicXML",
            Self::DawProject => "DAWproject",
            Self::Ableton => "Ableton Live",
            Self::FlStudio => "FL Studio",
            Self::Reaper => "REAPER",
            Self::Ardour => "Ardour",
            Self::Lmms => "LMMS",
            Self::Sfz => "SFZ",
        }
    }

    /// The format a name or an extension asks for.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('.').to_ascii_lowercase();
        let known = Self::ALL.into_iter().find(|format| {
            format.extension() == text || format.title().to_ascii_lowercase() == text
        });
        known.or(match text.as_str() {
            "midi" | "smf" => Some(Self::Midi),
            "xml" | "musicxml3" => Some(Self::MusicXml),
            "live" | "als" => Some(Self::Ableton),
            "fl" | "flstudio" => Some(Self::FlStudio),
            "reaper" | "rpp" => Some(Self::Reaper),
            "mmpz" | "lmms" => Some(Self::Lmms),
            _ => None,
        })
    }
}

/// The name a session's files are given.
#[must_use]
pub fn stem(session: &Session) -> String {
    let name: String = session
        .name
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            other => other,
        })
        .collect();
    let name = name.trim().trim_matches('.').to_string();
    if name.is_empty() {
        FALLBACK_NAME.to_string()
    } else {
        name
    }
}

/// Writes every project file for `session` into `root`.
///
/// The audio the session points at has to be in `root` already, which is what
/// [`crate::archive::write_dir`] puts there. Returns the files written, in the
/// order they were written.
pub fn write_all(root: impl AsRef<Path>, session: &Session) -> Result<Vec<PathBuf>> {
    write_these(root, session, &Format::ALL)
}

/// Writes the chosen project files for `session` into `root`.
pub fn write_these(
    root: impl AsRef<Path>,
    session: &Session,
    formats: &[Format],
) -> Result<Vec<PathBuf>> {
    let root = root.as_ref();
    let mut written = Vec::new();
    for format in formats {
        written.extend(write(root, session, *format)?);
    }
    Ok(written)
}

/// Writes one project file, and whatever else that format needs beside it.
pub fn write(root: impl AsRef<Path>, session: &Session, format: Format) -> Result<Vec<PathBuf>> {
    let root = root.as_ref();
    fs::create_dir_all(root)?;
    let name = stem(session);
    let path = root.join(format!(
        "{name}.{extension}",
        extension = format.extension()
    ));

    match format {
        Format::Midi => {
            let notes: Vec<_> = session.notes().copied().collect();
            let file = midi::from_notes(&notes, session.sample_rate, &name)?;
            midi::write_file(&path, &file)?;
        }
        Format::MusicXml => musicxml::write_file(&path, session)?,
        Format::DawProject => {
            dawproject::write_file(&path, session, &audio_entries(root, session)?)?;
        }
        Format::Ableton => ableton::write_file(&path, session)?,
        Format::FlStudio => flstudio::write_file(&path, session)?,
        Format::Reaper => reaper::write_file(&path, session)?,
        Format::Ardour => {
            ardour::write_file(&path, session)?;
            let mut written = vec![path];
            written.extend(ardour_midi(root, session, &name)?);
            return Ok(written);
        }
        Format::Lmms => lmms::write_file(&path, session)?,
        Format::Sfz => sfz::write_file(&path, session)?,
    }
    Ok(vec![path])
}

/// The MIDI files an Ardour session keeps its notes in.
fn ardour_midi(root: &Path, session: &Session, name: &str) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for track in session.instrument_tracks() {
        if track.notes.is_empty() {
            continue;
        }
        let path = root.join(ardour::midi_path(name, &track.name));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = midi::from_notes(&track.notes, session.sample_rate, &track.name)?;
        midi::write_file(&path, &file)?;
        written.push(path);
    }
    Ok(written)
}

/// Reads the audio a session points at, to be packed into an archive format.
fn audio_entries(root: &Path, session: &Session) -> Result<Vec<zip::Entry>> {
    let mut entries = Vec::new();
    for file in session.files() {
        let path = root.join(&file);
        if !path.is_file() {
            continue;
        }
        entries.push(zip::Entry::new(file, fs::read(&path)?));
    }
    Ok(entries)
}

/// Reads the archive in `root` and writes every project file beside its audio.
///
/// This is the whole of exporting: the manifest says what the decomposition
/// holds and where its audio is, and the projects are written to match.
pub fn export(root: impl AsRef<Path>, options: &SessionOptions) -> Result<Vec<PathBuf>> {
    export_these(root, options, &Format::ALL)
}

/// Exports an archive as the projects that were asked for.
pub fn export_these(
    root: impl AsRef<Path>,
    options: &SessionOptions,
    formats: &[Format],
) -> Result<Vec<PathBuf>> {
    let root = root.as_ref();
    let manifest = archive::read_manifest(root)?;
    let session = session_of(root, &manifest, options);
    write_these(root, &session, formats)
}

/// The session an archive holds, named after its directory when the manifest
/// left the name empty.
#[must_use]
pub fn session_of(
    root: impl AsRef<Path>,
    manifest: &crate::associative::schema::Manifest,
    options: &SessionOptions,
) -> Session {
    let mut session = Session::from_manifest(manifest, options);
    if session.name.is_empty() {
        session.name = root
            .as_ref()
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(FALLBACK_NAME)
            .to_string();
    }
    session
}

/// Reads back what was written, as far as each format can be read back.
///
/// Every format here is one this crate can also read, so an export can be
/// checked rather than trusted. Formats that hold no session of their own,
/// which is MIDI and SFZ, are checked by parsing instead.
pub fn read(path: impl AsRef<Path>) -> Result<Session> {
    let path = path.as_ref();
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let format = Format::parse(extension)
        .ok_or_else(|| Error::Format(format!("`{extension}` is not a project this crate reads")))?;
    match format {
        Format::DawProject => dawproject::read_file(path, None),
        Format::Ableton => ableton::read_file(path),
        Format::FlStudio => flstudio::read_file(path),
        Format::Reaper => reaper::read_file(path),
        Format::Ardour => ardour::read_file(path),
        Format::Lmms => lmms::read_file(path),
        Format::Midi | Format::MusicXml | Format::Sfz => Err(Error::Format(format!(
            "a {title} file holds no session of its own",
            title = format.title()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{wav, Audio, SampleFormat};
    use crate::decompose::model::Note;
    use crate::formats::session::{Clip, Instrument, Track, TrackKind};

    /// One beat at 120 bpm and 48 kHz.
    const BEAT: usize = 24_000;

    fn session() -> Session {
        Session {
            name: "song".to_string(),
            sample_rate: 48_000,
            tempo: 120.0,
            frames: 8 * BEAT,
            channels: 1,
            tracks: vec![
                Track {
                    name: "kick".to_string(),
                    kind: TrackKind::Audio,
                    clips: vec![Clip {
                        name: "kick".to_string(),
                        file: "samples/kick.wav".to_string(),
                        start: 0,
                        length: BEAT,
                        offset: 0,
                        gain: 1.0,
                        channel: 0,
                    }],
                    notes: Vec::new(),
                },
                Track {
                    name: "notes".to_string(),
                    kind: TrackKind::Instrument,
                    clips: Vec::new(),
                    notes: vec![Note {
                        channel: 0,
                        start: 0,
                        length: BEAT,
                        note: 60,
                        velocity: 100,
                        frequency: 261.625_565_300_598_6,
                        confidence: 1.0,
                    }],
                },
            ],
            instruments: vec![Instrument {
                id: 0,
                name: "kick".to_string(),
                file: "samples/kick.wav".to_string(),
                note: Some(36),
                frames: BEAT,
                peak: 0.9,
            }],
        }
    }

    /// A directory holding the audio a session points at.
    fn directory(label: &str) -> PathBuf {
        let mut root = std::env::temp_dir();
        root.push(format!(
            "audio-decomposer-project-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("samples")).unwrap();
        let audio = Audio::from_mono(48_000, SampleFormat::PcmI16, vec![0.25; BEAT]).unwrap();
        wav::write_file(root.join("samples/kick.wav"), &audio).unwrap();
        root
    }

    #[test]
    fn every_format_lands_beside_the_audio_it_points_at() {
        let root = directory("all");

        let written = write_all(&root, &session()).unwrap();

        for format in Format::ALL {
            let path = root.join(format!("song.{}", format.extension()));
            assert!(path.is_file(), "{} was not written", format.title());
            assert!(written.contains(&path));
            assert!(fs::metadata(&path).unwrap().len() > 0);
        }
        // Ardour keeps its notes in a MIDI file of their own.
        assert!(root.join("interchange/song/midifiles/notes.mid").is_file());
        assert_eq!(written.len(), Format::ALL.len() + 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn what_was_written_reads_back_as_the_session_it_came_from() {
        let root = directory("read");
        let expected = session();

        write_all(&root, &expected).unwrap();

        for format in [
            Format::DawProject,
            Format::Ableton,
            Format::FlStudio,
            Format::Reaper,
            Format::Ardour,
            Format::Lmms,
        ] {
            let read = read(root.join(format!("song.{}", format.extension())))
                .unwrap_or_else(|error| panic!("{}: {error}", format.title()));
            assert_eq!(read.sample_rate, 48_000, "{}", format.title());
            assert!((read.tempo - 120.0).abs() < 1e-6, "{}", format.title());
            let clip = read
                .audio_tracks()
                .flat_map(|track| &track.clips)
                .find(|clip| clip.file.ends_with("kick.wav"))
                .unwrap_or_else(|| panic!("{} lost the clip", format.title()));
            assert_eq!(clip.start, 0, "{}", format.title());
            assert_eq!(clip.length, BEAT, "{}", format.title());
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_dawproject_archive_carries_the_audio_inside_it() {
        let root = directory("packed");

        write(&root, &session(), Format::DawProject).unwrap();

        let entries = zip::read_file(root.join("song.dawproject")).unwrap();
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert!(names.contains(&"samples/kick.wav"), "{names:?}");
        let packed = entries
            .iter()
            .find(|entry| entry.name == "samples/kick.wav")
            .unwrap();
        assert_eq!(
            packed.data,
            fs::read(root.join("samples/kick.wav")).unwrap()
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_archive_exports_itself() {
        use crate::decompose::{self, DecomposeOptions};

        let root = directory("export");
        let _ = fs::remove_dir_all(&root);
        let frames: Vec<f64> = (0..12_000)
            .map(|at| {
                let phase = f64::from(at) * 220.0 * std::f64::consts::TAU / 48_000.0;
                phase.sin() * 0.3
            })
            .collect();
        let audio = Audio::from_mono(48_000, SampleFormat::PcmI16, frames).unwrap();
        let decomposition = decompose::decompose(&audio, &DecomposeOptions::default()).unwrap();
        archive::write_dir(&root, &decomposition).unwrap();

        let written = export(&root, &SessionOptions::default()).unwrap();

        // The manifest names the recording, so the projects carry that name.
        let name = archive::read_manifest(&root).unwrap().source.name;
        assert!(written
            .iter()
            .any(|path| path.file_name().unwrap() == format!("{name}.rpp").as_str()));
        let read = read(root.join(format!("{name}.rpp"))).unwrap();
        assert_eq!(read.sample_rate, 48_000);
        assert!(!read.tracks.is_empty());
        // Every clip the projects point at is a file that is really there.
        for file in Session::from_manifest(
            &archive::read_manifest(&root).unwrap(),
            &SessionOptions::default(),
        )
        .files()
        {
            assert!(root.join(&file).is_file(), "{file} is missing");
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_name_a_file_system_would_refuse_is_made_safe() {
        let mut session = session();
        session.name = "a/song: take 2?".to_string();
        assert_eq!(stem(&session), "a-song- take 2-");

        session.name = "  ...  ".to_string();
        assert_eq!(stem(&session), FALLBACK_NAME);
    }

    #[test]
    fn a_format_can_be_asked_for_by_name_or_by_extension() {
        assert_eq!(Format::parse("rpp"), Some(Format::Reaper));
        assert_eq!(Format::parse(".REAPER"), Some(Format::Reaper));
        assert_eq!(Format::parse("Ableton Live"), Some(Format::Ableton));
        assert_eq!(Format::parse("midi"), Some(Format::Midi));
        assert_eq!(Format::parse("mmpz"), Some(Format::Lmms));
        assert_eq!(Format::parse("wav"), None);
    }

    #[test]
    fn formats_that_hold_no_session_say_so() {
        let root = directory("noread");
        write(&root, &session(), Format::Midi).unwrap();

        let error = read(root.join("song.mid")).unwrap_err().to_string();
        assert!(error.contains("holds no session"), "{error}");
        let error = read(root.join("song.wav")).unwrap_err().to_string();
        assert!(error.contains("not a project"), "{error}");

        let _ = fs::remove_dir_all(&root);
    }
}
