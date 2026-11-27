//! A shared session's snapshot rules over the provider-neutral workbench log.

use crate::event::{Image, SessionAgent, StoredEvent, ThreadSummary, WbEvent};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub struct Metadata {
    pub id: String,
    pub title: String,
    pub agent: SessionAgent,
    pub model: Option<String>,
    pub cost_usd: f64,
    pub created_at: i64,
}

impl From<&ThreadSummary> for Metadata {
    fn from(thread: &ThreadSummary) -> Self {
        Self {
            id: thread.id.clone(),
            title: thread.title.clone(),
            agent: thread.agent,
            model: thread.model.clone(),
            cost_usd: thread.cost_usd,
            created_at: thread.created_at,
        }
    }
}

pub struct Snapshot {
    pub session: Value,
    pub images: Vec<Value>,
}

fn named(images: &[Image], kept: &mut Vec<Value>) -> Vec<String> {
    images
        .iter()
        .map(|image| {
            let id = format!("img_{}", kept.len() + 1);
            kept.push(json!({"id":id,"data":image.data,"name":image.name}));
            id
        })
        .collect()
}

pub fn snapshot(thread: &ThreadSummary, log: &[StoredEvent], thinking: bool) -> Snapshot {
    snapshot_metadata(&Metadata::from(thread), log, thinking)
}

pub fn snapshot_metadata(thread: &Metadata, log: &[StoredEvent], thinking: bool) -> Snapshot {
    let mut cancelled = HashSet::new();
    let mut edited = HashMap::new();
    for stored in log {
        match &stored.event {
            WbEvent::QueueCancel { seq } => {
                cancelled.insert(*seq);
            }
            WbEvent::QueueEdit { seq, text } => {
                edited.insert(*seq, text.as_str());
            }
            _ => {}
        }
    }
    let mut images = Vec::new();
    let mut events = Vec::new();
    let mut model = thread.model.clone();
    for stored in log {
        if let WbEvent::Session { model: Some(next), .. } = &stored.event {
            model = Some(next.clone());
        }
        let keys: &[&str] = match &stored.event {
            WbEvent::User { .. } if cancelled.contains(&stored.seq) => continue,
            WbEvent::User { .. } => &["kind", "text", "images"],
            WbEvent::Text { .. } => &["kind", "id", "text", "parent"],
            WbEvent::Thinking { .. } if thinking => &["kind", "id", "text", "parent"],
            WbEvent::Tool { .. } => &["kind", "id", "name", "input", "parent"],
            WbEvent::ToolResult { .. } => &["kind", "id", "output", "isError", "truncated", "images", "parent"],
            WbEvent::Approval { .. } => &[
                "kind",
                "requestId",
                "toolUseId",
                "tool",
                "input",
                "description",
                "parent",
            ],
            WbEvent::ApprovalResolved { .. } => &["kind", "requestId", "decision", "message"],
            WbEvent::Usage { .. } => &[
                "kind",
                "model",
                "inputTokens",
                "outputTokens",
                "cacheReadTokens",
                "cacheWriteTokens",
                "costUsd",
                "parent",
            ],
            WbEvent::TurnStart { .. } => &["kind", "turnId"],
            WbEvent::TurnEnd { .. } => &["kind", "turnId", "status", "costUsd", "durationMs", "error"],
            WbEvent::Task { .. } => &["kind", "taskId", "toolUseId", "agentType", "status", "description"],
            WbEvent::Notice { .. } => &["kind", "text"],
            WbEvent::Error { .. } => &["kind", "text", "detail"],
            _ => continue,
        };
        let Ok(Value::Object(mut event)) = serde_json::to_value(&stored.event) else {
            continue;
        };
        if let WbEvent::User { images: source, .. } = &stored.event {
            if let Some(text) = edited.get(&stored.seq) {
                event.insert("text".into(), json!(text));
            }
            let ids = named(source, &mut images);
            event.remove("images");
            if !ids.is_empty() {
                event.insert("images".into(), json!(ids));
            }
        } else if let WbEvent::ToolResult { images: source, .. } = &stored.event {
            let ids = named(source, &mut images);
            event.remove("images");
            if !ids.is_empty() {
                event.insert("images".into(), json!(ids));
            }
        }
        event.retain(|key, value| keys.contains(&key.as_str()) && !value.is_null());
        event.insert("at".into(), json!(stored.at));
        events.push(Value::Object(event));
    }
    let mut session = json!({
        "source":"desktop", "id":thread.id, "title":thread.title,
        "agent":if thread.agent == SessionAgent::Nexus { None } else { Some(thread.agent.as_str()) },
        "model":model,"costUsd":thread.cost_usd,"startedAt":thread.created_at,"events":events,
    });
    if let Some(object) = session.as_object_mut() {
        object.retain(|_, value| !value.is_null());
    }
    Snapshot { session, images }
}

