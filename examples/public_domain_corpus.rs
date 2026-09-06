//! Fetches real public-domain music and runs the whole pipeline over it.
//!
//! Every fixture in the test suite is synthesised, which keeps the suite fast,
//! offline and free of any copyright question — but synthesised audio is also
//! easier than the real thing. This example closes that gap the only way that
//! stays lawful: it asks Wikimedia Commons for recordings whose licence is
//! Public Domain or CC0, downloads them, writes down where each one came from,
//! and then decomposes and rebuilds each one and checks the result is the
//! recording it started from, sample for sample.
//!
//! ```text
//! cargo run --release --example public_domain_corpus
//! cargo run --release --example public_domain_corpus -- --dir corpus --count 6 --seconds 20
//! cargo run --release --example public_domain_corpus -- --fetch-only
//! ```
//!
//! The corpus directory it fills is what `AUDIO_DECOMPOSER_CORPUS` should point
//! at to make the integration suite run against real music as well:
//!
//! ```text
//! AUDIO_DECOMPOSER_CORPUS=corpus cargo test --test integration public_domain
//! ```
//!
//! Downloading needs `curl` on the path and a network connection. Nothing in
//! the test suite needs either, which is why this lives here and not there.

use std::path::{Path, PathBuf};
use std::process::Command;

use audio_decomposer::decompose::{decompose, DecomposeOptions};
use audio_decomposer::{audio, recompose, Audio, Container, Error, Result};

/// Searches run against Commons, chosen to cover different kinds of music.
///
/// One recording of one instrument would say very little. A cappella voices, a
/// solo piano, a bowed string, a wind band and a full orchestra stress the
/// onset detector, the stem separation and the matcher in different places.
const SEARCHES: [&str; 5] = [
    "filemime:audio/x-flac Palestrina",
    "filemime:audio/x-flac piano",
    "filemime:audio/x-flac violin",
    "filemime:audio/x-flac band march",
    "filemime:audio/x-flac orchestra",
];

/// How the example was asked to run.
struct Options {
    directory: PathBuf,
    count: usize,
    max_bytes: u64,
    seconds: f64,
    fetch_only: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("corpus"),
            count: 5,
            // Large enough for a whole movement, small enough to be polite to
            // a volunteer-funded archive.
            max_bytes: 25 * 1024 * 1024,
            // Matching pursuit is quadratic in the number of events, so a
            // whole symphony would take longer than anyone wants to wait.
            seconds: 15.0,
            fetch_only: false,
        }
    }
}

/// One recording, and everything needed to credit it.
#[derive(Clone)]
struct Recording {
    title: String,
    artist: String,
    licence: String,
    page: String,
    url: String,
    bytes: u64,
}

impl Recording {
    /// The file name it is saved under, with nothing surprising in it.
    fn file_name(&self) -> String {
        let stem: String = self
            .title
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let trimmed = stem.trim_matches('-');
        let squeezed = trimmed.split('-').filter(|part| !part.is_empty());
        let mut name = squeezed.collect::<Vec<_>>().join("-");
        name.truncate(80);
        let extension = Path::new(&self.url)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("flac");
        format!("{name}.{extension}")
    }
}

