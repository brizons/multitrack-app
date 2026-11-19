//! S4 without hardware: which devices a session can choose, how the default
//! output is found, and how a device is found again after it was lost.

use app::{DeviceChoice, OutputError};
use io_cpal::{Candidate, Probe, UNNAMED, candidates, select};

fn probe(id: Option<&str>, name: Option<&str>) -> Probe {
    Probe {
        id: id.map(str::to_owned),
        name: name.map(str::to_owned),
    }
}

fn listed(probes: &[(Option<&str>, Option<&str>)]) -> Vec<(Probe, usize)> {
    probes
        .iter()
        .enumerate()
        .map(|(index, &(id, name))| (probe(id, name), index))
        .collect()
}

fn summary(found: &[(Candidate, usize)]) -> Vec<(&str, &str, bool, usize)> {
    found
        .iter()
        .map(|(c, device)| (c.id.as_str(), c.name.as_str(), c.is_default, *device))
        .collect()
}

#[test]
fn two_identical_interfaces_are_told_apart_by_id_and_only_the_default_one_is_marked() {
    let found = candidates(
        listed(&[
            (Some("coreaudio:usb-A"), Some("Interface")),
            (Some("coreaudio:usb-B"), Some("Interface")),
        ]),
        Some((probe(Some("coreaudio:usb-B"), Some("Interface")), 99)),
    );
    assert_eq!(
        summary(&found),
        vec![
            ("coreaudio:usb-A", "Interface", false, 0),
            ("coreaudio:usb-B", "Interface", true, 1),
        ]
    );
    assert_eq!(select(&found, None), Ok(1), "the default is the second one");
    let pinned = DeviceChoice::Id("coreaudio:usb-B".to_owned());
    assert_eq!(select(&found, Some(&pinned)), Ok(1));

    // The pinned interface is unplugged: its twin is not taken instead.
    let without = candidates(
        listed(&[(Some("coreaudio:usb-A"), Some("Interface"))]),
        None,
    );
    assert!(matches!(
        select(&without, Some(&pinned)),
        Err(OutputError::DeviceNotFound { .. })
    ));
    assert!(matches!(
        select(&found, Some(&DeviceChoice::Named("Interface".to_owned()))),
        Err(OutputError::AmbiguousDevice { .. })
    ));
}

#[test]
fn a_default_output_that_is_not_listed_is_offered_and_found_again_by_its_id() {
    let found = candidates(
        listed(&[(Some("alsa:hw:CARD=PCH,DEV=0"), Some("HDA Intel PCH"))]),
        Some((probe(Some("alsa:default"), Some("Default Audio Device")), 7)),
    );
    assert_eq!(
        summary(&found),
        vec![
            ("alsa:hw:CARD=PCH,DEV=0", "HDA Intel PCH", false, 0),
            ("alsa:default", "Default Audio Device", true, 7),
        ]
    );
    let opened = select(&found, None).unwrap();
    assert_eq!(found[opened].1, 7, "the default device itself opens");

    // Later, the session asks for it again by the id it pinned.
    let pinned = DeviceChoice::Id(found[opened].0.id.clone());
    let again = candidates(
        listed(&[(Some("alsa:hw:CARD=PCH,DEV=0"), Some("HDA Intel PCH"))]),
        Some((probe(Some("alsa:default"), Some("Default Audio Device")), 8)),
    );
    let index = select(&again, Some(&pinned)).unwrap();
    assert_eq!(again[index].1, 8);
}

#[test]
fn a_device_whose_name_cannot_be_read_is_labelled_and_one_without_an_id_is_left_out() {
    let found = candidates(
        listed(&[
            (Some("coreaudio:speakers"), Some("Speakers")),
            (None, Some("Vanishing")),
            (Some("coreaudio:going"), None),
            (Some("coreaudio:speakers"), Some("Speakers again")),
        ]),
        Some((probe(None, Some("Speakers")), 9)),
    );
    assert_eq!(
        summary(&found),
        vec![
            ("coreaudio:speakers", "Speakers", false, 0),
            ("coreaudio:going", UNNAMED, false, 2),
        ]
    );
    assert_eq!(
        select(&found, None),
        Err(OutputError::NoDevice),
        "a default whose id cannot be read cannot be pinned"
    );
    assert_eq!(
        select(&found, Some(&DeviceChoice::Named("speakers".to_owned()))),
        Ok(0),
        "a name still matches ignoring ASCII case"
    );
    assert_eq!(
        select(&found, Some(&DeviceChoice::Named("Speakers".to_owned()))),
        Ok(0),
        "and exactly"
    );
}
