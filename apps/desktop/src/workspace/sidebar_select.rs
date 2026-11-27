//! Several Code sessions picked in the sidebar at once, as the Nexus app does it:
//! Ctrl-click (Cmd on a Mac) adds a row or takes it out, Shift-click picks every row from the last one clicked to this
//! one, in the order the sidebar draws them. A compact bar under the list says how many are picked and offers Archive,
//! Snooze (the same presets as a row's menu) and Clear; Esc clears too. A plain click opens a session and lets go of the
//! picked ones. Only Code sessions can be picked: Gateway chats' rows never take part, so no bulk action reaches them.

use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;
use std::collections::HashSet;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::api::UpdateArgs;

use super::code_dock::menu_box;
use super::sidebar::{ROW_TEXT, hint};
use super::{View, Workspace, glyph};
use crate::i18n::tr;
use crate::palette::Palette;
use crate::snooze;

/// The picked sessions, and where the next Shift-click's range starts.
#[derive(Default)]
pub(super) struct Picked {
    ids: HashSet<String>,
    anchor: Option<String>,
    /// The Snooze presets are showing above the bar.
    snooze_open: bool,
}

impl Picked {
    pub fn has(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Lets go of every picked row: a drag that carried them dropped them (`session_drag.rs`).
    pub fn clear(&mut self) {
        self.ids.clear();
        self.snooze_open = false;
    }

    fn toggle(&mut self, id: &str) {
        if !self.ids.remove(id) {
            self.ids.insert(id.to_string());
        }
        self.anchor = Some(id.to_string());
    }

    /// Every row of `order` from the anchor (or `fallback`, the open session) to `id`, both ends in, whichever way
    /// round. With neither drawn, only `id`. The anchor stays, so the range can be redrawn.
    fn range(&mut self, id: &str, order: &[String], fallback: Option<&str>) {
        let drawn = |candidate: &str| order.iter().position(|row| row == candidate);
        let from = self
            .anchor
            .as_deref()
            .and_then(drawn)
            .or_else(|| fallback.and_then(drawn));
        match (from, drawn(id)) {
            (Some(from), Some(to)) => {
                self.ids = order[from.min(to)..=from.max(to)].iter().cloned().collect();
                self.anchor = Some(order[from].clone());
            }
            _ => {
                self.ids = HashSet::from([id.to_string()]);
                self.anchor = Some(id.to_string());
            }
        }
    }
}

/// A picked row's fill: the primary tone, soft enough to keep the text readable, stronger than the open row's grey.
pub(super) fn picked_fill(palette: Palette) -> Hsla {
    palette.primary.opacity(if palette.dark { 0.22 } else { 0.16 })
}

/// The Code sessions of `listed` (each project with the sessions it shows) in the order the sidebar draws them: one
/// group per repository (`repo_groups.rs`), linked sessions under their lead, and nothing of a group whose lead is
/// folded, since the sidebar folds a repository by its lead's id.
fn drawn_order(
    listed: Vec<(&crate::code::CodeProject, Vec<&crate::code::CodeSession>)>,
    folded: &HashSet<String>,
) -> Vec<String> {
    super::repo_groups::group(listed)
        .into_iter()
        .filter(|repo| !folded.contains(&repo.lead.id))
        .flat_map(|repo| {
            super::session_tree::tree(&repo.sessions)
                .into_iter()
                .map(|(session, _)| session.id.clone())
                .collect::<Vec<_>>()
        })
        .collect()
}

impl Workspace {
    /// A click on a Code session's row. Ctrl (Cmd on a Mac) toggles it and Shift picks a range, and neither opens the
    /// session (true). A plain click lets go of the picked rows and is left to open it (false).
    pub(super) fn pick_click(&mut self, id: &str, modifiers: Modifiers, cx: &mut Context<Self>) -> bool {
        let chord = if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control
        };
        if chord {
            self.sidebar_filter.picked.toggle(id);
        } else if modifiers.shift {
            let order = self.drawn_code_ids();
            let open = match &self.view {
                View::Code(open) => Some(open.id.clone()),
                _ => None,
            };
            self.sidebar_filter.picked.range(id, &order, open.as_deref());
        } else {
            let picked = &mut self.sidebar_filter.picked;
            picked.ids.clear();
            picked.snooze_open = false;
            picked.anchor = Some(id.to_string());
            return false;
        }
        self.sidebar_filter.picked.snooze_open = false;
        cx.notify();
        true
    }

