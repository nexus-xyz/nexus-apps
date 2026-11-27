//! What the Workbench's Side chat entry tells about its side chat while the person is not looking at it: the overview's
//! row, the strip's tab and the "+" menu's row. A side chat never lists in the sidebar, so these are the only places
//! that say it works, waits on the person or failed.
//!
//! The state is the sidebar's rule (`nexus_workbench::attention::attention_of`), read from the side chat's summary,
//! which the sessions' overview keeps on its parent (`CodeSession::side_chats`). The marks follow brand v5's
//! StateMark: a shape for each state, and colour only for attention (amber) and failure (red).
//!
//! - Working: a turning arc in the muted ink, its own small view ([`TurningMark`]).
//! - Needs you: an amber dot, still. The entry's name rises to the foreground.
//! - Limited: an amber ring, still.
//! - Finished, unread: the tabs' unread dot.
//! - Error: a red cross. The entry's name rises to the foreground.
//!
//! The side chat on screen shows no mark: the person sees it. Every state that waits on the person is a still mark
//! (render rule 6); only the arc moves.

use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::*;
use nexus_workbench::attention::{self, Attention};

use super::{View, Workspace};
use crate::code::SideChatActivity;
use crate::palette::Palette;

/// One turn of the arc, as the agent page's session cards turn theirs.
const TURN: Duration = Duration::from_millis(900);
/// The arc's size: it sits in a row of 12.5 to 13 px text.
const ARC: f32 = 11.;
/// The amber dot and ring.
const DOT: f32 = 6.;
/// The tabs' unread dot (`side_pane.rs`).
const UNREAD: f32 = 5.;

/// The state side chat `side` shows, or None when there is nothing to tell. `shown`: it is on screen.
pub(super) fn state_of(side: &SideChatActivity, seen_at: Option<i64>, shown: bool) -> Option<Attention> {
    if shown {
        return None;
    }
    attention::attention_of(&side.activity, side.limited, seen_at, false)
}

/// Whether the state lifts the entry's name to the foreground: it asks the person to act.
pub(super) fn loud(state: Option<Attention>) -> bool {
    matches!(state, Some(Attention::Needs | Attention::Error))
}

/// The entry's tooltip: its name, then its state when it has one ("Side chat · Needs you").
pub(super) fn tooltip(name: &str, state: Option<Attention>) -> String {
    match state {
        Some(state) => format!("{name} · {}", state.label()),
        None => name.to_string(),
    }
}

/// The mark for `state`. `arc`: the window's turning mark; without it the arc stands still.
pub(super) fn mark(state: Attention, arc: Option<&Entity<TurningMark>>, palette: Palette) -> AnyElement {
    let dot = |size: f32| div().flex_none().size(px(size)).rounded_full();
    match state {
        Attention::Working => match arc {
            Some(arc) => arc.clone().into_any_element(),
            None => still_arc(palette.muted).into_any_element(),
        },
        Attention::Needs => dot(DOT).bg(palette.waiting()).into_any_element(),
        Attention::Limited => dot(DOT)
            .border(px(1.5))
            .border_color(palette.waiting())
            .into_any_element(),
        Attention::Unread => dot(UNREAD).bg(palette.muted).into_any_element(),
        Attention::Error => Icon::new(Lucide::X)
            .size(px(ARC))
            .flex_none()
            .text_color(palette.destructive)
            .into_any_element(),
    }
}

fn still_arc(color: Hsla) -> Icon {
    Icon::new(Lucide::LoaderCircle)
        .size(px(ARC))
        .flex_none()
        .text_color(color)
}

/// The Working arc: a view of its own, so its frames (30 a second) redraw only it, never the Workspace (render
/// rules 4 and 5). Under less motion it stands still and asks for no frames.
pub(super) struct TurningMark;

impl Render for TurningMark {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let color = cx
            .try_global::<crate::pages::Theme>()
            .map(|theme| theme.0.muted)
            .unwrap_or_else(gpui_kit::white);
        let arc = still_arc(color);
        if cx.reduce_motion() {
            return arc.into_any_element();
        }
        arc.with_animation(
            "side-chat-arc",
            Animation::new(TURN).repeat().with_max_fps(crate::motion::LOOP_FPS),
            |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
        )
        .into_any_element()
    }
}

impl Workspace {
    /// The state the open session's side chat shows, or None.
    pub(super) fn side_chat_state(&self, cx: &App) -> Option<Attention> {
        let View::Code(view) = &self.view else {
            return None;
        };
        let id = self.side.side_chat_for.get(&view.id)?;
        let side = self.session_by_id(&view.id)?.side_chat_activity(id)?;
        let shown = self.side.open && self.side.side_chat_visible;
        state_of(side, Self::side_chat_seen_at(id, cx), shown)
    }

    /// The mark for the open session's side chat, if it shows one.
    pub(super) fn side_chat_mark(&self, state: Option<Attention>, palette: Palette) -> Option<AnyElement> {
        Some(mark(state?, self.side.side_chat_arc.as_ref(), palette))
    }
}

#[cfg(test)]
#[path = "side_chat_status_tests.rs"]
mod tests;