fn main() -> Result<()> {
    let options = parse()?;
    std::fs::create_dir_all(&options.directory)
        .map_err(|error| audio_decomposer::error::io_at(&options.directory, &error))?;

    // One search at a time, taking a single hit from each before coming round
    // again. Draining the first search would fill the corpus with five
    // movements of one mass, which says little that the first one did not.
    let mut hits: Vec<Vec<Recording>> = SEARCHES
        .into_iter()
        .filter_map(|search| match ask_commons(search, &options) {
            Ok(recordings) => Some(recordings),
            Err(error) => {
                eprintln!("search {search:?} failed: {error}");
                None
            }
        })
        .collect();

    let mut cursors = vec![0_usize; hits.len()];
    let mut found: Vec<Recording> = Vec::new();
    while found.len() < options.count {
        let mut took = false;
        for (search, cursor) in hits.iter_mut().zip(&mut cursors) {
            if found.len() >= options.count {
                break;
            }
            let Some(recording) = search.get(*cursor) else {
                continue;
            };
            *cursor += 1;
            took = true;
            if found.iter().any(|kept| kept.url == recording.url) {
                continue;
            }
            println!("found  {} [{}]", recording.title, recording.licence);
            found.push(recording.clone());
        }
        if !took {
            break;
        }
    }
    if found.is_empty() {
        eprintln!("no public-domain recordings matched; nothing to do");
        return Ok(());
    }

    let mut fetched = Vec::new();
    for recording in &found {
        let path = options.directory.join(recording.file_name());
        if path.exists() {
            println!("have   {}", path.display());
        } else {
            println!("fetch  {} ({} bytes)", path.display(), recording.bytes);
            if let Err(error) = download(&recording.url, &path) {
                eprintln!("       failed: {error}");
                continue;
            }
        }
        fetched.push((recording, path));
    }
    credits(&options.directory, &found)?;

    if options.fetch_only {
        println!(
            "\n{} recording(s) in {}",
            fetched.len(),
            options.directory.display()
        );
        return Ok(());
    }

    println!();
    let mut exact = 0_usize;
    for (recording, path) in &fetched {
        match round_trip(path, options.seconds) {
            Ok(report) => {
                exact += usize::from(report.is_exact());
                println!(
                    "{}\n  {:.1} s, {} Hz, {} channel(s): {} sample(s), {} placement(s), \
                     {} note(s), reuse {:.2}x, {}",
                    recording.title,
                    report.duration_seconds(),
                    report.sample_rate,
                    report.channels,
                    report.samples,
                    report.placements,
                    report.notes,
                    report.reuse(),
                    if report.is_exact() {
                        "exact".to_string()
                    } else {
                        format!("off by {}", report.deviation)
                    }
                );
            }
            Err(error) => println!("{}\n  failed: {error}", recording.title),
        }
    }
    println!("\n{exact}/{} reconstructions were exact", fetched.len());
    Ok(())
}

/// Reads the command line, which is all optional.
fn parse() -> Result<Options> {
    let mut options = Options::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| Error::InvalidArgument(format!("{argument} needs a value")))
        };
        match argument.as_str() {
            "--dir" => options.directory = PathBuf::from(value()?),
            "--count" => options.count = number(&value()?)? as usize,
            "--max-mb" => options.max_bytes = number(&value()?)? as u64 * 1024 * 1024,
            "--seconds" => options.seconds = number(&value()?)?,
            "--fetch-only" => options.fetch_only = true,
            other => {
                return Err(Error::InvalidArgument(format!(
                    "unknown option {other}; see the documentation at the top of this example"
                )))
            }
        }
    }
    Ok(options)
}

/// Parses a number, saying which text was not one.
fn number(text: &str) -> Result<f64> {
    text.parse()
        .map_err(|_| Error::InvalidArgument(format!("{text} is not a number")))
}

/// Asks Commons for files matching a search, keeping only the free ones.
fn ask_commons(search: &str, options: &Options) -> Result<Vec<Recording>> {
    let query = format!(
        "https://commons.wikimedia.org/w/api.php?action=query&format=json\
         &generator=search&gsrnamespace=6&gsrlimit=40&gsrsearch={}\
         &prop=imageinfo&iiprop=url|size|mime|extmetadata",
        escape(search)
    );
    let body = curl(&["-s", "--fail", "--max-time", "60", &query])?;
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|error| Error::Parse(format!("Commons answered with something else: {error}")))?;

    let mut recordings: Vec<Recording> = json["query"]["pages"]
        .as_object()
        .into_iter()
        .flat_map(|pages| pages.values())
        .filter_map(|page| recording(page, options))
        .collect();
    // Smallest first: the point is coverage of different music, not of length.
    recordings.sort_by_key(|recording| recording.bytes);
    Ok(recordings)
}

