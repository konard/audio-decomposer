//! SFZ, the open instrument format samplers agree on.
//!
//! The bank a decomposition produces is an instrument: a handful of waveforms
//! and the note each of them was recognised as. SFZ is the format that says
//! exactly that, in text, and it is read by Sforzando, sfizz, Bitwig, Cakewalk,
//! Kontakt via conversion and every free sampler in between.
//!
//! Two kinds of sample come out of a decomposition and they are mapped
//! differently:
//!
//! - Pitched samples get the key they were recognised as, and the keys around
//!   them up to the midpoint towards the next sample, so the whole keyboard
//!   plays even from a bank of three notes.
//! - Unpitched samples are laid out one per key from the General MIDI kick
//!   upwards on channel 10, the drum channel, where nothing transposes them.

use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
use crate::formats::session::Session;

/// Where the unpitched samples start on the drum channel: the General MIDI
/// bass drum.
pub const DRUM_ORIGIN: u8 = 36;

/// One mapped sample.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    /// Path of the waveform, relative to the instrument file.
    pub sample: String,
    /// Lowest key that plays it.
    pub low_key: u8,
    /// Highest key that plays it.
    pub high_key: u8,
    /// Key at which it plays untransposed.
    pub key_center: u8,
    /// Lowest MIDI channel that plays it, counted from one.
    pub low_channel: u8,
    /// Highest MIDI channel that plays it, counted from one.
    pub high_channel: u8,
}

/// Writes the bank of a session as an SFZ instrument.
#[must_use]
pub fn write(session: &Session) -> String {
    let mut out = format!(
        "// {} — instrument built from a decomposed recording\n\
         // {} sample(s), {} Hz\n\n<control>\n\n<global>\nampeg_attack=0\nampeg_release=0.05\n",
        session.name,
        session.instruments.len(),
        session.sample_rate
    );

    let all = regions(session);
    let (pitched, unpitched): (Vec<_>, Vec<_>) =
        all.iter().partition(|region| region.low_channel < 10);
    if !pitched.is_empty() {
        out.push_str("\n<group> // recognised notes, played across the keyboard\n");
        for region in pitched {
            out.push_str(&line(region));
        }
    }
    if !unpitched.is_empty() {
        out.push_str("\n<group> // unpitched samples, one per key on the drum channel\n");
        for region in unpitched {
            out.push_str(&line(region));
        }
    }
    out
}

/// Writes the instrument to a file.
pub fn write_file(path: impl AsRef<Path>, session: &Session) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, write(session))?;
    Ok(())
}

/// The regions a session maps to, pitched ones first.
#[must_use]
pub fn regions(session: &Session) -> Vec<Region> {
    let mut pitched: Vec<(u8, &str)> = session
        .instruments
        .iter()
        .filter_map(|instrument| Some((instrument.note?, instrument.file.as_str())))
        .collect();
    pitched.sort_by_key(|(note, _)| *note);
    // One sample per key. A bank often holds several recordings of the same
    // note; mapping them all to it would stack them into one louder voice, and
    // would leave the zone around a repeated note inside out.
    pitched.dedup_by_key(|(note, _)| *note);

    let mut out = Vec::with_capacity(session.instruments.len());
    for (index, (note, file)) in pitched.iter().enumerate() {
        let low = if index == 0 {
            0
        } else {
            let previous = pitched[index - 1].0;
            previous + (note - previous).div_ceil(2)
        };
        let high = if index + 1 == pitched.len() {
            127
        } else {
            let next = pitched[index + 1].0;
            note + (next - note).div_ceil(2) - 1
        };
        out.push(Region {
            sample: (*file).to_string(),
            low_key: low,
            high_key: high,
            key_center: *note,
            low_channel: 1,
            high_channel: 9,
        });
    }

    let unpitched = session
        .instruments
        .iter()
        .filter(|instrument| instrument.note.is_none());
    for (index, instrument) in unpitched.enumerate() {
        let Ok(offset) = u8::try_from(index) else {
            break;
        };
        let Some(key) = DRUM_ORIGIN.checked_add(offset).filter(|key| *key <= 127) else {
            break;
        };
        out.push(Region {
            sample: instrument.file.clone(),
            low_key: key,
            high_key: key,
            key_center: key,
            low_channel: 10,
            high_channel: 10,
        });
    }
    out
}

