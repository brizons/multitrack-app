//! S1: song package model and validation.

mod common;

use std::time::{Duration, Instant};

use common::{RATE, SPEC_EXAMPLE, SPEC_LENGTH, info, spec_infos, spec_package, stem};
use domain::{
    IdKind, MAX_SONG_HOURS, ManifestError, MapKind, SectionType, SongPackage, Stem,
    StemPathProblem, StemRole, TempoPoint, TimeSignaturePoint, ValidationError, ValidationWarning,
    validate,
};

fn errors_of(package: &SongPackage, infos: &[domain::StemInfo]) -> Vec<ValidationError> {
    validate(package, infos).expect_err("package must be rejected")
}

fn assert_message_names(error: &ValidationError, parts: &[&str]) {
    let message = error.to_string();
    for part in parts {
        assert!(
            message.contains(part),
            "message {message:?} does not name {part:?}"
        );
    }
}

#[test]
fn spec_example_round_trips_and_validates() {
    let package = spec_package();

    let json = package.to_json().expect("serializes");
    let reparsed = SongPackage::from_json(&json).expect("reparses");
    assert_eq!(reparsed, package);
    let written: serde_json::Value = serde_json::from_str(&json).unwrap();
    let original: serde_json::Value = serde_json::from_str(SPEC_EXAMPLE).unwrap();
    assert_eq!(written, original, "the written manifest keeps every field");

    let validated = validate(&package, &spec_infos()).expect("spec example is valid");
    assert!(validated.warnings.is_empty());
    let song = validated.song;
    assert_eq!(song.title(), "Example Song");
    assert_eq!(song.artist(), Some("Example Artist"));
    assert_eq!(song.key(), Some("G"));
    assert_eq!(song.sample_rate(), RATE);
    assert_eq!(song.length(), SPEC_LENGTH);
    assert_eq!(song.tempo_map(), &[TempoPoint { at: 0, bpm: 72.0 }]);
    assert_eq!(
        song.time_signature_map(),
        &[TimeSignaturePoint {
            at: 0,
            numerator: 4,
            denominator: 4
        }]
    );
    let roles: Vec<(&str, StemRole)> = song
        .stems()
        .iter()
        .map(|s| (s.id.as_str(), s.role))
        .collect();
    assert_eq!(
        roles,
        [("drums", StemRole::Track), ("click", StemRole::Click)]
    );
    assert_eq!(song.stems()[0].file, "stems/drums.flac");
    assert_eq!(song.stems()[0].part, "drums");

    let sections: Vec<_> = song
        .sections()
        .iter()
        .map(|s| {
            (
                s.id.as_str(),
                s.section_type,
                s.number,
                s.start,
                s.end,
                s.length(),
            )
        })
        .collect();
    assert_eq!(
        sections,
        [
            ("s1", SectionType::Intro, None, 0, 640_000, 640_000),
            (
                "s2",
                SectionType::Verse,
                Some(1),
                640_000,
                SPEC_LENGTH,
                640_000
            ),
        ]
    );
    assert_eq!(song.section("s2").map(|s| s.start), Some(640_000));
    assert!(song.section("s9").is_none());

    let default = song.arrangement("default").expect("default arrangement");
    assert_eq!(default.name, "Default");
    assert_eq!(default.sections, ["s1", "s2"]);
    assert!(default.infinite_loop.is_empty());
    assert_eq!(song.arrangements().len(), 1);
}

