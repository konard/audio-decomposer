//! DAWproject, the interchange format Bitwig, Studio One and Cubase read.
//!
//! A DAWproject file is a ZIP holding `project.xml`, `metadata.xml` and the
//! audio the project refers to. Of all the formats here it is the only one
//! designed to be written by something other than the DAW that reads it, so it
//! is the one that carries the most of a decomposition: a track per stem and
//! per placed sample, a clip per placement, and the recognised notes as a
//! note clip.
//!
//! Two details are worth knowing. Times on the arrangement are in beats, so a
//! session written at the wrong tempo still plays at the right moments but
//! reads oddly in the grid; the tempo the session carries is written into the
//! transport so it stays consistent. And the format has no per-clip gain, so
//! the gain a placement was matched with is written into the clip comment,
//! where DAWs preserve it and [`read`] can find it again.

use std::fs;
use std::path::Path;

use crate::decompose::model::Note;
use crate::error::{Error, Result};
use crate::formats::session::{Clip, Session, Track, TrackKind};
use crate::formats::xml::{self, Element};
use crate::formats::zip;

/// Name of the project inside the archive.
pub const PROJECT_FILE: &str = "project.xml";
/// Name of the metadata inside the archive.
pub const METADATA_FILE: &str = "metadata.xml";
/// Version of the format that is written.
pub const VERSION: &str = "1.0";
/// Prefix of the gain note left in a clip comment.
const GAIN_COMMENT: &str = "gain ";

/// Hands out the identifiers the format wants on nearly every element.
#[derive(Debug, Default)]
struct Ids(usize);

impl Ids {
    fn next(&mut self) -> String {
        self.0 += 1;
        format!("id{}", self.0)
    }
}

/// Writes `project.xml`.
#[must_use]
pub fn project(session: &Session) -> String {
    let mut ids = Ids::default();
    let tracks: Vec<(String, &Track)> = session
        .tracks
        .iter()
        .map(|track| (ids.next(), track))
        .collect();

    let structure = tracks
        .iter()
        .fold(Element::new("Structure"), |structure, (id, track)| {
            structure.child(track_element(id, track, session, &mut ids))
        });

    let mut lanes = Element::new("Lanes")
        .attribute("timeUnit", "beats")
        .attribute("id", ids.next());
    for (id, track) in &tracks {
        lanes = lanes.child(track_lanes(id, track, session, &mut ids));
    }

    let document = Element::new("Project")
        .attribute("version", VERSION)
        .child(
            Element::new("Application")
                .attribute("name", "audio-decomposer")
                .attribute("version", env!("CARGO_PKG_VERSION")),
        )
        .child(
            Element::new("Transport")
                .child(
                    Element::new("Tempo")
                        .attribute("unit", "bpm")
                        .attribute("value", format!("{:.6}", session.tempo))
                        .attribute("min", "20.0")
                        .attribute("max", "999.0")
                        .attribute("id", ids.next()),
                )
                .child(
                    Element::new("TimeSignature")
                        .attribute("numerator", "4")
                        .attribute("denominator", "4")
                        .attribute("id", ids.next()),
                ),
        )
        .child(structure)
        .child(
            Element::new("Arrangement")
                .attribute("id", ids.next())
                .child(lanes),
        );
    xml::document(&document)
}

/// Writes `metadata.xml`.
#[must_use]
pub fn metadata(session: &Session) -> String {
    xml::document(
        &Element::new("MetaData")
            .child(Element::leaf("Title", session.name.clone()))
            .child(Element::leaf("Producer", "audio-decomposer"))
            .child(Element::leaf(
                "Comment",
                format!(
                    "Decomposed by audio-decomposer: {} tracks, {} bank samples, {} Hz.",
                    session.tracks.len(),
                    session.instruments.len(),
                    session.sample_rate
                ),
            )),
    )
}

/// The entries a DAWproject archive is made of.
///
/// The audio the session refers to has to come from somewhere; pass it in as
/// entries named the way [`Session::files`] names them.
#[must_use]
pub fn entries(session: &Session, audio: &[zip::Entry]) -> Vec<zip::Entry> {
    let mut entries = vec![
        zip::Entry::text(PROJECT_FILE, project(session)),
        zip::Entry::text(METADATA_FILE, metadata(session)),
    ];
    entries.extend(audio.iter().cloned());
    entries
}

