//! The desktop shell (S6): the Perform screen for Mac, Windows and Linux,
//! drawn with GPUI and calling the `app` crate directly.
//!
//! This library holds everything the screen decides without drawing, so it
//! is tested without a window: [`view`] maps a [`app::Snapshot`] to what the
//! screen shows, [`action`] maps a control to the [`app::Command`] it
//! sends, [`hold`] times the hold-to-panic control, [`keys`] maps the
//! keyboard to the same controls, and [`song`] opens a song in place of
//! the current one and closes it with its ramp. The GPUI views in the binary only draw a
//! [`view::PerformView`] and dispatch these actions.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod action;
pub mod hold;
pub mod keys;
pub mod song;
pub mod view;
