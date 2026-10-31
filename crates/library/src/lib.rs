//! Song package I/O for the multitrack player.
//!
//! [`load_song`] reads a song folder (`song.json` plus WAV stems), validates it
//! with the `domain` crate, and decodes every stem to `f32` for the engine.
//! [`write_wav_stereo_f32`] writes a rendered mix.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use domain::{
    ManifestError, Song, SongPackage, Stem, StemInfo, ValidationError, ValidationWarning, validate,
};
use engine::StemAudio;
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use thiserror::Error;

/// Name of the manifest inside a song folder.
pub const MANIFEST_FILE: &str = "song.json";

/// A validated song with its decoded stems.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSong {
    /// The validated song.
    pub song: Song,
    /// Findings that do not block playback.
    pub warnings: Vec<ValidationWarning>,
    /// Decoded stems, in package order.
    pub stems: Vec<StemAudio>,
}

/// Why a song folder could not be loaded.
#[derive(Debug, Error)]
pub enum LoadError {
    /// `song.json` could not be read.
    #[error("cannot read {}: {source}", path.display())]
    ReadManifest {
        /// Path of the manifest.
        path: PathBuf,
        /// The I/O error.
        source: io::Error,
    },
    /// `song.json` is not a valid v1 manifest.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// The song folder could not be resolved.
    #[error("cannot open song folder {}: {source}", path.display())]
    SongFolder {
        /// The folder.
        path: PathBuf,
        /// The I/O error.
        source: io::Error,
    },
    /// The package breaks one or more rules; every problem is listed.
    #[error("the song package has {} problem(s)", .0.len())]
    Invalid(Vec<SongProblem>),
    /// A stem passed validation but its audio could not be decoded.
    #[error("stem \"{id}\" file \"{file}\" cannot be decoded: {source}")]
    Decode {
        /// Stem id.
        id: String,
        /// Stem file, as in the manifest.
        file: String,
        /// The decoder error.
        source: hound::Error,
    },
}

/// One problem found while checking a song folder.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum SongProblem {
    /// A package rule from [`domain::validate`].
    #[error(transparent)]
    Validation(ValidationError),
    /// The stem file resolves, through symlinks or otherwise, outside the song
    /// folder.
    #[error("stem \"{id}\" file \"{file}\" resolves outside the song folder")]
    StemOutsideFolder {
        /// Stem id.
        id: String,
        /// Stem file, as in the manifest.
        file: String,
    },
    /// The stem file is missing or is not a readable WAV file.
    #[error("stem \"{id}\" file \"{file}\" cannot be read: {reason}")]
    StemUnreadable {
        /// Stem id.
        id: String,
        /// Stem file, as in the manifest.
        file: String,
        /// What went wrong.
        reason: String,
    },
    /// The stem is a WAV file in a format this version does not decode.
    #[error(
        "stem \"{id}\" file \"{file}\" is {format}; supported stems are 16-, 24- or 32-bit integer or 32-bit float WAV, mono or stereo"
    )]
    UnsupportedStemFormat {
        /// Stem id.
        id: String,
        /// Stem file, as in the manifest.
        file: String,
        /// The format found, such as "8-bit integer, 1 channel".
        format: String,
    },
}

/// Why a WAV file could not be written.
#[derive(Debug, Error)]
pub enum WriteError {
    /// The sample count is odd, so it is not interleaved stereo.
    #[error("{samples} samples are not interleaved stereo; the count must be even")]
    NotStereo {
        /// Number of samples given.
        samples: usize,
    },
    /// The file could not be created or written.
    #[error("cannot write {}: {source}", path.display())]
    Wav {
        /// Output path.
        path: PathBuf,
        /// The encoder error.
        source: hound::Error,
    },
}

/// Reads, validates, and decodes the song folder `dir`.
///
/// Every stem path is resolved inside the song folder; one that leaves it,
/// symlinks included, is rejected before the file is opened. When the folder
/// has problems, every one is returned together in [`LoadError::Invalid`]:
/// stem file problems first, in package order, then the package rules
/// [`domain::validate`] reports. A stem with a file problem is not reported a
/// second time as missing its audio information, and a stem whose path breaks
/// the package rules reports only that rule.
pub fn load_song(dir: &Path) -> Result<LoadedSong, LoadError> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let json = fs::read_to_string(&manifest_path).map_err(|source| LoadError::ReadManifest {
        path: manifest_path,
        source,
    })?;
    let package = SongPackage::from_json(&json)?;
    let folder = fs::canonicalize(dir).map_err(|source| LoadError::SongFolder {
        path: dir.to_owned(),
        source,
    })?;

    let mut file_problems = Vec::new();
    let mut infos = Vec::with_capacity(package.stems.len());
    let mut readers = Vec::with_capacity(package.stems.len());
    for stem in &package.stems {
        match open_stem(&folder, stem) {
            Ok(reader) => {
                infos.push(StemInfo {
                    id: stem.id.clone(),
                    sample_rate: reader.spec().sample_rate,
                    length_samples: u64::from(reader.duration()),
                });
                readers.push(Some(reader));
            }
            Err(problem) => {
                file_problems.push(problem);
                readers.push(None);
            }
        }
    }

    match validate(&package, &infos) {
        Ok(validated) if file_problems.is_empty() => {
            let stems = package
                .stems
                .iter()
                .zip(readers)
                .filter_map(|(stem, reader)| reader.map(|reader| decode(stem, reader)))
                .collect::<Result<_, _>>()?;
            Ok(LoadedSong {
                song: validated.song,
                warnings: validated.warnings,
                stems,
            })
        }
        Ok(_) => Err(LoadError::Invalid(file_problems)),
        Err(errors) => Err(LoadError::Invalid(merge_problems(file_problems, errors))),
    }
}