/// Packs a DAWproject archive.
#[must_use]
pub fn write(session: &Session, audio: &[zip::Entry]) -> Vec<u8> {
    zip::write(&entries(session, audio))
}

/// Writes a DAWproject archive to a file.
pub fn write_file(path: impl AsRef<Path>, session: &Session, audio: &[zip::Entry]) -> Result<()> {
    zip::write_file(path, &entries(session, audio))
}

/// One track and the channel it plays through.
fn track_element(id: &str, track: &Track, session: &Session, ids: &mut Ids) -> Element {
    let content = match track.kind {
        TrackKind::Audio => "audio",
        TrackKind::Instrument => "notes",
    };
    Element::new("Track")
        .attribute("id", id)
        .attribute("name", track.name.clone())
        .attribute("contentType", content)
        .attribute("loaded", "true")
        .child(
            Element::new("Channel")
                .attribute("id", ids.next())
                .attribute("role", "regular")
                .attribute("audioChannels", session.channels.max(1).to_string())
                .attribute("solo", "false")
                .child(
                    Element::new("Volume")
                        .attribute("unit", "linear")
                        .attribute("value", "1.0")
                        .attribute("min", "0.0")
                        .attribute("max", "2.0")
                        .attribute("id", ids.next()),
                )
                .child(
                    Element::new("Pan")
                        .attribute("unit", "normalized")
                        .attribute("value", "0.5")
                        .attribute("min", "0.0")
                        .attribute("max", "1.0")
                        .attribute("id", ids.next()),
                )
                .child(
                    Element::new("Mute")
                        .attribute("value", "false")
                        .attribute("id", ids.next()),
                ),
        )
}

/// The clips of one track.
fn track_lanes(id: &str, track: &Track, session: &Session, ids: &mut Ids) -> Element {
    let mut clips = Element::new("Clips").attribute("id", ids.next());
    for clip in &track.clips {
        clips = clips.child(audio_clip(clip, session, ids));
    }
    if !track.notes.is_empty() {
        clips = clips.child(note_clip(&track.notes, session, ids));
    }
    Element::new("Lanes")
        .attribute("track", id)
        .attribute("id", ids.next())
        .child(clips)
}

/// One placed piece of audio.
fn audio_clip(clip: &Clip, session: &Session, ids: &mut Ids) -> Element {
    let element = Element::new("Clip")
        .attribute("name", clip.name.clone())
        .attribute("time", beats(session, clip.start))
        .attribute("duration", beats(session, clip.length))
        .attribute("contentTimeUnit", "seconds")
        .attribute("playStart", seconds(session, clip.offset))
        .attribute(
            "comment",
            format!("{GAIN_COMMENT}{gain:.9}", gain = clip.gain),
        );
    element.child(
        Element::new("Audio")
            .attribute("algorithm", "raw")
            .attribute("channels", "1")
            .attribute("duration", seconds(session, clip.offset + clip.length))
            .attribute("sampleRate", session.sample_rate.to_string())
            .attribute("id", ids.next())
            .child(Element::new("File").attribute("path", clip.file.clone())),
    )
}

/// The notes of a track, as one clip covering everything they span.
fn note_clip(notes: &[Note], session: &Session, ids: &mut Ids) -> Element {
    let start = notes.iter().map(|note| note.start).min().unwrap_or(0);
    let end = notes.iter().map(Note::end).max().unwrap_or(0);
    let mut element = Element::new("Notes").attribute("id", ids.next());
    for note in notes {
        element = element.child(
            Element::new("Note")
                .attribute("time", beats(session, note.start - start.min(note.start)))
                .attribute("duration", beats(session, note.length))
                .attribute("channel", note.channel.to_string())
                .attribute("key", note.note.to_string())
                .attribute("vel", format!("{:.9}", f64::from(note.velocity) / 127.0))
                .attribute("rel", "0.5"),
        );
    }
    Element::new("Clip")
        .attribute("name", "notes")
        .attribute("time", beats(session, start))
        .attribute("duration", beats(session, end - start))
        .attribute("playStart", "0.0")
        .child(element)
}

