//! The desktop shell (S6, S4): the Perform screen for Mac, Windows and
//! Linux, drawn with GPUI and calling the `app` crate directly, for one
//! song or a setlist.
//!
//! This library holds everything the screen decides without drawing, so it
//! is tested without a window: [`view`] maps a [`app::Snapshot`] to what the
//! screen shows, [`strip`] maps a [`app::SetlistSnapshot`] to the setlist
//! strip and its problems, [`catalog`] names a setlist's items from the
//! library's songs, [`action`] maps a control to the [`app::Command`] or
//! [`app::SetlistCommand`] it sends, [`hold`] times the hold-to-panic
//! control, [`keys`] maps the keyboard to the same controls, [`song`] opens
//! a song in place of the current one and closes it with its ramp, [`show`]
//! does the same for a song or a setlist, and [`ending`] decides when
//! quitting or closing the window asks first. The GPUI views in the binary
//! only draw a [`view::PerformView`] and a [`strip::SetlistView`] and
//! dispatch these actions.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod action;
pub mod catalog;
pub mod ending;
pub mod hold;
pub mod keys;
pub mod show;
pub mod song;
pub mod strip;
pub mod view;
