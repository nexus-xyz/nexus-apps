//! The workbench's notice at the foot of the window, as the Nexus app's (`notice-host.tsx`, `.wb-toast`): one line,
//! its action (Undo after an archive) when it has one, and Dismiss. One at a time, and a newer one replaces the older
//! (`nexus_desktop_core::notice`). It stays over the foot of whichever page is open, so it outlives the menu or the
//! session that raised it.

use std::time::Duration;

use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::notice::Notices;

use super::Workspace;
use crate::pages::ui;
use crate::palette::Palette;

/// What a notice's action does, run by this window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeDo {
    /// Undo: bring these sessions back from the archive (`restoreSessions`).
    Restore(Vec<String>),
    /// Restore only the command receipts deleted by this notice's batch.
    RestoreCommands(Entity<crate::side::shared_terminals::SharedTerminals>, Vec<String>),
    /// Undo: put back what the dropped sessions changed (`session_drop_undo.rs`).
    UndoDrop(Vec<super::session_drop_undo::DropUndo>),
    ForceDelete {
        session: String,
        title: String,
    },
    ConfirmDelete {
        session: String,
        dismiss: FocusHandle,
    },
}

impl NoticeDo {
    fn is_delete(&self) -> bool {
        matches!(self, Self::ForceDelete { .. } | Self::ConfirmDelete { .. })
    }
}

/// The notice's state in one app window.
pub type NoticeState = Notices<NoticeDo>;

