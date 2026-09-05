//! Ableton Live sets, the gzipped XML `.als` format.
//!
//! A Live set is one XML document compressed with gzip. The document is large
//! and mostly made of state this crate has nothing to say about, so what is
//! written here is the part that carries a decomposition: a tempo, an audio
//! track per stem and per placed sample with an arranger clip pointing at the
//! file the archive holds, and a MIDI track holding the recognised notes as
//! key tracks.
//!
//! Two things are deliberately not guessed at. Live stores clip gain as a
//! mapped value whose mapping this crate cannot verify, so the gain a placement
//! was matched with is written into the clip annotation, which Live keeps and
//! [`parse`] reads back. And an unwarped clip is what an exported decomposition
//! wants — warping would resample the audio and the reconstruction would stop
//! being exact — so clips are written with warping off.

use std::fs;
use std::path::Path;

use crate::decompose::model::Note;
use crate::error::{Error, Result};
use crate::formats::gzip;
use crate::formats::session::{Clip, Session, Track, TrackKind};
use crate::formats::xml::{self, Element};

/// Sample rate assumed when a set holds no audio to read one from.
pub const FALLBACK_SAMPLE_RATE: u32 = 48_000;
/// Prefix of the gain note left in a clip annotation.
const GAIN_ANNOTATION: &str = "gain ";
/// The Live version the document claims to come from.
const MINOR_VERSION: &str = "11.0_11300";

/// An element holding nothing but a value, which is most of the format.
fn value(name: &str, value: impl Into<String>) -> Element {
    Element::new(name).attribute("Value", value)
}

/// Hands out the identifiers Live puts on tracks and clips.
#[derive(Debug, Default)]
struct Ids(usize);

impl Ids {
    fn next(&mut self) -> String {
        self.0 += 1;
        self.0.to_string()
    }
}

/// Writes a session as the XML inside a Live set.
#[must_use]
pub fn write(session: &Session) -> String {
    xml::document(&document(session))
}

/// Writes a session as a compressed `.als` file.
pub fn write_file(path: impl AsRef<Path>, session: &Session) -> Result<()> {
    gzip::write_file(path, write(session).as_bytes())
}

/// Builds the document.
#[must_use]
pub fn document(session: &Session) -> Element {
    let mut ids = Ids::default();
    let mut tracks = Element::new("Tracks");
    for track in &session.tracks {
        tracks = tracks.child(track_element(track, session, &mut ids));
    }
    Element::new("Ableton")
        .attribute("MajorVersion", "5")
        .attribute("MinorVersion", MINOR_VERSION)
        .attribute("SchemaChangeCount", "3")
        .attribute(
            "Creator",
            format!("audio-decomposer {}", env!("CARGO_PKG_VERSION")),
        )
        .attribute("Revision", "")
        .child(
            Element::new("LiveSet")
                .child(value("OverwriteProtectionNumber", "2819"))
                .child(tracks)
                .child(main_track(session)),
        )
}

/// The main track, which is where Live keeps the tempo.
fn main_track(session: &Session) -> Element {
    Element::new("MainTrack").child(
        Element::new("DeviceChain").child(
            Element::new("Mixer").child(
                Element::new("Tempo")
                    .child(value("Manual", format!("{:.6}", session.tempo)))
                    .child(value("Min", "20"))
                    .child(value("Max", "999")),
            ),
        ),
    )
}

/// One track and the clips on it.
fn track_element(track: &Track, session: &Session, ids: &mut Ids) -> Element {
    let name = Element::new("Name")
        .child(value("EffectiveName", track.name.clone()))
        .child(value("UserName", track.name.clone()))
        .child(value("Annotation", ""));
    match track.kind {
        TrackKind::Audio => {
            let mut events = Element::new("Events");
            for clip in &track.clips {
                events = events.child(audio_clip(clip, session, ids));
            }
            Element::new("AudioTrack")
                .attribute("Id", ids.next())
                .child(name)
                .child(
                    Element::new("DeviceChain").child(
                        Element::new("MainSequencer").child(
                            Element::new("Sample")
                                .child(Element::new("ArrangerAutomation").child(events)),
                        ),
                    ),
                )
        }
        TrackKind::Instrument => Element::new("MidiTrack")
            .attribute("Id", ids.next())
            .child(name)
            .child(Element::new("DeviceChain").child(
                Element::new("MainSequencer").child(
                    Element::new("ClipTimeable").child(Element::new("ArrangerAutomation").child(
                        Element::new("Events").child(midi_clip(&track.notes, session, ids)),
                    )),
                ),
            )),
    }
}

