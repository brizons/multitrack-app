//! The words a section is shown with ("Verse 2", "Pre-chorus"): one label
//! for the player's song description and the library's song listing.

use domain::SectionType;

const NAMES: [(SectionType, &str); 15] = [
    (SectionType::CountOff, "Count-off"),
    (SectionType::Intro, "Intro"),
    (SectionType::Verse, "Verse"),
    (SectionType::PreChorus, "Pre-chorus"),
    (SectionType::Chorus, "Chorus"),
    (SectionType::PostChorus, "Post-chorus"),
    (SectionType::Bridge, "Bridge"),
    (SectionType::Instrumental, "Instrumental"),
    (SectionType::Interlude, "Interlude"),
    (SectionType::Tag, "Tag"),
    (SectionType::Vamp, "Vamp"),
    (SectionType::Turnaround, "Turnaround"),
    (SectionType::Breakdown, "Breakdown"),
    (SectionType::Outro, "Outro"),
    (SectionType::Ending, "Ending"),
];

#[test]
fn every_section_type_has_its_name_and_a_number_follows_it() {
    for (section_type, name) in NAMES {
        assert_eq!(section_type.name(), name);
        assert_eq!(section_type.label(None), name);
        assert_eq!(section_type.label(Some(2)), format!("{name} 2"));
    }
    assert_eq!(SectionType::Verse.label(Some(10)), "Verse 10");
}
