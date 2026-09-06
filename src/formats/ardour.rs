//! Ardour sessions, the XML `.ardour` format.
//!
//! An Ardour session names its audio in `<Sources>`, cuts it into `<Regions>`,
//! arranges those regions on `<Playlists>` and gives each playlist to a
//! `<Route>`. This module writes that chain, which is the whole of what a
//! decomposition needs: every placement becomes a region of the sample it was
//! matched to, and the gain it was matched with is a region's
//! `scale-amplitude`, so unlike most project formats nothing has to be hidden
//! in a comment.
//!
//! Ardour keeps notes outside the session file, in a MIDI file under
//! `interchange`, so an instrument track is written as a MIDI route whose
//! region points at [`midi_path`]. The bundle that writes a session is what
//! puts that file there; reading a session back therefore gives an instrument
//! track carrying its clip, not its notes.
//!
//! Positions are sample counts, which is what Ardour 6 writes and what keeps a
//! decomposition exact. Ardour 7 prefixes them with `a` for audio time, and
//! [`parse`] accepts that spelling too.

use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
use crate::formats::midi;
use crate::formats::session::{Clip, Session, Track, TrackKind};
use crate::formats::xml::{self, Element};

/// Session format this module writes.
pub const VERSION: &str = "6000";
/// Directory Ardour keeps the MIDI of a session in.
pub const MIDI_DIRECTORY: &str = "interchange";

/// Where the MIDI of an instrument track lives inside a session directory.
#[must_use]
pub fn midi_path(session: &str, track: &str) -> String {
    format!("{MIDI_DIRECTORY}/{session}/midifiles/{track}.mid")
}

/// Hands out the identifiers Ardour gives to everything in a session.
#[derive(Debug)]
struct Ids(u64);

impl Ids {
    /// Ardour keeps low identifiers for itself, so counting starts above them.
    const fn new() -> Self {
        Self(100)
    }

    fn next(&mut self) -> String {
        self.0 += 1;
        self.0.to_string()
    }
}

/// Writes a session as the XML of an `.ardour` file.
#[must_use]
pub fn write(session: &Session) -> String {
    xml::document(&document(session))
}

/// Writes a session as an `.ardour` file.
pub fn write_file(path: impl AsRef<Path>, session: &Session) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(fs::write(path, write(session))?)
}

/// Builds the session document.
#[must_use]
pub fn document(session: &Session) -> Element {
    let mut ids = Ids::new();
    let mut sources = Element::new("Sources");
    let mut regions = Element::new("Regions");
    let mut playlists = Element::new("Playlists");
    let mut routes = Element::new("Routes");

    for track in &session.tracks {
        let route = ids.next();
        let playlist = ids.next();
        let midi = track.kind == TrackKind::Instrument;
        let kind = if midi { "midi" } else { "audio" };

        let mut lane = Element::new("Playlist")
            .attribute("id", playlist.clone())
            .attribute("name", track.name.clone())
            .attribute("type", kind)
            .attribute("orig-track-id", route.clone())
            .attribute("frozen", "no")
            .attribute("combine-ops", "0");

        for clip in clips_of(track, session) {
            let source = ids.next();
            let region = ids.next();
            sources = sources.child(source_element(&clip, &source, kind));
            let element = region_element(&clip, &region, &source, kind);
            regions = regions.child(
                element
                    .clone()
                    .attribute("whole-file", "0")
                    .attribute("hidden", "0"),
            );
            lane = lane.child(element);
        }

        playlists = playlists.child(lane);
        routes = routes.child(route_element(track, &route, kind));
    }

    Element::new("Session")
        .attribute("version", VERSION)
        .attribute("name", session.name.clone())
        .attribute("sample-rate", session.sample_rate.to_string())
        .attribute("id-counter", ids.next())
        .attribute("event-counter", "0")
        .child(Element::new("ProgramVersion").attribute(
            "created-with",
            format!("audio-decomposer {VERSION_OF_CRATE}"),
        ))
        .child(tempo_map(session))
        .child(sources)
        .child(regions)
        .child(playlists)
        .child(routes)
}

/// Version of this crate, written into the session for provenance.
const VERSION_OF_CRATE: &str = env!("CARGO_PKG_VERSION");

/// The clips of a track, an instrument track standing in for its MIDI file.
fn clips_of(track: &Track, session: &Session) -> Vec<Clip> {
    if track.kind == TrackKind::Audio {
        return track.clips.clone();
    }
    if !track.clips.is_empty() {
        return track.clips.clone();
    }
    if track.notes.is_empty() {
        return Vec::new();
    }
    let start = track.notes.iter().map(|note| note.start).min().unwrap_or(0);
    let end = track
        .notes
        .iter()
        .map(crate::decompose::model::Note::end)
        .max()
        .unwrap_or(0);
    vec![Clip {
        name: track.name.clone(),
        file: midi_path(&session.name, &track.name),
        start,
        length: end.saturating_sub(start),
        offset: 0,
        gain: 1.0,
        channel: 0,
    }]
}

