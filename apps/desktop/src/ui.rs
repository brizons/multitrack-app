//! The Perform screen's presentational code: draws a [`PerformView`] after
//! the design canvas's Main artboard, reduced to S6, and reports what the
//! person does as an [`Intent`]. It decides nothing.

use app::JumpTiming;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, ElementId, FocusHandle, FontWeight, IntoElement,
    MouseButton, Rgba, SharedString, Stateful, Window, div, prelude::*, px, relative, svg,
};
use multitrack_desktop::action::UiAction;
use multitrack_desktop::view::{
    FadeButton, Level, NextCard, NowCard, Pad, PerformView, SongMap, StatusView, TransportView,
};

use crate::screen::{Intent, Perform};
use crate::theme;

/// The keys, as the status area lists them.
const KEYS: &str = if cfg!(target_os = "macos") {
    "Space play/pause · S stop · B start · 1–9 queue · N timing · L loop · I infinite · F fade · C cancel · hold X panic · ⌘O open · ⌘Q quit"
} else {
    "Space play/pause · S stop · B start · 1–9 queue · N timing · L loop · I infinite · F fade · C cancel · hold X panic · Ctrl-O open · Ctrl-Q quit"
};

/// How the empty screen names the open shortcut.
const OPEN_HINT: &str = if cfg!(target_os = "macos") {
    "⌘O opens a song at any time"
} else {
    "Ctrl-O opens a song at any time"
};

/// The window's root: background, type, and the keyboard focus.
pub fn root(focus: &FocusHandle, cx: &mut Context<Perform>) -> Div {
    let focus_on_click = focus.clone();
    div()
        .track_focus(focus)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_, _, window, _| focus_on_click.focus(window)),
        )
        .size_full()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(16.))
        .bg(theme::bg())
        .text_color(theme::text())
        .text_size(px(15.))
}

/// The screen with a song open.
pub fn perform(view: &PerformView, hold: f32, cx: &mut Context<Perform>) -> impl IntoElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(header(view, hold, cx))
        .child(now_playing(view, cx))
        .child(status(&view.status))
}

/// The screen with no song: why, and the way to open one.
pub fn no_song(errors: &[String], cx: &mut Context<Perform>) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(14.))
        .child(
            div()
                .text_size(px(26.))
                .font_weight(FontWeight::BOLD)
                .child("No song loaded"),
        )
        .child(
            div()
                .text_color(theme::text_muted())
                .child("Open a song folder: song.json and the WAV stems it lists."),
        )
        .children(errors.iter().map(|line| {
            div()
                .max_w(px(900.))
                .text_color(theme::danger())
                .child(line.clone())
        }))
        .child(
            button("open-empty", cx, Intent::OpenSong)
                .h(px(48.))
                .px(px(18.))
                .bg(theme::text())
                .text_color(theme::on_fill())
                .font_weight(FontWeight::BOLD)
                .child(icon("folder", theme::on_fill(), 20.))
                .child("Open song…"),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(theme::text_faint())
                .child(OPEN_HINT),
        )
}

