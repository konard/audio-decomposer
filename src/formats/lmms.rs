//! LMMS projects, the open `.mmp` format.
//!
//! LMMS counts time in ticks: a bar is 192 of them, so a beat of a four-four
//! bar is 48. That is the one thing to know about this exporter, because it is
//! the one thing it cannot do exactly — a placement that falls between ticks is
//! written to the nearest one. Everything the decomposition asserts about
//! frames stays in the archive and in the rendered audio; the project is the
//! editable view of it.
//!
//! Audio clips become sample tracks, one clip each, pointing at the files the
//! archive holds. Recognised notes become an instrument track playing an
//! `audiofileprocessor` loaded with the first sample of the bank, with a
//! pattern of notes on it.

use std::fs;
use std::path::Path;

use crate::decompose::model::Note;
use crate::error::{Error, Result};
use crate::formats::session::{Clip, Session, Track, TrackKind};
use crate::formats::xml::{self, Element};

/// Ticks in a bar, the resolution LMMS works at.
pub const TICKS_PER_BAR: u32 = 192;
/// The version of LMMS the file claims to come from.
const CREATOR_VERSION: &str = "1.2.2";
/// Track type of a sample track.
const SAMPLE_TRACK: &str = "2";
/// Track type of an instrument track.
const INSTRUMENT_TRACK: &str = "0";

/// Writes a session as an LMMS project.
#[must_use]
pub fn write(session: &Session) -> String {
    xml::document(&project(session))
}

/// Writes a session as an LMMS project file.
pub fn write_file(path: impl AsRef<Path>, session: &Session) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, write(session))?;
    Ok(())
}

/// Builds the project element.
#[must_use]
pub fn project(session: &Session) -> Element {
    let mut container = Element::new("trackcontainer")
        .attribute("type", "song")
        .attribute("x", "5")
        .attribute("y", "5")
        .attribute("width", "600")
        .attribute("height", "300")
        .attribute("maximized", "0")
        .attribute("visible", "1");
    for track in &session.tracks {
        container = container.child(track_element(track, session));
    }

    Element::new("lmms-project")
        .attribute("type", "song")
        .attribute("creator", "audio-decomposer")
        .attribute("creatorversion", CREATOR_VERSION)
        .attribute("version", "1.0")
        .child(
            Element::new("head")
                .attribute("bpm", format!("{:.6}", session.tempo))
                .attribute("mastervol", "100")
                .attribute("masterpitch", "0")
                .attribute("timesig_numerator", "4")
                .attribute("timesig_denominator", "4"),
        )
        .child(Element::new("song").child(container))
}

/// One track with its clips or its pattern.
fn track_element(track: &Track, session: &Session) -> Element {
    let element = Element::new("track")
        .attribute("name", track.name.clone())
        .attribute("muted", "0")
        .attribute("solo", "0");
    match track.kind {
        TrackKind::Audio => {
            let mut element = element.attribute("type", SAMPLE_TRACK).child(
                Element::new("sampletrack")
                    .attribute("vol", "100")
                    .attribute("pan", "0"),
            );
            for clip in &track.clips {
                element = element.child(sample_clip(clip, session));
            }
            element
        }
        TrackKind::Instrument => {
            let file = session
                .instruments
                .first()
                .map_or_else(String::new, |instrument| instrument.file.clone());
            let base = session
                .instruments
                .first()
                .and_then(|instrument| instrument.note)
                .unwrap_or(60);
            element
                .attribute("type", INSTRUMENT_TRACK)
                .child(
                    Element::new("instrumenttrack")
                        .attribute("vol", "100")
                        .attribute("pan", "0")
                        .attribute("pitch", "0")
                        .attribute("basenote", base.to_string())
                        .attribute("fxch", "0")
                        .child(
                            Element::new("instrument")
                                .attribute("name", "audiofileprocessor")
                                .child(
                                    Element::new("audiofileprocessor")
                                        .attribute("src", file)
                                        .attribute("looped", "0")
                                        .attribute("amp", "100")
                                        .attribute("sframe", "0")
                                        .attribute("eframe", "1")
                                        .attribute("reversed", "0"),
                                ),
                        ),
                )
                .child(pattern(&track.notes, session))
        }
    }
}