pub fn trim_note(trimmed: nexus_client::session_share::Trim) -> Option<String> {
    let mut parts = Vec::new();
    if trimmed.outputs > 0 {
        parts.push(format!("{} tool results cut short", trimmed.outputs));
    }
    if trimmed.inputs > 0 {
        parts.push(format!("{} strings in tool inputs cut short", trimmed.inputs));
    }
    if trimmed.images > 0 {
        parts.push(format!("{} images left out", trimmed.images));
    }
    if trimmed.events > 0 {
        parts.push(format!("the first {} events left out", trimmed.events));
    }
    (!parts.is_empty()).then(|| format!("To fit, {}.", parts.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread() -> ThreadSummary {
        serde_json::from_value(json!({"id":"t","projectId":"p","title":"Fix sign-in","agent":"claude","model":null,"permissionMode":"default","via":"nexus","status":"idle","costUsd":1.25,"nexusSessions":[],"contextTokens":null,"contextWindow":null,"createdAt":1000,"updatedAt":2000,"archived":false})).expect("a thread")
    }
    fn log(events: Value) -> Vec<StoredEvent> {
        let events: Vec<WbEvent> = serde_json::from_value(events).expect("valid event fixtures");
        events
            .into_iter()
            .enumerate()
            .map(|(ix, event)| StoredEvent {
                thread_id: "t".into(),
                seq: ix as i64 + 1,
                at: 2000 + ix as i64,
                event,
            })
            .collect()
    }
    #[test]
    fn keeps_edits_omits_cancelled_prompts_and_thinking_is_opt_in() {
        let events = log(json!([
            {"kind":"user","text":"before"}, {"kind":"queueEdit","seq":1,"text":"after"},
            {"kind":"user","text":"cancelled"}, {"kind":"queueCancel","seq":3},
            {"kind":"thinking","id":"th","text":"private thought","parent":null},
            {"kind":"text","id":"answer","text":"done","parent":null}
        ]));
        let built = snapshot(&thread(), &events, false);
        assert_eq!(built.session["events"].as_array().expect("events").len(), 2);
        assert_eq!(built.session["events"][0]["text"], "after");
        assert_eq!(
            snapshot(&thread(), &events, true).session["events"]
                .as_array()
                .expect("events")
                .len(),
            3
        );
    }
    #[test]
    fn images_get_named_and_transport_bookkeeping_stays_private() {
        let events = log(json!([
            {"kind":"user","text":"look","images":[{"mediaType":"image/png","data":"AAAA","name":"a.png"}]},
            {"kind":"toolResult","id":"shot","output":"image","isError":false,"truncated":false,"images":[{"mediaType":"image/png","data":"BBBB","name":null}]},
            {"kind":"session","nexusSessionId":"secret-run","nativeId":"private-process","model":"model-new","via":"nexus","resumed":true}
        ]));
        let built = snapshot(&thread(), &events, false);
        assert_eq!(built.images.len(), 2);
        assert_eq!(built.session["events"][0]["images"], json!(["img_1"]));
        assert_eq!(built.session["events"][1]["images"], json!(["img_2"]));
        assert_eq!(built.session["model"], "model-new");
        assert!(!built.session.to_string().contains("private-process"));
        assert!(!built.session.to_string().contains("AAAA"));
    }
    #[test]
    fn the_gateway_fitter_is_shared_with_the_cli() {
        let events =
            log(json!([{"kind":"toolResult","id":"x","output":"x".repeat(100_000),"isError":false,"truncated":false}]));
        let built = snapshot(&thread(), &events, false);
        let (fitted, _, trimmed) =
            nexus_client::session_share::fit_session_snapshot(&built.session, &built.images).expect("fits");
        assert!(fitted["events"][0]["output"].as_str().expect("text").len() <= 64 * 1024);
        assert_eq!(trimmed.outputs, 1);
        assert!(trim_note(trimmed).is_some());
        assert_eq!(trim_note(Default::default()), None);
    }
}
