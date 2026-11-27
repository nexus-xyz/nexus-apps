//! Resolve saved chat references without blocking the transcript or resending an incomplete prompt.
use std::sync::atomic::Ordering;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::chat_rewind::{AttachmentRef, restored_prompt};
use serde_json::Value;

use super::{Workspace, chat_rewind, transcript::Entry};
use crate::attachments::{self, Draft};
use crate::backend::Query;
use crate::i18n::tr;
use crate::model::Role;
use crate::palette::Palette;

#[derive(Default, Hash)]
pub(super) enum Load {
    #[default]
    Ready,
    Loading,
    Failed(Vec<String>),
}
impl Load {
    pub fn ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Decode each attachment independently so one unavailable file does not hide the others.
fn restore(reads: Vec<(AttachmentRef, Result<Value, String>)>) -> (Draft, Load) {
    let mut draft = Draft::default();
    let mut errors = Vec::new();
    for (reference, mut answer) in reads {
        if let Ok(Value::Object(value)) = &mut answer
            && value.get("name").and_then(Value::as_str).is_none_or(str::is_empty)
            && let Some(name) = &reference.name
        {
            value.insert("name".into(), Value::String(name.clone()));
        }
        let result = restored_prompt("", vec![answer]).and_then(|prompt| {
            let prepared = attachments::prepare(chat_rewind::incoming(&prompt));
            attachments::merge(&mut draft, prepared).map_or(Ok(()), Err)
        });
        if let Err(error) = result {
            errors.push(format!(
                "{}: {error}",
                reference
                    .name
                    .as_deref()
                    .map_or_else(|| tr("Attachment").to_string(), str::to_string)
            ));
        }
    }
    if let Some(problem) = attachments::chat_problem(&draft) {
        errors.push(problem);
    }
    let load = if errors.is_empty() {
        Load::Ready
    } else {
        Load::Failed(errors)
    };
    (draft, load)
}

/// Capture every query's account before yielding. Only the original entry may receive its bytes.
fn start(entry: &mut Entry, queries: Vec<Query>, cx: &mut Context<Workspace>) {
    entry.attachment_load = Load::Loading;
    let id = entry.id;
    let references = entry.stored.attachments.clone();
    let generations: Vec<_> = queries.iter().filter_map(|query| query.generation.clone()).collect();
    cx.spawn(async move |this, cx| {
        let answers = futures::future::join_all(queries).await;
        let reads = references.into_iter().zip(answers).collect();
        // Image decoding and thumbnail creation stay off the UI thread.
        let (draft, load) = cx.background_executor().spawn(async move { restore(reads) }).await;
        let _ = this.update(cx, |this, cx| {
            if generations
                .iter()
                .any(|(epoch, expected)| epoch.load(Ordering::SeqCst) != *expected)
            {
                return;
            }
            // Entries have fresh ids after each open, including warm-cache refreshes. A live chat keeps its ids
            // while detached, so its files can still finish loading away from the screen.
            if let Some(entry) = this.chat_entry_mut(id) {
                // A referenced artifact's source came as a data file: its chip already shows it.
                let mut draft = draft;
                let sources: Vec<_> = entry
                    .attachments
                    .artifacts()
                    .iter()
                    .map(|reference| reference.source_file())
                    .collect();
                draft.files.retain(|file| !sources.contains(&file.name));
                let artifacts = entry.attachments.files.iter().filter(|file| file.artifact.is_some());
                draft.files.extend(artifacts.cloned());
                entry.attachments = draft;
                entry.attachment_load = load;
                cx.notify();
            }
        });
    })
    .detach();
}

impl Workspace {
    pub(super) fn restore_chat_attachments(&self, entry: &mut Entry, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if entry.role != Role::User || entry.stored.attachments.is_empty() {
            return;
        }
        let Some(session) = self.current.as_deref() else { return };
        let queries = entry
            .stored
            .attachments
            .iter()
            .map(|reference| {
                account_gateway.query(format!(
                    "/v1/chat/sessions/{}/attachments/{}",
                    nexus_client::url::enc(session),
                    nexus_client::url::enc(&reference.id),
                ))
            })
            .collect();
        start(entry, queries, cx);
    }

    fn reload_chat_attachments(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(session) = self.current.as_deref() else { return };
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return;
        };
        if !matches!(entry.attachment_load, Load::Failed(_)) {
            return;
        }
        let queries = entry
            .stored
            .attachments
            .iter()
            .map(|reference| {
                account_gateway.query(format!(
                    "/v1/chat/sessions/{}/attachments/{}",
                    nexus_client::url::enc(session),
                    nexus_client::url::enc(&reference.id),
                ))
            })
            .collect();
        start(entry, queries, cx);
        cx.notify();
    }

    pub(super) fn render_attachment_load(
        &self,
        entry: &Entry,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = entry.id;
        let text = match &entry.attachment_load {
            Load::Ready => return None,
            Load::Loading => {
                let names: Vec<_> = entry
                    .stored
                    .attachments
                    .iter()
                    .map(|file| {
                        file.name
                            .as_deref()
                            .map_or_else(|| tr("Attachment").to_string(), str::to_string)
                    })
                    .collect();
                tr("Loading attachments: {names}").replace("{names}", &names.join(", "))
            }
            Load::Failed(errors) => {
                tr("Some attachments are unavailable. {errors}").replace("{errors}", &errors.join(" "))
            }
        };
        Some(
            v_flex()
                .max_w_full()
                .gap(px(4.))
                .text_size(px(12.))
                .text_color(palette.muted)
                .child(text)
                .when(matches!(entry.attachment_load, Load::Failed(_)), |column| {
                    column.child(
                        h_flex()
                            .id(("reload-chat-attachments", id))
                            .debug_selector(|| "reload-chat-attachments".into())
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.hover))
                            .on_click(cx.listener(move |this, _, _, cx| this.reload_chat_attachments(id, cx)))
                            .child(tr("Retry attachments")),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "chat_attachments_tests.rs"]
mod tests;