/// The tempo and the meter of the session.
fn tempo_map(session: &Session) -> Element {
    Element::new("TempoMap")
        .child(
            Element::new("Tempo")
                .attribute("pulse", "0")
                .attribute("frame", "0")
                .attribute("movable", "no")
                .attribute("lock-style", "AudioTime")
                .attribute("beats-per-minute", format!("{:.6}", session.tempo))
                .attribute("note-type", "4")
                .attribute("clamped", "0")
                .attribute("end-beats-per-minute", format!("{:.6}", session.tempo))
                .attribute("active", "yes"),
        )
        .child(
            Element::new("Meter")
                .attribute("pulse", "0")
                .attribute("frame", "0")
                .attribute("movable", "no")
                .attribute("lock-style", "AudioTime")
                .attribute("note-type", "4")
                .attribute("divisions-per-bar", "4"),
        )
}

/// The file a region plays.
fn source_element(clip: &Clip, id: &str, kind: &str) -> Element {
    Element::new("Source")
        .attribute("name", file_name(&clip.file))
        .attribute("type", kind)
        .attribute("flags", if kind == "midi" { "Writable" } else { "" })
        .attribute("id", id)
        .attribute("origin", clip.file.clone())
        .attribute("channel", clip.channel.to_string())
        .attribute("natural-position", "0")
}

/// One placed piece of a source.
fn region_element(clip: &Clip, id: &str, source: &str, kind: &str) -> Element {
    Element::new("Region")
        .attribute("name", clip.name.clone())
        .attribute("muted", "0")
        .attribute("opaque", "1")
        .attribute("locked", "0")
        .attribute("position", clip.start.to_string())
        .attribute("length", clip.length.to_string())
        .attribute("start", clip.offset.to_string())
        .attribute("sync-position", "0")
        .attribute("stretch", "1")
        .attribute("shift", "1")
        .attribute("layering-index", "0")
        .attribute("envelope-active", "0")
        .attribute("scale-amplitude", format!("{:.9}", clip.gain))
        .attribute("id", id)
        .attribute("type", kind)
        .attribute("first-edit", "nothing")
        .attribute("source-0", source)
        .attribute("master-source-0", source)
        .attribute("channels", "1")
}

/// The route a playlist plays through.
fn route_element(track: &Track, id: &str, kind: &str) -> Element {
    Element::new("Route")
        .attribute("id", id)
        .attribute("name", track.name.clone())
        .attribute("default-type", kind)
        .attribute("strict-io", "1")
        .attribute("active", "yes")
        .attribute("denormal-protection", "no")
        .attribute("meter-point", "MeterPostFader")
        .attribute("mode", "Normal")
        .child(
            Element::new("IO")
                .attribute("name", track.name.clone())
                .attribute("direction", "Input")
                .attribute("default-type", kind),
        )
        .child(
            Element::new("IO")
                .attribute("name", track.name.clone())
                .attribute("direction", "Output")
                .attribute("default-type", "audio"),
        )
}

/// The last part of a path, which is what Ardour calls a source.
fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Reads an `.ardour` file back into a session.
///
/// Ardour keeps the notes of a MIDI track outside the session file, so
/// [`parse`] alone gives an instrument track its region and no notes. Reading
/// from disk can do better: the MIDI files sit at a known place beside the
/// session, and any that are still there are read back onto their tracks.
pub fn read_file(path: impl AsRef<Path>) -> Result<Session> {
    let path = path.as_ref();
    let mut session = parse(&fs::read_to_string(path)?)?;
    read_midi_beside(
        &mut session,
        path.parent().unwrap_or_else(|| Path::new(".")),
    );
    Ok(session)
}

/// Fills in the notes of every instrument track from the MIDI file its region
/// points at. A file that is missing or unreadable simply leaves the track as
/// [`parse`] left it, because a session without its interchange directory is
/// still a session.
fn read_midi_beside(session: &mut Session, root: &Path) {
    let sample_rate = session.sample_rate;
    for track in &mut session.tracks {
        if track.kind != TrackKind::Instrument {
            continue;
        }
        let mut notes = Vec::new();
        for clip in &track.clips {
            if let Ok(file) = midi::read_file(root.join(&clip.file)) {
                notes.extend(midi::to_notes(&file, sample_rate));
            }
        }
        notes.sort_by_key(|note| (note.start, note.note));
        if !notes.is_empty() {
            track.notes = notes;
        }
    }
}