/// Writes one region.
fn line(region: &Region) -> String {
    format!(
        "<region> sample={} lokey={} hikey={} pitch_keycenter={} lochan={} hichan={}\n",
        region.sample,
        region.low_key,
        region.high_key,
        region.key_center,
        region.low_channel,
        region.high_channel
    )
}

/// Reads the regions out of an instrument file.
///
/// Only the opcodes this crate writes are understood; anything else is skipped,
/// which is what an SFZ reader is supposed to do with opcodes it does not know.
pub fn parse(text: &str) -> Result<Vec<Region>> {
    let mut regions: Vec<Region> = Vec::new();
    let mut prefix = String::new();
    let mut inside = false;
    for token in tokens(text) {
        match token {
            Token::Header(name) => {
                inside = name == "region";
                if inside {
                    regions.push(Region {
                        sample: String::new(),
                        low_key: 0,
                        high_key: 127,
                        key_center: 60,
                        low_channel: 1,
                        high_channel: 16,
                    });
                }
            }
            Token::Opcode(key, value) => {
                if key == "default_path" {
                    prefix = value.to_string();
                    continue;
                }
                let Some(region) = regions.last_mut().filter(|_| inside) else {
                    continue;
                };
                let number = |what: &str| {
                    value
                        .parse::<u8>()
                        .map_err(|_| Error::Parse(format!("`{value}` is not a value for {what}")))
                };
                match key {
                    "sample" => region.sample = format!("{prefix}{value}"),
                    "lokey" => region.low_key = number("lokey")?,
                    "hikey" => region.high_key = number("hikey")?,
                    "pitch_keycenter" => region.key_center = number("pitch_keycenter")?,
                    "lochan" => region.low_channel = number("lochan")?,
                    "hichan" => region.high_channel = number("hichan")?,
                    _ => {}
                }
            }
        }
    }
    Ok(regions)
}

/// Reads the regions out of an instrument file on disk.
pub fn read_file(path: impl AsRef<Path>) -> Result<Vec<Region>> {
    parse(&fs::read_to_string(path)?)
}

/// A header or an opcode.
enum Token<'a> {
    /// `<region>`, without the angle brackets.
    Header(&'a str),
    /// `key=value`.
    Opcode(&'a str, &'a str),
}

