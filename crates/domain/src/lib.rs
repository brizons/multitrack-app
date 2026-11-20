//! Song model and timeline compiler for the multitrack player.
//!
//! This crate is pure: it knows nothing about files, audio devices, or threads.
//! The library layer reads `song.json` and the stem headers, hands them to
//! [`validate`], and gets a [`Song`]. [`compile()`] turns a song, an arrangement,
//! and [`CompileOptions`] into a [`RenderPlan`] that the engine plays against a
//! single sample counter. A live player that walks an arrangement itself
//! queries bars and clicks in source time through [`BeatGrid`]. A [`Setlist`]
//! orders songs and click-only tiles; [`Setlist::validate`] checks it against
//! what a [`SongLookup`] knows about the library's songs.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod compile;
pub mod grid;
pub mod package;
pub mod setlist;
pub mod song;
mod timeline;

pub use compile::{
    ClickEvent, ClickLevel, CompileError, CompileOptions, CueEvent, CueTiming,
    DEFAULT_SEAM_CROSSFADE, RenderPlan, Segment, compile, seam_fade,
};
pub use grid::{BarBeat, BeatGrid, SourceClick};
pub use package::{
    Arrangement, DEFAULT_ARRANGEMENT_ID, FORMAT_VERSION, IdKind, MAX_BPM, MAX_SONG_HOURS, MIN_BPM,
    ManifestError, MapKind, SectionMarker, SectionType, SongPackage, Stem, StemInfo,
    StemPathProblem, StemRole, TempoPoint, TimeSignaturePoint, Validated, ValidationError,
    ValidationWarning, validate,
};
pub use setlist::{
    ArrangementOutline, CROSSFADE_SECONDS, ClickItem, PositionBound, SETLIST_FORMAT_VERSION,
    SONG_FADE_OUT_SECONDS, Setlist, SetlistEditError, SetlistFileError, SetlistItem,
    SetlistValidationError, SongItem, SongLookup, SongOutline, TimeSignature, Transition,
};
pub use song::{Section, Song};
