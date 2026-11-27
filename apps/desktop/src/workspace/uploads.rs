//! A sent message's large files while they go up in pieces: each such file's chip shows how much has gone, and its ✕ cancels the send. The pieces and
//! the counting are `nexus_workbench::attachment_upload`'s; this file shows them and puts things back after a cancel.
//!
//! A cancel stops the upload and deletes what went up; the message is not sent. Every file
//! stays: in an open session the text and all the files go back to the composer, and a new session's start fails
//! with the cancel, its Try again and Edit still holding them all.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::attachment_upload::{CANCELLED, Outcome, UploadRun};

use super::{View, Workspace};
use crate::attachments::Draft;
use crate::palette::Palette;

/// How often the chips read the upload's progress while it runs.
const POLL: Duration = Duration::from_millis(100);

/// What the composer shows after the person sends files in pieces, until the upload ends.
pub(super) struct Uploading {
    /// The Code view the message was sent from: a session's id, or a new session's draft id.
    view: String,
    text: String,
    draft: Draft,
    run: Arc<UploadRun>,
}

/// After a cancel or a failure: every one of the message's files, ready to send again.
fn kept(mut draft: Draft) -> Draft {
    draft.upload = None;
    draft
}

/// The text of a message that did not go, back in the composer ahead of what was typed since; None when it had none.
fn restored(sent: &str, typed: &str) -> Option<String> {
    if sent.trim().is_empty() {
        return None;
    }
    Some(if typed.trim().is_empty() {
        sent.to_string()
    } else {
        format!("{sent}\n\n{typed}")
    })
}

/// `front`'s attachments ahead of `back`'s, as the composer shows them.
fn ahead(front: Draft, back: Draft) -> Draft {
    Draft {
        images: front.images.into_iter().chain(back.images).collect(),
        files: front.files.into_iter().chain(back.files).collect(),
        upload: None,
    }
}

impl Workspace {
    /// Give `draft` an upload the chips follow when a file goes in pieces. Call [`Workspace::upload_view`] once the
    /// view that sent it is open.
    pub(super) fn begin_upload(&mut self, draft: &mut Draft, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !draft.sends_in_pieces() {
            return;
        }
        let run = Arc::new(UploadRun::default());
        draft.upload = Some(run.clone());
        self.uploading = Some(Uploading {
            view: String::new(),
            text: text.to_string(),
            draft: draft.clone(),
            run: run.clone(),
        });
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let running = this.update_in(cx, |this, window, cx| {
                    let ours = this.uploading.as_ref().is_some_and(|up| Arc::ptr_eq(&up.run, &run));
                    if !ours {
                        return false;
                    }
                    if let Some(outcome) = run.outcome() {
                        this.upload_ended(outcome, window, cx);
                    }
                    cx.notify();
                    run.outcome().is_none()
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// The upload belongs to the Code view open now.
    pub(super) fn upload_view(&mut self) {
        if let (Some(up), View::Code(view)) = (&mut self.uploading, &self.view) {
            if up.view.is_empty() {
                up.view = view.id.clone();
            }
        }
    }

    /// Whether a send of files in pieces still runs: the composer waits for it.
    pub(super) fn uploading(&self) -> bool {
        self.uploading.is_some()
    }

    /// A cloud start can fail before uploading begins, so its tracker has no outcome yet.
    pub(super) fn fail_start_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.upload_ended(Outcome::Failed, window, cx);
    }

    /// A new session's start whose files have not finished going up: a failure of the start ends them too. A local
    /// start can fail before its upload begins (the worktree's bases), and then its tracker never gets an outcome.
    pub(super) fn start_upload_unsettled(&self) -> bool {
        self.uploading
            .as_ref()
            .is_some_and(|up| up.view.starts_with("draft-") && up.run.outcome().is_none())
    }

    fn upload_ended(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        let Some(up) = self.uploading.take() else {
            return;
        };
        if outcome == Outcome::Uploaded {
            return;
        }
        let open = matches!(&self.view, View::Code(view) if view.id == up.view);
        if !open {
            // Another view is open: a session message's text goes to the clipboard, so it is never lost. A new
            // session's start keeps its own text (its Try again and Edit).
            if !up.view.starts_with("draft-") && !up.text.trim().is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(up.text));
                self.composer_note = Some("The files did not send; the message's text is on the clipboard.".into());
            }
            return;
        }
        let View::Code(view) = &mut self.view else {
            return;
        };
        // A new session that has not started: its start fails with the cancel, and keeps its files.
        if let Some(pending) = &mut view.pending {
            let draft = std::mem::take(&mut pending.new.attachments);
            pending.new.attachments = kept(draft);
            return;
        }
        self.draft = ahead(kept(up.draft), std::mem::take(&mut self.draft));
        // What was typed since stays, under the message's text.
        let typed = self.composer.read(cx).value().to_string();
        if let Some(value) = restored(&up.text, &typed) {
            self.composer.update(cx, |state, cx| state.set_value(value, window, cx));
            self.composer_empty = false;
        }
        if matches!(outcome, Outcome::Cancelled(_)) {
            self.composer_note = Some(CANCELLED.into());
        }
    }

