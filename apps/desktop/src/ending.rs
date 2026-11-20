//! Ending the show (S8): quitting or closing the window while a song plays
//! asks first, so a stray Cmd-Q or a click on the window's close button
//! cannot stop the show. Stop, the transport control, stays immediate and
//! never comes here.

use app::TransportState;

use crate::action::is_playing;

/// What ends the show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// Quitting the app (Cmd-Q on the Mac, Ctrl-Q elsewhere).
    Quit,
    /// Closing the window, which also quits the app.
    CloseWindow,
}

/// What a request to end the show does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndRequest {
    /// Nothing plays: end at once; the song, if one is open, stops with
    /// its ramp first.
    EndNow,
    /// A song plays: ask [`Ending::question`] first, and end only on
    /// [`END_ANSWER`].
    Ask,
    /// The question is already on screen: wait for its answer.
    Wait,
}

/// What a request to end the show does, given the transport of the open
/// song (`None`: no song is open) and whether the question is already on
/// screen. A song asks while it [`is_playing`]: playing, or waiting for the
/// disk to start.
pub fn end_request(transport: Option<TransportState>, asking: bool) -> EndRequest {
    if asking {
        EndRequest::Wait
    } else if transport.is_some_and(is_playing) {
        EndRequest::Ask
    } else {
        EndRequest::EndNow
    }
}

/// The question asked before ending the show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Question {
    /// The question.
    pub message: &'static str,
    /// What ending does.
    pub detail: &'static str,
    /// The answers in the order the dialog lists them: [`END_ANSWER`]
    /// first, so it is the one Return picks on the Mac, and
    /// [`KEEP_ANSWER`] second, as the dialog's cancel answer (Escape on
    /// the Mac and Windows).
    pub answers: [&'static str; 2],
}

/// The answer that stops the song and ends the show.
pub const END_ANSWER: usize = 0;

/// The answer that keeps the song playing.
pub const KEEP_ANSWER: usize = 1;

impl Ending {
    /// The question to ask before ending this way while a song plays.
    pub fn question(self) -> Question {
        Question {
            message: "A song is playing",
            detail: match self {
                Self::Quit => "Quitting stops the song.",
                Self::CloseWindow => "Closing the window stops the song and quits.",
            },
            answers: ["Stop and quit", "Keep playing"],
        }
    }
}

/// Whether `answer`, the index of the answer chosen (`None` when the dialog
/// went away unanswered), ends the show.
pub fn ends(answer: Option<usize>) -> bool {
    answer == Some(END_ANSWER)
}