/// One placed piece of audio.
fn audio_clip(clip: &Clip, session: &Session, ids: &mut Ids) -> Element {
    let start = session.beats(clip.start);
    let end = session.beats(clip.start + clip.length);
    Element::new("AudioClip")
        .attribute("Id", ids.next())
        .attribute("Time", format!("{start:.9}"))
        .child(value("CurrentStart", format!("{start:.9}")))
        .child(value("CurrentEnd", format!("{end:.9}")))
        .child(value("Name", clip.name.clone()))
        .child(value(
            "Annotation",
            format!("{GAIN_ANNOTATION}{gain:.9}", gain = clip.gain),
        ))
        .child(
            Element::new("Loop")
                .child(value(
                    "LoopStart",
                    format!("{:.9}", session.seconds(clip.offset)),
                ))
                .child(value(
                    "LoopEnd",
                    format!("{:.9}", session.seconds(clip.offset + clip.length)),
                ))
                .child(value("StartRelative", "0"))
                .child(value("LoopOn", "false")),
        )
        .child(
            Element::new("SampleRef")
                .child(
                    Element::new("FileRef")
                        .child(value("RelativePathType", "3"))
                        .child(value("RelativePath", clip.file.clone()))
                        .child(value("Path", clip.file.clone()))
                        .child(value("Type", "1")),
                )
                .child(value(
                    "DefaultDuration",
                    (clip.offset + clip.length).to_string(),
                ))
                .child(value("DefaultSampleRate", session.sample_rate.to_string())),
        )
        .child(value("IsWarped", "false"))
        .child(value("WarpMode", "0"))
        .child(value("HiQ", "true"))
        .child(value("Disabled", "false"))
}

/// The notes of a track, as one clip of key tracks.
fn midi_clip(notes: &[Note], session: &Session, ids: &mut Ids) -> Element {
    let start = notes.iter().map(|note| note.start).min().unwrap_or(0);
    let end = notes.iter().map(Note::end).max().unwrap_or(0);

    let mut keys: Vec<u8> = notes.iter().map(|note| note.note).collect();
    keys.sort_unstable();
    keys.dedup();

    let mut tracks = Element::new("KeyTracks");
    for key in keys {
        let mut events = Element::new("Notes");
        for note in notes.iter().filter(|note| note.note == key) {
            events = events.child(
                Element::new("MidiNoteEvent")
                    .attribute("Time", format!("{:.9}", session.beats(note.start - start)))
                    .attribute("Duration", format!("{:.9}", session.beats(note.length)))
                    .attribute("Velocity", format!("{:.9}", f64::from(note.velocity)))
                    .attribute("OffVelocity", "64")
                    .attribute("IsEnabled", "true"),
            );
        }
        tracks = tracks.child(
            Element::new("KeyTrack")
                .attribute("Id", ids.next())
                .child(events)
                .child(value("MidiKey", key.to_string())),
        );
    }

    Element::new("MidiClip")
        .attribute("Id", ids.next())
        .attribute("Time", format!("{:.9}", session.beats(start)))
        .child(value(
            "CurrentStart",
            format!("{:.9}", session.beats(start)),
        ))
        .child(value("CurrentEnd", format!("{:.9}", session.beats(end))))
        .child(value("Name", "notes"))
        .child(Element::new("Notes").child(tracks))
}

/// Reads a `.als` file back into a session.
pub fn read_file(path: impl AsRef<Path>) -> Result<Session> {
    read(&fs::read(path)?)
}