/// A point in time, in beats, the way the format writes numbers.
fn beats(session: &Session, frames: usize) -> String {
    format!("{:.9}", session.beats(frames))
}

/// A point in time, in seconds.
fn seconds(session: &Session, frames: usize) -> String {
    format!("{:.9}", session.seconds(frames))
}

/// Reads a DAWproject archive back into a session.
///
/// The sample rate comes from the audio in the project; a project without
/// audio has none to read, so the caller can pass one in.
pub fn read(bytes: &[u8], sample_rate: Option<u32>) -> Result<Session> {
    let entries = zip::read(bytes)?;
    let project = entries
        .iter()
        .find(|entry| entry.name == PROJECT_FILE)
        .ok_or_else(|| Error::Format(format!("the archive has no `{PROJECT_FILE}`")))?;
    let text = String::from_utf8(project.data.clone())
        .map_err(|error| Error::Format(error.to_string()))?;
    let name = entries
        .iter()
        .find(|entry| entry.name == METADATA_FILE)
        .and_then(|entry| String::from_utf8(entry.data.clone()).ok())
        .and_then(|text| xml::parse(&text).ok())
        .and_then(|metadata| metadata.find("Title").map(Element::content))
        .unwrap_or_default();
    parse(&text, &name, sample_rate)
}

/// Reads a DAWproject file back into a session.
pub fn read_file(path: impl AsRef<Path>, sample_rate: Option<u32>) -> Result<Session> {
    read(&fs::read(path)?, sample_rate)
}

/// Reads `project.xml` back into a session.
pub fn parse(text: &str, name: &str, sample_rate: Option<u32>) -> Result<Session> {
    let root = xml::parse(text)?;
    if root.name != "Project" {
        return Err(Error::Format(format!(
            "`{}` is not a DAWproject project",
            root.name
        )));
    }
    let tempo = root
        .find("Transport")
        .and_then(|transport| transport.find("Tempo"))
        .and_then(|tempo| tempo.get("value"))
        .map_or(Ok(crate::formats::session::DEFAULT_TEMPO), |value| {
            number(value, "tempo")
        })?;
    let structure = root
        .find("Structure")
        .ok_or_else(|| Error::Format("the project has no structure".to_string()))?;
    let lanes = root
        .find("Arrangement")
        .and_then(|arrangement| arrangement.find("Lanes"))
        .ok_or_else(|| Error::Format("the project has no arrangement".to_string()))?;

    let rate = sample_rate
        .or_else(|| {
            lanes
                .find_all("Lanes")
                .flat_map(|lane| lane.find_all("Clips"))
                .flat_map(|clips| clips.find_all("Clip"))
                .filter_map(|clip| clip.find("Audio"))
                .find_map(|audio| audio.get("sampleRate").and_then(|rate| rate.parse().ok()))
        })
        .ok_or_else(|| {
            Error::Format("the project holds no audio, so it carries no sample rate".to_string())
        })?;

    let mut session = Session {
        name: name.to_string(),
        sample_rate: rate,
        tempo,
        frames: 0,
        channels: 1,
        tracks: Vec::new(),
        instruments: Vec::new(),
    };

    for element in structure.find_all("Track") {
        let id = element.get("id").unwrap_or_default().to_string();
        let kind = if element.get("contentType") == Some("notes") {
            TrackKind::Instrument
        } else {
            TrackKind::Audio
        };
        if let Some(channels) = element
            .find("Channel")
            .and_then(|channel| channel.get("audioChannels"))
            .and_then(|value| value.parse::<usize>().ok())
        {
            session.channels = session.channels.max(channels);
        }
        let mut track = Track {
            name: element.get("name").unwrap_or_default().to_string(),
            kind,
            clips: Vec::new(),
            notes: Vec::new(),
        };
        for lane in lanes
            .find_all("Lanes")
            .filter(|lane| lane.get("track") == Some(&id))
        {
            read_lane(lane, &session, &mut track)?;
        }
        session.tracks.push(track);
    }
    session.frames = session.end();
    Ok(session)
}