    /// A message the core did not take (`CodeEvent::Unsent`): its text and files go back to the composer of its open
    /// session, ahead of what was typed since, with the reason under it. With another session open, the text goes to
    /// the clipboard instead, so it is never lost. Files that failed in pieces were put back already (`upload_ended`).
    pub(super) fn unsent(
        &mut self,
        thread_id: &str,
        text: String,
        attachments: Draft,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let upload = attachments.upload.as_ref().and_then(|run| run.outcome());
        if let Some(outcome) = upload
            && outcome != Outcome::Uploaded
        {
            if !matches!(outcome, Outcome::Cancelled(_)) {
                self.composer_note = Some(error.into());
            }
            return;
        }
        let open = matches!(&self.view, View::Code(view) if view.id == thread_id && view.pending.is_none());
        if !open {
            if !text.trim().is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            self.composer_note =
                Some(format!("{error} The message was not sent; its text is on the clipboard.").into());
            return;
        }
        self.draft = ahead(kept(attachments), std::mem::take(&mut self.draft));
        let typed = self.composer.read(cx).value().to_string();
        if let Some(value) = restored(&text, &typed) {
            self.composer.update(cx, |state, cx| state.set_value(value, window, cx));
            self.composer_empty = false;
        }
        self.composer_note = Some(format!("{error} The message was not sent; it is back in the composer.").into());
    }