#[test]
fn optional_fields_may_be_omitted() {
    let json = SPEC_EXAMPLE
        .replace(r#""artist": "Example Artist","#, "")
        .replace(r#""key": "G","#, "")
        .replace(r#""number": null, "#, "");
    let package = SongPackage::from_json(&json).expect("optional fields are optional");
    assert_eq!(package.artist, None);
    assert_eq!(package.key, None);
    assert_eq!(package.sections[0].number, None);

    let written = package.to_json().unwrap();
    assert!(!written.contains("artist"));
    assert!(!written.contains("\"key\""));
    let song = validate(&package, &spec_infos()).unwrap().song;
    assert_eq!(song.artist(), None);
    assert_eq!(song.key(), None);
}

#[test]
fn section_types_use_kebab_case() {
    let names = [
        "count-off",
        "intro",
        "verse",
        "pre-chorus",
        "chorus",
        "post-chorus",
        "bridge",
        "instrumental",
        "interlude",
        "tag",
        "vamp",
        "turnaround",
        "breakdown",
        "outro",
        "ending",
    ];
    let json = serde_json::to_string(&names).unwrap();
    let types: Vec<SectionType> = serde_json::from_str(&json).expect("every type parses");
    assert_eq!(
        types,
        [
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
        ]
    );
    assert_eq!(serde_json::to_string(&types).unwrap(), json);

    let unknown = SPEC_EXAMPLE.replace(r#""type": "intro""#, r#""type": "Intro""#);
    let error = SongPackage::from_json(&unknown).expect_err("type names are kebab-case");
    assert!(error.to_string().starts_with("song.json: "));
}

#[test]
fn stem_roles_are_track_click_or_guide() {
    let guide = SPEC_EXAMPLE.replace(r#""role": "click""#, r#""role": "guide""#);
    let package = SongPackage::from_json(&guide).unwrap();
    assert_eq!(package.stems[1].role, StemRole::Guide);

    let unknown = SPEC_EXAMPLE.replace(r#""role": "click""#, r#""role": "pad""#);
    assert!(SongPackage::from_json(&unknown).is_err());
}

#[test]
fn rejects_unsupported_format_version() {
    let mut package = spec_package();
    package.format_version = 2;
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [ValidationError::UnsupportedFormatVersion { found: 2 }]
    );
    assert_message_names(&errors[0], &["formatVersion 2", "1"]);
}

#[test]
fn rejects_zero_sample_rate() {
    let mut package = spec_package();
    package.sample_rate = 0;
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [
            ValidationError::InvalidSampleRate,
            ValidationError::StemSampleRateMismatch {
                id: "drums".into(),
                found: RATE,
                expected: 0
            },
            ValidationError::StemSampleRateMismatch {
                id: "click".into(),
                found: RATE,
                expected: 0
            },
        ]
    );
    assert_message_names(&errors[0], &["sampleRate"]);
}

#[test]
fn rejects_missing_default_arrangement() {
    let mut package = spec_package();
    package.arrangements[0].id = "live".into();
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(errors, [ValidationError::MissingDefaultArrangement]);
    assert_message_names(&errors[0], &["Default", "\"default\""]);
}

#[test]
fn rejects_unknown_section_references() {
    let mut package = spec_package();
    package.arrangements[0].sections = vec!["s1".into(), "s9".into(), "s9".into()];
    package.arrangements[0].infinite_loop = vec!["s8".into()];
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [
            ValidationError::UnknownSectionReference {
                arrangement: "default".into(),
                section: "s9".into()
            },
            ValidationError::UnknownSectionReference {
                arrangement: "default".into(),
                section: "s8".into()
            },
        ]
    );
    assert_message_names(&errors[0], &["\"default\"", "\"s9\""]);
}

#[test]
fn rejects_unsorted_section_starts() {
    let mut package = spec_package();
    package
        .sections
        .push(common::marker("s3", SectionType::Chorus, None, 320_000));
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [ValidationError::SectionStartsNotIncreasing {
            id: "s3".into(),
            start: 320_000,
            previous_id: "s2".into(),
            previous_start: 640_000
        }]
    );
    assert_message_names(&errors[0], &["\"s3\"", "320000", "\"s2\"", "640000"]);
}

#[test]
fn rejects_duplicate_section_starts() {
    let mut package = spec_package();
    package.sections[1].start = 0;
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [ValidationError::SectionStartsNotIncreasing {
            id: "s2".into(),
            start: 0,
            previous_id: "s1".into(),
            previous_start: 0
        }]
    );
}

#[test]
fn rejects_first_section_not_at_zero() {
    let mut package = spec_package();
    package.sections[0].start = 100;
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [ValidationError::FirstSectionNotAtZero {
            id: "s1".into(),
            start: 100
        }]
    );
    assert_message_names(&errors[0], &["\"s1\"", "100"]);
}

