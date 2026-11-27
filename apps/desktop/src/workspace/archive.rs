//! Archiving Code sessions with a way back, as the Nexus app's (`archive.ts`): the notice at the foot of the window
//! says what was archived and offers Undo for `notice::UNDO`, which restores them. Every place that archives comes
//! here: the row's and the top bar's menu and the palette (through `change_session`), and the multi-select bar.
//!
//! A cloud session archives by deleting its machine, so it has no Undo; it goes through `end_cloud`, which
//! asks first when the machine holds commits on no remote.

use futures::channel::oneshot;
use futures::future::join_all;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::notice::{self, archived_text, restore_failed_text};
use nexus_workbench::api::UpdateArgs;
use serde_json::{Value, json};

use super::Workspace;
use super::notice_host::NoticeDo;
use crate::cloud::words::End;

type Answer = oneshot::Receiver<Result<Value, String>>;

impl Workspace {
    /// Archive sessions `ids`: a cloud one with its machine, the others with Undo.
    pub(super) fn archive_sessions(&mut self, ids: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let mut here = Vec::new();
        for id in ids {
            match self.cloud_machine_for(&id) {
                Some(machine) => self.end_cloud(&id, &machine, End::Archive, false, window, cx),
                None => here.push(id),
            }
        }
        self.archive_with_undo(here, cx);
    }

    /// Archive the sessions among `ids` not archived yet, then offer Undo for the ones that were (`archiveSessions`).
    /// The first failure shows over the composer; the others still archive.
    pub(super) fn archive_with_undo(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        let mut asked = Vec::new();
        for id in ids {
            let Some(session) = self.session_by_id(&id).filter(|session| !session.archived) else {
                continue;
            };
            let title = session.title.clone();
            let Some(runs) = self.runs(&id) else {
                self.refuse_read_only();
                continue;
            };
            let answer = runs.call("wb_thread_update", archived(&id, true));
            asked.push(((id, title), answer));
        }
        cx.notify();
        if asked.is_empty() {
            return;
        }
        self.after(asked, cx, |this, done, titles, failed, cx| {
            if !done.is_empty() {
                this.show_notice(
                    archived_text(&titles),
                    Some(("Undo", NoticeDo::Restore(done))),
                    notice::UNDO,
                    cx,
                );
            }
            if let Some(error) = failed {
                this.composer_note = Some(error.into());
            }
        });
    }

    /// Undo: back from the archive. A failure shows in the notice, since the button that asked is gone.
    pub(super) fn restore_sessions(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        let asked: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                let answer = self.runs(&id)?.call("wb_thread_update", archived(&id, false));
                Some(((id, String::new()), answer))
            })
            .collect();
        self.after(asked, cx, |this, _, _, failed, cx| {
            if let Some(error) = failed {
                this.show_notice(restore_failed_text(&error), None, notice::NOTICE, cx);
            }
        });
    }

    /// Once every call in `asked` answers: the ids and titles that changed, and the first failure. The sessions are
    /// read again either way.
    fn after(
        &mut self,
        asked: Vec<((String, String), Answer)>,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Vec<String>, Vec<String>, Option<String>, &mut Context<Self>) + 'static,
    ) {
        let (sessions, answers): (Vec<_>, Vec<_>) = asked.into_iter().unzip();
        cx.spawn(async move |this, cx| {
            let answers = join_all(answers).await;
            let _ = this.update(cx, |this, cx| {
                let (mut done, mut titles, mut failed) = (Vec::new(), Vec::new(), None);
                for ((id, title), answer) in sessions.into_iter().zip(answers) {
                    match answer {
                        Ok(Ok(_)) => {
                            done.push(id);
                            titles.push(title);
                        }
                        Ok(Err(error)) => {
                            failed.get_or_insert(error);
                        }
                        Err(_) => {
                            failed.get_or_insert("The command's answer was lost.".to_string());
                        }
                    }
                }
                then(this, done, titles, failed, cx);
                this.refresh_code(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

/// `wb_thread_update`'s arguments that archive session `id`, or bring it back.
fn archived(id: &str, archived: bool) -> Value {
    json!(UpdateArgs {
        thread_id: id.to_string(),
        archived: Some(archived),
        ..UpdateArgs::default()
    })
}
