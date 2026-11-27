//! A region of the workspace drawn as its own view, so GPUI can keep its last layout and paint (`cached`) while
//! something else in the window changes. A page's frames arrive up to 60 times a second; without this, each one
//! would lay out and paint the sidebar and the chat again. A pane draws again when the workspace changes
//! (`cx.notify` on it), or when something inside the pane does (an input, an animation, a hover).
//!
//! GPUI draws again every view on the path from a changed view up to the window, so the composer's caret blink
//! (its text box is a view) draws the main area again. And a cached view that draws again draws every cached view
//! inside it again too. So on the chat and a Code session, the main area is not cached, and their transcripts are panes
//! of their own inside it, beside the composer rather than around it: a blink keeps the transcript's layout and paint,
//! and draws again only the composer and the thin column around it.

use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace};
use crate::palette::Palette;

#[derive(Clone, Copy)]
pub(super) enum Region {
    Sidebar,
    Center,
    /// The chat's transcript, under its composer (`transcript.rs`).
    ChatTranscript,
    /// A Code session's transcript, above its composer (`code_view.rs`).
    CodeTranscript,
}

/// The transcripts' panes, inside the main area.
pub(super) struct Transcripts {
    chat: Entity<Pane>,
    code: Entity<Pane>,
}

impl Transcripts {
    /// The transcripts draw again when the workspace changes, and when the main area is told to draw again on its own
    /// (`cx.notify` on `center`), as for a streamed answer.
    pub(super) fn new(workspace: &Entity<Workspace>, center: &Entity<Pane>, cx: &mut App) -> Transcripts {
        let mut pane = |region| {
            cx.new(|cx| {
                let mut pane = Pane::new(workspace, region, cx);
                pane._center_changed = Some(cx.observe(center, |_, _, cx| cx.notify()));
                pane
            })
        };
        Transcripts {
            chat: pane(Region::ChatTranscript),
            code: pane(Region::CodeTranscript),
        }
    }
}

impl Workspace {
    /// A transcript's pane, cached, where the transcript's area was: the room the column leaves it (`flex_1`, and no
    /// less than nothing).
    pub(super) fn render_transcript_pane(&self, region: Region) -> AnyElement {
        let pane = match region {
            Region::ChatTranscript => &self.transcripts.chat,
            Region::CodeTranscript => &self.transcripts.code,
            Region::Sidebar | Region::Center => unreachable!("not a transcript"),
        };
        let mut style = StyleRefinement::default();
        style.flex_grow = Some(1.);
        style.flex_shrink = Some(1.);
        style.flex_basis = Some(relative(0.).into());
        style.size.width = Some(relative(1.).into());
        style.min_size.height = Some(px(0.).into());
        pane.clone().cached(style).into_any_element()
    }
}

/// How many times each pane drew on this thread, for tests that guard what a change draws again.
#[cfg(test)]
pub(super) mod drawn {
    use std::cell::RefCell;

    thread_local! {
        static DRAWN: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    }

    pub(in crate::workspace) fn record(region: super::Region) {
        DRAWN.with(|drawn| {
            let mut drawn = drawn.borrow_mut();
            let ix = region as usize;
            if drawn.len() <= ix {
                drawn.resize(ix + 1, 0);
            }
            drawn[ix] += 1;
        });
    }

    /// How many times the pane of `region` drew so far.
    pub(in crate::workspace) fn count(region: super::Region) -> usize {
        DRAWN.with(|drawn| drawn.borrow().get(region as usize).copied().unwrap_or(0))
    }
}

impl Workspace {
    /// Draw the main area again, and not the sidebar: for streamed text, which only the main area shows. A
    /// `cx.notify` on the Workspace draws both panes again (each pane observes it), up to 20 times a second while an
    /// answer streams. The Workspace itself still draws, since GPUI draws every view above a changed one. A window
    /// with no panes (the panel) draws as before.
    pub(super) fn notify_main(&self, cx: &mut Context<Self>) {
        match &self.panes {
            Some((_, center)) => center.update(cx, |_, cx| cx.notify()),
            None => cx.notify(),
        }
    }
}

pub(super) struct Pane {
    workspace: WeakEntity<Workspace>,
    region: Region,
    _workspace_changed: Subscription,
    _center_changed: Option<Subscription>,
}

impl Pane {
    pub(super) fn new(workspace: &Entity<Workspace>, region: Region, cx: &mut Context<Self>) -> Pane {
        Pane {
            workspace: workspace.downgrade(),
            region,
            _workspace_changed: cx.observe(workspace, |_, _, cx| cx.notify()),
            _center_changed: None,
        }
    }
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(workspace) = self.workspace.upgrade() else {
            return div().into_any_element();
        };
        let region = self.region;
        #[cfg(test)]
        drawn::record(region);
        workspace.update(cx, |workspace, cx| {
            let palette = Palette::for_choice(workspace.theme, window.appearance());
            match region {
                Region::Sidebar => {
                    // The marks the sidebar shows now replace the ones it showed when it last drew.
                    workspace.sidebar_marks.clear();
                    workspace.render_sidebar_panel(palette, window, cx).into_any_element()
                }
                // A row the size of the pane, so a view that grows with `flex_1` fills it.
                Region::Center => div()
                    .flex()
                    .size_full()
                    .child(workspace.render_center(palette, window, cx))
                    .into_any_element(),
                // A column the size of the pane, so the transcript's `flex_1` fills it.
                Region::ChatTranscript => v_flex()
                    .size_full()
                    .child(workspace.render_chat_scroll(palette, cx))
                    .child(redraw_when_followed(&workspace.chat_list.list))
                    .into_any_element(),
                Region::CodeTranscript => match &workspace.view {
                    View::Code(view) => v_flex()
                        .size_full()
                        .child(workspace.render_code_scroll(view, palette, cx))
                        .child(redraw_when_followed(&view.list))
                        .into_any_element(),
                    _ => div().into_any_element(),
                },
            }
        })
    }
}

/// After the list: the list may start following its end again as it lays out, after the transcript chose whether
/// to show its jump to the latest. Before it was a pane, the next caret blink drew it again; now it asks for the
/// next frame itself when that changed.
fn redraw_when_followed(list: &ListState) -> impl IntoElement {
    let drawn = list.is_following_tail();
    let list = list.clone();
    canvas(
        move |_, window, _| {
            if list.is_following_tail() != drawn {
                window.request_animation_frame();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_0()
}
