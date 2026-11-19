//! How a [`DeviceChoice`] picks a device among `(id, name)` pairs.

use app::{DeviceChoice, OutputError};

const DEVICES: [(&str, &str); 4] = [
    ("host:speakers", "Speakers"),
    ("host:usb-1", "Interface"),
    ("host:usb-2", "Interface"),
    ("host:lower", "speakers"),
];

#[test]
fn an_id_picks_its_device_and_nothing_else() {
    let id = |id: &str| DeviceChoice::Id(id.to_owned()).find(&DEVICES);
    assert_eq!(id("host:usb-2"), Ok(2));
    assert!(matches!(
        id("Speakers"),
        Err(OutputError::DeviceNotFound { .. })
    ));
}

#[test]
fn a_name_picks_by_id_then_exact_name_then_name_ignoring_case() {
    let named = |text: &str| DeviceChoice::Named(text.to_owned()).find(&DEVICES);
    assert_eq!(named("host:usb-1"), Ok(1));
    assert_eq!(named("Speakers"), Ok(0), "the exact name wins");
    assert_eq!(named("speakers"), Ok(3));
    assert_eq!(
        named("SPEAKERS"),
        Err(OutputError::AmbiguousDevice {
            name: "SPEAKERS".to_owned(),
            ids: vec!["host:speakers".to_owned(), "host:lower".to_owned()],
        })
    );
    assert_eq!(
        named("Interface"),
        Err(OutputError::AmbiguousDevice {
            name: "Interface".to_owned(),
            ids: vec!["host:usb-1".to_owned(), "host:usb-2".to_owned()],
        })
    );
    assert_eq!(
        named("Headphones"),
        Err(OutputError::DeviceNotFound {
            name: "Headphones".to_owned(),
            available: vec![
                "Speakers".to_owned(),
                "Interface".to_owned(),
                "Interface".to_owned(),
                "speakers".to_owned(),
            ],
        })
    );
}