#[test]
fn rejects_section_start_at_or_beyond_song_end() {
    let mut package = spec_package();
    package.sections[1].start = SPEC_LENGTH;
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [ValidationError::SectionStartBeyondSongEnd {
            id: "s2".into(),
            start: SPEC_LENGTH,
            song_end: SPEC_LENGTH
        }]
    );
    assert_message_names(&errors[0], &["\"s2\"", "1280000"]);

    package.sections[1].start = SPEC_LENGTH + 5;
    assert_eq!(
        errors_of(&package, &spec_infos()),
        [ValidationError::SectionStartBeyondSongEnd {
            id: "s2".into(),
            start: SPEC_LENGTH + 5,
            song_end: SPEC_LENGTH
        }]
    );
}

#[test]
fn rejects_duplicate_ids() {
    let mut stems = spec_package();
    stems.stems.push(stem("drums", StemRole::Track));
    stems.stems.push(stem("drums", StemRole::Track));
    let errors = errors_of(&stems, &spec_infos());
    assert_eq!(
        errors,
        [ValidationError::DuplicateId {
            kind: IdKind::Stem,
            id: "drums".into()
        }]
    );
    assert_message_names(&errors[0], &["stem", "\"drums\""]);

    let mut sections = spec_package();
    sections.sections[1].id = "s1".into();
    sections.arrangements[0].sections = vec!["s1".into()];
    assert_eq!(
        errors_of(&sections, &spec_infos()),
        [ValidationError::DuplicateId {
            kind: IdKind::Section,
            id: "s1".into()
        }]
    );

    let mut arrangements = spec_package();
    arrangements
        .arrangements
        .push(arrangements.arrangements[0].clone());
    let errors = errors_of(&arrangements, &spec_infos());
    assert_eq!(
        errors,
        [ValidationError::DuplicateId {
            kind: IdKind::Arrangement,
            id: "default".into()
        }]
    );
    assert_message_names(&errors[0], &["arrangement", "\"default\""]);
}

#[test]
fn rejects_more_than_one_click_or_guide_stem() {
    let mut package = spec_package();
    package.stems.push(stem("click2", StemRole::Click));
    package.stems.push(stem("guide1", StemRole::Guide));
    package.stems.push(stem("guide2", StemRole::Guide));
    let mut infos = spec_infos();
    for id in ["click2", "guide1", "guide2"] {
        infos.push(info(id, RATE, SPEC_LENGTH));
    }
    let errors = errors_of(&package, &infos);
    assert_eq!(
        errors,
        [
            ValidationError::TooManyStemsWithRole {
                role: StemRole::Click,
                ids: vec!["click".into(), "click2".into()]
            },
            ValidationError::TooManyStemsWithRole {
                role: StemRole::Guide,
                ids: vec!["guide1".into(), "guide2".into()]
            },
        ]
    );
    assert_message_names(&errors[0], &["click", "\"click\"", "\"click2\""]);
    assert_message_names(&errors[1], &["guide", "\"guide1\"", "\"guide2\""]);
}

#[test]
fn rejects_stems_whose_sample_rate_differs_from_the_package() {
    let package = spec_package();
    let infos = [
        info("drums", 44_100, SPEC_LENGTH),
        info("click", 44_100, SPEC_LENGTH),
    ];
    let errors = errors_of(&package, &infos);
    assert_eq!(
        errors,
        [
            ValidationError::StemSampleRateMismatch {
                id: "drums".into(),
                found: 44_100,
                expected: RATE
            },
            ValidationError::StemSampleRateMismatch {
                id: "click".into(),
                found: 44_100,
                expected: RATE
            },
        ]
    );
    assert_message_names(&errors[0], &["\"drums\"", "44100", "48000"]);
}

#[test]
fn rejects_stems_whose_sample_rates_differ_from_each_other() {
    let package = spec_package();
    let infos = [
        info("drums", RATE, SPEC_LENGTH),
        info("click", 44_100, SPEC_LENGTH),
    ];
    assert_eq!(
        errors_of(&package, &infos),
        [ValidationError::StemSampleRateMismatch {
            id: "click".into(),
            found: 44_100,
            expected: RATE
        }]
    );
}

#[test]
fn rejects_stem_without_stem_info() {
    let package = spec_package();
    let errors = errors_of(&package, &[info("drums", RATE, SPEC_LENGTH)]);
    assert_eq!(
        errors,
        [ValidationError::MissingStemInfo {
            id: "click".into(),
            file: "stems/click.flac".into()
        }]
    );
    assert_message_names(&errors[0], &["\"click\"", "stems/click.flac"]);
}