/// One placed sample.
fn sample_clip(clip: &Clip, session: &Session) -> Element {
    Element::new("sampletco")
        .attribute("name", clip.name.clone())
        .attribute("src", clip.file.clone())
        .attribute("pos", ticks(session, clip.start).to_string())
        .attribute("len", ticks(session, clip.length).max(1).to_string())
        .attribute("off", ticks(session, clip.offset).to_string())
        .attribute("muted", "0")
        // LMMS has no per-clip gain; it ignores what it does not know, and
        // [`parse`] reads this back so a round trip through this crate keeps it.
        .attribute("gain", format!("{:.9}", clip.gain))
}

/// The notes of a track, as one pattern.
fn pattern(notes: &[Note], session: &Session) -> Element {
    let start = notes.iter().map(|note| note.start).min().unwrap_or(0);
    let end = notes.iter().map(Note::end).max().unwrap_or(0);
    let mut element = Element::new("pattern")
        .attribute("name", "notes")
        .attribute("pos", ticks(session, start).to_string())
        .attribute("len", ticks(session, end - start).max(1).to_string())
        .attribute("type", "1")
        .attribute("steps", "16")
        .attribute("muted", "0");
    for note in notes {
        element = element.child(
            Element::new("note")
                .attribute("key", note.note.to_string())
                .attribute("pos", ticks(session, note.start - start).to_string())
                .attribute("len", ticks(session, note.length).max(1).to_string())
                .attribute("vol", format!("{:.9}", volume(note.velocity)))
                .attribute("pan", "0"),
        );
    }
    element
}

/// A MIDI velocity as the percentage LMMS calls volume.
fn volume(velocity: u8) -> f64 {
    f64::from(velocity) * 100.0 / 127.0
}

/// The percentage back as a velocity.
fn velocity(volume: f64) -> u8 {
    let value = (volume * 127.0 / 100.0).round();
    if value <= 0.0 {
        0
    } else if value >= 127.0 {
        127
    } else {
        value as u8
    }
}

/// Ticks a number of frames lasts.
fn ticks(session: &Session, frames: usize) -> u64 {
    let bars = session.beats(frames) / 4.0;
    (bars * f64::from(TICKS_PER_BAR)).round().max(0.0) as u64
}

/// Reads a project file back into a session.
pub fn read_file(path: impl AsRef<Path>) -> Result<Session> {
    parse(&fs::read_to_string(path)?)
}

/// Reads a project back into a session.
pub fn parse(text: &str) -> Result<Session> {
    let root = xml::parse(text)?;
    if root.name != "lmms-project" {
        return Err(Error::Format(format!(
            "`{}` is not an LMMS project",
            root.name
        )));
    }
    let tempo = root
        .find("head")
        .and_then(|head| head.get("bpm"))
        .map_or(Ok(crate::formats::session::DEFAULT_TEMPO), |value| {
            number(value, "tempo")
        })?;
    let container = root
        .find("song")
        .and_then(|song| song.find("trackcontainer"))
        .ok_or_else(|| Error::Format("the project has no tracks".to_string()))?;

    let mut session = Session {
        name: String::new(),
        // LMMS projects hold no sample rate: the audio does, and the crate
        // reads it from there. What matters here is only that time in ticks
        // maps back to frames the same way it was written.
        sample_rate: 48_000,
        tempo,
        frames: 0,
        channels: 2,
        tracks: Vec::new(),
        instruments: Vec::new(),
    };

    for element in container.find_all("track") {
        let instrument = element.get("type") == Some(INSTRUMENT_TRACK);
        let mut track = Track {
            name: element.get("name").unwrap_or_default().to_string(),
            kind: if instrument {
                TrackKind::Instrument
            } else {
                TrackKind::Audio
            },
            clips: Vec::new(),
            notes: Vec::new(),
        };
        for clip in element
            .find_all("sampletco")
            .chain(element.find_all("sampleclip"))
        {
            track.clips.push(read_clip(clip, &session)?);
        }
        for pattern in element
            .find_all("pattern")
            .chain(element.find_all("midiclip"))
        {
            read_pattern(pattern, &session, &mut track)?;
        }
        session.tracks.push(track);
    }
    session.frames = session.end();
    Ok(session)
}

