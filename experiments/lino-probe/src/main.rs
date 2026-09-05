//! Probes how `links-notation` parses and formats the document shapes the
//! decomposition manifest wants to use. Run with `cargo run` from this
//! directory.

use links_notation::format_config::FormatConfig;
use links_notation::{format_links, parse_lino_to_links, LiNo};

fn show(document: &str) {
    println!("--- input ---\n{document}");
    match parse_lino_to_links(document) {
        Ok(links) => {
            for link in &links {
                println!("parsed: {link:?}");
            }
            let formatted = format_links(&links);
            println!("formatted:\n{formatted}");
            let reparsed = parse_lino_to_links(&formatted).unwrap();
            println!("round trip stable: {}", reparsed == links);
        }
        Err(error) => println!("parse error: {error}"),
    }
    println!();
}

fn main() {
    show("(decomposition: version 1)\n(sample-rate: 44100)");
    show("sample-rate: 44100\nchannels: 2");
    show("(sample s1: (frames 1024) (peak 0.5))");
    show("(placement: s1 0 0 1.0)\n(placement: s1 44100 0 0.5)");
    show("(hash: 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08)");
    show("(note: 60 0.0 0.5 100)");
    show("(text: \"hello world\")");
    show("(negative: -0.25)");
    show("(sample: s1 (frames 1024) (peak 0.5))");
    show("(source: path/to/song.wav)");
    show("(nested: (a: (b: c)))");
    show("(empty:)");
    show("(id: 42)\n\n(id: 43)");
    show("(pair: alpha beta) (pair: gamma delta)");

    let built = LiNo::Link {
        id: Some("sample".to_string()),
        values: vec![
            LiNo::Ref("s1".to_string()),
            LiNo::Link {
                id: None,
                values: vec![
                    LiNo::Ref("frames".to_string()),
                    LiNo::Ref("1024".to_string()),
                ],
            },
        ],
    };
    println!("built: {built}");
    println!(
        "built (less parens): {}",
        built.format_with_config(&FormatConfig {
            less_parentheses: true,
            ..FormatConfig::default()
        })
    );
}