#[test]
fn rejects_package_without_stems() {
    let mut package = spec_package();
    package.stems.clear();
    let errors = errors_of(&package, &[]);
    assert_eq!(errors, [ValidationError::NoStems]);
    assert_message_names(&errors[0], &["no stems"]);
}

#[test]
fn rejects_empty_maps() {
    let mut package = spec_package();
    package.tempo_map.clear();
    package.time_signature_map.clear();
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [
            ValidationError::EmptyMap {
                map: MapKind::Tempo
            },
            ValidationError::EmptyMap {
                map: MapKind::TimeSignature
            },
        ]
    );
    assert_message_names(&errors[0], &["tempoMap"]);
    assert_message_names(&errors[1], &["timeSignatureMap"]);
}

#[test]
fn rejects_maps_not_starting_at_zero() {
    let mut package = spec_package();
    package.tempo_map[0].at = 10;
    package.time_signature_map[0].at = 20;
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [
            ValidationError::MapNotStartingAtZero {
                map: MapKind::Tempo,
                at: 10
            },
            ValidationError::MapNotStartingAtZero {
                map: MapKind::TimeSignature,
                at: 20
            },
        ]
    );
    assert_message_names(&errors[0], &["tempoMap", "10"]);
}

#[test]
fn rejects_unsorted_or_duplicate_map_positions() {
    let mut package = spec_package();
    package.tempo_map = vec![
        TempoPoint { at: 0, bpm: 120.0 },
        TempoPoint {
            at: 2_000,
            bpm: 100.0,
        },
        TempoPoint {
            at: 1_000,
            bpm: 90.0,
        },
    ];
    let meter = |at| TimeSignaturePoint {
        at,
        numerator: 4,
        denominator: 4,
    };
    package.time_signature_map = vec![meter(0), meter(5_000), meter(5_000)];
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [
            ValidationError::MapNotIncreasing {
                map: MapKind::Tempo,
                at: 1_000,
                previous_at: 2_000
            },
            ValidationError::MapNotIncreasing {
                map: MapKind::TimeSignature,
                at: 5_000,
                previous_at: 5_000
            },
        ]
    );
    assert_message_names(&errors[0], &["tempoMap", "1000", "2000"]);
    assert_message_names(&errors[1], &["timeSignatureMap", "5000"]);
}

#[test]
fn rejects_non_positive_non_finite_or_out_of_range_bpm() {
    let mut package = spec_package();
    let bpms = [0.0, -10.0, f64::INFINITY, f64::NAN, 1_000.0, 0.5];
    package.tempo_map = bpms
        .iter()
        .enumerate()
        .map(|(i, &bpm)| TempoPoint {
            at: i as u64 * 1_000,
            bpm,
        })
        .collect();
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(errors.len(), bpms.len());
    for (i, error) in errors.iter().enumerate() {
        let expected_at = i as u64 * 1_000;
        assert!(
            matches!(error, ValidationError::InvalidBpm { at, bpm }
                if *at == expected_at && (bpm.to_bits() == bpms[i].to_bits())),
            "unexpected error {error:?}"
        );
    }
    assert_message_names(&errors[1], &["tempoMap", "-10", "1000"]);

    package.tempo_map = vec![
        TempoPoint { at: 0, bpm: 1.0 },
        TempoPoint {
            at: 1_000,
            bpm: 999.0,
        },
    ];
    assert!(
        validate(&package, &spec_infos()).is_ok(),
        "range bounds are valid"
    );
}

#[test]
fn rejects_zero_numerator() {
    let mut package = spec_package();
    package.time_signature_map[0].numerator = 0;
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(errors, [ValidationError::InvalidNumerator { at: 0 }]);
    assert_message_names(&errors[0], &["numerator 0"]);
}