/// Reads one sample clip.
fn read_clip(element: &Element, session: &Session) -> Result<Clip> {
    let src = element
        .get("src")
        .ok_or_else(|| Error::Format("a sample clip has no file".to_string()))?;
    Ok(Clip {
        name: element.get("name").unwrap_or(src).to_string(),
        file: src.to_string(),
        start: frames(
            session,
            number(element.get("pos").unwrap_or("0"), "position")?,
        ),
        length: frames(
            session,
            number(element.get("len").unwrap_or("0"), "length")?,
        ),
        offset: frames(
            session,
            number(element.get("off").unwrap_or("0"), "offset")?,
        ),
        gain: number(element.get("gain").unwrap_or("1"), "gain")?,
        channel: 0,
    })
}

/// Reads the notes of one pattern onto a track.
fn read_pattern(element: &Element, session: &Session, track: &mut Track) -> Result<()> {
    let start = frames(
        session,
        number(element.get("pos").unwrap_or("0"), "position")?,
    );
    for note in element.find_all("note") {
        let key: i64 = note
            .get("key")
            .unwrap_or("60")
            .parse()
            .map_err(|_| Error::Parse("a note has a key that is not a number".to_string()))?;
        let key = u8::try_from(key.clamp(0, 127))
            .map_err(|_| Error::Format(format!("{key} is not a MIDI note")))?;
        track.notes.push(Note {
            channel: 0,
            start: start + frames(session, number(note.get("pos").unwrap_or("0"), "position")?),
            length: frames(session, number(note.get("len").unwrap_or("0"), "length")?),
            note: key,
            velocity: velocity(number(note.get("vol").unwrap_or("100"), "volume")?),
            frequency: 440.0 * ((f64::from(key) - 69.0) / 12.0).exp2(),
            confidence: 1.0,
        });
    }
    track.notes.sort_by_key(|note| (note.start, note.note));
    Ok(())
}

/// Frames a number of ticks lasts.
fn frames(session: &Session, ticks: f64) -> usize {
    if session.tempo <= 0.0 {
        return 0;
    }
    let beats = ticks / f64::from(TICKS_PER_BAR) * 4.0;
    let frames = (beats * 60.0 / session.tempo * f64::from(session.sample_rate)).round();
    if frames <= 0.0 {
        0
    } else {
        frames as usize
    }
}

