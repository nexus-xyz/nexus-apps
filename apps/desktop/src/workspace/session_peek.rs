//! The card a Code session's sidebar row shows once the pointer rests on it (Codex's thread hover card): the whole
//! title, its age and state, its parent when that is in another project (`cross_project.rs`), where it works (project,
//! server, branch), the agent and model, and its pull request.
//! It opens beside the sidebar, level with the row, so the list under the pointer stays in sight.
//!
//! The first card waits out [`DELAY`], so passing over the list opens nothing. Once one shows, the next row's card
//! replaces it at once, with no entrance, as a toolbar's tooltips do after the first.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::sidebar::{hint, session_state_word, status_mark};
use super::{Workspace, glyph};
use crate::code::{self, CodeSession, Status};
use crate::motion;
use crate::palette::Palette;

/// How long the pointer rests on a row before its card opens.
const DELAY: Duration = Duration::from_millis(500);
/// After a card closes, the next row's card opens at once for this long: the pointer is reading down the list.
const WARM: Duration = Duration::from_millis(300);
const WIDTH: f32 = 280.;
/// Pull requests get room for their titles and review text.
const WIDE: f32 = 384.;
const CLOSE_DELAY: Duration = Duration::from_millis(180);

/// The session row under the pointer, and whether its card shows.
#[derive(Default)]
pub(super) struct Peek {
    hovered: Option<String>,
    shown: bool,
    over_card: bool,
    /// It opened straight from another row's card, so it plays no entrance.
    instant: bool,
    /// When a shown card last closed.
    closed_at: Option<Instant>,
    /// Counts openings, so each entrance plays under an id of its own.
    epoch: usize,
    wait: Option<Task<()>>,
}

impl Peek {
    /// The pointer is on row `id`: its actions show at once.
    pub(super) fn hovers(&self, id: &str) -> bool {
        self.hovered.as_deref() == Some(id)
    }

    fn shows(&self, id: &str) -> bool {
        self.shown && self.hovers(id)
    }

    /// The pointer came onto row `id`. True when its card shows now, because another card is open or just closed.
    fn enter(&mut self, id: &str, now: Instant) -> bool {
        let warm = self.shown
            || self
                .closed_at
                .is_some_and(|at| now.saturating_duration_since(at) < WARM);
        self.hovered = Some(id.to_string());
        self.over_card = false;
        self.wait = None;
        self.shown = warm;
        self.instant = warm;
        warm
    }

    /// The pointer left row `id`. Moving between rows can report the new row first, so another row's leaving is
    /// ignored.
    fn leave(&mut self, id: &str, now: Instant) {
        if !self.hovers(id) {
            return;
        }
        if self.shown {
            self.closed_at = Some(now);
        }
        self.hovered = None;
        self.over_card = false;
        self.shown = false;
        self.wait = None;
    }

    /// The delay ran out. True when row `id`'s card opens now.
    fn ripen(&mut self, id: &str) -> bool {
        if !self.hovers(id) || self.shown {
            return false;
        }
        self.shown = true;
        self.instant = false;
        self.epoch = self.epoch.wrapping_add(1);
        true
    }

    /// A press on the row: its card goes until the pointer comes back, and the next row's card waits again.
    pub(super) fn dismiss(&mut self) {
        self.shown = false;
        self.over_card = false;
        self.closed_at = None;
        self.wait = None;
    }
}