/// Reads the XML of an `.ardour` file back into a session.
pub fn parse(text: &str) -> Result<Session> {
    let root = xml::parse(text)?;
    if root.name != "Session" {
        return Err(Error::Format(format!(
            "`{}` is not an Ardour session",
            root.name
        )));
    }
    let sample_rate = root
        .get("sample-rate")
        .and_then(|rate| rate.parse().ok())
        .ok_or_else(|| Error::Format("the session has no sample rate".to_string()))?;
    let tempo = root
        .find("TempoMap")
        .and_then(|map| map.find("Tempo"))
        .and_then(|tempo| tempo.get("beats-per-minute"))
        .map_or(Ok(crate::formats::session::DEFAULT_TEMPO), |value| {
            number(value, "tempo")
        })?;

    let sources = sources_of(&root);
    let playlists = root
        .find("Playlists")
        .ok_or_else(|| Error::Format("the session has no playlists".to_string()))?;
    let routes = root.find("Routes");

    let mut session = Session {
        name: root.get("name").unwrap_or_default().to_string(),
        sample_rate,
        tempo,
        frames: 0,
        channels: 2,
        tracks: Vec::new(),
        instruments: Vec::new(),
    };

    for playlist in playlists.find_all("Playlist") {
        let kind = if playlist.get("type") == Some("midi") {
            TrackKind::Instrument
        } else {
            TrackKind::Audio
        };
        let name = routes
            .and_then(|routes| {
                routes
                    .find_all("Route")
                    .find(|route| route.get("id") == playlist.get("orig-track-id"))
            })
            .and_then(|route| route.get("name"))
            .or_else(|| playlist.get("name"))
            .unwrap_or_default()
            .to_string();

        let mut track = Track {
            name,
            kind,
            clips: Vec::new(),
            notes: Vec::new(),
        };
        for region in playlist.find_all("Region") {
            track.clips.push(read_region(region, &sources)?);
        }
        session.tracks.push(track);
    }

    session.frames = session.end();
    Ok(session)
}

/// The origin of every source in the session, by identifier.
fn sources_of(root: &Element) -> Vec<(String, String, usize)> {
    let Some(sources) = root.find("Sources") else {
        return Vec::new();
    };
    sources
        .find_all("Source")
        .map(|source| {
            (
                source.get("id").unwrap_or_default().to_string(),
                source
                    .get("origin")
                    .filter(|origin| !origin.is_empty())
                    .or_else(|| source.get("name"))
                    .unwrap_or_default()
                    .to_string(),
                source
                    .get("channel")
                    .and_then(|channel| channel.parse().ok())
                    .unwrap_or(0),
            )
        })
        .collect()
}

/// Reads one region as a clip.
fn read_region(region: &Element, sources: &[(String, String, usize)]) -> Result<Clip> {
    let source = region.get("source-0").unwrap_or_default();
    let found = sources.iter().find(|(id, _, _)| id == source);
    Ok(Clip {
        name: region.get("name").unwrap_or_default().to_string(),
        file: found
            .map(|(_, origin, _)| origin.clone())
            .ok_or_else(|| Error::Format(format!("region `{source}` has no source")))?,
        start: frames(region.get("position"), "region position")?,
        length: frames(region.get("length"), "region length")?,
        offset: frames(region.get("start"), "region start")?,
        gain: region
            .get("scale-amplitude")
            .map_or(Ok(1.0), |value| number(value, "region gain"))?,
        channel: found.map_or(0, |(_, _, channel)| *channel),
    })
}

/// Reads a sample count, which Ardour 7 marks as audio time with an `a`.
fn frames(value: Option<&str>, what: &str) -> Result<usize> {
    let text = value.unwrap_or("0");
    let text = text.strip_prefix('a').unwrap_or(text);
    text.trim()
        .parse()
        .map_err(|_| Error::Parse(format!("`{text}` is not a {what}")))
}