fn header(view: &PerformView, hold: f32, cx: &mut Context<Perform>) -> impl IntoElement {
    div()
        .h(px(64.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(12.))
        .pb(px(12.))
        .border_b_1()
        .border_color(theme::divider())
        .child(
            chip_button("open", cx, Intent::OpenSong)
                .child(icon("folder", theme::text_muted(), 20.))
                .child("Open song…"),
        )
        .child(transport(&view.transport, cx))
        .child(
            div()
                .flex_none()
                .min_w(px(150.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .font_family(theme::MONO)
                        .text_size(px(22.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(view.clock.clone()),
                )
                .child(
                    div()
                        .font_family(theme::MONO)
                        .text_size(px(12.))
                        .text_color(theme::text_muted())
                        .child(format!("{} · {}", view.bar_beat, view.transport.state)),
                ),
        )
        .child(div().flex_1())
        .child(fade(&view.fade, cx))
        .child(panic(hold, cx))
}

fn transport(transport: &TransportView, cx: &mut Context<Perform>) -> impl IntoElement {
    let (play_icon, play_label) = if transport.playing {
        ("pause", "Pause")
    } else {
        ("play", "Play")
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            square_button("back", cx, Intent::Ui(UiAction::BackToStart)).child(icon(
                "back-to-start",
                theme::text(),
                22.,
            )),
        )
        .child(
            button("play", cx, Intent::Ui(UiAction::TogglePlay))
                .w(px(96.))
                .h(px(52.))
                .justify_center()
                .rounded(px(14.))
                .bg(theme::accent())
                .text_color(theme::on_fill())
                .font_weight(FontWeight::BOLD)
                .child(icon(play_icon, theme::on_fill(), 26.))
                .child(play_label),
        )
        .child(
            square_button("stop", cx, Intent::Ui(UiAction::Stop)).child(icon(
                "stop",
                theme::text(),
                22.,
            )),
        )
}

fn fade(fade: &FadeButton, cx: &mut Context<Perform>) -> impl IntoElement {
    let (bg, fg) = if fade.active {
        (theme::text(), theme::on_fill())
    } else {
        (theme::raised(), theme::text())
    };
    button("fade", cx, Intent::Ui(UiAction::ToggleFade))
        .h(px(48.))
        .min_w(px(150.))
        .px(px(16.))
        .bg(bg)
        .text_color(fg)
        .border_1()
        .border_color(theme::border())
        .font_weight(FontWeight::SEMIBOLD)
        .child(icon("fade", fg, 20.))
        .child(fade.label.clone())
}

/// Panic fires only after a full hold; the fill shows how far it is.
fn panic(hold: f32, cx: &mut Context<Perform>) -> impl IntoElement {
    div()
        .id("panic")
        .relative()
        .overflow_hidden()
        .h(px(48.))
        .px(px(18.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(10.))
        .border_1()
        .border_color(theme::danger())
        .bg(theme::danger_bg())
        .text_color(theme::danger_text())
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|screen, _, window, cx| screen.handle(Intent::PanicDown, window, cx)),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|screen, _, window, cx| screen.handle(Intent::PanicUp, window, cx)),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|screen, _, window, cx| screen.handle(Intent::PanicUp, window, cx)),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(relative(hold))
                .bg(theme::danger_fill()),
        )
        .child(icon("panic", theme::danger_text(), 20.))
        .child("Panic")
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::danger_hint())
                .child("hold"),
        )
}

fn now_playing(view: &PerformView, cx: &mut Context<Perform>) -> impl IntoElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(14.))
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border())
        .rounded(px(14.))
        .child(
            div()
                .h(px(68.))
                .flex_none()
                .flex()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(px(4.))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(26.))
                                .font_weight(FontWeight::BOLD)
                                .child(view.title.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(14.))
                                .text_color(theme::text_muted())
                                .child(view.subtitle.clone()),
                        ),
                )
                .child(now_card(&view.now))
                .child(next_card(&view.next, cx)),
        )
        .child(song_map(&view.map))
        .child(section_controls(view, cx))
        .child(pads(&view.pads, cx))
}

fn now_card(now: &NowCard) -> impl IntoElement {
    div()
        .w(px(240.))
        .flex_none()
        .flex()
        .flex_col()
        .justify_center()
        .px(px(12.))
        .rounded(px(12.))
        .bg(theme::raised())
        .border_2()
        .border_color(theme::text())
        .child(small_heading("Now", theme::text_muted()))
        .child(
            div()
                .truncate()
                .text_size(px(20.))
                .font_weight(FontWeight::BOLD)
                .child(now.name.clone()),
        )
        .child(mono_note(now.detail.clone()))
}