/// Reads the bytes of a `.als` file back into a session.
pub fn read(bytes: &[u8]) -> Result<Session> {
    let text = String::from_utf8(gzip::decompress(bytes)?)
        .map_err(|error| Error::Format(error.to_string()))?;
    parse(&text)
}

/// Reads the XML of a Live set back into a session.
pub fn parse(text: &str) -> Result<Session> {
    let root = xml::parse(text)?;
    if root.name != "Ableton" {
        return Err(Error::Format(format!(
            "`{}` is not an Ableton Live set",
            root.name
        )));
    }
    let set = root
        .find("LiveSet")
        .ok_or_else(|| Error::Format("the document holds no live set".to_string()))?;
    let tempo = set
        .find("MainTrack")
        .or_else(|| set.find("MasterTrack"))
        .and_then(|track| track.find("DeviceChain"))
        .and_then(|chain| chain.find("Mixer"))
        .and_then(|mixer| mixer.find("Tempo"))
        .and_then(|tempo| tempo.find("Manual"))
        .and_then(|manual| manual.get("Value"))
        .map_or(Ok(crate::formats::session::DEFAULT_TEMPO), |value| {
            number(value, "tempo")
        })?;
    let tracks = set
        .find("Tracks")
        .ok_or_else(|| Error::Format("the live set has no tracks".to_string()))?;

    let mut session = Session {
        name: String::new(),
        sample_rate: sample_rate(tracks),
        tempo,
        frames: 0,
        channels: 2,
        tracks: Vec::new(),
        instruments: Vec::new(),
    };

    for element in tracks.elements() {
        let audio = element.name == "AudioTrack";
        if !audio && element.name != "MidiTrack" {
            continue;
        }
        let mut track = Track {
            name: element
                .find("Name")
                .and_then(|name| name.find("EffectiveName"))
                .and_then(|name| name.get("Value"))
                .unwrap_or_default()
                .to_string(),
            kind: if audio {
                TrackKind::Audio
            } else {
                TrackKind::Instrument
            },
            clips: Vec::new(),
            notes: Vec::new(),
        };
        for events in element.find_all("DeviceChain") {
            read_events(events, &session, &mut track)?;
        }
        session.tracks.push(track);
    }
    session.frames = session.end();
    Ok(session)
}

/// The sample rate the audio in the set was recorded at.
fn sample_rate(tracks: &Element) -> u32 {
    let mut found = None;
    walk(tracks, &mut |element| {
        if element.name == "DefaultSampleRate" {
            if let Some(rate) = element.get("Value").and_then(|value| value.parse().ok()) {
                found.get_or_insert(rate);
            }
        }
    });
    found.unwrap_or(FALLBACK_SAMPLE_RATE)
}

/// Visits an element and everything under it.
fn walk(element: &Element, visit: &mut impl FnMut(&Element)) {
    visit(element);
    for child in element.elements() {
        walk(child, visit);
    }
}

/// Reads every clip under a device chain onto a track.
fn read_events(element: &Element, session: &Session, track: &mut Track) -> Result<()> {
    let mut audio = Vec::new();
    let mut midi = Vec::new();
    walk(element, &mut |element| match element.name.as_str() {
        "AudioClip" => audio.push(element.clone()),
        "MidiClip" => midi.push(element.clone()),
        _ => {}
    });
    for clip in &audio {
        track.clips.push(read_audio_clip(clip, session)?);
    }
    for clip in &midi {
        read_midi_clip(clip, session, track)?;
    }
    track.notes.sort_by_key(|note| (note.start, note.note));
    Ok(())
}

