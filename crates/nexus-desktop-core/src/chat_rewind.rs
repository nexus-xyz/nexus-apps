//! Rewind and Edit on a person's message in a Gateway chat, as Nexus UI's `chat/message-rewind.ts` and
//! `chat/editable-prompt.tsx` do them. A rewind reads every attachment of the message first, so a failed read cuts
//! nothing; then Gateway marks the message and the later ones `rewound`, and they stay, dimmed. Rewind puts the message
//! back in the composer; Edit sends the new text with the same attachments. The shell makes the Gateway calls.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

/// Rewind's confirmation line (`editable-prompt.tsx`).
pub const REWIND_NOTE: &str = "The prompt returns to the composer. Later messages stay visible, dimmed. Saved pages, \
                               past commands, connector changes, and pushes stay unchanged.";

/// The editor's line (`editable-prompt.tsx`).
pub const EDIT_NOTE: &str = "Sending replaces this message and later replies. Saved pages, past commands, connector \
                             changes, and pushes stay unchanged.";

/// What a rewind asked while an answer or another rewind runs says (`session-view.tsx`'s `rewindMessage`).
pub const BUSY: &str = "Wait for the current action to finish.";

/// An attachment of a stored message (`ChatAttachmentRef`): its id, to read its bytes, and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRef {
    pub id: String,
    pub name: Option<String>,
}