/// Turns one search hit into a recording, or discards it.
fn recording(page: &serde_json::Value, options: &Options) -> Option<Recording> {
    let info = page["imageinfo"].get(0)?;
    let url = info["url"].as_str()?.split('?').next()?.to_string();
    // Only what this crate can actually open; Commons also carries Ogg Vorbis.
    Container::of_path(&url)?;

    let bytes = info["size"].as_u64()?;
    if bytes > options.max_bytes {
        return None;
    }

    let metadata = &info["extmetadata"];
    let licence = text(&metadata["LicenseShortName"]);
    let terms = text(&metadata["UsageTerms"]);
    if !is_free(&licence) && !is_free(&terms) {
        return None;
    }

    Some(Recording {
        title: text(&metadata["ObjectName"]),
        artist: text(&metadata["Artist"]),
        licence,
        page: info["descriptionurl"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        url,
        bytes,
    })
}

/// Whether a licence puts the recording beyond any copyright question.
///
/// Deliberately strict. Attribution licences are free too, but the issue this
/// corpus answers asks for public domain, and a false positive here would put
/// a file in the repository's working tree that does not belong there.
fn is_free(licence: &str) -> bool {
    let licence = licence.to_ascii_lowercase();
    licence.contains("public domain") || licence.contains("cc0") || licence.contains("pd-")
}

/// Reads one metadata field, with the markup Commons wraps it in taken off.
fn text(field: &serde_json::Value) -> String {
    let raw = field["value"].as_str().unwrap_or_default();
    let mut plain = String::with_capacity(raw.len());
    let mut inside = false;
    for character in raw.chars() {
        match character {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => plain.push(character),
            _ => {}
        }
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Percent-encodes a search term for a query string.
fn escape(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Saves a URL to a file, atomically enough that a broken download is obvious.
fn download(url: &str, path: &Path) -> Result<()> {
    let partial = path.with_extension("partial");
    curl(&[
        "-s",
        "--fail",
        "--location",
        "--max-time",
        "600",
        "-o",
        &partial.to_string_lossy(),
        url,
    ])?;
    std::fs::rename(&partial, path).map_err(|error| audio_decomposer::error::io_at(path, &error))
}

/// Runs `curl`, turning its failures into this crate's errors.
fn curl(arguments: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("curl")
        .args(arguments)
        .arg("--user-agent")
        .arg("audio-decomposer example (https://github.com/konard/audio-decomposer)")
        .output()
        .map_err(|error| {
            Error::Io(std::io::Error::other(format!(
                "could not run curl: {error}"
            )))
        })?;
    if !output.status.success() {
        return Err(Error::Io(std::io::Error::other(format!(
            "curl exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))));
    }
    Ok(output.stdout)
}

/// Writes down where every recording came from and under what licence.
fn credits(directory: &Path, recordings: &[Recording]) -> Result<()> {
    use std::fmt::Write;

    let mut text = String::from(
        "# Corpus credits\n\n\
         Every recording here is Public Domain or CC0. This file is written by\n\
         `cargo run --example public_domain_corpus`; nothing in it is committed\n\
         to the repository.\n\n",
    );
    for recording in recordings {
        let _ = write!(
            text,
            "## {}\n\n- File: `{}`\n- Artist: {}\n- Licence: {}\n- Source: {}\n- Download: {}\n\n",
            recording.title,
            recording.file_name(),
            if recording.artist.is_empty() {
                "unstated"
            } else {
                &recording.artist
            },
            recording.licence,
            recording.page,
            recording.url,
        );
    }
    let path = directory.join("CREDITS.md");
    std::fs::write(&path, text).map_err(|error| audio_decomposer::error::io_at(&path, &error))
}

/// Decomposes the first `seconds` of a recording and rebuilds it.
fn round_trip(path: &Path, seconds: f64) -> Result<recompose::Report> {
    let whole = audio::read_file(path)?;
    let wanted = (seconds * f64::from(whole.sample_rate())) as usize;
    let original = trim(&whole, wanted);

    let options = DecomposeOptions {
        name: path.file_stem().map_or_else(
            || "recording".to_string(),
            |stem| stem.to_string_lossy().into_owned(),
        ),
        ..DecomposeOptions::default()
    };
    let decomposition = decompose(&original, &options)?;
    recompose::verify(&decomposition, &original)
}

/// The opening of a recording, or all of it when it is already short enough.
fn trim(audio: &Audio, frames: usize) -> Audio {
    if frames == 0 || frames >= audio.frames() {
        audio.clone()
    } else {
        audio.segment(0, frames)
    }
}