/// Reads one audio clip.
fn read_audio_clip(clip: &Element, session: &Session) -> Result<Clip> {
    let start = beats_to_frames(session, number(child(clip, "CurrentStart")?, "clip start")?);
    let end = beats_to_frames(session, number(child(clip, "CurrentEnd")?, "clip end")?);
    let file = clip
        .find("SampleRef")
        .and_then(|reference| reference.find("FileRef"))
        .and_then(|reference| {
            reference
                .find("RelativePath")
                .or_else(|| reference.find("Path"))
        })
        .and_then(|path| path.get("Value"))
        .ok_or_else(|| Error::Format("an audio clip has no file".to_string()))?;
    let offset = clip
        .find("Loop")
        .and_then(|looping| looping.find("LoopStart"))
        .and_then(|start| start.get("Value"))
        .map_or(Ok(0.0), |value| number(value, "clip offset"))?;
    Ok(Clip {
        name: clip
            .find("Name")
            .and_then(|name| name.get("Value"))
            .unwrap_or_default()
            .to_string(),
        file: file.to_string(),
        start,
        length: end.saturating_sub(start),
        offset: seconds_to_frames(session, offset),
        gain: gain_of(
            clip.find("Annotation")
                .and_then(|annotation| annotation.get("Value")),
        )?,
        channel: 0,
    })
}

/// Reads the notes of one MIDI clip onto a track.
fn read_midi_clip(clip: &Element, session: &Session, track: &mut Track) -> Result<()> {
    let start = beats_to_frames(session, number(child(clip, "CurrentStart")?, "clip start")?);
    let Some(keys) = clip.find("Notes").and_then(|notes| notes.find("KeyTracks")) else {
        return Ok(());
    };
    for key in keys.find_all("KeyTrack") {
        let pitch: u8 = key
            .find("MidiKey")
            .and_then(|midi| midi.get("Value"))
            .unwrap_or("60")
            .parse()
            .map_err(|_| Error::Parse("a key track has no key".to_string()))?;
        let Some(events) = key.find("Notes") else {
            continue;
        };
        for event in events.find_all("MidiNoteEvent") {
            let at = beats_to_frames(
                session,
                number(event.get("Time").unwrap_or("0"), "note time")?,
            );
            track.notes.push(Note {
                channel: 0,
                start: start + at,
                length: beats_to_frames(
                    session,
                    number(event.get("Duration").unwrap_or("0"), "note length")?,
                ),
                note: pitch,
                velocity: number(event.get("Velocity").unwrap_or("100"), "velocity")?
                    .round()
                    .clamp(0.0, 127.0) as u8,
                frequency: 440.0 * ((f64::from(pitch) - 69.0) / 12.0).exp2(),
                confidence: 1.0,
            });
        }
    }
    Ok(())
}

/// The value of a child element that has to be there.
fn child<'a>(element: &'a Element, name: &str) -> Result<&'a str> {
    element
        .find(name)
        .and_then(|child| child.get("Value"))
        .ok_or_else(|| Error::Format(format!("a clip has no `{name}`")))
}

/// Reads the gain out of a clip annotation, defaulting to unity.
fn gain_of(annotation: Option<&str>) -> Result<f64> {
    annotation
        .and_then(|text| text.strip_prefix(GAIN_ANNOTATION).map(str::trim))
        .map_or(Ok(1.0), |value| number(value, "clip gain"))
}