    /// The sent message's files, over the composer, while they go up.
    pub(super) fn render_uploading(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let up = self.uploading.as_ref()?;
        let View::Code(view) = &self.view else {
            return None;
        };
        if view.id != up.view {
            return None;
        }
        let workspace = cx.weak_entity();
        let cancel: Rc<dyn Fn(u64, &mut Window, &mut App)> = Rc::new(move |id, _, cx| {
            let _ = workspace.update(cx, |this, cx| {
                if let Some(up) = &this.uploading {
                    up.run.cancel(id);
                }
                cx.notify();
            });
        });
        Some(
            div()
                .pt(px(14.))
                .pr(px(12.))
                .pl(px(16.))
                .child(super::composer::attachment_chips(
                    "uploading",
                    &up.draft,
                    palette,
                    None,
                    Some(cancel),
                    None,
                ))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{ahead, kept};
    use crate::attachments::{Attachment, Draft};

    fn file(id: u64, name: &str) -> Attachment {
        Attachment {
            artifact: None,
            id,
            name: name.into(),
            bytes: Arc::new(vec![0; 4]),
            media_type: None,
            thumbnail: None,
        }
    }

    /// A cancel keeps every file, the cancelled one too: ready to send again.
    #[test]
    fn a_cancel_keeps_every_file() {
        let draft = Draft {
            images: vec![],
            files: vec![file(1, "a.bin"), file(2, "b.bin")],
            upload: Some(Arc::default()),
        };
        let kept = kept(draft);
        assert_eq!(kept.files.iter().map(|file| file.id).collect::<Vec<_>>(), [1, 2]);
        assert!(kept.upload.is_none());
    }

    #[test]
    fn kept_files_go_ahead_of_files_attached_since() {
        let sent = Draft {
            files: vec![file(1, "sent.bin")],
            ..Draft::default()
        };
        let since = Draft {
            files: vec![file(2, "new.bin")],
            ..Draft::default()
        };
        let names: Vec<_> = ahead(sent, since).files.into_iter().map(|file| file.name).collect();
        assert_eq!(names, ["sent.bin", "new.bin"]);
    }

    fn workspace(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (gpui_kit::Entity<super::Workspace>, &mut gpui_kit::VisualTestContext) {
        use super::super::{Workspace, machine_source::MachineSource};

        cx.update(gpui_kit::init);
        let (gateway, _gateway_events) = crate::gateway::Gateway::start();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let gateway: Arc<dyn crate::backend::Backend> = Arc::new(gateway);
        cx.add_window_view(|window, cx| {
            let mut workspace =
                Workspace::with_machine_source(gateway, events, arrivals, false, MachineSource::isolated(), window, cx);
            // No Code store poll: its reads run on the process-wide `nexus-code` runtime, outside the test scheduler.
            workspace._code_poll = gpui_kit::Task::ready(());
            workspace
        })
    }

    /// A cancelled send of large files puts its text back even when the next message was typed meanwhile,
    /// ahead of it, as a message the core did not take does (`unsent`). It dropped the sent text.
    #[gpui_kit::test]
    fn a_cancel_keeps_the_sent_text_ahead_of_what_was_typed_since(cx: &mut gpui_kit::TestAppContext) {
        use super::super::{View, code_view::CodeView};
        use nexus_workbench::attachment_upload::Outcome;

        let (workspace, cx) = workspace(cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.view = View::Code(CodeView::new("thread-1".into(), window, cx));
            workspace
                .composer
                .update(cx, |state, cx| state.set_value("the next one", window, cx));
            workspace.composer_empty = false;
            workspace.uploading = Some(super::Uploading {
                view: "thread-1".into(),
                text: "look at these logs".into(),
                draft: Draft {
                    files: vec![file(1, "big.log")],
                    ..Draft::default()
                },
                run: Arc::default(),
            });
            workspace.upload_ended(Outcome::Cancelled(1), window, cx);

            assert_eq!(
                workspace.composer.read(cx).value().to_string(),
                "look at these logs\n\nthe next one"
            );
            assert_eq!(workspace.draft.files.len(), 1, "the file is back too");
        });
    }

    /// A failed send of large files from a session that is no longer open puts its text on the clipboard,
    /// as a message the core did not take does (`unsent`). It was dropped.
    #[gpui_kit::test]
    fn a_failure_in_a_closed_session_puts_the_text_on_the_clipboard(cx: &mut gpui_kit::TestAppContext) {
        use super::super::View;
        use nexus_workbench::attachment_upload::Outcome;

        let (workspace, cx) = workspace(cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.view = View::Chat;
            workspace.uploading = Some(super::Uploading {
                view: "thread-1".into(),
                text: "look at these logs".into(),
                draft: Draft {
                    files: vec![file(1, "big.log")],
                    ..Draft::default()
                },
                run: Arc::default(),
            });
            workspace.upload_ended(Outcome::Failed, window, cx);

            assert!(!workspace.uploading());
            let copied = cx.read_from_clipboard().and_then(|item| item.text());
            assert_eq!(copied.as_deref(), Some("look at these logs"));
        });
    }

    /// A new local session whose start fails before its files go up (the worktree's bases) ends the
    /// upload with it. It stayed "sending" for good, and every later send said "Files are still sending".
    #[gpui_kit::test]
    fn a_start_that_fails_before_its_upload_ends_the_upload(cx: &mut gpui_kit::TestAppContext) {
        use super::super::{View, code_view::CodeView};
        use crate::code_run::CodeEvent;

        let (workspace, cx) = workspace(cx);
        workspace.update_in(cx, |workspace, window, cx| {
            let draft = Draft {
                files: vec![file(1, "big.log")],
                upload: Some(Arc::default()),
                ..Draft::default()
            };
            let new = workspace.new_session("project".into(), "start here".into(), draft.clone());
            let view = CodeView::pending(new, window, cx);
            let id = view.id.clone();
            workspace.view = View::Code(view);
            workspace.uploading = Some(super::Uploading {
                view: id,
                text: "start here".into(),
                draft,
                run: Arc::default(),
            });
            workspace.code_form.starting = Some(workspace.uploading.as_ref().unwrap().view.clone());
            let draft = workspace.code_form.starting.clone().unwrap();
            workspace.on_code_events(
                vec![CodeEvent::StartFailed {
                    draft,
                    error: "no worktree bases".into(),
                }],
                window,
                cx,
            );

            assert!(
                !workspace.uploading(),
                "a later send is not told files are still sending"
            );
            let View::Code(view) = &workspace.view else {
                panic!("the start's view stays open");
            };
            let pending = view.pending.as_ref().expect("the start shows as failed");
            assert!(pending.failed.is_some());
            assert_eq!(pending.new.attachments.files.len(), 1, "Try again still holds the file");
        });
    }
}