    /// How many picked sessions still exist.
    pub(super) fn picked_count(&self) -> usize {
        self.picked_sessions().len()
    }

    /// The ids of the picked sessions that still exist, in no order.
    pub(super) fn picked_ids(&self) -> Vec<String> {
        self.picked_sessions()
            .iter()
            .map(|session| session.id.clone())
            .collect()
    }

    /// The Code sessions in the order the sidebar draws them (folded projects' rows are not drawn, so never picked).
    pub(super) fn drawn_code_ids(&self) -> Vec<String> {
        let now = crate::code::now_ms();
        if self.filtering() {
            // A search draws the same repository groups, and folds nothing.
            return drawn_order(self.filter_kept().projects, &HashSet::new());
        }
        let listed = self
            .projects
            .iter()
            .map(|project| {
                // The open session stays in its project while it is on screen, as the sidebar keeps it.
                let active = project
                    .sessions
                    .iter()
                    .filter(|session| {
                        let open = matches!(&self.view, View::Code(open) if open.id == session.id);
                        session.shown_aside(now, open).is_none()
                    })
                    .collect();
                (project, active)
            })
            .collect();
        drawn_order(listed, &self.folded_projects)
    }

    /// Esc lets go of the picked rows, unless an overlay or a title field is open (Esc closes that first).
    pub(super) fn picked_escape(&mut self, cx: &mut Context<Self>) -> bool {
        // Picks whose sessions are gone (deleted since, or another organization's) show no bar: they never take an Esc.
        let gone: Vec<String> = self
            .sidebar_filter
            .picked
            .ids
            .iter()
            .filter(|id| self.session_by_id(id).is_none())
            .cloned()
            .collect();
        let picked = &mut self.sidebar_filter.picked;
        for id in &gone {
            picked.ids.remove(id);
        }
        if picked.ids.is_empty() || self.overlay.is_some() || self.renaming.is_some() {
            return false;
        }
        if picked.snooze_open {
            picked.snooze_open = false;
        } else {
            picked.ids.clear();
        }
        cx.notify();
        true
    }

    /// The picked sessions that still exist (deleted since, or another organization's, drop out), in no order.
    fn picked_sessions(&self) -> Vec<&crate::code::CodeSession> {
        self.sidebar_filter
            .picked
            .ids
            .iter()
            .filter_map(|id| self.session_by_id(id))
            .collect()
    }

    fn clear_picked(&mut self, cx: &mut Context<Self>) {
        self.sidebar_filter.picked.ids.clear();
        self.sidebar_filter.picked.snooze_open = false;
        cx.notify();
    }