/// Frames a number of beats lasts.
fn beats_to_frames(session: &Session, beats: f64) -> usize {
    if session.tempo <= 0.0 {
        return 0;
    }
    seconds_to_frames(session, beats * 60.0 / session.tempo)
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
                    notes: vec![
                        note(0, BEAT, 60, 100),
                        note(2 * BEAT, BEAT, 67, 80),
                        note(2 * BEAT, BEAT, 60, 90),
                    ],
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
        assert_eq!(read.sample_rate, 48_000);
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
            let mut notes = expected.notes.clone();
            notes.sort_by_key(|note| (note.start, note.note));
            assert_eq!(read.notes, notes);
        }
    }

    #[test]
    fn the_document_looks_like_a_live_set() {
        let text = write(&session());

        assert!(text.contains("<Ableton MajorVersion=\"5\""), "{text}");
        assert!(text.contains("<AudioTrack Id="), "{text}");
        assert!(text.contains("<MidiTrack Id="), "{text}");
        assert!(
            text.contains("<RelativePath Value=\"samples/kick.wav\"/>"),
            "{text}"
        );
        assert!(text.contains("<Manual Value=\"120.000000\"/>"), "{text}");
        // An exported decomposition is exact only if Live plays it unwarped.
        assert!(text.contains("<IsWarped Value=\"false\"/>"), "{text}");
    }

    #[test]
    fn notes_of_the_same_pitch_share_one_key_track() {
        let text = write(&session());

        // Three notes on two pitches: two key tracks.
        assert_eq!(text.matches("<KeyTrack ").count(), 2);
        assert_eq!(text.matches("<MidiNoteEvent ").count(), 3);
    }

    #[test]
    fn clip_gain_is_kept_where_the_format_hides_it() {
        assert!(write(&session()).contains("gain 0.500000000"));
        assert_eq!(gain_of(Some("gain 0.25")).unwrap(), 0.25);
        assert_eq!(gain_of(None).unwrap(), 1.0);
        assert_eq!(gain_of(Some("a note to self")).unwrap(), 1.0);
    }

    #[test]
    fn a_set_written_by_live_is_understood() {
        // Live 10 called the tempo track `MasterTrack`; both names are read.
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="5" MinorVersion="10.0_377" Creator="Ableton Live 10.1.30">
  <LiveSet>
    <Tracks>
      <AudioTrack Id="8">
        <Name><EffectiveName Value="Guitar"/></Name>
        <DeviceChain><MainSequencer><Sample><ArrangerAutomation><Events>
          <AudioClip Id="0" Time="4">
            <CurrentStart Value="4"/>
            <CurrentEnd Value="8"/>
            <Name Value="take 3"/>
            <SampleRef><FileRef>
              <RelativePath Value="Samples/Recorded/take.wav"/>
            </FileRef><DefaultSampleRate Value="44100"/></SampleRef>
          </AudioClip>
        </Events></ArrangerAutomation></Sample></MainSequencer></DeviceChain>
      </AudioTrack>
    </Tracks>
    <MasterTrack><DeviceChain><Mixer><Tempo><Manual Value="96"/></Tempo></Mixer></DeviceChain></MasterTrack>
  </LiveSet>
</Ableton>"#;

        let session = parse(text).unwrap();

        assert_eq!(session.sample_rate, 44_100);
        assert!((session.tempo - 96.0).abs() < 1e-9);
        let clip = &session.tracks[0].clips[0];
        assert_eq!(clip.name, "take 3");
        assert_eq!(clip.file, "Samples/Recorded/take.wav");
        assert_eq!(clip.start, (4.0 * 60.0 / 96.0 * 44_100.0) as usize);
        assert_eq!(clip.length, (4.0 * 60.0 / 96.0 * 44_100.0) as usize);
    }

    #[test]
    fn broken_sets_are_reported() {
        let complain = |text: &str| parse(text).unwrap_err().to_string();

        assert!(complain("<LiveSet/>").contains("not an Ableton Live set"));
        assert!(complain("<Ableton/>").contains("no live set"));
        assert!(complain("<Ableton><LiveSet/></Ableton>").contains("no tracks"));
        assert!(complain(
            "<Ableton><LiveSet><Tracks><AudioTrack><DeviceChain><AudioClip><CurrentStart Value=\"0\"/><CurrentEnd Value=\"1\"/></AudioClip></DeviceChain></AudioTrack></Tracks></LiveSet></Ableton>"
        )
        .contains("no file"));
        assert!(read(b"not gzip").unwrap_err().to_string().contains("gzip"));
    }

    #[test]
    fn a_file_round_trips_through_the_disk_compressed() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-ableton-{}-{:?}/song.als",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = path.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(&root);

        write_file(&path, &session()).unwrap();

        // The file on disk is gzip, not XML.
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
        assert_eq!(read_file(&path).unwrap().tracks.len(), 2);

        let _ = fs::remove_dir_all(&root);
    }
}
