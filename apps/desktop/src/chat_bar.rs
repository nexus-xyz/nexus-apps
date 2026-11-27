//! The Mac chat bar: the Assistant's small bar that floats over any app. The panel's global shortcut
//! (⌥Space by default) opens it instead of the Ask Nexus panel while Settings › App has "Chat bar" on (the default;
//! the tray still opens the panel). macOS only: elsewhere [`enabled`] is false and the shortcut opens the panel.
//!
//! - The bar is a borderless pop-up window (`WindowKind::PopUp`: an `NSPanel`, over other apps and Spaces, with no Dock
//!   entry). It takes the keyboard without activating Nexus, so the app you came from keeps its place. Closing it drops
//!   the window: GPUI cannot hide one (`panel.rs`).
//! - Enter asks the Assistant through the agent API the thread uses, with `surface: "desktop"`, in the thread's
//!   conversation, so the question and the answer sit in the thread. The answer streams in the bar: the step
//!   line, then the text. Esc closes the bar and stops the answer. A click on another app closes an idle bar.
//! - ⌘↩ (Open in Nexus) brings the app window to the Assistant thread, read again and scrolled to the answer
//!   (`Workspace::open_chat_bar_answer`).
//! - The bar never writes. A write the Assistant asks for is an approval card in the thread: the bar says "Needs you"
//!   and Open in Nexus goes there.
//! - The shortcut is Carbon's `RegisterEventHotKey` through `global-hotkey` (`prefs.rs`). It needs no Accessibility
//!   permission, so the bar asks for none. Nor does it read the app in front: a chip with the front app's selection
//!   needs that permission and is not built.

mod view;

use std::time::Instant;

use gpui_kit::*;

use crate::agent::{Entry, Message, Pane};

/// The bar's width, and its composer row's height, from the design.
pub const WIDTH: f32 = 640.;
pub const HEADER: f32 = 56.;
/// Room around the card for its shadow: the window is transparent.
pub const MARGIN: f32 = 14.;
/// The answer scrolls past this height.
pub const ANSWER_MAX: f32 = 340.;
/// What the Assistant is asked from.
pub const SURFACE: &str = "desktop";

/// Whether the shortcut opens the bar: on macOS, unless the person turned it off in Settings › App.
pub fn enabled() -> bool {
    cfg!(target_os = "macos") && crate::desktop_settings::load().opens_chat_bar()
}

/// What the bar's body shows under the composer.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Nothing asked yet.
    Empty,
    /// The step line ("Thinking", "Searching Linear") before the first word.
    Thinking(String),
    /// The answer, streaming.
    Writing,
    /// The answer, finished. `needs_you`: it carries a write that waits for an approval in the thread.
    Answered { needs_you: bool },
    /// The ask failed.
    Failed { title: String, detail: String },
}

/// What the bar shows for `pane`.
pub fn body_of(pane: &Pane) -> Body {
    if let Some(draft) = &pane.draft {
        return if draft.text.is_empty() {
            Body::Thinking(draft.thinking_line().unwrap_or("Thinking").to_string())
        } else {
            Body::Writing
        };
    }
    if let Some(problem) = &pane.problem {
        return Body::Failed {
            title: problem.title.clone(),
            detail: problem.detail.clone(),
        };
    }
    match pane.entries.last() {
        Some(Entry::Agent(_)) => Body::Answered {
            needs_you: pane.needs_you(),
        },
        _ => Body::Empty,
    }
}

/// The latest reply's text, and the model that wrote it.
pub fn answer_of(pane: &Pane) -> Option<(String, Option<String>)> {
    let Some(Entry::Agent(messages)) = pane.entries.last() else {
        return None;
    };
    let mut model = None;
    let mut text: Vec<&str> = Vec::new();
    for message in messages {
        if let Message::Answer {
            text: words,
            model: name,
            ..
        } = message
        {
            if !name.is_empty() {
                model = Some(nexus_desktop_core::agent::model_name(name).to_string());
            }
            text.push(words);
        }
    }
    (!text.is_empty()).then(|| (text.join("\n\n"), model))
}