#[test]
fn rejects_denominator_that_is_not_a_power_of_two_up_to_32() {
    let mut package = spec_package();
    let denominators = [3, 64, 0, 12];
    package.time_signature_map = denominators
        .iter()
        .enumerate()
        .map(|(i, &denominator)| TimeSignaturePoint {
            at: i as u64 * 1_000,
            numerator: 4,
            denominator,
        })
        .collect();
    let errors = errors_of(&package, &spec_infos());
    let expected: Vec<_> = denominators
        .iter()
        .enumerate()
        .map(|(i, &denominator)| ValidationError::InvalidDenominator {
            at: i as u64 * 1_000,
            denominator,
        })
        .collect();
    assert_eq!(errors, expected);
    assert_message_names(&errors[0], &["denominator 3"]);

    package.time_signature_map = [1, 2, 4, 8, 16, 32]
        .iter()
        .enumerate()
        .map(|(i, &denominator)| TimeSignaturePoint {
            at: i as u64 * 1_000,
            numerator: 7,
            denominator,
        })
        .collect();
    assert!(validate(&package, &spec_infos()).is_ok());
}

#[test]
fn reports_every_error_at_once() {
    let mut package = spec_package();
    package.format_version = 3;
    package.arrangements[0].id = "live".into();
    package.arrangements[0].sections.push("s7".into());
    package.time_signature_map[0].numerator = 0;
    package.sections[0].start = 5;
    let errors = errors_of(&package, &[info("drums", RATE, SPEC_LENGTH)]);
    assert_eq!(
        errors,
        [
            ValidationError::UnsupportedFormatVersion { found: 3 },
            ValidationError::MissingStemInfo {
                id: "click".into(),
                file: "stems/click.flac".into()
            },
            ValidationError::InvalidNumerator { at: 0 },
            ValidationError::FirstSectionNotAtZero {
                id: "s1".into(),
                start: 5
            },
            ValidationError::MissingDefaultArrangement,
            ValidationError::UnknownSectionReference {
                arrangement: "live".into(),
                section: "s7".into()
            },
        ]
    );
}

#[test]
fn warns_when_stem_lengths_differ_by_more_than_one_second() {
    let package = spec_package();
    let short = SPEC_LENGTH - u64::from(RATE) - 1;
    let validated = validate(
        &package,
        &[info("drums", RATE, SPEC_LENGTH), info("click", RATE, short)],
    )
    .expect("a length difference is only a warning");
    assert_eq!(
        validated.warnings,
        [ValidationWarning::StemLengthsDiffer {
            shortest: "click".into(),
            shortest_length: short,
            longest: "drums".into(),
            longest_length: SPEC_LENGTH
        }]
    );
    assert_eq!(
        validated.song.length(),
        SPEC_LENGTH,
        "song end is the longest stem"
    );
    let message = validated.warnings[0].to_string();
    assert!(message.contains("\"click\"") && message.contains("\"drums\""));

    let exactly_one_second = SPEC_LENGTH - u64::from(RATE);
    let validated = validate(
        &package,
        &[
            info("drums", RATE, exactly_one_second),
            info("click", RATE, SPEC_LENGTH),
        ],
    )
    .unwrap();
    assert!(validated.warnings.is_empty());
    assert_eq!(validated.song.length(), SPEC_LENGTH);
}

#[test]
fn checks_format_version_before_the_rest_of_the_shape() {
    let future = r#"{ "formatVersion": 2, "name": "a future shape", "tracks": [] }"#;
    let error = SongPackage::from_json(future).expect_err("future version");
    assert!(
        matches!(error, ManifestError::UnsupportedFormatVersion { found: 2 }),
        "{error:?}"
    );
    assert!(error.to_string().contains("unsupported formatVersion 2"));

    let broken = r#"{ "formatVersion": 1, "title": "x" }"#;
    let error = SongPackage::from_json(broken).expect_err("v1 with missing fields");
    assert!(matches!(error, ManifestError::Json(_)), "{error:?}");
    assert!(error.to_string().contains("missing field"));

    let unversioned = r#"{ "title": "x" }"#;
    let error = SongPackage::from_json(unversioned).expect_err("no version");
    assert!(matches!(error, ManifestError::Json(_)), "{error:?}");
    assert!(error.to_string().contains("formatVersion"));
}

