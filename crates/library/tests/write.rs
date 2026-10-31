//! S5/S6: the rendered WAV file stays within the format's 4 GiB limit and
//! replaces an existing output only when it is complete.

use std::fs;
use std::path::Path;

use library::{
    MAX_WAV_STEREO_F32_FRAMES, WriteError, check_wav_stereo_f32_frames, write_wav_stereo_f32,
};

/// File names in `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn the_frame_limit_matches_the_written_header_and_the_32_bit_riff_size() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.wav");
    write_wav_stereo_f32(&file, 8_000, &[0.25; 6]).unwrap();
    let header = fs::metadata(&file).unwrap().len() - 6 * 4;
    assert_eq!(header, 68, "WAVE_FORMAT_EXTENSIBLE header");

    // The RIFF size (file size - 8) must fit in a u32.
    let riff_size = |frames: u64| header - 8 + frames * 8;
    assert!(riff_size(MAX_WAV_STEREO_F32_FRAMES) <= u64::from(u32::MAX));
    assert!(riff_size(MAX_WAV_STEREO_F32_FRAMES + 1) > u64::from(u32::MAX));
}

#[test]
fn renders_over_the_limit_are_rejected_naming_the_longest_duration() {
    assert!(check_wav_stereo_f32_frames(MAX_WAV_STEREO_F32_FRAMES, 8_000).is_ok());

    let error = check_wav_stereo_f32_frames(MAX_WAV_STEREO_F32_FRAMES + 1, 192_000).unwrap_err();

    assert!(
        matches!(
            error,
            WriteError::TooLong {
                frames,
                max_frames: MAX_WAV_STEREO_F32_FRAMES,
                sample_rate: 192_000
            } if frames == MAX_WAV_STEREO_F32_FRAMES + 1
        ),
        "{error}"
    );
    let message = error.to_string();
    for part in ["4 GiB", "46:36", "192000 Hz"] {
        assert!(message.contains(part), "{message:?} does not name {part:?}");
    }
}

#[test]
fn a_successful_write_replaces_an_existing_output_and_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.wav");
    fs::write(&file, "previous render").unwrap();

    write_wav_stereo_f32(&file, 8_000, &[0.5, -0.5]).unwrap();

    let mut reader = hound::WavReader::open(&file).unwrap();
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples, [0.5, -0.5]);
    assert_eq!(entries(dir.path()), ["out.wav"]);
}

#[test]
fn a_failed_write_leaves_an_existing_output_untouched_and_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.wav");
    fs::write(&file, "previous render").unwrap();

    assert!(matches!(
        write_wav_stereo_f32(&file, 8_000, &[0.0; 3]),
        Err(WriteError::NotStereo { samples: 3 })
    ));
    assert_eq!(fs::read(&file).unwrap(), b"previous render");
    assert_eq!(entries(dir.path()), ["out.wav"]);

    // The target is a folder: the rename fails after the samples are written.
    let folder = dir.path().join("folder.wav");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("inside"), "kept").unwrap();
    let error = write_wav_stereo_f32(&folder, 8_000, &[0.5, -0.5]).unwrap_err();
    assert!(matches!(error, WriteError::Wav { .. }), "{error}");
    assert!(error.to_string().contains("folder.wav"), "{error}");
    assert_eq!(entries(dir.path()), ["folder.wav", "out.wav"]);
    assert_eq!(entries(&folder), ["inside"]);
}