impl AttachmentRef {
    /// A stored message's attachments (`attachments`), in order; an entry with no id is left out.
    pub fn list(message: &Value) -> Vec<AttachmentRef> {
        message
            .get("attachments")
            .and_then(Value::as_array)
            .map(|attachments| {
                attachments
                    .iter()
                    .filter_map(|attachment| {
                        let id = attachment
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())?;
                        Some(AttachmentRef {
                            id: id.to_string(),
                            name: attachment
                                .get("name")
                                .and_then(Value::as_str)
                                .filter(|name| !name.is_empty())
                                .map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The editor's line on a message's attachments: "Attachments stay with this message: a.png, Image". `None` without
/// any.
pub fn attachments_note(attachments: &[AttachmentRef]) -> Option<String> {
    if attachments.is_empty() {
        return None;
    }
    let names: Vec<&str> = attachments
        .iter()
        .map(|attachment| attachment.name.as_deref().unwrap_or("Image"))
        .collect();
    Some(format!("Attachments stay with this message: {}", names.join(", ")))
}

/// Whether a message takes Rewind and Edit now (`editable-prompt.tsx`'s `enabled`): stored (it has Gateway's id), not
/// rewound, and nothing else running (an answer, another rewind).
pub fn can_act(stored: bool, rewound: bool, busy: bool) -> bool {
    stored && !rewound && !busy
}

/// Whether the editor may send (`editable-prompt.tsx`'s Save & send): some text or an attachment, and not sending.
pub fn can_send_edit(text: &str, attachments: usize, busy: bool) -> bool {
    !busy && (!text.trim().is_empty() || attachments > 0)
}

/// An attachment read back (`GET /v1/chat/sessions/{id}/attachments/{attachmentId}`), ready for the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredFile {
    pub name: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// A message as it goes back to the composer: its text, its images and its data files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoredPrompt {
    pub text: String,
    pub images: Vec<RestoredFile>,
    pub files: Vec<RestoredFile>,
}

/// The prompt a rewind gives back (`restoredPrompt`), from the message's text and each attachment as Gateway answered
/// it, in order. One failed read fails the whole prompt, so the rewind does not cut a message it cannot give back.
pub fn restored_prompt(text: &str, attachments: Vec<Result<Value, String>>) -> Result<RestoredPrompt, String> {
    let mut prompt = RestoredPrompt {
        text: text.to_string(),
        ..RestoredPrompt::default()
    };
    for attachment in attachments {
        let attachment = attachment?;
        let media_type = attachment
            .get("mediaType")
            .and_then(Value::as_str)
            .filter(|media_type| !media_type.is_empty())
            .ok_or("Gateway sent an attachment with no type.")?;
        let bytes = attachment
            .get("data")
            .and_then(Value::as_str)
            .and_then(|data| STANDARD.decode(data).ok())
            .ok_or("Gateway sent an attachment that does not read.")?;
        let file = RestoredFile {
            name: attachment
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("Attachment")
                .to_string(),
            media_type: media_type.to_string(),
            bytes,
        };
        if file.media_type.starts_with("image/") {
            prompt.images.push(file);
        } else {
            prompt.files.push(file);
        }
    }
    Ok(prompt)
}

/// The composer's text once a rewound message comes back (`mergeRestoredPrompt`): what is already typed stays first.
pub fn restored_text(draft: &str, prompt: &str) -> String {
    if draft.trim().is_empty() {
        prompt.to_string()
    } else {
        format!("{}\n\n{prompt}", draft.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn attachment(id: &str, name: &str, media_type: &str) -> Value {
        json!({ "id": id, "name": name, "mediaType": media_type, "bytes": 3, "sessionId": "session", "messageId": "message", "data": "YWJj" })
    }

    /// `message-rewind.test.ts`: "restores every image and data file before rewinding".
    #[test]
    fn restores_every_image_and_data_file_before_rewinding() {
        let prompt = restored_prompt(
            "Original",
            vec![
                Ok(attachment("image", "picture.png", "image/png")),
                Ok(attachment("file", "notes.txt", "text/plain")),
            ],
        )
        .expect("a prompt");
        assert_eq!(prompt.text, "Original");
        assert_eq!(prompt.images.len(), 1);
        assert_eq!(prompt.images[0].name, "picture.png");
        assert_eq!(prompt.images[0].media_type, "image/png");
        assert_eq!(prompt.images[0].bytes, b"abc");
        assert_eq!(prompt.files.len(), 1);
        assert_eq!(prompt.files[0].media_type, "text/plain");
        assert_eq!(restored_text("Unsent draft", &prompt.text), "Unsent draft\n\nOriginal");
    }

    /// `message-rewind.test.ts`: "fails without offering a partial prompt when an attachment cannot load".
    #[test]
    fn fails_without_a_partial_prompt_when_an_attachment_cannot_load() {
        let failed = restored_prompt(
            "Original",
            vec![
                Ok(attachment("image", "picture.png", "image/png")),
                Err("attachment unavailable".into()),
            ],
        );
        assert_eq!(failed, Err("attachment unavailable".into()));
        let unreadable = restored_prompt("Original", vec![Ok(json!({ "mediaType": "image/png", "data": "%%" }))]);
        assert!(unreadable.is_err());
    }

    #[test]
    fn a_nameless_attachment_is_named_and_a_message_without_any_restores_its_text() {
        let prompt =
            restored_prompt("", vec![Ok(json!({ "mediaType": "text/csv", "data": "YQ==" }))]).expect("a prompt");
        assert_eq!(prompt.files[0].name, "Attachment");
        assert_eq!(restored_prompt("Hi", Vec::new()).expect("a prompt").text, "Hi");
    }

    #[test]
    fn typed_text_stays_first() {
        assert_eq!(restored_text("", "fix the bug"), "fix the bug");
        assert_eq!(restored_text("  ", "fix the bug"), "fix the bug");
        assert_eq!(restored_text("also this \n", "fix the bug"), "also this\n\nfix the bug");
    }

    #[test]
    fn reads_a_messages_attachments_and_says_they_stay() {
        let message =
            json!({ "attachments": [{ "id": "a1", "name": "chart.png" }, { "id": "a2" }, { "name": "no id" }] });
        let refs = AttachmentRef::list(&message);
        assert_eq!(refs.len(), 2);
        assert_eq!(
            refs[1],
            AttachmentRef {
                id: "a2".into(),
                name: None
            }
        );
        assert_eq!(
            attachments_note(&refs).as_deref(),
            Some("Attachments stay with this message: chart.png, Image")
        );
        assert_eq!(attachments_note(&[]), None);
        assert!(AttachmentRef::list(&json!({})).is_empty());
    }

    #[test]
    fn rewind_and_edit_wait_for_a_stored_message_and_a_quiet_chat() {
        assert!(can_act(true, false, false));
        assert!(!can_act(false, false, false));
        assert!(!can_act(true, true, false));
        assert!(!can_act(true, false, true));
        assert!(can_send_edit("new text", 0, false));
        assert!(can_send_edit("  ", 1, false));
        assert!(!can_send_edit("  ", 0, false));
        assert!(!can_send_edit("new text", 0, true));
    }
}
