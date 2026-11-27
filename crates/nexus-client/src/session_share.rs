//! Fit coding-session snapshots to Gateway limits, shared by the CLI and desktop.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

/// The checked timeline's most bytes, as JSON (`SESSION_SHARE_LIMITS` in the kit, Gateway's `MAX_SESSION_BYTES`).
pub const SESSION_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMAGES: usize = 32;
/// One image, decoded.
const ONE_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// All images together, decoded.
const IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// One tool result's text: the desktop timeline's cap.
const TOOL_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_EVENTS: usize = 20_000;
/// Tool results' caps, longest first: each step is tried until the snapshot fits.
/// The kit's `STEPS`: tool results are cut first, step by step, and tool inputs only once results at 256 bytes are not
/// enough. Each pair is (the cap on a result, the cap on each string of an input, if inputs are cut).
const STEPS: [(usize, Option<usize>); 9] = [
    (TOOL_OUTPUT_BYTES, None),
    (16 * 1024, None),
    (4 * 1024, None),
    (1024, None),
    (256, None),
    (256, Some(16 * 1024)),
    (256, Some(4 * 1024)),
    (256, Some(1024)),
    (256, Some(256)),
];
/// What a cut string of a tool's input ends with, so a reader knows it was shortened (the kit's `CUT_MARK`).
pub const CUT_MARK: &str = " … [cut to fit]";
/// Room left for what Gateway adds: it measures the checked timeline, not this body. That is the image list and the
/// format, plus up to `PER_EVENT` bytes an event for the defaults it fills in (`isError`, `truncated`, token counts).
pub const HEADROOM: usize = 64 * 1024;
pub const PER_EVENT: usize = 64;

/// What [`fit_session_snapshot`] left out to fit: tool results cut, strings in tool inputs cut, images dropped, events
/// dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Trim {
    pub outputs: usize,
    pub inputs: usize,
    pub images: usize,
    pub events: usize,
}

/// `text` cut to at most `max` bytes of UTF-8, at a character boundary.
pub fn cut_utf8(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Every string in a tool's input cut to `max` bytes; the count of strings cut goes to `cut`.
fn cut_strings(value: &Value, max: usize, cut: &mut usize) -> Value {
    match value {
        Value::String(text) => {
            let short = cut_utf8(text, max);
            if short.len() == text.len() {
                return value.clone();
            }
            *cut += 1;
            json!(format!("{short}{CUT_MARK}"))
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| cut_strings(item, max, cut)).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, item)| (key.clone(), cut_strings(item, max, cut)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The decoded size of base64 (or a data: URL).
fn decoded_bytes(data: &str) -> usize {
    let body = data.find(',').map_or(data, |comma| &data[comma + 1..]);
    body.trim_end_matches('=').len() * 3 / 4
}

/// The image ids an event names: `None` when it names none (no `images`, or `null`).
fn named(event: &Value) -> Option<Vec<&str>> {
    let images = event.get("images").filter(|images| !images.is_null())?;
    Some(
        images
            .as_array()
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default(),
    )
}

/// The snapshot inside Gateway's limits (the kit's `fitSessionSnapshot`): at most 20,000 events (the newest), the
/// first 32 images the events name within 20 MB, each at most 5 MB (the others are dropped from the events that name
/// them), and tool
/// results and tool inputs cut shorter, step by step, until the whole is under 4 MB. `Err` is the size at the shortest
/// step, when even that is too large.
pub fn fit_session_snapshot(
    session: &Value,
    images: &[Value],
) -> std::result::Result<(Value, Vec<Value>, Trim), usize> {
    let all = session["events"].as_array().cloned().unwrap_or_default();
    let mut trimmed = Trim {
        events: all.len().saturating_sub(MAX_EVENTS),
        ..Trim::default()
    };
    let events = &all[all.len().saturating_sub(MAX_EVENTS)..];
    let by_id: HashMap<&str, &Value> = images
        .iter()
        .filter_map(|image| Some((image["id"].as_str()?, image)))
        .collect();
    let mut kept: Vec<&Value> = Vec::new();
    let mut kept_ids: HashSet<&str> = HashSet::new();
    let mut every: HashSet<&str> = HashSet::new();
    let mut image_bytes = 0;
    for event in events {
        for id in named(event).unwrap_or_default() {
            every.insert(id);
            let Some(image) = by_id.get(id) else { continue };
            if kept_ids.contains(id) {
                continue;
            }
            let bytes = decoded_bytes(image["data"].as_str().unwrap_or(""));
            if bytes > ONE_IMAGE_BYTES {
                continue;
            }
            if kept.len() >= MAX_IMAGES || image_bytes + bytes > IMAGE_BYTES {
                continue;
            }
            kept.push(image);
            kept_ids.insert(id);
            image_bytes += bytes;
        }
    }
    trimmed.images = every.len() - kept_ids.len();
    let named_events: Vec<Value> = events
        .iter()
        .map(|event| {
            let Some(ids) = named(event) else { return event.clone() };
            let ids: Vec<&str> = ids.into_iter().filter(|id| kept_ids.contains(id)).collect();
            let mut event = event.clone();
            if let Some(fields) = event.as_object_mut() {
                fields.remove("images");
                if !ids.is_empty() {
                    fields.insert("images".into(), json!(ids));
                }
            }
            event
        })
        .collect();
    let budget = (SESSION_BYTES - HEADROOM).saturating_sub(PER_EVENT * named_events.len());
    let mut bytes = 0;
    for (cap, input_cap) in STEPS {
        let (mut outputs, mut inputs) = (0, 0);
        let capped: Vec<Value> = named_events
            .iter()
            .map(|event| {
                let kind = event["kind"].as_str().unwrap_or("");
                if kind == "toolResult" {
                    let output = event["output"].as_str().unwrap_or("");
                    let short = cut_utf8(output, cap);
                    if short.len() == output.len() {
                        return event.clone();
                    }
                    outputs += 1;
                    let mut event = event.clone();
                    event["output"] = json!(short);
                    event["truncated"] = json!(true);
                    return event;
                }
                // Inputs are cut only once outputs alone were not enough: a tool's input is what the agent wrote.
                if let Some(input_cap) = input_cap
                    && (kind == "tool" || kind == "approval")
                    && event.get("input").is_some()
                {
                    let mut event = event.clone();
                    event["input"] = cut_strings(&event["input"], input_cap, &mut inputs);
                    return event;
                }
                event.clone()
            })
            .collect();
        let mut fitted = session.clone();
        fitted["events"] = Value::Array(capped);
        bytes = fitted.to_string().len();
        if bytes <= budget {
            trimmed.outputs = outputs;
            trimmed.inputs = inputs;
            return Ok((fitted, kept.into_iter().cloned().collect(), trimmed));
        }
    }
    Err(bytes)
}