#[test]
fn rejects_package_without_sections_and_empty_arrangements() {
    let mut package = spec_package();
    package.sections.clear();
    package.arrangements[0].sections.clear();
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [
            ValidationError::NoSections,
            ValidationError::EmptyArrangement {
                arrangement: "default".into()
            },
        ]
    );
    assert_message_names(&errors[0], &["no sections"]);
    assert_message_names(&errors[1], &["\"default\"", "no sections"]);

    let mut package = spec_package();
    package
        .arrangements
        .push(common::arrangement("acoustic", &[]));
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [ValidationError::EmptyArrangement {
            arrangement: "acoustic".into()
        }]
    );
    assert_message_names(&errors[0], &["\"acoustic\""]);
}

#[test]
fn rejects_songs_longer_than_24_hours() {
    let package = spec_package();
    let max = MAX_SONG_HOURS * 3_600 * u64::from(RATE);

    let started = Instant::now();
    let errors = errors_of(
        &package,
        &[
            info("drums", RATE, u64::MAX),
            info("click", RATE, SPEC_LENGTH),
        ],
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "rejected quickly"
    );
    assert_eq!(
        errors,
        [ValidationError::SongTooLong {
            stem: "drums".into(),
            length: u64::MAX,
            max
        }]
    );
    assert_message_names(&errors[0], &["\"drums\"", "24 hours"]);

    let exactly = validate(
        &package,
        &[info("drums", RATE, max), info("click", RATE, max)],
    );
    assert_eq!(exactly.expect("24 hours is accepted").song.length(), max);
    assert_eq!(
        errors_of(
            &package,
            &[info("drums", RATE, max), info("click", RATE, max + 1)]
        ),
        [ValidationError::SongTooLong {
            stem: "click".into(),
            length: max + 1,
            max
        }]
    );
}

#[test]
fn rejects_arrangements_longer_than_24_hours() {
    let max = MAX_SONG_HOURS * 3_600 * u64::from(RATE);
    // A 23-hour song: s2 is 23 hours minus 640 000 samples long.
    let song_length = 23 * 3_600 * u64::from(RATE);
    let infos = [
        info("drums", RATE, song_length),
        info("click", RATE, song_length),
    ];
    let mut package = spec_package();
    package.arrangements.push(domain::Arrangement {
        id: "endless".into(),
        name: "Endless".into(),
        sections: vec!["s2".to_owned(); 1_000_000],
        infinite_loop: Vec::new(),
    });

    let started = Instant::now();
    let errors = errors_of(&package, &infos);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "rejected quickly"
    );
    assert_eq!(
        errors,
        [ValidationError::ArrangementTooLong {
            arrangement: "endless".into(),
            max
        }]
    );
    assert_message_names(&errors[0], &["\"endless\"", "24 hours"]);

    // Exactly 24 hours is accepted: s2 (640 000 samples) repeated to the cap.
    let mut package = spec_package();
    let repeats = usize::try_from(max / 640_000).unwrap();
    assert_eq!(max % 640_000, 0);
    package.arrangements.push(domain::Arrangement {
        id: "full-day".into(),
        name: "Full day".into(),
        sections: vec!["s2".to_owned(); repeats],
        infinite_loop: Vec::new(),
    });
    assert!(validate(&package, &spec_infos()).is_ok(), "24 hours fits");
    package.arrangements[1].sections.push("s2".to_owned());
    assert_eq!(
        errors_of(&package, &spec_infos()),
        [ValidationError::ArrangementTooLong {
            arrangement: "full-day".into(),
            max
        }]
    );
}