impl Workspace {
    /// A session row's `on_hover`.
    pub(super) fn hover_session_row(&mut self, id: &str, hovered: bool, cx: &mut Context<Self>) {
        let now = Instant::now();
        if !hovered && self.peek.hovers(id) && self.peek.over_card {
            return;
        }
        if !hovered && self.peek.shows(id) {
            let id = id.to_string();
            self.peek.wait = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(CLOSE_DELAY).await;
                if let Err(error) = this.update(cx, |this, cx| {
                    this.peek.leave(&id, Instant::now());
                    cx.notify();
                }) {
                    eprintln!("nexus-desktop-gpui: closing the session hover card: {error}");
                }
            }));
        } else if !hovered {
            self.peek.leave(id, now);
        } else if !self.peek.enter(id, now) {
            let id = id.to_string();
            self.peek.wait = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(DELAY).await;
                let _ = this.update(cx, |this, cx| {
                    if this.peek.ripen(&id) {
                        cx.notify();
                    }
                });
            }));
        }
        cx.notify();
    }

    /// Row `session`'s card, hung from the row's right edge; None while it does not show.
    pub(super) fn render_session_peek(
        &self,
        session: &CodeSession,
        mark: Status,
        now: i64,
        palette: Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.peek.shows(&session.id) {
            return None;
        }
        let id = session.id.clone();
        let card = self
            .session_peek_card(session, mark, now, palette, window)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !this.peek.shows(&id) {
                    return;
                }
                this.peek.over_card = *hovered;
                if *hovered {
                    this.peek.wait = None;
                } else {
                    this.hover_session_row(&id, false, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation());
        let card = if self.peek.instant || cx.reduce_motion() {
            card.into_any_element()
        } else {
            // It comes from the row, on its left: a short slide toward the reader with a fade.
            card.with_animation(
                ("session-peek", self.peek.epoch),
                Animation::new(motion::UI).with_easing(motion::ease_out),
                |card, t| card.opacity(t).left(px(-4. * (1. - t))),
            )
            .into_any_element()
        };
        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .child(
                    deferred(
                        anchored()
                            .anchor(Anchor::TopLeft)
                            // Past the sidebar's 8px inset, with a gap; the title sits level with the row's.
                            .offset(point(px(14.), px(-4.)))
                            .snap_to_window_with_margin(px(8.))
                            .child(card),
                    )
                    .with_priority(1),
                )
                .into_any_element(),
        )
    }

    fn session_peek_card(
        &self,
        session: &CodeSession,
        mark: Status,
        now: i64,
        palette: Palette,
        window: &Window,
    ) -> Stateful<Div> {
        let project = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .map(|project| project.name.clone());
        let machine = session.machine.as_ref().map(|id| {
            self.servers
                .iter()
                .find(|server| server.id == *id)
                .map(|server| server.name.clone())
                .or_else(|| {
                    self.cloud_machines
                        .iter()
                        .find(|machine| machine.id == *id)
                        .map(|machine| machine.name.to_string())
                })
                .unwrap_or_else(|| "A server".into())
        });
        let agent = match &session.model {
            Some(model) => format!(
                "{} · {}",
                super::code_title::code_meter::agent_name(session.agent),
                code::model_label(model)
            ),
            None => super::code_title::code_meter::agent_name(session.agent).to_string(),
        };
        let state: Option<AnyElement> = match mark {
            Status::Idle => None,
            Status::Failed => Some(div().text_color(palette.destructive).child("Failed").into_any_element()),
            _ => session_state_word(mark, session.background_count(), palette),
        };
        let background = super::session_peek_background::section(
            session,
            mark,
            now,
            |id| self.session_by_id(id).map(|linked| linked.title.clone()),
            palette,
        );
        let row = |mark: AnyElement, text: String| {
            h_flex()
                .gap(px(8.))
                .min_w(px(0.))
                .child(mark)
                .child(div().flex_1().min_w(px(0.)).truncate().child(text))
        };
        let line = |icon: Lucide, text: String| row(glyph(icon, palette.faint).size(px(14.)).into_any_element(), text);
        // The harness's own logo, as the new-session form and the session view draw it.
        let harness = super::model_menu::maker_mark(super::new_code::agent_maker(session.agent), 14., palette);
        let pull = crate::pr_watch::summary(&session.id)
            .and_then(|(watch, summary)| super::session_peek_pull::body(&watch, &summary, palette));
        let has_pull = pull.is_some();
        let parent = self.cross_project_line(session);
        let content = v_flex()
            .w_full()
            .flex_none()
            .p(px(12.))
            .gap(px(8.))
            .text_size(px(12.))
            .text_color(palette.muted)
            .child(
                h_flex()
                    .items_start()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.strong)
                            .debug_selector(|| "session-peek-title".into())
                            .child(session.title.clone()),
                    )
                    .child(
                        div()
                            .pt(px(1.))
                            .child(hint(code::age(session.updated_at, now), palette)),
                    ),
            )
            .when_some(state, |card, state| {
                card.child(
                    h_flex()
                        .gap(px(6.))
                        .child(status_mark(&format!("peek-{}", session.id), mark, palette))
                        .child(state),
                )
            })
            .children(background)
            .child(
                v_flex()
                    .gap(px(6.))
                    .when_some(parent, |lines, parent| lines.child(line(Lucide::Workflow, parent)))
                    .when_some(project, |lines, project| lines.child(line(Lucide::Folder, project)))
                    .when_some(machine, |lines, machine| lines.child(line(Lucide::Server, machine)))
                    .when_some(session.branch.clone(), |lines, branch| {
                        lines.child(line(Lucide::GitBranch, branch))
                    })
                    .child(row(harness, agent)),
            )
            .when_some(pull, |card, pull| {
                card.child(div().pt(px(10.)).border_t_1().border_color(palette.border).child(pull))
            });
        frame(content, has_pull, palette, window)
    }
}