/// Lines `text` takes at the bar's width, roughly: the window grows to it before the text is laid out.
pub fn lines(text: &str) -> usize {
    const PER_LINE: usize = 66;
    text.split('\n')
        .map(|line| line.chars().count().div_ceil(PER_LINE).max(1))
        .sum()
}

/// The window's height for `body` showing `text`, in points.
pub fn height(body: &Body, text: &str) -> f32 {
    let card = match body {
        Body::Empty => HEADER,
        Body::Thinking(_) => HEADER + 1. + 2. + 22. + 16.,
        Body::Writing | Body::Answered { .. } => {
            let words = (lines(text) as f32 * 22.).min(ANSWER_MAX);
            let footer = if matches!(body, Body::Answered { .. }) {
                12. + 18.
            } else {
                0.
            };
            HEADER + 1. + 2. + words + footer + 16.
        }
        Body::Failed { .. } => HEADER + 1. + 2. + 44. + 16.,
    };
    card + 2. * MARGIN
}

/// The bar's window.
struct Current {
    window: AnyWindowHandle,
}

impl Global for Current {}

fn open(cx: &App) -> Option<AnyWindowHandle> {
    cx.try_global::<Current>().map(|current| current.window)
}

/// The shortcut: open the bar, or close it when it is open.
pub fn toggle(cx: &mut App) {
    if open(cx).is_some() {
        hide(cx);
    } else {
        show(cx);
    }
}

/// Open the bar over whatever is in front, on the display with the menu bar, a fifth of the way down.
pub fn show(cx: &mut App) {
    if !crate::windows::running(cx) || open(cx).is_some() {
        return;
    }
    let gateway = crate::windows::gateway(cx);
    let conversation = crate::windows::assistant_conversation(cx);
    let (width, height) = (WIDTH + 2. * MARGIN, height(&Body::Empty, ""));
    let display = cx.primary_display();
    let bounds = display
        .as_ref()
        .map(|display| {
            let area = display.visible_bounds();
            Bounds::new(
                point(
                    area.origin.x + (area.size.width - px(width)) / 2.,
                    area.origin.y + area.size.height * 0.2,
                ),
                size(px(width), px(height)),
            )
        })
        .unwrap_or_else(|| Bounds::centered(None, size(px(width), px(height)), cx));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        display_id: display.map(|display| display.id()),
        app_id: Some(crate::app_id::get()),
        ..WindowOptions::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| view::ChatBar::new(gateway, conversation, window, cx))
    });
    match opened {
        Ok((window, _)) => {
            cx.set_global(Current { window });
            if let Err(error) = window.update(cx, |_, window, _| window.activate_window()) {
                eprintln!("nexus-desktop-gpui: the chat bar did not take the focus: {error}");
            }
        }
        Err(error) => eprintln!("nexus-desktop-gpui: the chat bar did not open: {error}"),
    }
}

/// Close the bar, if it is open. An answer still streaming stops.
pub fn hide(cx: &mut App) {
    let Some(window) = open(cx) else { return };
    cx.remove_global::<Current>();
    if let Err(error) = window.update(cx, |_, window, _| window.remove_window()) {
        eprintln!("nexus-desktop-gpui: the chat bar did not close: {error}");
    }
}

/// Close the bar once the window being updated now is done (Esc, losing the focus).
fn hide_later(cx: &mut App) {
    cx.defer(hide);
}

/// Open in Nexus (⌘↩): the bar closes, and the app window shows the thread at `conversation`, at the answer.
fn open_in_nexus(conversation: Option<String>, cx: &mut App) {
    cx.defer(move |cx| {
        hide(cx);
        match conversation {
            Some(conversation) => crate::windows::with_workspace(cx, move |workspace, window, cx| {
                workspace.open_chat_bar_answer(conversation, window, cx)
            }),
            None => crate::windows::run(crate::tray::Command::Open, cx),
        }
    });
}

/// Seconds an answer took, for the footer ("Auto → Claude Sonnet 5.5 · 3 s").
fn took(since: Instant) -> u64 {
    since.elapsed().as_secs().max(1)
}

#[cfg(test)]
#[path = "chat_bar_tests.rs"]
mod tests;
