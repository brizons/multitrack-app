//! S8: quitting or closing the window while a song plays asks first. Stop,
//! the transport control, stays immediate: it never goes through these
//! rules (its command is in view_model.rs).

use app::TransportState;
use multitrack_desktop::action::is_playing;
use multitrack_desktop::ending::{END_ANSWER, EndRequest, Ending, KEEP_ANSWER, end_request, ends};

const ALL: [TransportState; 7] = [
    TransportState::Stopped,
    TransportState::Starting,
    TransportState::Playing,
    TransportState::Pausing,
    TransportState::Paused,
    TransportState::Stopping,
    TransportState::Panicking,
];

#[test]
fn playing_or_waiting_for_the_disk_counts_as_playing() {
    for transport in ALL {
        let expected = matches!(
            transport,
            TransportState::Playing | TransportState::Starting
        );
        assert_eq!(is_playing(transport), expected, "{transport:?}");
    }
}

#[test]
fn ending_asks_first_only_while_a_song_plays() {
    for transport in ALL {
        let expected = if is_playing(transport) {
            EndRequest::Ask
        } else {
            EndRequest::EndNow
        };
        assert_eq!(
            end_request(Some(transport), false),
            expected,
            "{transport:?}"
        );
    }
    assert_eq!(end_request(None, false), EndRequest::EndNow, "no song");
}

#[test]
fn a_second_request_waits_for_the_question_on_screen() {
    for transport in ALL.map(Some).into_iter().chain([None]) {
        assert_eq!(
            end_request(transport, true),
            EndRequest::Wait,
            "{transport:?}"
        );
    }
}

#[test]
fn only_the_ending_answer_ends_the_show() {
    assert_ne!(END_ANSWER, KEEP_ANSWER);
    assert!(ends(Some(END_ANSWER)));
    assert!(!ends(Some(KEEP_ANSWER)));
    assert!(
        !ends(None),
        "a dialog that went away unanswered keeps playing"
    );
    assert!(!ends(Some(7)));
}

#[test]
fn the_question_says_what_ending_does() {
    for ending in [Ending::Quit, Ending::CloseWindow] {
        let question = ending.question();
        assert_eq!(question.message, "A song is playing");
        assert_eq!(question.answers[END_ANSWER], "Stop and quit");
        assert_eq!(question.answers[KEEP_ANSWER], "Keep playing");
    }
    assert_eq!(Ending::Quit.question().detail, "Quitting stops the song.");
    assert_eq!(
        Ending::CloseWindow.question().detail,
        "Closing the window stops the song and quits."
    );
}