fn next_card(next: &NextCard, cx: &mut Context<Perform>) -> impl IntoElement {
    let card = div()
        .w(px(330.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(10.))
        .pl(px(12.))
        .pr(px(8.))
        .rounded(px(12.));
    let card = if next.queued {
        card.border_2()
            .border_dashed()
            .border_color(theme::queued())
            .bg(theme::queued_bg())
    } else {
        card.border_1()
            .border_color(theme::border())
            .bg(theme::bg())
    };
    let heading_color = if next.queued {
        theme::queued()
    } else {
        theme::text_muted()
    };
    card.child(
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(small_heading(next.heading.clone(), heading_color))
            .child(
                div()
                    .truncate()
                    .text_size(px(20.))
                    .font_weight(FontWeight::BOLD)
                    .child(next.name.clone()),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(next.detail.clone()),
            ),
    )
    .when(next.queued, |card| {
        card.child(
            square_button("cancel", cx, Intent::Ui(UiAction::CancelJump))
                .bg(theme::queued_bg())
                .border_color(theme::queued_border())
                .child(icon("close", theme::queued(), 18.)),
        )
    })
}

/// The arrangement in play order, sized by length, with the playhead and
/// the queued jump.
fn song_map(map: &SongMap) -> impl IntoElement {
    let strip = div()
        .h(px(30.))
        .flex_none()
        .flex()
        .children(map.blocks.iter().map(|block| {
            let color = theme::section(block.kind);
            let cell = div()
                .size_full()
                .overflow_hidden()
                .px(px(5.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .bg(color)
                .text_color(theme::on_fill())
                .text_size(px(13.))
                .font_weight(FontWeight::BOLD)
                .whitespace_nowrap()
                .child(block.short.clone());
            let cell = if block.is_now {
                cell.border_2().border_color(theme::text())
            } else if block.is_queued {
                cell.border_2()
                    .border_dashed()
                    .border_color(theme::queued())
            } else {
                cell
            };
            div()
                .w(relative(block.width))
                .h_full()
                .pr(px(2.))
                .child(cell)
        }));
    let track = div()
        .relative()
        .h(px(40.))
        .flex_none()
        .flex()
        .children(map.blocks.iter().map(|block| {
            let color = theme::section(block.kind);
            div().w(relative(block.width)).h_full().pr(px(2.)).child(
                div()
                    .size_full()
                    .rounded(px(3.))
                    .bg(theme::faded(color, 0.3)),
            )
        }))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(relative(map.playhead))
                .bg(theme::faded(theme::text(), 0.12)),
        )
        .when_some(map.jump.as_ref(), |track, jump| {
            track
                .child(
                    div()
                        .absolute()
                        .top(px(-6.))
                        .bottom(px(-6.))
                        .left(relative(jump.at))
                        .border_l_2()
                        .border_dashed()
                        .border_color(theme::queued()),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .left(relative(jump.at))
                        .ml(px(6.))
                        .px(px(6.))
                        .py(px(2.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .rounded(px(5.))
                        .bg(theme::queued())
                        .text_color(theme::on_fill())
                        .text_size(px(12.))
                        .font_weight(FontWeight::BOLD)
                        .whitespace_nowrap()
                        .child(icon("jump", theme::on_fill(), 12.))
                        .child(jump.label.clone()),
                )
        })
        .child(
            div()
                .absolute()
                .top(px(-6.))
                .bottom(px(-6.))
                .left(relative(map.playhead))
                .w(px(2.))
                .bg(theme::text()),
        );
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(strip)
        .child(track)
}

fn section_controls(view: &PerformView, cx: &mut Context<Perform>) -> impl IntoElement {
    div()
        .h(px(44.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Sections"),
        )
        .child(mono_note(format!("{}", view.pads.len())))
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(13.))
                .text_color(theme::text_muted())
                .child("Jump at"),
        )
        .child(
            div()
                .flex()
                .p(px(3.))
                .gap(px(2.))
                .rounded(px(10.))
                .bg(theme::bg())
                .border_1()
                .border_color(theme::border())
                .child(timing_button(
                    "timing-section-end",
                    view.jump_timing,
                    JumpTiming::SectionEnd,
                    "Section end",
                    "section-end",
                    cx,
                ))
                .child(timing_button(
                    "timing-next-bar",
                    view.jump_timing,
                    JumpTiming::NextBar,
                    "Next bar",
                    "next-bar",
                    cx,
                )),
        )
        .child(toggle(
            "loop",
            "Loop section",
            "loop",
            view.live_loop,
            Intent::Ui(UiAction::ToggleLiveLoop),
            cx,
        ))
        .child(toggle(
            "infinite",
            "Infinite",
            "infinite",
            view.infinite,
            Intent::Ui(UiAction::ToggleInfinite),
            cx,
        ))
}

/// One side of the jump timing toggle; `current` is the timing in force.
fn timing_button(
    id: &'static str,
    current: JumpTiming,
    timing: JumpTiming,
    label: &'static str,
    icon_name: &'static str,
    cx: &mut Context<Perform>,
) -> Stateful<Div> {
    let selected = current == timing;
    let (bg, fg) = if selected {
        (theme::selected(), theme::text())
    } else {
        (theme::bg(), theme::text_muted())
    };
    button(id, cx, Intent::Ui(UiAction::SetJumpTiming(timing)))
        .h(px(36.))
        .px(px(12.))
        .rounded(px(7.))
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .bg(bg)
        .text_color(fg)
        .child(icon(icon_name, fg, 16.))
        .child(label)
}

fn toggle(
    id: &'static str,
    label: &'static str,
    icon_name: &'static str,
    on: bool,
    intent: Intent,
    cx: &mut Context<Perform>,
) -> Stateful<Div> {
    let color = if on { theme::accent() } else { theme::text() };
    button(id, cx, intent)
        .h(px(44.))
        .px(px(14.))
        .bg(theme::raised())
        .border_1()
        .border_color(if on { theme::accent() } else { theme::border() })
        .text_color(color)
        .font_weight(FontWeight::SEMIBOLD)
        .child(icon(icon_name, color, 18.))
        .child(label)
}

fn pads(pads: &[Pad], cx: &mut Context<Perform>) -> impl IntoElement {
    div()
        .id("pads")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_wrap()
        .content_start()
        .gap(px(8.))
        .children(pads.iter().map(|pad| pad_button(pad, cx)))
}

fn pad_button(pad: &Pad, cx: &mut Context<Perform>) -> AnyElement {
    let color = theme::section(pad.kind);
    let id = ElementId::Name(format!("pad-{}", pad.position).into());
    let button = div()
        .id(id)
        .relative()
        .w(px(156.))
        .h(px(100.))
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(12.))
        .bg(theme::raised())
        .cursor_pointer()
        .on_click(on_click(
            cx,
            Intent::Ui(UiAction::QueueSection(pad.position)),
        ));
    let button = if pad.is_now {
        button.border_2().border_color(theme::text())
    } else if pad.is_queued {
        button
            .border_2()
            .border_dashed()
            .border_color(theme::queued())
    } else if pad.is_next {
        button
            .border_2()
            .border_color(theme::faded(theme::queued(), 0.5))
    } else {
        button.border_1().border_color(theme::border())
    };
    let badge = |label: &'static str, bg: Rgba| {
        div()
            .px(px(6.))
            .py(px(1.))
            .rounded(px(5.))
            .bg(bg)
            .text_color(theme::on_fill())
            .text_size(px(11.))
            .font_weight(FontWeight::BOLD)
            .child(label)
    };
    button
        .child(div().h(px(6.)).flex_none().bg(color))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .justify_between()
                .p(px(10.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .when_some(pad.key, |row, key| {
                            row.child(
                                div()
                                    .font_family(theme::MONO)
                                    .text_size(px(12.))
                                    .text_color(theme::text_faint())
                                    .child(key.to_string()),
                            )
                        })
                        .child(div().flex_1())
                        .when(pad.is_now, |row| row.child(badge("Now", theme::text())))
                        .when(pad.is_next, |row| row.child(badge("Next", theme::queued()))),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(20.))
                        .font_weight(FontWeight::BOLD)
                        .child(pad.label.clone()),
                ),
        )
        .into_any_element()
}