/// Parses one of the numbers the format writes as an attribute.
fn number(text: &str, what: &str) -> Result<f64> {
    text.trim()
        .parse()
        .map_err(|_| Error::Parse(format!("`{text}` is not a {what}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::session::Instrument;

    /// One beat at 120 bpm and 48 kHz.
    const BEAT: usize = 24_000;

    fn clip(name: &str, file: &str, start: usize, length: usize, gain: f64) -> Clip {
        Clip {
            name: name.to_string(),
            file: file.to_string(),
            start,
            length,
            offset: 0,
            gain,
            channel: 0,
        }
    }

    fn note(start: usize, length: usize, pitch: u8, velocity: u8) -> Note {
        Note {
            channel: 0,
            start,
            length,
            note: pitch,
            velocity,
            frequency: 440.0 * ((f64::from(pitch) - 69.0) / 12.0).exp2(),
            confidence: 1.0,
        }
    }

    fn session() -> Session {
        Session {
            name: "song".to_string(),
            sample_rate: 48_000,
            tempo: 120.0,
            frames: 8 * BEAT,
            channels: 2,
            tracks: vec![
                Track {
                    name: "kick".to_string(),
                    kind: TrackKind::Audio,
                    clips: vec![
                        clip("kick", "samples/kick.wav", 0, BEAT, 1.0),
                        clip("kick", "samples/kick.wav", 4 * BEAT, BEAT, 0.5),
                    ],
                    notes: Vec::new(),
                },
                Track {
                    name: "notes".to_string(),
                    kind: TrackKind::Instrument,
                    clips: Vec::new(),
                    notes: vec![note(0, BEAT, 60, 100), note(2 * BEAT, BEAT, 67, 80)],
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

    #[test]
    fn a_session_survives_the_round_trip() {
        let session = session();

        let read = parse(&write(&session)).unwrap();

        assert!((read.tempo - 120.0).abs() < 1e-9);
        assert_eq!(read.tracks.len(), 2);
        for (read, expected) in read.tracks.iter().zip(&session.tracks) {
            assert_eq!((&read.name, read.kind), (&expected.name, expected.kind));
            assert_eq!(read.clips.len(), expected.clips.len());
            for (read, expected) in read.clips.iter().zip(&expected.clips) {
                assert_eq!(
                    (&read.file, read.start, read.length, read.offset),
                    (
                        &expected.file,
                        expected.start,
                        expected.length,
                        expected.offset
                    )
                );
                assert!((read.gain - expected.gain).abs() < 1e-9);
            }
            assert_eq!(read.notes, expected.notes);
        }
    }

    #[test]
    fn the_file_looks_like_an_lmms_project() {
        let text = write(&session());

        assert!(text.contains("<lmms-project"), "{text}");
        assert!(text.contains("bpm=\"120.000000\""), "{text}");
        assert!(text.contains("type=\"2\""), "{text}");
        assert!(text.contains("<sampletco"), "{text}");
        assert!(text.contains("src=\"samples/kick.wav\""), "{text}");
        assert!(text.contains("<audiofileprocessor"), "{text}");
    }

    #[test]
    fn time_is_written_in_ticks() {
        let session = session();

        // A beat is a quarter of a 192-tick bar.
        assert_eq!(ticks(&session, BEAT), 48);
        assert_eq!(ticks(&session, 4 * BEAT), 192);
        assert_eq!(frames(&session, 48.0), BEAT);
    }

    #[test]
    fn velocities_come_back_as_they_went_in() {
        for value in 0..=127u8 {
            assert_eq!(velocity(volume(value)), value);
        }
    }

    #[test]
    fn a_project_written_by_a_newer_lmms_is_understood() {
        // LMMS 1.3 renamed the elements; both spellings are read.
        let text = r#"<?xml version="1.0"?>
<lmms-project type="song" creatorversion="1.3.0">
  <head bpm="140"/>
  <song>
    <trackcontainer type="song">
      <track type="2" name="loop">
        <sampleclip name="loop" src="audio/loop.wav" pos="192" len="192" off="0"/>
      </track>
      <track type="0" name="lead">
        <midiclip name="lead" pos="0" len="192">
          <note key="72" pos="48" len="48" vol="100"/>
        </midiclip>
      </track>
    </trackcontainer>
  </song>
</lmms-project>"#;

        let session = parse(text).unwrap();

        assert!((session.tempo - 140.0).abs() < 1e-9);
        assert_eq!(session.tracks[0].clips[0].file, "audio/loop.wav");
        assert_eq!(session.tracks[1].notes[0].note, 72);
        assert_eq!(session.tracks[1].notes[0].velocity, 127);
    }

    #[test]
    fn broken_projects_are_reported() {
        let complain = |text: &str| parse(text).unwrap_err().to_string();

        assert!(complain("<project/>").contains("not an LMMS project"));
        assert!(complain("<lmms-project/>").contains("no tracks"));
        assert!(complain(
            "<lmms-project><song><trackcontainer><track type=\"2\"><sampletco pos=\"0\"/></track></trackcontainer></song></lmms-project>"
        )
        .contains("no file"));
    }

    #[test]
    fn a_file_round_trips_through_the_disk() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-lmms-{}-{:?}/song.mmp",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = path.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(&root);

        write_file(&path, &session()).unwrap();

        assert_eq!(read_file(&path).unwrap().tracks.len(), 2);

        let _ = fs::remove_dir_all(&root);
    }
}
