//! The player's song description and the library's song listing show a
//! section with the same words.

use std::fs;

use app::{Session, SessionOptions};
use domain::{
    Arrangement, SectionMarker, SectionType, SongPackage, Stem, StemRole, TempoPoint,
    TimeSignaturePoint,
};
use hound::{SampleFormat, WavSpec, WavWriter};
use library::Library;

const RATE: u32 = 8_000;
const TYPES: [SectionType; 15] = [
    SectionType::CountOff,
    SectionType::Intro,
    SectionType::Verse,
    SectionType::PreChorus,
    SectionType::Chorus,
    SectionType::PostChorus,
    SectionType::Bridge,
    SectionType::Instrumental,
    SectionType::Interlude,
    SectionType::Tag,
    SectionType::Vamp,
    SectionType::Turnaround,
    SectionType::Breakdown,
    SectionType::Outro,
    SectionType::Ending,
];

#[test]
fn the_player_and_the_listing_label_every_section_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Every.song");
    fs::create_dir_all(&folder).unwrap();
    // Every type twice: without a number, then with one.
    let sections: Vec<SectionMarker> = TYPES
        .iter()
        .flat_map(|&section_type| [(section_type, None), (section_type, Some(3))])
        .enumerate()
        .map(|(index, (section_type, number))| SectionMarker {
            id: format!("s{index}"),
            section_type,
            number,
            start: index as u64 * u64::from(RATE),
        })
        .collect();
    let package = SongPackage {
        format_version: 1,
        title: "Every section".to_owned(),
        artist: None,
        sample_rate: RATE,
        key: None,
        tempo_map: vec![TempoPoint { at: 0, bpm: 120.0 }],
        time_signature_map: vec![TimeSignaturePoint {
            at: 0,
            numerator: 4,
            denominator: 4,
        }],
        stems: vec![Stem {
            id: "keys".to_owned(),
            file: "keys.wav".to_owned(),
            part: "keys".to_owned(),
            role: StemRole::Track,
        }],
        arrangements: vec![Arrangement {
            id: "default".to_owned(),
            name: "Default".to_owned(),
            sections: sections.iter().map(|s| s.id.clone()).collect(),
            infinite_loop: Vec::new(),
        }],
        sections,
    };
    fs::write(folder.join("song.json"), package.to_json().unwrap()).unwrap();
    let spec = WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(folder.join("keys.wav"), spec).unwrap();
    for _ in 0..30 * RATE {
        writer.write_sample(0i16).unwrap();
    }
    writer.finalize().unwrap();

    let player: Vec<String> = Session::prepare(&folder, SessionOptions::default())
        .unwrap()
        .song()
        .positions
        .iter()
        .map(|p| p.label.clone())
        .collect();
    let listing = Library::new(dir.path()).list_songs().unwrap();
    let listed: Vec<String> = listing.songs[0].arrangements[0]
        .positions
        .iter()
        .map(|p| p.label.clone())
        .collect();

    assert_eq!(player, listed);
    let expected: Vec<String> = TYPES
        .iter()
        .flat_map(|t| [t.label(None), t.label(Some(3))])
        .collect();
    assert_eq!(player, expected);
    assert_eq!(player[6], "Pre-chorus");
    assert_eq!(player[7], "Pre-chorus 3");
}