type StemReader = WavReader<BufReader<File>>;

/// Opens the stem's WAV file after checking that it resolves inside `folder`
/// (already canonical) and that its format is supported.
fn open_stem(folder: &Path, stem: &Stem) -> Result<StemReader, SongProblem> {
    let unreadable = |reason: String| SongProblem::StemUnreadable {
        id: stem.id.clone(),
        file: stem.file.clone(),
        reason,
    };
    let resolved =
        fs::canonicalize(folder.join(&stem.file)).map_err(|error| unreadable(error.to_string()))?;
    if !resolved.starts_with(folder) {
        return Err(SongProblem::StemOutsideFolder {
            id: stem.id.clone(),
            file: stem.file.clone(),
        });
    }
    let reader = WavReader::open(&resolved).map_err(|error| unreadable(error.to_string()))?;
    let spec = reader.spec();
    let supported = matches!(
        (spec.sample_format, spec.bits_per_sample),
        (SampleFormat::Int, 16 | 24 | 32) | (SampleFormat::Float, 32)
    ) && matches!(spec.channels, 1 | 2);
    if !supported {
        return Err(SongProblem::UnsupportedStemFormat {
            id: stem.id.clone(),
            file: stem.file.clone(),
            format: describe(spec),
        });
    }
    Ok(reader)
}

/// A WAV format in words, such as "8-bit integer, 1 channel".
fn describe(spec: WavSpec) -> String {
    let format = match spec.sample_format {
        SampleFormat::Int => "integer",
        SampleFormat::Float => "float",
    };
    let channels = match spec.channels {
        1 => "1 channel".to_owned(),
        n => format!("{n} channels"),
    };
    format!("{}-bit {format}, {channels}", spec.bits_per_sample)
}

/// Combines file problems with validation errors without saying the same
/// thing twice about one stem.
fn merge_problems(
    file_problems: Vec<SongProblem>,
    errors: Vec<ValidationError>,
) -> Vec<SongProblem> {
    let invalid_paths: HashSet<String> = errors
        .iter()
        .filter_map(|error| match error {
            ValidationError::InvalidStemPath { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let file_problems: Vec<SongProblem> = file_problems
        .into_iter()
        .filter(|problem| !problem_stem(problem).is_some_and(|id| invalid_paths.contains(id)))
        .collect();
    let explained: HashSet<String> = file_problems
        .iter()
        .filter_map(problem_stem)
        .chain(invalid_paths.iter().map(String::as_str))
        .map(str::to_owned)
        .collect();
    let rules = errors.into_iter().filter(|error| {
        !matches!(error, ValidationError::MissingStemInfo { id, .. } if explained.contains(id))
    });
    file_problems
        .into_iter()
        .chain(rules.map(SongProblem::Validation))
        .collect()
}

/// Id of the stem a file problem is about.
fn problem_stem(problem: &SongProblem) -> Option<&str> {
    match problem {
        SongProblem::StemOutsideFolder { id, .. }
        | SongProblem::StemUnreadable { id, .. }
        | SongProblem::UnsupportedStemFormat { id, .. } => Some(id),
        SongProblem::Validation(_) => None,
    }
}

/// Decodes a supported WAV stem to interleaved `f32`: integer samples are
/// divided by `2^(bits - 1)`, float samples are kept as they are.
fn decode(stem: &Stem, mut reader: StemReader) -> Result<StemAudio, LoadError> {
    let spec = reader.spec();
    let frames = u64::from(reader.duration());
    let decode_error = |source| LoadError::Decode {
        id: stem.id.clone(),
        file: stem.file.clone(),
        source,
    };
    let mut samples = Vec::with_capacity(reader.len() as usize);
    match spec.sample_format {
        SampleFormat::Int => {
            // 2^-(bits - 1) is exact in f32, so the division adds no rounding.
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            for sample in reader.samples::<i32>() {
                samples.push(sample.map_err(decode_error)? as f32 * scale);
            }
        }
        SampleFormat::Float => {
            for sample in reader.samples::<f32>() {
                samples.push(sample.map_err(decode_error)?);
            }
        }
    }
    Ok(StemAudio {
        id: stem.id.clone(),
        role: stem.role,
        channels: spec.channels,
        samples,
        frames,
    })
}

/// Writes interleaved stereo `samples` as a 32-bit float WAV file.
pub fn write_wav_stereo_f32(
    path: &Path,
    sample_rate: u32,
    samples: &[f32],
) -> Result<(), WriteError> {
    if !samples.len().is_multiple_of(2) {
        return Err(WriteError::NotStereo {
            samples: samples.len(),
        });
    }
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let wav_error = |source| WriteError::Wav {
        path: path.to_owned(),
        source,
    };
    let mut writer = WavWriter::create(path, spec).map_err(wav_error)?;
    for &sample in samples {
        writer.write_sample(sample).map_err(wav_error)?;
    }
    writer.finalize().map_err(wav_error)
}