fn status(status: &StatusView) -> impl IntoElement {
    let dot = match status.device_level {
        Level::Ok => theme::accent(),
        Level::Error => theme::danger(),
    };
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(14.))
        .py(px(10.))
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border())
        .rounded(px(14.))
        .text_size(px(13.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(icon("device", theme::text_muted(), 18.))
                .child(div().size(px(8.)).rounded(px(4.)).bg(dot))
                .child(div().min_w_0().truncate().child(status.device.clone()))
                .child(
                    div()
                        .flex_none()
                        .font_family(theme::MONO)
                        .text_color(theme::text_muted())
                        .child(status.underruns.clone()),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(theme::text_faint())
                        .child(KEYS),
                ),
        )
        .when_some(status.last_error.clone(), |area, error| {
            area.child(
                div()
                    .text_color(theme::danger())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Last problem: {error}")),
            )
        })
        .children(status.warnings.iter().map(|warning| {
            div()
                .text_color(theme::text_muted())
                .child(format!("Warning: {warning}"))
        }))
}

/// A clickable row of an icon and a label that reports `intent`.
fn button(id: &'static str, cx: &mut Context<Perform>, intent: Intent) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .rounded(px(10.))
        .cursor_pointer()
        .on_click(on_click(cx, intent))
}

/// A 48 px square icon button.
fn square_button(id: &'static str, cx: &mut Context<Perform>, intent: Intent) -> Stateful<Div> {
    button(id, cx, intent)
        .size(px(48.))
        .justify_center()
        .bg(theme::raised())
        .border_1()
        .border_color(theme::border())
}

/// A bordered button in the header.
fn chip_button(id: &'static str, cx: &mut Context<Perform>, intent: Intent) -> Stateful<Div> {
    button(id, cx, intent)
        .h(px(48.))
        .px(px(14.))
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border())
        .font_weight(FontWeight::SEMIBOLD)
}

fn on_click(
    cx: &mut Context<Perform>,
    intent: Intent,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |screen, _, window, cx| screen.handle(intent, window, cx))
}

fn icon(name: &str, color: Rgba, size: f32) -> impl IntoElement {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .flex_none()
        .size(px(size))
        .text_color(color)
}

fn small_heading(text: impl Into<SharedString>, color: Rgba) -> impl IntoElement {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(text.into())
}

fn mono_note(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .font_family(theme::MONO)
        .text_size(px(12.))
        .text_color(theme::text_muted())
        .child(text.into())
}