#[test]
fn rejects_stem_files_outside_the_song_folder() {
    let cases = [
        ("empty", "", StemPathProblem::Empty),
        ("root", "/etc/passwd", StemPathProblem::Absolute),
        ("unc", r"\\server\share\a.wav", StemPathProblem::Absolute),
        ("drive", r"C:\stems\a.wav", StemPathProblem::Absolute),
        ("drive_relative", "c:a.wav", StemPathProblem::Absolute),
        ("parent", "../a.wav", StemPathProblem::ParentDirectory),
        (
            "nested",
            "stems/../../a.wav",
            StemPathProblem::ParentDirectory,
        ),
        (
            "windows",
            r"stems\..\..\a.wav",
            StemPathProblem::ParentDirectory,
        ),
        ("trailing", "stems/..", StemPathProblem::ParentDirectory),
    ];
    let mut package = spec_package();
    package.stems = cases
        .iter()
        .map(|&(id, file, _)| Stem {
            id: id.into(),
            file: file.into(),
            part: id.into(),
            role: StemRole::Track,
        })
        .collect();
    let infos: Vec<_> = cases
        .iter()
        .map(|&(id, ..)| info(id, RATE, SPEC_LENGTH))
        .collect();
    let errors = errors_of(&package, &infos);
    let expected: Vec<_> = cases
        .iter()
        .map(|&(id, file, problem)| ValidationError::InvalidStemPath {
            id: id.into(),
            file: file.into(),
            problem,
        })
        .collect();
    assert_eq!(errors, expected);
    assert_message_names(&errors[1], &["\"root\"", "/etc/passwd", "absolute"]);
    assert_message_names(&errors[5], &["\"parent\"", "..", "song folder"]);
    assert_message_names(&errors[0], &["\"empty\"", "empty"]);

    for file in [
        "stems/a.wav",
        "./stems/a.wav",
        r"stems\a.wav",
        "stems/..a.wav",
        "a..b.wav",
    ] {
        let mut package = spec_package();
        package.stems[0].file = file.into();
        assert!(
            validate(&package, &spec_infos()).is_ok(),
            "{file} is inside the folder"
        );
    }
}

#[test]
fn hostile_values_are_rejected_without_panicking() {
    for bpm in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300] {
        let mut package = spec_package();
        package.tempo_map[0].bpm = bpm;
        let errors = errors_of(&package, &spec_infos());
        assert!(
            matches!(errors[..], [ValidationError::InvalidBpm { at: 0, .. }]),
            "bpm {bpm}: {errors:?}"
        );
    }

    let mut package = spec_package();
    package.sections[1].start = u64::MAX;
    assert_eq!(
        errors_of(&package, &spec_infos()),
        [ValidationError::SectionStartBeyondSongEnd {
            id: "s2".into(),
            start: u64::MAX,
            song_end: SPEC_LENGTH
        }]
    );

    let zero_length = [info("drums", RATE, 0), info("click", RATE, 0)];
    assert_eq!(
        errors_of(&spec_package(), &zero_length),
        [
            ValidationError::SectionStartBeyondSongEnd {
                id: "s1".into(),
                start: 0,
                song_end: 0
            },
            ValidationError::SectionStartBeyondSongEnd {
                id: "s2".into(),
                start: 640_000,
                song_end: 0
            },
        ]
    );

    for json in [
        SPEC_EXAMPLE.replace("640000", "18446744073709551616"),
        SPEC_EXAMPLE.replace("72.0", "1e999"),
        String::new(),
        "[]".to_owned(),
        r#"{ "formatVersion": -1 }"#.to_owned(),
    ] {
        assert!(
            matches!(SongPackage::from_json(&json), Err(ManifestError::Json(_))),
            "{json:?} is a parse error"
        );
    }
}

/// The stem-path, bpm, and time-signature rules are shared with setlists; the
/// package still reports them exactly as before.
#[test]
fn rules_shared_with_setlists_keep_their_package_errors_and_messages() {
    let mut package = spec_package();
    package.stems[0].file = "../drums.flac".into();
    package.tempo_map[0].bpm = 1_000.0;
    package.time_signature_map[0].numerator = 0;
    package.time_signature_map[0].denominator = 3;
    let errors = errors_of(&package, &spec_infos());
    assert_eq!(
        errors,
        [
            ValidationError::InvalidStemPath {
                id: "drums".into(),
                file: "../drums.flac".into(),
                problem: StemPathProblem::ParentDirectory
            },
            ValidationError::InvalidBpm {
                at: 0,
                bpm: 1_000.0
            },
            ValidationError::InvalidNumerator { at: 0 },
            ValidationError::InvalidDenominator {
                at: 0,
                denominator: 3
            },
        ]
    );
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        messages,
        [
            "stem \"drums\" file \"../drums.flac\" has a \"..\" component; it must be a relative path inside the song folder",
            "tempoMap entry at 0 has bpm 1000; bpm must be from 1 to 999",
            "timeSignatureMap entry at 0 has numerator 0; it must be at least 1",
            "timeSignatureMap entry at 0 has denominator 3; it must be 1, 2, 4, 8, 16, or 32",
        ]
    );
}