/// Reads the clips of one lane onto a track.
fn read_lane(lane: &Element, session: &Session, track: &mut Track) -> Result<()> {
    for clips in lane.find_all("Clips") {
        for clip in clips.find_all("Clip") {
            let start = frames(
                session,
                number(clip.get("time").unwrap_or("0"), "clip time")?,
            );
            if let Some(notes) = clip.find("Notes") {
                read_notes(notes, session, start, track)?;
                continue;
            }
            let Some(audio) = clip.find("Audio") else {
                continue;
            };
            let file = audio
                .find("File")
                .and_then(|file| file.get("path"))
                .ok_or_else(|| Error::Format("an audio clip has no file".to_string()))?;
            let length = frames(
                session,
                number(clip.get("duration").unwrap_or("0"), "clip duration")?,
            );
            let offset = seconds_to_frames(
                session,
                number(clip.get("playStart").unwrap_or("0"), "clip offset")?,
            );
            track.clips.push(Clip {
                name: clip.get("name").unwrap_or_default().to_string(),
                file: file.to_string(),
                start,
                length,
                offset,
                gain: gain_of(clip.get("comment"))?,
                channel: 0,
            });
        }
    }
    Ok(())
}

/// Reads the notes of a note clip onto a track.
fn read_notes(notes: &Element, session: &Session, start: usize, track: &mut Track) -> Result<()> {
    for note in notes.find_all("Note") {
        let velocity = number(note.get("vel").unwrap_or("1"), "velocity")? * 127.0;
        let key: i64 = note
            .get("key")
            .unwrap_or("60")
            .parse()
            .map_err(|_| Error::Parse("a note has a key that is not a number".to_string()))?;
        let key = u8::try_from(key.clamp(0, 127))
            .map_err(|_| Error::Format(format!("{key} is not a MIDI note")))?;
        track.notes.push(Note {
            channel: note
                .get("channel")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            start: start
                + frames(
                    session,
                    number(note.get("time").unwrap_or("0"), "note time")?,
                ),
            length: frames(
                session,
                number(note.get("duration").unwrap_or("0"), "note length")?,
            ),
            note: key,
            velocity: velocity.round().clamp(0.0, 127.0) as u8,
            frequency: 440.0 * ((f64::from(key) - 69.0) / 12.0).exp2(),
            confidence: 1.0,
        });
    }
    Ok(())
}

/// Reads the gain out of a clip comment, defaulting to unity.
fn gain_of(comment: Option<&str>) -> Result<f64> {
    comment
        .and_then(|comment| comment.strip_prefix(GAIN_COMMENT).map(str::trim))
        .map_or(Ok(1.0), |value| number(value, "clip gain"))
}

/// Frames a number of beats lasts.
fn frames(session: &Session, beats: f64) -> usize {
    if session.tempo <= 0.0 {
        return 0;
    }
    let seconds = beats * 60.0 / session.tempo;
    seconds_to_frames(session, seconds)
}

/// Frames a number of seconds lasts.
fn seconds_to_frames(session: &Session, seconds: f64) -> usize {
    let frames = (seconds * f64::from(session.sample_rate)).round();
    if frames <= 0.0 {
        0
    } else {
        frames as usize
    }
}

