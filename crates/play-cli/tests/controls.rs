//! S5: the single-key controls, the one-line status, and the device list.

use std::time::Duration;

use app::{
    Command, DeviceInfo, DeviceStatus, FadeState, FadeView, JumpKind, JumpTiming, PositionInfo,
    QueuedView, RuntimeError, Snapshot, SongInfo, StreamInfo, TransportState,
};
use play_cli::{Action, action_for_key, device_list, status_line};

fn song() -> SongInfo {
    let position = |id: &str, label: &str, start_bar| PositionInfo {
        section_id: id.to_owned(),
        label: label.to_owned(),
        start_frame: 0,
        length_frames: 96_000,
        start_bar,
    };
    SongInfo {
        title: "Example".to_owned(),
        artist: None,
        sample_rate: 48_000,
        arrangement: "default".to_owned(),
        positions: vec![
            position("intro", "Intro", 1),
            position("verse", "Verse 1", 3),
            position("chorus", "Chorus", 5),
        ],
        warnings: Vec::new(),
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        transport: TransportState::Playing,
        output_frame: 120_000,
        source_frame: 120_000,
        bar: 3,
        beat: 2,
        position: 1,
        section: "Verse 1".to_owned(),
        queued: None,
        jump_timing: JumpTiming::SectionEnd,
        infinite: false,
        fade: FadeView {
            state: FadeState::In,
            level: 1.0,
            duration: Duration::from_secs(3),
        },
        device: DeviceStatus::Connected(StreamInfo {
            device: "Speakers".to_owned(),
            sample_rate: 48_000,
            buffer_frames: 256,
            channels: 2,
        }),
        underruns: 0,
        decode_errors: 0,
        last_error: None,
    }
}

fn with(transport: TransportState) -> Snapshot {
    Snapshot {
        transport,
        ..snapshot()
    }
}

#[test]
fn space_plays_or_pauses_depending_on_the_transport() {
    let play = Some(Action::Send(Command::Play));
    let pause = Some(Action::Send(Command::Pause));
    for (transport, expected) in [
        (TransportState::Stopped, &play),
        (TransportState::Paused, &play),
        (TransportState::Pausing, &play),
        (TransportState::Stopping, &play),
        (TransportState::Panicking, &play),
        (TransportState::Playing, &pause),
        (TransportState::Starting, &pause),
    ] {
        assert_eq!(
            &action_for_key(' ', &with(transport)),
            expected,
            "{transport:?}"
        );
    }
}

#[test]
fn every_control_key_maps_to_its_command() {
    let now = snapshot();
    let send = |command| Some(Action::Send(command));
    assert_eq!(action_for_key('s', &now), send(Command::Stop));
    assert_eq!(action_for_key('b', &now), send(Command::BackToStart));
    assert_eq!(
        action_for_key('1', &now),
        send(Command::QueueJump { position: 0 })
    );
    assert_eq!(
        action_for_key('9', &now),
        send(Command::QueueJump { position: 8 })
    );
    assert_eq!(
        action_for_key('n', &now),
        send(Command::SetJumpTiming(JumpTiming::NextBar))
    );
    let next_bar = Snapshot {
        jump_timing: JumpTiming::NextBar,
        ..snapshot()
    };
    assert_eq!(
        action_for_key('n', &next_bar),
        send(Command::SetJumpTiming(JumpTiming::SectionEnd))
    );
    assert_eq!(action_for_key('l', &now), send(Command::LiveLoop));
    assert_eq!(action_for_key('i', &now), send(Command::SetInfinite(true)));
    let infinite = Snapshot {
        infinite: true,
        ..snapshot()
    };
    assert_eq!(
        action_for_key('i', &infinite),
        send(Command::SetInfinite(false))
    );
    assert_eq!(action_for_key('f', &now), send(Command::ToggleFade));
    assert_eq!(action_for_key('c', &now), send(Command::CancelJump));
    assert_eq!(action_for_key('x', &now), send(Command::Panic));
    assert_eq!(action_for_key('q', &now), Some(Action::Quit));
    assert_eq!(action_for_key('S', &now), send(Command::Stop), "any case");
    for key in ['0', 'a', 'z', '\n'] {
        assert_eq!(action_for_key(key, &now), None, "{key:?}");
    }
}

#[test]
fn the_status_line_shows_transport_playhead_section_timing_fade_and_device() {
    assert_eq!(
        status_line(&snapshot(), &song()),
        "PLAYING  | bar 3.2 | Verse 1 (2/3) | next: - | timing: section end | fade: in | 48000 Hz, 256 frames, 5.3 ms on \"Speakers\""
    );
}

#[test]
fn the_status_line_shows_the_queue_loops_fade_progress_and_problems() {
    let busy = Snapshot {
        transport: TransportState::Starting,
        queued: Some(QueuedView {
            target: 2,
            target_label: "Chorus".to_owned(),
            kind: JumpKind::Jump,
            executes_at: 192_000,
            frames_until: 72_000,
            deferrals: 2,
        }),
        jump_timing: JumpTiming::NextBar,
        infinite: true,
        fade: FadeView {
            state: FadeState::FadingOut,
            level: 0.25,
            duration: Duration::from_secs(3),
        },
        underruns: 4,
        last_error: Some(RuntimeError::DecodeFailed { blocks: 3 }),
        ..snapshot()
    };
    assert_eq!(
        status_line(&busy, &song()),
        "LOADING  | bar 3.2 | Verse 1 (2/3) | next: jump to Chorus in 1.5 s, waited 2 | timing: next bar | infinite | fade: out 25% | 48000 Hz, 256 frames, 5.3 ms on \"Speakers\" | underruns: 4 | ! the disk could not decode 3 block(s) of audio; they play as silence"
    );

    let looping = Snapshot {
        transport: TransportState::Paused,
        queued: Some(QueuedView {
            target: 1,
            target_label: "Verse 1".to_owned(),
            kind: JumpKind::Loop,
            executes_at: 192_000,
            frames_until: 72_000,
            deferrals: 0,
        }),
        fade: FadeView {
            state: FadeState::Out,
            level: 0.0,
            duration: Duration::from_secs(3),
        },
        device: DeviceStatus::Disconnected {
            device: "Interface".to_owned(),
            reason: "the device was disconnected".to_owned(),
        },
        ..snapshot()
    };
    assert_eq!(
        status_line(&looping, &song()),
        "PAUSED   | bar 3.2 | Verse 1 (2/3) | next: loop Verse 1 in 1.5 s | timing: section end | fade: out | NO DEVICE: \"Interface\" (the device was disconnected)"
    );
}

#[test]
fn the_device_list_shows_each_device_with_its_output_channels_and_the_default() {
    let devices = [
        DeviceInfo {
            name: "Speakers".to_owned(),
            output_channels: 2,
            is_default: true,
        },
        DeviceInfo {
            name: "Interface".to_owned(),
            output_channels: 18,
            is_default: false,
        },
        DeviceInfo {
            name: "Meeting".to_owned(),
            output_channels: 1,
            is_default: false,
        },
    ];
    assert_eq!(
        device_list(&devices),
        "* Speakers (2 output channels)\n  Interface (18 output channels)\n  Meeting (1 output channel)\n"
    );
    assert_eq!(device_list(&[]), "no audio output device found\n");
}