pub(super) fn frame(content: Div, has_pull: bool, palette: Palette, window: &Window) -> Stateful<Div> {
    let width = if has_pull { WIDE } else { WIDTH };
    div()
        .id("session-peek-card")
        .relative()
        .w(px(width).min((window.viewport_size().width - px(16.)).max(px(0.))))
        .max_h(px(560.).min((window.viewport_size().height - px(16.)).max(px(0.))))
        .overflow_y_scroll()
        .whitespace_normal()
        .text_size(px(12.))
        .line_height(px(18.))
        .rounded(px(12.))
        .bg(palette.menu)
        .shadow(palette.pop_shadow(palette.border))
        .debug_selector(|| "session-peek-card".into())
        .child(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui_kit::*` brings its own `test` macro.
    use core::prelude::v1::test;

    #[test]
    fn the_first_card_waits_and_a_press_closes_it() {
        let now = Instant::now();
        let mut peek = Peek::default();
        assert!(!peek.enter("a", now), "the first card waits out the delay");
        assert!(peek.hovers("a") && !peek.shows("a"));
        assert!(peek.ripen("a"));
        assert!(peek.shows("a") && !peek.instant);
        peek.dismiss();
        assert!(
            !peek.shows("a") && peek.hovers("a"),
            "the actions stay while the pointer stays"
        );
        assert!(!peek.ripen("b"), "a row the pointer left never opens");
    }

    #[test]
    fn the_next_rows_card_opens_at_once_in_either_event_order() {
        let now = Instant::now();
        // The old row's leaving arrives first.
        let mut peek = Peek::default();
        peek.enter("a", now);
        peek.ripen("a");
        peek.leave("a", now);
        assert!(peek.enter("b", now + Duration::from_millis(20)));
        assert!(peek.shows("b") && peek.instant);
        // The new row's entering arrives first.
        let mut peek = Peek::default();
        peek.enter("a", now);
        peek.ripen("a");
        assert!(peek.enter("b", now));
        peek.leave("a", now);
        assert!(peek.shows("b"));
    }

    #[test]
    fn a_card_long_closed_waits_again() {
        let now = Instant::now();
        let mut peek = Peek::default();
        peek.enter("a", now);
        peek.ripen("a");
        peek.leave("a", now);
        assert!(!peek.enter("b", now + WARM));
        assert!(!peek.shows("b"));
    }
}
