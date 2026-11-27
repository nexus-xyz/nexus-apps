//! Export a Code session's complete conversation as Markdown.

use crate::workspace::overlay::OverlaySlot;
use gpui_kit::*;
use nexus_workbench::event::{StoredEvent, WbEvent};

use super::Workspace;

impl Workspace {
    pub(super) fn export_code_session(&mut self, id: &str, download: bool, cx: &mut Context<Self>) {
        self.overlay.close_session_menu();
        let title = self
            .session_by_id(id)
            .map(|session| session.title.clone())
            .unwrap_or_default();
        let Some(runs) = self.runs(id) else {
            self.notice = Some(self.read_only_note());
            cx.notify();
            return;
        };
        let request = runs.session_events(id.to_string());
        cx.spawn(async move |this, cx| {
            let events = match request.await {
                Ok(events) => events,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.notice = Some(format!("Couldn't export the session: {error}").into());
                        cx.notify();
                    });
                    return;
                }
            };
            let markdown = conversation_markdown(&title, &events);
            if !download {
                let _ = this.update(cx, |this, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(markdown));
                    this.notice = Some("Session copied to clipboard.".into());
                    cx.notify();
                });
                return;
            }
            let folder = crate::attachments::downloads_dir();
            let name = super::chat_menu::markdown_filename(&title);
            let picked = cx.update(|cx| cx.prompt_for_new_path(&folder, Some(&name)));
            if let Ok(Ok(Some(file))) = picked.await {
                let result = cx
                    .background_executor()
                    .spawn(async move { std::fs::write(&file, markdown) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.notice = Some(match result {
                        Ok(()) => "Session exported.".into(),
                        Err(error) => format!("Couldn't save the session: {error}").into(),
                    });
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }
}

/// Text blocks keep their paragraph breaks. Tool calls and protocol events stay outside the conversation export.
fn conversation_markdown(title: &str, events: &[StoredEvent]) -> String {
    let mut result = format!("# {}\n", title.replace(['\r', '\n'], " "));
    let mut answering = false;
    for stored in events {
        match &stored.event {
            WbEvent::User { text, .. } => {
                if answering {
                    result.push('\n');
                }
                result.push_str(&format!("\n## User\n\n{text}\n"));
                answering = false;
            }
            WbEvent::Text { text, parent, .. } if parent.is_none() => {
                if !answering {
                    result.push_str("\n## Assistant\n\n");
                    answering = true;
                } else {
                    result.push_str("\n\n");
                }
                result.push_str(text);
            }
            WbEvent::TurnEnd { .. } => {
                if answering {
                    result.push('\n');
                }
                answering = false;
            }
            _ => {}
        }
    }
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn exports_messages_with_paragraph_breaks_and_without_internal_events() {
        let events: Vec<StoredEvent> = [
            serde_json::json!({"kind": "user", "text": "Hello"}),
            serde_json::json!({"kind": "text", "id": "answer:0", "text": "Hi there"}),
            serde_json::json!({"kind": "text", "id": "answer:1", "text": "Another paragraph"}),
            serde_json::json!({"kind": "text", "id": "child:0", "parent": "tool", "text": "Internal answer"}),
            serde_json::json!({"kind": "notice", "text": "Internal notice"}),
            serde_json::json!({"kind": "user", "text": "Next"}),
        ]
        .into_iter()
        .enumerate()
        .map(|(seq, event)| {
            serde_json::from_value(serde_json::json!({"threadId": "session", "seq": seq, "at": 0, "event": event}))
                .expect("stored conversation event")
        })
        .collect();
        let markdown = conversation_markdown("A\nTitle", &events);
        assert_eq!(
            markdown,
            "# A Title\n\n## User\n\nHello\n\n## Assistant\n\nHi there\n\nAnother paragraph\n\n## User\n\nNext\n"
        );
    }
}