/// Parses one of the many numbers the format writes as text.
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

        let read = read(&write(&session, &[]), None).unwrap();

        assert_eq!(read.name, session.name);
        assert_eq!(read.sample_rate, session.sample_rate);
        assert!((read.tempo - session.tempo).abs() < 1e-9);
        assert_eq!(read.channels, session.channels);
        assert_eq!(read.tracks.len(), session.tracks.len());
        for (read, expected) in read.tracks.iter().zip(&session.tracks) {
            assert_eq!(read.name, expected.name);
            assert_eq!(read.kind, expected.kind);
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
            assert_eq!(read.notes.len(), expected.notes.len());
            for (read, expected) in read.notes.iter().zip(&expected.notes) {
                assert_eq!(
                    (read.start, read.length, read.note, read.velocity),
                    (
                        expected.start,
                        expected.length,
                        expected.note,
                        expected.velocity
                    )
                );
            }
        }
    }

    #[test]
    fn the_archive_holds_the_files_a_daw_expects() {
        let audio = vec![zip::Entry::new("samples/kick.wav", vec![1u8, 2, 3])];

        let entries = zip::read(&write(&session(), &audio)).unwrap();

        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, [PROJECT_FILE, METADATA_FILE, "samples/kick.wav"]);
        assert_eq!(entries[2].data, vec![1u8, 2, 3]);
    }

    #[test]
    fn the_metadata_names_the_recording() {
        let text = metadata(&session());

        assert!(text.contains("<Title>song</Title>"), "{text}");
        assert!(text.contains("48000 Hz"), "{text}");
    }

    #[test]
    fn every_element_has_its_own_identifier() {
        let text = project(&session());

        let mut ids: Vec<&str> = text
            .match_indices("id=\"")
            .map(|(at, _)| {
                let rest = &text[at + 4..];
                &rest[..rest.find('"').unwrap()]
            })
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "identifiers repeat in {text}");
    }

    #[test]
    fn clip_gain_is_kept_where_the_format_has_no_room_for_it() {
        let text = project(&session());

        assert!(text.contains("comment=\"gain 0.500000000\""), "{text}");
        assert_eq!(gain_of(Some("gain 0.25")).unwrap(), 0.25);
        assert_eq!(gain_of(None).unwrap(), 1.0);
        assert_eq!(gain_of(Some("written by hand")).unwrap(), 1.0);
    }

    #[test]
    fn a_project_written_elsewhere_is_understood() {
        // The shape Bitwig writes: a track, a lane, a clip with warps around
        // the audio. The warps are ignored, the audio is not.
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<Project version="1.0">
  <Transport><Tempo unit="bpm" value="140.0" id="t"/></Transport>
  <Structure>
    <Track contentType="audio" loaded="true" id="track" name="guitar">
      <Channel audioChannels="2" role="regular" id="chan"/>
    </Track>
  </Structure>
  <Arrangement id="arr">
    <Lanes timeUnit="beats" id="lanes">
      <Lanes track="track" id="lane">
        <Clips id="clips">
          <Clip time="2.0" duration="2.0" playStart="0.0" contentTimeUnit="seconds">
            <Audio algorithm="stretch" channels="2" duration="1.0" sampleRate="44100" id="audio">
              <File path="audio/guitar.wav"/>
            </Audio>
          </Clip>
        </Clips>
      </Lanes>
    </Lanes>
  </Arrangement>
</Project>"#;

        let session = parse(text, "take", None).unwrap();

        assert_eq!(session.sample_rate, 44_100);
        assert!((session.tempo - 140.0).abs() < 1e-9);
        assert_eq!(session.channels, 2);
        let clip = &session.tracks[0].clips[0];
        assert_eq!(clip.file, "audio/guitar.wav");
        // Two beats at 140 bpm is 6/7 of a second.
        assert_eq!(
            clip.start,
            (2.0_f64 * 60.0 / 140.0 * 44_100.0).round() as usize
        );
    }

    #[test]
    fn broken_projects_are_reported() {
        let complain = |text: &str| parse(text, "x", Some(48_000)).unwrap_err().to_string();

        assert!(complain("<Session/>").contains("not a DAWproject"));
        assert!(complain("<Project/>").contains("no structure"));
        assert!(complain("<Project><Structure/></Project>").contains("no arrangement"));
        assert!(parse(
            "<Project><Structure/><Arrangement><Lanes/></Arrangement></Project>",
            "x",
            None
        )
        .unwrap_err()
        .to_string()
        .contains("no sample rate"));
        assert!(read(b"not a zip", None)
            .unwrap_err()
            .to_string()
            .contains("zip"));
    }

    #[test]
    fn a_file_round_trips_through_the_disk() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-dawproject-{}-{:?}/song.dawproject",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = path.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(&root);

        write_file(&path, &session(), &[]).unwrap();

        let read = read_file(&path, None).unwrap();
        assert_eq!(read.name, "song");
        assert_eq!(read.tracks.len(), 2);

        let _ = fs::remove_dir_all(&root);
    }
}