impl Workspace {
    /// Show `text` at the foot of the window for `stay`, with `action`'s button when given, in place of any notice on
    /// screen. Answers the notice's id, for its owner to take it away early.
    pub(super) fn show_notice(
        &mut self,
        text: impl Into<String>,
        action: Option<(&str, NoticeDo)>,
        stay: Duration,
        cx: &mut Context<Self>,
    ) -> u64 {
        let id = self
            .notices
            .show(text, action.map(|(label, action)| (label.to_string(), action)));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(stay).await;
            let _ = this.update(cx, |this, cx| {
                // A newer notice is not this one's to take away.
                if this.notices.dismiss(Some(id)) {
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
        id
    }

    pub(super) fn show_delete_warning(&mut self, session: String, title: String, cx: &mut Context<Self>) {
        let text = format!("“{title}” has uncommitted changes or unpushed commits.");
        self.notices.show(
            text,
            Some(("Force delete".into(), NoticeDo::ForceDelete { session, title })),
        );
        cx.notify();
    }

    pub(super) fn dismiss_delete_notice(&mut self) {
        if let Some(notice) = self.notices.current()
            && notice.action.as_ref().is_some_and(|(_, action)| action.is_delete())
        {
            self.notices.dismiss(Some(notice.id));
        }
    }

    fn run_notice_action(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        match self.notices.run_action(id) {
            Some(NoticeDo::Restore(ids)) => self.restore_sessions(ids, cx),
            Some(NoticeDo::RestoreCommands(source, ids)) => {
                source.update(cx, |source, cx| source.undo_delete(ids, cx));
            }
            Some(NoticeDo::UndoDrop(undo)) => self.undo_drop(undo, cx),
            Some(NoticeDo::ForceDelete { session, title }) => {
                let dismiss = cx.focus_handle();
                window.focus(&dismiss, cx);
                self.notices.show(
                    format!("This deletes “{title}” and its worktree, including uncommitted changes and any nested repositories or submodules. Local-only commits in nested repositories or on a detached HEAD can be lost."),
                    Some(("Confirm delete".into(), NoticeDo::ConfirmDelete { session, dismiss })),
                );
            }
            Some(NoticeDo::ConfirmDelete { session, .. }) => self.delete_session_on_runner(session, true, window, cx),
            None => {}
        }
        cx.notify();
    }

    /// The notice over the foot of the window, centred, with its action and Dismiss.
    pub(super) fn render_notice(&mut self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active = self.notices.current().is_some();
        let opacity = self.notice_motion.sample(active, cx);
        if let Some(notice) = self.notices.current() {
            self.closing_notice = Some(notice.clone());
        }
        if !active && !self.notice_motion.visible() {
            self.closing_notice = None;
            return None;
        }
        let notice = self.closing_notice.as_ref()?;
        let id = notice.id;
        let card = h_flex()
            .id("wb-notice")
            .opacity(opacity)
            .relative()
            .top(px(6. * (1. - opacity)))
            .occlude()
            .max_w(px(560.))
            .gap(px(12.))
            .py(px(8.))
            .px(px(12.))
            .rounded(px(12.))
            .bg(palette.pop)
            .border_1()
            .border_color(palette.border)
            .shadow(palette.pop_shadow(palette.border))
            .text_size(px(13.))
            .text_color(palette.muted)
            .child(
                h_flex()
                    .min_w(px(0.))
                    .gap(px(8.))
                    // The notice's dot (`.wb-notice::before`).
                    .child(div().flex_none().size(px(5.)).rounded_full().bg(palette.faint))
                    .child(div().min_w(px(0.)).child(SharedString::from(notice.text.clone()))),
            )
            .children(notice.action.as_ref().map(|(label, action)| {
                let label = SharedString::from(label.clone());
                let destructive = action.is_delete();
                let button = if destructive {
                    ui::danger_button("wb-notice-action", label, palette)
                } else {
                    ui::button("wb-notice-action", label, palette)
                };
                button.when(active, |button| {
                    button.on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if !destructive || event.click_count() == 1 {
                            this.run_notice_action(id, window, cx);
                        }
                    }))
                })
            }))
            .child(
                ui::button("wb-notice-dismiss", "Dismiss", palette)
                    .when_some(
                        notice.action.as_ref().and_then(|(_, action)| match action {
                            NoticeDo::ConfirmDelete { dismiss, .. } => Some(dismiss),
                            _ => None,
                        }),
                        |button, dismiss| {
                            button
                                .track_focus(dismiss)
                                .border_1()
                                .border_color(transparent_black())
                                .focus(|style| style.border_color(palette.muted))
                        },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.notices.dismiss(Some(id));
                        cx.notify();
                    })),
            );
        Some(
            deferred(
                h_flex()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(16.))
                    .px(px(16.))
                    .justify_center()
                    .child(card),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn workspace(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
        cx.update(gpui_kit::init);
        let _agents = crate::code_run::fixture_agents(Vec::new());
        let (gateway, _events) = crate::gateway::Gateway::start();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::with_machine_source(
                std::sync::Arc::new(gateway),
                events,
                arrivals,
                false,
                crate::workspace::machine_source::MachineSource::isolated(),
                window,
                cx,
            );
            workspace._code_poll = Task::ready(());
            workspace
        })
    }

    #[gpui_kit::test]
    fn dirty_worktree_delete_uses_the_warning_for_both_presses(cx: &mut TestAppContext) {
        let (workspace, cx) = workspace(cx);
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        workspace.update_in(cx, |workspace, window, cx| {
            let sent = requests.clone();
            let runs = super::super::runs::Runs::deferred(
                "test-machine",
                std::sync::Arc::new(move |command, args| {
                    sent.lock().unwrap().push((command, args));
                    let (send, receive) = futures::channel::oneshot::channel();
                    let _ = send.send(Ok(serde_json::Value::Null));
                    receive
                }),
            );
            workspace.run_fixtures.insert("dirty".into(), runs);
            workspace.show_delete_warning("dirty".into(), "Dirty worktree".into(), cx);
            let warning = workspace.notices.current().unwrap();
            let first = warning.id;
            assert_eq!(warning.action.as_ref().unwrap().0, "Force delete");
            workspace.run_notice_action(first, window, cx);
            let confirmation = workspace.notices.current().unwrap();
            let second = confirmation.id;
            assert_eq!(confirmation.action.as_ref().unwrap().0, "Confirm delete");
            assert!(confirmation.text.contains("submodules"));
            assert!(confirmation.text.contains("detached HEAD"));
            workspace.run_notice_action(first, window, cx);
            assert!(requests.lock().unwrap().is_empty());
            workspace.run_notice_action(second, window, cx);
            assert_eq!(
                *requests.lock().unwrap(),
                vec![(
                    "wb_thread_delete",
                    serde_json::json!({
                        "threadId": "dirty",
                        "discardUncommitted": true,
                        "offerDiscardUncommitted": false,
                    })
                )]
            );
        });
    }

    #[gpui_kit::test]
    fn dismissal_retains_the_notice_for_exit_and_replacement_keeps_the_new_action(cx: &mut TestAppContext) {
        let (workspace, cx) = workspace(cx);
        workspace.update_in(cx, |workspace, _, cx| {
            let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
            let first = workspace.notices.show(
                "Deleted commands",
                Some(("Undo".into(), NoticeDo::Restore(vec!["first".into()]))),
            );
            assert!(workspace.render_notice(palette, cx).is_some());
            assert!(workspace.notices.dismiss(Some(first)));
            let closing = workspace.render_notice(palette, cx);
            if cx.reduce_motion() {
                assert!(closing.is_none());
                assert!(workspace.closing_notice.is_none());
            } else {
                assert!(closing.is_some(), "dismissal must keep the exit frame mounted");
                assert_eq!(workspace.closing_notice.as_ref().unwrap().id, first);
            }
            let second = workspace.notices.show(
                "New deletion",
                Some(("Undo".into(), NoticeDo::Restore(vec!["second".into()]))),
            );
            assert!(workspace.render_notice(palette, cx).is_some());
            assert_eq!(workspace.closing_notice.as_ref().unwrap().id, second);
            assert!(!workspace.notices.dismiss(Some(first)));
            assert_eq!(workspace.notices.run_action(first), None);
            assert_eq!(
                workspace.notices.run_action(second),
                Some(NoticeDo::Restore(vec!["second".into()])),
            );
            assert_eq!(workspace.notices.run_action(second), None);
        });
    }
}