/// Reads one of the numbers the format writes as an attribute.
fn number(text: &str, what: &str) -> Result<f64> {
    text.trim()
        .parse()
        .map_err(|_| Error::Parse(format!("`{text}` is not a {what}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decompose::model::Note;
    use crate::formats::session::Instrument;

    /// One beat at 120 bpm and 48 kHz.
    const BEAT: usize = 24_000;

    fn clip(name: &str, file: &str, start: usize, gain: f64) -> Clip {
        Clip {
            name: name.to_string(),
            file: file.to_string(),
            start,
            length: BEAT,
            offset: 0,
            gain,
            channel: 0,
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
                        clip("kick.1", "samples/kick.wav", 0, 1.0),
                        clip("kick.2", "samples/kick.wav", 4 * BEAT, 0.5),
                    ],
                    notes: Vec::new(),
                },
                Track {
                    name: "notes".to_string(),
                    kind: TrackKind::Instrument,
                    clips: Vec::new(),
                    notes: vec![Note {
                        channel: 0,
                        start: 0,
                        length: 2 * BEAT,
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

    #[test]
    fn a_session_survives_the_round_trip() {
        let expected = session();

        let read = parse(&write(&expected)).unwrap();

        assert_eq!(read.name, "song");
        assert_eq!(read.sample_rate, 48_000);
        assert!((read.tempo - 120.0).abs() < 1e-9);
        assert_eq!(read.tracks.len(), 2);
        assert_eq!(read.tracks[0].name, "kick");
        assert_eq!(read.tracks[0].kind, TrackKind::Audio);
        assert_eq!(read.tracks[0].clips, expected.tracks[0].clips);
        assert_eq!(read.tracks[1].kind, TrackKind::Instrument);
        assert_eq!(read.frames, 5 * BEAT);
    }

    #[test]
    fn the_notes_of_a_track_are_a_midi_file_beside_the_session() {
        let read = parse(&write(&session())).unwrap();

        let notes = &read.tracks[1].clips[0];
        assert_eq!(notes.file, "interchange/song/midifiles/notes.mid");
        assert_eq!(notes.start, 0);
        assert_eq!(notes.length, 2 * BEAT);
        // The route and its playlist say midi, so Ardour makes a MIDI track.
        let text = write(&session());
        assert!(text.contains("default-type=\"midi\""), "{text}");
        assert!(text.contains("type=\"midi\""), "{text}");
    }

    #[test]
    fn a_region_carries_its_own_gain() {
        let text = write(&session());

        assert!(text.contains("scale-amplitude=\"0.500000000\""), "{text}");
        assert_eq!(parse(&text).unwrap().tracks[0].clips[1].gain, 0.5);
    }

    #[test]
    fn every_identifier_in_the_session_is_its_own() {
        let text = write(&session());

        let mut ids: Vec<&str> = text
            .match_indices(" id=\"")
            .map(|(at, _)| {
                let rest = &text[at + 5..];
                &rest[..rest.find('"').unwrap_or(0)]
            })
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        // Regions and sources appear in both the pool and the playlist, so
        // repeats are expected there but nowhere else: three sources, three
        // regions, two routes and two playlists.
        assert_eq!(ids.len(), 10, "{ids:?}");
        assert_eq!(count, 13);
    }

    #[test]
    fn a_session_written_by_ardour_is_understood() {
        // Ardour 7 marks a sample count as audio time and drops the origin of
        // a source that lives in the session directory.
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<Session version="7000" name="demo" sample-rate="44100">
  <TempoMap><Tempo beats-per-minute="132" note-type="4"/></TempoMap>
  <Sources>
    <Source name="guitar-1.wav" type="audio" id="17" origin="" channel="1"/>
  </Sources>
  <Regions/>
  <Playlists>
    <Playlist id="20" name="Guitar.1" type="audio" orig-track-id="19">
      <Region name="guitar-1.1" position="a44100" length="a22050" start="a0" id="21" source-0="17"/>
    </Playlist>
  </Playlists>
  <Routes>
    <Route id="19" name="Guitar" default-type="audio"/>
  </Routes>
</Session>"#;

        let session = parse(text).unwrap();

        assert_eq!(session.sample_rate, 44_100);
        assert!((session.tempo - 132.0).abs() < 1e-9);
        assert_eq!(session.tracks[0].name, "Guitar");
        let clip = &session.tracks[0].clips[0];
        assert_eq!(clip.file, "guitar-1.wav");
        assert_eq!((clip.start, clip.length, clip.channel), (44_100, 22_050, 1));
        assert_eq!(clip.gain, 1.0);
    }

    #[test]
    fn broken_sessions_are_reported() {
        let complain = |text: &str| parse(text).unwrap_err().to_string();

        assert!(complain("<Ardour/>").contains("not an Ardour session"));
        assert!(complain("<Session/>").contains("no sample rate"));
        assert!(complain("<Session sample-rate=\"48000\"/>").contains("no playlists"));
        assert!(complain(
            "<Session sample-rate=\"48000\"><Playlists><Playlist><Region source-0=\"3\"/></Playlist></Playlists></Session>"
        )
        .contains("no source"));
        assert!(complain(
            "<Session sample-rate=\"48000\"><TempoMap><Tempo beats-per-minute=\"soon\"/></TempoMap><Playlists/></Session>"
        )
        .contains("is not a tempo"));
    }

    #[test]
    fn a_file_round_trips_through_the_disk() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-ardour-{}-{:?}/song.ardour",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = path.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(&root);

        write_file(&path, &session()).unwrap();
        let read = read_file(&path).unwrap();

        assert_eq!(read.tracks.len(), 2);
        assert_eq!(read.name, "song");

        let _ = fs::remove_dir_all(&root);
    }
}