/// Splits instrument text into headers and opcodes.
///
/// A value runs to the next opcode or header, because `sample=` is allowed to
/// name a file with spaces in it.
fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or_default().trim();
        let mut rest = line;
        while !rest.is_empty() {
            if let Some(tail) = rest.strip_prefix('<') {
                let Some(end) = tail.find('>') else { break };
                tokens.push(Token::Header(&tail[..end]));
                rest = tail[end + 1..].trim_start();
                continue;
            }
            let Some(equals) = rest.find('=') else { break };
            let key = rest[..equals].trim();
            let value = rest[equals + 1..].trim_start();
            let end = value
                .char_indices()
                .filter(|(_, character)| *character == ' ')
                .find(|(index, _)| {
                    let ahead = &value[index + 1..];
                    ahead.starts_with('<')
                        || ahead
                            .split(' ')
                            .next()
                            .is_some_and(|word| word.contains('='))
                })
                .map_or(value.len(), |(index, _)| index);
            tokens.push(Token::Opcode(key, value[..end].trim_end()));
            rest = value[end..].trim_start();
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::session::Instrument;

    fn session(instruments: Vec<Instrument>) -> Session {
        Session {
            name: "song".to_string(),
            sample_rate: 44_100,
            tempo: 120.0,
            frames: 44_100,
            channels: 1,
            tracks: Vec::new(),
            instruments,
        }
    }

    fn instrument(id: usize, name: &str, note: Option<u8>) -> Instrument {
        Instrument {
            id,
            name: name.to_string(),
            file: format!("samples/{name}.wav"),
            note,
            frames: 100,
            peak: 0.5,
        }
    }

    #[test]
    fn two_recordings_of_one_note_map_to_one_key_zone() {
        // A bank routinely holds the same note twice. Mapping both would leave
        // the second zone inside out: hikey below the key it is centred on.
        let session = session(vec![
            instrument(0, "first", Some(60)),
            instrument(1, "second", Some(60)),
            instrument(2, "higher", Some(72)),
        ]);

        let regions = regions(&session);

        assert_eq!(regions.len(), 2);
        for region in &regions {
            assert!(
                region.low_key <= region.key_center && region.key_center <= region.high_key,
                "{region:?} is inside out"
            );
        }
        assert_eq!(regions[0].sample, "samples/first.wav");
    }

    #[test]
    fn pitched_samples_cover_the_whole_keyboard() {
        let session = session(vec![
            instrument(0, "a", Some(69)),
            instrument(1, "c", Some(60)),
            instrument(2, "e", Some(64)),
        ]);

        let regions = regions(&session);

        assert_eq!(regions.len(), 3);
        assert_eq!(
            regions
                .iter()
                .map(|region| (region.low_key, region.key_center, region.high_key))
                .collect::<Vec<_>>(),
            [(0, 60, 61), (62, 64, 66), (67, 69, 127)]
        );
        // Nothing is left unplayable between the samples.
        for key in 0..=127u8 {
            assert!(
                regions
                    .iter()
                    .any(|region| (region.low_key..=region.high_key).contains(&key)),
                "key {key} plays nothing"
            );
        }
    }

    #[test]
    fn unpitched_samples_land_on_the_drum_channel() {
        let session = session(vec![
            instrument(0, "kick", None),
            instrument(1, "snare", None),
            instrument(2, "bass", Some(36)),
        ]);

        let regions = regions(&session);

        assert_eq!(regions[0].key_center, 36);
        assert_eq!(regions[0].low_channel, 1);
        assert_eq!(regions[1].sample, "samples/kick.wav");
        assert_eq!(
            (
                regions[1].low_key,
                regions[1].high_key,
                regions[1].low_channel
            ),
            (36, 36, 10)
        );
        assert_eq!(regions[2].low_key, 37);
    }

    #[test]
    fn an_instrument_survives_the_round_trip() {
        let session = session(vec![
            instrument(0, "kick", None),
            instrument(1, "c", Some(60)),
            instrument(2, "g", Some(67)),
        ]);

        let text = write(&session);
        assert!(text.contains("<region> sample=samples/c.wav"));

        assert_eq!(parse(&text).unwrap(), regions(&session));
    }

    #[test]
    fn an_instrument_written_elsewhere_is_understood() {
        let text = "// a hand-written instrument\n\
             <control>\n\
             default_path=Samples/\n\
             <global> volume=-3\n\
             <region> sample=Grand Piano C3.wav lokey=48 hikey=59 pitch_keycenter=48 tune=4\n\
             <region>\n  sample=hat.wav\n  lokey=42 hikey=42 pitch_keycenter=42 lochan=10 hichan=10\n";

        let regions = parse(text).unwrap();

        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].sample, "Samples/Grand Piano C3.wav");
        assert_eq!((regions[0].low_key, regions[0].high_key), (48, 59));
        assert_eq!(regions[1].sample, "Samples/hat.wav");
        assert_eq!(regions[1].low_channel, 10);
    }

    #[test]
    fn nonsense_values_are_reported() {
        let error = parse("<region> sample=a.wav lokey=x\n")
            .unwrap_err()
            .to_string();

        assert!(error.contains("lokey"), "{error}");
    }

    #[test]
    fn a_bank_without_notes_still_writes_an_instrument() {
        let session = session(vec![instrument(0, "noise", None)]);

        let text = write(&session);

        assert!(text.contains("drum channel"));
        assert!(!text.contains("across the keyboard"));
        assert_eq!(parse(&text).unwrap().len(), 1);
    }

    #[test]
    fn an_empty_bank_writes_a_header_and_nothing_else() {
        let text = write(&session(Vec::new()));

        assert!(text.contains("<global>"));
        assert!(parse(&text).unwrap().is_empty());
    }

    #[test]
    fn a_file_round_trips_through_the_disk() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "audio-decomposer-sfz-{}-{:?}/song.sfz",
            std::process::id(),
            std::thread::current().id()
        ));
        let root = path.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(&root);

        let session = session(vec![instrument(0, "c", Some(60))]);
        write_file(&path, &session).unwrap();

        assert_eq!(read_file(&path).unwrap(), regions(&session));

        let _ = fs::remove_dir_all(&root);
    }
}
