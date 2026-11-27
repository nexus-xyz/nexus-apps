//! How wide the conversation and the composer are (Settings › Appearance). The setting is the Nexus app's
//! `chatWidth` in its `settings.json`, read and written through `nexus-desktop-core` (`desktop_settings.rs`), so both
//! apps keep one choice. The formula is `nexus_desktop_core::chat_width::widths`; this file holds the choice and reads
//! the file again every few seconds, so a change made in the Nexus app applies here. The default is Standard.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use gpui_kit::*;
use nexus_desktop_core::chat_width::{CH_PX, ChatWidth, STANDARD_PX, Widths, widths};

/// The body font, as `workspace::FONT`.
const FONT: &str = "DM Sans";
/// The body text's size, at 100%.
const BODY: f32 = 14.;
/// How often `settings.json` is read again for a change made in the Nexus app.
const FOLLOW_EVERY: Duration = Duration::from_secs(3);

/// The choice every window draws with (`now`), and the one in the file as last read (`file`). They differ while a
/// slider in Settings moves, before it is let go and saved.
struct Choice {
    now: ChatWidth,
    file: ChatWidth,
}

impl Global for Choice {}

/// Read the saved choice, and follow the file. Before the first window opens.
pub fn start(cx: &mut App) {
    let file = saved();
    cx.set_global(Choice { now: file, file });
    follow_file(cx);
}

/// The choice as saved; the default (Standard) when there is none.
fn saved() -> ChatWidth {
    crate::desktop_settings::load().chat_width.unwrap_or_default()
}

/// The choice now.
pub fn choice(cx: &App) -> ChatWidth {
    cx.try_global::<Choice>().map(|choice| choice.now).unwrap_or_default()
}

/// Draw every window with `next` without saving it: a slider in Settings while it moves.
pub fn preview(next: ChatWidth, cx: &mut App) {
    if next == choice(cx) {
        return;
    }
    let file = cx.try_global::<Choice>().map(|choice| choice.file).unwrap_or_default();
    cx.set_global(Choice { now: next, file });
    cx.refresh_windows();
}

/// Save `next` in the shared settings and draw every window with it. Refused, with a reason, while the Nexus app owns
/// the settings (`desktop_settings::READ_ONLY`); then every window goes back to the saved choice.
pub fn set(next: ChatWidth, cx: &mut App) -> Result<(), String> {
    let saved = crate::desktop_settings::update(|settings| {
        settings.chat_width = Some(next);
        Ok(())
    });
    let file = match &saved {
        Ok(_) => next,
        Err(_) => cx.try_global::<Choice>().map(|choice| choice.file).unwrap_or_default(),
    };
    cx.set_global(Choice { now: file, file });
    cx.refresh_windows();
    saved.map(|_| ())
}

/// Read the file again every few seconds, off the UI thread: a choice made in the Nexus app applies to every window.
fn follow_file(cx: &mut App) {
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        loop {
            executor.timer(FOLLOW_EVERY).await;
            let file = executor.spawn(async { saved() }).await;
            cx.update(|cx| {
                let known = cx.try_global::<Choice>().map(|choice| choice.file);
                if known != Some(file) {
                    cx.set_global(Choice { now: file, file });
                    cx.refresh_windows();
                }
            });
        }
    })
    .detach();
}

/// The advance of "0" in the body font at 14 px, measured once; [`CH_PX`] when the font has no "0". Settings' preview
/// counts characters a line with it; the widths do not, so both apps draw a choice at the same pixels.
pub fn ch(cx: &App) -> f32 {
    static CH: OnceLock<f32> = OnceLock::new();
    *CH.get_or_init(|| {
        let text = cx.text_system();
        let id = text.resolve_font(&font(FONT));
        text.advance(id, px(BODY), '0')
            .map(|size| f32::from(size.width))
            .ok()
            .filter(|width| width.is_finite() && *width > 0.)
            .unwrap_or(CH_PX)
    })
}

/// The widths in a room `room` logical pixels wide, with the current choice. A character counts [`CH_PX`], as in the
/// Nexus app's CSS: DM Sans measures its "0" wider here, which drew every preset about a fifth wider than there.
pub fn in_room(room: f32, cx: &App) -> Widths {
    widths(choice(cx).shape(), room, CH_PX)
}

thread_local! {
    /// Each window's widths at its last frame, for a page drawn in the main area (`pages/agent.rs`).
    static WINDOWS: RefCell<HashMap<WindowId, Widths>> = RefCell::new(HashMap::new());
}

/// Keep `window`'s widths for this frame (`workspace/chat_column.rs`).
pub fn remember(window: WindowId, widths: Widths) {
    WINDOWS.with(|windows| windows.borrow_mut().insert(window, widths));
}

/// `window`'s widths at its last frame; Standard's lane before its first.
pub fn of(window: WindowId) -> Widths {
    WINDOWS
        .with(|windows| windows.borrow().get(&window).copied())
        .unwrap_or(Widths {
            column: STANDARD_PX,
            wide: STANDARD_PX,
            composer: STANDARD_PX,
        })
}