    /// Archive the picked sessions as a row's menu does (`archive.rs`): one notice, with one Undo for all of them.
    fn bulk_archive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self
            .picked_sessions()
            .iter()
            .map(|session| session.id.clone())
            .collect();
        self.clear_picked(cx);
        self.archive_sessions(ids, window, cx);
    }

    /// Snooze the picked sessions through the runner a row's menu uses; read-only while the Nexus app holds them.
    fn bulk_update(&mut self, change: impl Fn(&mut UpdateArgs), cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .picked_sessions()
            .iter()
            .map(|session| session.id.clone())
            .collect();
        match &self.runner {
            Some(runner) => {
                for id in ids {
                    let mut args = UpdateArgs {
                        thread_id: id,
                        ..UpdateArgs::default()
                    };
                    change(&mut args);
                    runner.update_session(args);
                }
                self.clear_picked(cx);
            }
            None => {
                self.refuse_read_only();
                cx.notify();
            }
        }
    }

    /// The bar under the list while sessions are picked: "N selected", Snooze, Archive and Clear.
    pub(super) fn render_bulk_bar(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let count = self.picked_sessions().len();
        if count == 0 {
            return None;
        }
        let snooze_open = self.sidebar_filter.picked.snooze_open;
        // Icon buttons with a tooltip: "N selected" keeps the room in a 244px bar.
        let button = |id: &'static str, icon: Lucide, label: SharedString, tip: bool| {
            div()
                .id(id)
                .size(px(26.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .cursor_pointer()
                .hover(|style| style.bg(palette.selected))
                .active(|style| style.opacity(0.8))
                .when(tip, |button| {
                    button.tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
                })
                .child(glyph(icon, palette.icon).size(px(15.)))
        };
        let local = chrono::Local::now();
        let presets = snooze_open.then(|| {
            let body = menu_box("bulk-snooze-menu", 300., palette)
                .text_size(px(ROW_TEXT))
                .children(
                    snooze::presets(local)
                        .into_iter()
                        .enumerate()
                        .map(|(index, (label, at))| {
                            h_flex()
                                .id(SharedString::from(format!("bulk-snooze-{index}")))
                                .h(px(30.))
                                .px(px(8.))
                                .gap(px(8.))
                                .rounded(px(8.))
                                .cursor_pointer()
                                .hover(|style| menu_item_hover(style, palette))
                                .child(div().flex_1().child(label))
                                .child(hint(snooze::wake_text(at, local), palette))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.bulk_update(|args| args.snoozed_until = Some(at), cx);
                                }))
                        }),
                )
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.sidebar_filter.picked.snooze_open = false;
                    cx.notify();
                }));
            div().absolute().bottom(px(40.)).left_0().child(
                deferred(
                    anchored()
                        .anchor(Anchor::BottomLeft)
                        .offset(point(px(0.), px(-2.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(body),
                )
                .with_priority(1),
            )
        });
        Some(
            v_flex()
                .px(px(8.))
                .pb(px(8.))
                .child(
                    h_flex()
                        .relative()
                        .h(px(36.))
                        .pl(px(10.))
                        .pr(px(4.))
                        .gap(px(2.))
                        .rounded(px(10.))
                        .bg(palette.raised)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette.foreground)
                                .child(format!("{count} selected")),
                        )
                        .child(
                            button("bulk-snooze", Lucide::AlarmClock, tr("Snooze"), !snooze_open).menu_trigger(
                                snooze_open,
                                cx,
                                |this| this.sidebar_filter.picked.snooze_open,
                                |this, _, cx| {
                                    let picked = &mut this.sidebar_filter.picked;
                                    picked.snooze_open = !picked.snooze_open;
                                    cx.notify();
                                },
                            ),
                        )
                        .child(
                            button("bulk-archive", Lucide::Archive, tr("Archive"), !snooze_open).on_click(cx.listener(
                                |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.bulk_archive(window, cx);
                                },
                            )),
                        )
                        .child(
                            div()
                                .id("bulk-clear")
                                .size(px(26.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(7.))
                                .cursor_pointer()
                                .hover(|style| style.bg(palette.selected))
                                .child(glyph(Lucide::X, palette.icon).size(px(14.)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.clear_picked(cx);
                                })),
                        )
                        .children(presets),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Picked, drawn_order};
    use crate::code::{CodeProject, CodeSession};
    use std::collections::HashSet;

    fn project(id: &str, machine: Option<&str>) -> CodeProject {
        CodeProject {
            id: id.into(),
            name: "nexus".into(),
            branch: Some("main".into()),
            pinned: false,
            remotes: vec!["https://github.com/acme/nexus".into()],
            sessions: Vec::new(),
            machine: machine.map(String::from),
            added_at: 0,
        }
    }

    /// A session the person last wrote to at `at`.
    fn session(id: &str, project: &str, at: i64) -> CodeSession {
        let thread: nexus_workbench::event::ThreadSummary = serde_json::from_value(serde_json::json!({
            "id": id, "projectId": project, "title": id, "agent": "claude", "model": null,
            "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
            "contextTokens": null, "contextWindow": null, "createdAt": 0, "updatedAt": at, "actedAt": at,
            "archived": false,
        }))
        .expect("thread");
        CodeSession::of(thread)
    }

    #[test]
    fn a_shift_click_never_reaches_a_folded_repository_s_cloud_copy() {
        let (local, cloud, other) = (project("local", None), project("cloud", Some("m1")), {
            let mut other = project("other", None);
            other.remotes = vec!["https://github.com/acme/other".into()];
            other
        });
        let (a, b) = (session("a", "local", 1), session("b", "cloud", 2));
        let c = session("c", "other", 3);
        let listed = || vec![(&cloud, vec![&b]), (&local, vec![&a]), (&other, vec![&c])];
        // Unfolded, the group draws its copies' sessions together, latest first.
        assert_eq!(drawn_order(listed(), &HashSet::new()), ["b", "a", "c"]);
        // Folded by its lead (the local copy), neither copy's sessions are drawn, so a range can't pick them.
        let folded = HashSet::from(["local".to_string()]);
        assert_eq!(drawn_order(listed(), &folded), ["c"]);
        let mut picked = Picked::default();
        picked.range("c", &drawn_order(listed(), &folded), Some("b"));
        assert_eq!(sorted(&picked), ["c"]);
    }

    #[test]
    fn a_range_follows_the_groups_whatever_the_store_order() {
        let (cloud, local) = (project("x-cloud", Some("m1")), project("x-local", None));
        let mut other = project("y", None);
        other.remotes = vec!["https://github.com/acme/other".into()];
        let (a, b, c) = (
            session("a", "x-cloud", 3),
            session("b", "y", 2),
            session("c", "x-local", 1),
        );
        // Store order: X's cloud copy, Y, X's local copy. The sidebar draws X (both copies), then Y.
        let listed = vec![(&cloud, vec![&a]), (&other, vec![&b]), (&local, vec![&c])];
        let order = drawn_order(listed, &HashSet::new());
        assert_eq!(order, ["a", "c", "b"]);
        // From X's first row to its last: Y stays out.
        let mut picked = Picked::default();
        picked.range("a", &order, None);
        picked.range("c", &order, None);
        assert_eq!(sorted(&picked), ["a", "c"]);
    }

    fn order() -> Vec<String> {
        ["a", "b", "c", "d", "e"].map(String::from).to_vec()
    }

    fn sorted(picked: &Picked) -> Vec<&str> {
        let mut ids: Vec<&str> = picked.ids.iter().map(String::as_str).collect();
        ids.sort();
        ids
    }

    #[test]
    fn toggles_one_row_in_and_out() {
        let mut picked = Picked::default();
        picked.toggle("b");
        picked.toggle("d");
        assert_eq!(sorted(&picked), ["b", "d"]);
        picked.toggle("b");
        assert_eq!(sorted(&picked), ["d"]);
    }

    #[test]
    fn ranges_either_way_round_and_redraws_from_the_anchor() {
        let mut picked = Picked::default();
        picked.toggle("b");
        picked.range("d", &order(), None);
        assert_eq!(sorted(&picked), ["b", "c", "d"]);
        picked.range("a", &order(), None);
        assert_eq!(sorted(&picked), ["a", "b"]);
    }

    #[test]
    fn range_without_an_anchor_starts_at_the_open_session_else_picks_one() {
        let mut picked = Picked::default();
        picked.range("c", &order(), Some("a"));
        assert_eq!(sorted(&picked), ["a", "b", "c"]);
        let mut picked = Picked::default();
        picked.range("c", &order(), None);
        assert_eq!(sorted(&picked), ["c"]);
        let mut picked = Picked::default();
        picked.anchor = Some("gone".into());
        picked.range("b", &order(), Some("d"));
        assert_eq!(sorted(&picked), ["b", "c", "d"]);
    }
}
