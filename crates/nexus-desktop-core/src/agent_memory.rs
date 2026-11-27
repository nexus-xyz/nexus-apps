//! What Nexus knows for a native shell: `GET /agent/memory`'s body read strictly, as the kit's `memoryViewOf`;
//! the notes as Markdown files by kind, as Nexus UI's `lib/agent-memory-files.ts`; and the export, a ZIP of those files
//! and `memory.json`, as its `lib/agent-memory-export.ts`. Nexus GPUI draws it
//! (`desktop-gpui/src/pages/agent/memory.rs`).

use serde_json::{Map, Value, json};

use crate::agent::Problem;
use crate::agent_approvals::view_problem;

/// The kinds of a note: the memory file it goes in. A note with none goes in `notes.md`.
pub const NOTE_KINDS: [&str; 4] = ["people", "preferences", "places", "routines"];

/// Where a note came from, with the memory controls on.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// `conversation`, `told` or `added`.
    pub kind: String,
    pub surface: Option<String>,
    /// The errand whose thread it came from: its id and goal.
    pub errand: Option<(String, String)>,
}

/// One note of `GET /agent/memory`.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: String,
    pub text: String,
    pub created_at: String,
    /// `dm` or `channel`.
    pub audience: String,
    pub platform: Option<String>,
    pub kind: Option<String>,
    pub source: Option<Source>,
    pub edited_at: Option<String>,
}

/// The learning switch.
#[derive(Debug, Clone, PartialEq)]
pub struct Learning {
    pub paused: bool,
    pub paused_at: Option<String>,
}

/// `GET /agent/memory`: every note, newest first, and the saved model.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryView {
    pub notes: Vec<Note>,
    pub saved_model: String,
    pub learning: Option<Learning>,
    /// The person may add and edit notes and pause learning.
    pub controls: bool,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(String::from)
}

/// One note, or an add's or an edit's `note`, read strictly; None when it is not one.
pub fn note_of(value: &Value) -> Option<Note> {
    let audience = text(value, "audience").filter(|audience| audience == "dm" || audience == "channel")?;
    let source = value.get("source").and_then(|source| {
        let kind = text(source, "kind").filter(|kind| matches!(kind.as_str(), "conversation" | "told" | "added"))?;
        let errand = source.get("errand").and_then(|errand| {
            let id = text(errand, "id").filter(|id| !id.is_empty())?;
            Some((id, text(errand, "goal")?))
        });
        Some(Source {
            kind,
            surface: text(source, "surface"),
            errand,
        })
    });
    Some(Note {
        id: text(value, "id")?,
        text: text(value, "text")?,
        created_at: text(value, "createdAt")?,
        audience,
        platform: text(value, "platform"),
        kind: text(value, "kind").filter(|kind| NOTE_KINDS.contains(&kind.as_str())),
        source,
        edited_at: text(value, "editedAt"),
    })
}

/// The learning switch, from the view or from `PUT /agent/memory/learning`'s `learning`.
pub fn learning_of(value: &Value) -> Option<Learning> {
    Some(Learning {
        paused: value.get("paused")?.as_bool()?,
        paused_at: text(value, "pausedAt"),
    })
}

/// The body, read strictly; None when it is not one. A note that is not one is left out.
pub fn memory_view_of(body: &Value) -> Option<MemoryView> {
    let notes = body.get("notes")?.as_array()?.iter().filter_map(note_of).collect();
    Some(MemoryView {
        notes,
        saved_model: text(body, "savedModel")?,
        learning: body.get("learning").and_then(learning_of),
        controls: body.get("controls") == Some(&Value::Bool(true)),
    })
}

fn platform_name(slug: &str) -> Option<&'static str> {
    Some(match slug {
        "slack" => "Slack",
        "telegram" => "Telegram",
        _ => return None,
    })
}

/// A note's audience: where the agent may use it, and one line on it.
pub fn audience(note: &Note) -> (String, &'static str) {
    if note.audience == "dm" {
        return ("DM".into(), "Used only in your direct messages with Nexus.");
    }
    let label = match note.platform.as_deref().and_then(platform_name) {
        Some(name) => format!("{name} channel"),
        None => "Channel".into(),
    };
    (label, "Used only in the channel where you said it.")
}

/// Where a note came from, in words: "You added it", "Errand: Dinner on Friday", "From a conversation in Slack".
pub fn source_text(note: &Note) -> Option<String> {
    let source = note.source.as_ref()?;
    if source.kind == "added" {
        return Some("You added it".into());
    }
    if let Some((_, goal)) = &source.errand {
        return Some(format!("Errand: {goal}"));
    }
    let what = if source.kind == "told" {
        "You told it"
    } else {
        "From a conversation"
    };
    let place = source.surface.as_deref().and_then(|surface| {
        if surface == "api" {
            Some("Nexus")
        } else {
            platform_name(surface)
        }
    });
    Some(match place {
        Some(place) => format!("{what} in {place}"),
        None => what.to_string(),
    })
}

/// Strip a list item's or a heading's marker from the start of a line.
fn strip_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    let rest = if let Some(rest) = trimmed.strip_prefix(['-', '*', '+', '•']) {
        rest
    } else if trimmed.starts_with('#') {
        let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
        if hashes > 6 {
            return trimmed;
        }
        &trimmed[hashes..]
    } else {
        let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
        match trimmed[digits..].strip_prefix(['.', ')']) {
            Some(rest) if (1..=3).contains(&digits) => rest,
            _ => return trimmed,
        }
    };
    // A marker counts only when whitespace follows it.
    if rest.starts_with(char::is_whitespace) {
        rest.trim_start()
    } else {
        trimmed
    }
}

/// `**bold**` keeps its text, and so does `code`.
fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(at) = rest.find("**") {
        let after = &rest[at + 2..];
        match after.find("**").filter(|end| *end > 0) {
            Some(end) => {
                out.push_str(&rest[..at]);
                out.push_str(&after[..end]);
                rest = &after[end + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out.replace('`', "")
}

/// A note as the lines a person reads (Nexus UI's `noteLines`): each line loses its list or heading marker, `**bold**`
/// and backticks keep only their text, and blank lines go. The lines stay text, never Markdown.
pub fn note_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| plain(strip_marker(line)).trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// A note's text as typed, or why it cannot be kept, before it is sent.
pub fn note_draft(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("Write something for Nexus to keep, or forget the note.".into());
    }
    let length = trimmed.encode_utf16().count();
    if length > crate::agent::NOTE_CHARS {
        return Err(format!(
            "A note is at most {} characters. This one has {length}.",
            crate::agent::NOTE_CHARS
        ));
    }
    Ok(trimmed.to_string())
}

/// An add's or an edit's body: `{"text": …}`.
pub fn note_body(text: &str) -> String {
    json!({ "text": text }).to_string()
}

/// The saved model, in words.
pub fn saved_model_text(saved: &str) -> String {
    match saved {
        "auto" => "Auto: Nexus picks a model at each question.".into(),
        "fast" => "Fast: a fast model your policy allows, picked at each question.".into(),
        "best" => "Best: the strongest model your policy allows, picked at each question.".into(),
        model => crate::agent::model_name(model).to_string(),
    }
}

/// Whose memory this is (the page's header): Personal, else the organization's, kept by its Assistant.
pub fn memory_scope(personal: bool, org_name: &str) -> Option<(String, String)> {
    if personal {
        return Some(("Personal".into(), "Kept by your own Assistant. Only you see it.".into()));
    }
    let name = org_name.trim();
    if name.is_empty() {
        return None;
    }
    Some((
        name.to_string(),
        format!("Kept by {name}’s Assistant. Your personal memory is kept apart."),
    ))
}

/* --------------------------------------------------------------------- files */

/// One line of a file: its text, the note it belongs to (an index into the notes), and its place in that note.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryLine {
    pub text: String,
    pub note: usize,
    pub index: usize,
}

/// One memory file.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryFile {
    /// Unique among the files: `people.md`, or `errands/<slug>.md`.
    pub path: String,
    /// The name the list shows: `people.md`, or `<slug>.md` for an errand.
    pub name: String,
    /// The heading: "People", or the errand's goal.
    pub title: String,
    pub lines: Vec<MemoryLine>,
    /// When a note of the file was last made or edited (ISO 8601).
    pub changed_at: String,
    /// The errand's id and goal, on an errand's own file.
    pub errand: Option<(String, String)>,
}

fn kind_title(kind: &str) -> &'static str {
    match kind {
        "people" => "People",
        "preferences" => "Preferences",
        "places" => "Places",
        "routines" => "Routines",
        _ => "Notes",
    }
}

/// A file name from an errand's goal: lowercase ASCII words joined by `-`, at most 40 characters.
pub fn errand_slug(goal: &str) -> String {
    let mut slug = String::new();
    for ch in goal.chars().flat_map(fold_accent) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(40).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() { "errand".into() } else { slug }
}

/// A Latin letter without its accent, as NFKD and dropping the marks does for the common ones.
fn fold_accent(ch: char) -> Vec<char> {
    let base = match ch {
        'à'..='å' | 'ā' => 'a',
        'À'..='Å' | 'Ā' => 'A',
        'ç' => 'c',
        'Ç' => 'C',
        'è'..='ë' | 'ē' => 'e',
        'È'..='Ë' | 'Ē' => 'E',
        'ì'..='ï' => 'i',
        'Ì'..='Ï' => 'I',
        'ñ' => 'n',
        'Ñ' => 'N',
        'ò'..='ö' | 'ō' => 'o',
        'Ò'..='Ö' | 'Ō' => 'O',
        'ù'..='ü' | 'ū' => 'u',
        'Ù'..='Ü' | 'Ū' => 'U',
        'ý' | 'ÿ' => 'y',
        'Ý' => 'Y',
        other => other,
    };
    vec![base]
}

/// The files of `notes` (newest first, as the view sends them): the kinds in their order, then `notes.md`, then one
/// file per errand, the newest errand first. A file with no lines is left out. Lines keep the notes' order.
pub fn memory_files(notes: &[Note]) -> Vec<MemoryFile> {
    let mut kinds: Vec<MemoryFile> = Vec::new();
    let mut errands: Vec<MemoryFile> = Vec::new();
    for (at, note) in notes.iter().enumerate() {
        let lines: Vec<MemoryLine> = note_lines(&note.text)
            .into_iter()
            .enumerate()
            .map(|(index, text)| MemoryLine { text, note: at, index })
            .collect();
        if lines.is_empty() {
            continue;
        }
        let changed_at = match &note.edited_at {
            Some(edited) if *edited > note.created_at => edited.clone(),
            _ => note.created_at.clone(),
        };
        let file = match note.source.as_ref().and_then(|source| source.errand.clone()) {
            Some((id, goal)) => match errands
                .iter()
                .position(|file| file.errand.as_ref().map(|e| &e.0) == Some(&id))
            {
                Some(found) => &mut errands[found],
                None => {
                    let mut slug = errand_slug(&goal);
                    let mut n = 2;
                    while errands.iter().any(|file| file.name == format!("{slug}.md")) {
                        slug = format!("{}-{n}", errand_slug(&goal));
                        n += 1;
                    }
                    errands.push(MemoryFile {
                        path: format!("errands/{slug}.md"),
                        name: format!("{slug}.md"),
                        title: goal.clone(),
                        lines: Vec::new(),
                        changed_at: changed_at.clone(),
                        errand: Some((id, goal)),
                    });
                    errands.last_mut().expect("just pushed")
                }
            },
            None => {
                let kind = note.kind.clone().unwrap_or_else(|| "notes".into());
                match kinds.iter().position(|file| file.path == format!("{kind}.md")) {
                    Some(found) => &mut kinds[found],
                    None => {
                        kinds.push(MemoryFile {
                            path: format!("{kind}.md"),
                            name: format!("{kind}.md"),
                            title: kind_title(&kind).into(),
                            lines: Vec::new(),
                            changed_at: changed_at.clone(),
                            errand: None,
                        });
                        kinds.last_mut().expect("just pushed")
                    }
                }
            }
        };
        file.lines.extend(lines);
        if changed_at > file.changed_at {
            file.changed_at = changed_at;
        }
    }
    let order = |file: &MemoryFile| {
        NOTE_KINDS
            .iter()
            .position(|kind| file.path == format!("{kind}.md"))
            .unwrap_or(NOTE_KINDS.len())
    };
    kinds.sort_by_key(order);
    kinds.extend(errands);
    kinds
}

/// How many facts the files hold: one per line.
pub fn fact_count(files: &[MemoryFile]) -> usize {
    files.iter().map(|file| file.lines.len()).sum()
}

/// "1 fact", "14 facts".
pub fn facts_text(count: usize) -> String {
    format!("{count} {}", if count == 1 { "fact" } else { "facts" })
}

/// A note's text without one of its lines, for Delete on that line: the other lines as a Markdown list. None when the
/// note has no other line, so the whole note is forgotten instead.
pub fn without_line(note: &Note, index: usize) -> Option<String> {
    let rest: Vec<String> = note_lines(&note.text)
        .into_iter()
        .enumerate()
        .filter(|(at, _)| *at != index)
        .map(|(_, line)| line)
        .collect();
    match rest.len() {
        0 => None,
        1 => rest.into_iter().next(),
        _ => Some(
            rest.iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    }
}

/// What the view says once notes are forgotten. `assistant`: the Assistant's name.
pub fn forgotten_text(one: bool, count: u64, assistant: &str) -> String {
    if one {
        return format!("{assistant} forgot the note.");
    }
    format!(
        "{assistant} forgot {count} {}.",
        if count == 1 { "note" } else { "notes" }
    )
}

/// What the view says when a read or a forget fails.
pub fn memory_problem(status: u16, body: Option<&Value>) -> Problem {
    view_problem(status, body, |detail| {
        let (title, fallback) = if status == 503 {
            ("Memory is not available now", "Try again in a moment.")
        } else {
            (
                "Memory did not open",
                "Nexus could not reach the Assistant. Try again in a moment.",
            )
        };
        Problem {
            title: title.into(),
            detail: if detail.is_empty() { fallback.into() } else { detail },
        }
    })
}

/// What the view says when an add, an edit or the learning switch fails.
pub fn memory_change_problem(status: u16, body: Option<&Value>) -> Problem {
    view_problem(status, body, |detail| Problem {
        title: "Memory did not change".into(),
        detail: if detail.is_empty() {
            "Nexus could not reach the Assistant. Try again in a moment.".into()
        } else {
            detail
        },
    })
}

/* -------------------------------------------------------------------- export */

/// The export's file name.
pub const EXPORT_FILE: &str = "nexus-memory.zip";

/// Where a line came from, as the export writes it: its audience, its source and its dates.
pub fn line_source(note: &Note) -> String {
    let mut parts = vec![audience(note).0];
    parts.extend(source_text(note));
    parts.push(note.created_at.chars().take(10).collect());
    if let Some(edited) = &note.edited_at {
        parts.push(format!("edited {}", edited.chars().take(10).collect::<String>()));
    }
    parts.join(" · ")
}

/// One memory file as Markdown: its heading, then one list item per line with its source.
pub fn file_markdown(file: &MemoryFile, notes: &[Note]) -> String {
    let mut out = vec![format!("# {}", file.title), String::new()];
    out.extend(
        file.lines
            .iter()
            .map(|line| format!("- {} ({})", line.text, line_source(&notes[line.note]))),
    );
    out.push(String::new());
    out.join("\n")
}

/// The export's Markdown files, by their path in the ZIP.
pub fn memory_markdown_files(view: &MemoryView) -> Vec<(String, String)> {
    let files = memory_files(&view.notes);
    if files.is_empty() {
        return vec![("notes.md".into(), "# Notes\n\nNexus keeps no notes about you.\n".into())];
    }
    files
        .iter()
        .map(|file| (file.path.clone(), file_markdown(file, &view.notes)))
        .collect()
}

fn note_json(note: &Note) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(note.id));
    out.insert("text".into(), json!(note.text));
    out.insert("createdAt".into(), json!(note.created_at));
    out.insert("audience".into(), json!(note.audience));
    if let Some(platform) = &note.platform {
        out.insert("platform".into(), json!(platform));
    }
    if let Some(kind) = &note.kind {
        out.insert("kind".into(), json!(kind));
    }
    if let Some(source) = &note.source {
        let mut kept = Map::new();
        kept.insert("kind".into(), json!(source.kind));
        if let Some(surface) = &source.surface {
            kept.insert("surface".into(), json!(surface));
        }
        if let Some((id, goal)) = &source.errand {
            kept.insert("errand".into(), json!({ "id": id, "goal": goal }));
        }
        out.insert("source".into(), Value::Object(kept));
    }
    if let Some(edited) = &note.edited_at {
        out.insert("editedAt".into(), json!(edited));
    }
    Value::Object(out)
}

/// The notes as JSON, as the agent API sends them, with the organization and the time of the export.
pub fn memory_json(view: &MemoryView, org: &str, exported_at: &str) -> String {
    let mut out = Map::new();
    out.insert("organization".into(), json!(org));
    out.insert("exportedAt".into(), json!(exported_at));
    out.insert("notes".into(), Value::Array(view.notes.iter().map(note_json).collect()));
    out.insert("savedModel".into(), json!(view.saved_model));
    if let Some(learning) = &view.learning {
        let mut kept = Map::new();
        kept.insert("paused".into(), json!(learning.paused));
        if let Some(at) = &learning.paused_at {
            kept.insert("pausedAt".into(), json!(at));
        }
        out.insert("learning".into(), Value::Object(kept));
    }
    if view.controls {
        out.insert("controls".into(), json!(true));
    }
    let text = serde_json::to_string_pretty(&Value::Object(out)).unwrap_or_default();
    format!("{text}\n")
}

/// A moment in UTC, for the ZIP's file times.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZipTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// The export: a ZIP of one Markdown file per memory file, and `memory.json`.
pub fn memory_zip(view: &MemoryView, org: &str, exported_at: &str, at: ZipTime) -> Vec<u8> {
    let mut files = memory_markdown_files(view);
    files.push(("memory.json".into(), memory_json(view, org, exported_at)));
    stored_zip(&files, at)
}

/// CRC-32 (IEEE), as ZIP keeps it.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                0xedb8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0xffff_ffff
}

/// A ZIP with each file stored, not compressed: a few kilobytes of text need no more, and every unzip reads it.
pub fn stored_zip(files: &[(String, String)], at: ZipTime) -> Vec<u8> {
    let time = (u16::from(at.hour) << 11) | (u16::from(at.minute) << 5) | u16::from(at.second / 2);
    let date = (at.year.saturating_sub(1980) << 9) | (u16::from(at.month) << 5) | u16::from(at.day);
    let mut locals = Vec::new();
    let mut centrals = Vec::new();
    for (name, text) in files {
        let name = name.as_bytes();
        let data = text.as_bytes();
        let crc = crc32(data);
        let size = u32::try_from(data.len()).unwrap_or(u32::MAX);
        let name_len = u16::try_from(name.len()).unwrap_or(u16::MAX);
        let offset = u32::try_from(locals.len()).unwrap_or(u32::MAX);
        // Version 2.0, flag 0x0800 (UTF-8 names), method 0 (stored).
        let common = [20u16, 0x0800, 0, time, date];
        locals.extend(0x0403_4b50u32.to_le_bytes());
        common.iter().for_each(|value| locals.extend(value.to_le_bytes()));
        locals.extend(crc.to_le_bytes());
        locals.extend(size.to_le_bytes());
        locals.extend(size.to_le_bytes());
        locals.extend(name_len.to_le_bytes());
        locals.extend(0u16.to_le_bytes());
        locals.extend(name);
        locals.extend(data);
        centrals.extend(0x0201_4b50u32.to_le_bytes());
        centrals.extend(20u16.to_le_bytes());
        common.iter().for_each(|value| centrals.extend(value.to_le_bytes()));
        centrals.extend(crc.to_le_bytes());
        centrals.extend(size.to_le_bytes());
        centrals.extend(size.to_le_bytes());
        centrals.extend(name_len.to_le_bytes());
        // Extra, comment, disk, internal and external attributes.
        centrals.extend([0u8; 12]);
        centrals.extend(offset.to_le_bytes());
        centrals.extend(name);
    }
    let count = u16::try_from(files.len()).unwrap_or(u16::MAX);
    let central_size = u32::try_from(centrals.len()).unwrap_or(u32::MAX);
    let central_at = u32::try_from(locals.len()).unwrap_or(u32::MAX);
    let mut out = locals;
    out.extend(centrals);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(count.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend(central_size.to_le_bytes());
    out.extend(central_at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Value {
        json!({
            "savedModel": "auto",
            "controls": true,
            "learning": { "paused": false },
            "notes": [
                { "id": "n3", "text": "- **Priya N.**: slow to reply\n- nudge after 3 days", "createdAt": "2026-11-04T09:02:00Z", "audience": "dm",
                  "kind": "people", "source": { "kind": "told", "surface": "api", "errand": { "id": "i7", "goal": "Northwind Labs SOW" } } },
                { "id": "n2", "text": "Aisle seats. No shellfish.", "createdAt": "2026-11-03T09:00:00Z", "audience": "dm", "kind": "preferences",
                  "source": { "kind": "added" } },
                { "id": "n1", "text": "# Ana R.\nmy manager at `Acme`", "createdAt": "2026-10-01T09:00:00Z", "editedAt": "2026-11-05T10:00:00Z",
                  "audience": "channel", "platform": "slack", "kind": "people", "source": { "kind": "conversation", "surface": "slack" } },
                { "id": "n0", "text": "Old note", "createdAt": "2026-09-01T09:00:00Z", "audience": "dm" },
                { "id": "bad", "text": "x", "createdAt": "2026-09-01T09:00:00Z", "audience": "everyone" }
            ]
        })
    }

    #[test]
    fn reads_the_memory_strictly() {
        let view = memory_view_of(&body()).expect("reads");
        assert_eq!(view.notes.len(), 4, "the note for everyone is left out");
        assert!(view.controls);
        assert_eq!(
            view.learning,
            Some(Learning {
                paused: false,
                paused_at: None
            })
        );
        assert_eq!(memory_view_of(&json!({ "notes": [] })), None, "no saved model");
        assert_eq!(
            source_text(&view.notes[0]).as_deref(),
            Some("Errand: Northwind Labs SOW")
        );
        assert_eq!(source_text(&view.notes[1]).as_deref(), Some("You added it"));
        assert_eq!(
            source_text(&view.notes[2]).as_deref(),
            Some("From a conversation in Slack")
        );
        assert_eq!(source_text(&view.notes[3]), None);
        assert_eq!(audience(&view.notes[2]).0, "Slack channel");
    }

    #[test]
    fn lines_lose_their_markers_and_keep_their_text() {
        assert_eq!(
            note_lines("- **Ana R.**: my manager\n\n2. `cortex-sim`\n## Never\n-not a marker"),
            ["Ana R.: my manager", "cortex-sim", "Never", "-not a marker"]
        );
        assert_eq!(
            note_draft("  "),
            Err("Write something for Nexus to keep, or forget the note.".into())
        );
        assert!(note_draft(&"a".repeat(601)).is_err());
        assert_eq!(note_body("Hi \"you\""), r#"{"text":"Hi \"you\""}"#);
    }

    #[test]
    fn files_go_by_kind_then_notes_then_one_per_errand() {
        let view = memory_view_of(&body()).expect("reads");
        let files = memory_files(&view.notes);
        assert_eq!(
            files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
            [
                "people.md",
                "preferences.md",
                "notes.md",
                "errands/northwind-labs-sow.md"
            ]
        );
        assert_eq!(files[0].lines.len(), 2);
        assert_eq!(files[0].changed_at, "2026-11-05T10:00:00Z", "the edit counts");
        assert_eq!(files[3].name, "northwind-labs-sow.md");
        assert_eq!(files[3].errand, Some(("i7".into(), "Northwind Labs SOW".into())));
        assert_eq!(facts_text(fact_count(&files)), "6 facts");
        assert_eq!(errand_slug("Café für Ana!"), "cafe-fur-ana");
        assert_eq!(errand_slug("!!!"), "errand");
        assert_eq!(without_line(&view.notes[0], 0).as_deref(), Some("nudge after 3 days"));
        assert_eq!(without_line(&view.notes[1], 0), None);
    }

    #[test]
    fn the_scope_names_the_organizations_assistant() {
        assert_eq!(
            memory_scope(false, "Acme"),
            Some((
                "Acme".into(),
                "Kept by Acme’s Assistant. Your personal memory is kept apart.".into()
            ))
        );
        assert_eq!(memory_scope(true, "Acme").map(|scope| scope.0), Some("Personal".into()));
        assert_eq!(memory_scope(false, " "), None);
    }

    #[test]
    fn the_export_is_a_stored_zip_of_each_file_and_the_json() {
        let view = memory_view_of(&body()).expect("reads");
        let files = memory_markdown_files(&view);
        assert_eq!(files[0].0, "people.md");
        assert!(files[0].1.starts_with(
            "# People\n\n- Ana R. (Slack channel · From a conversation in Slack · 2026-10-01 · edited 2026-11-05)"
        ));
        let json = memory_json(&view, "acme", "2026-11-17T10:00:00Z");
        let read: Value = serde_json::from_str(&json).expect("json");
        assert_eq!(read["organization"], "acme");
        assert_eq!(memory_view_of(&read).map(|again| again.notes.len()), Some(4));
        let at = ZipTime {
            year: 2026,
            month: 11,
            day: 17,
            hour: 10,
            minute: 0,
            second: 0,
        };
        let zip = memory_zip(&view, "acme", "2026-11-17T10:00:00Z", at);
        assert_eq!(&zip[..4], &[0x50, 0x4b, 0x03, 0x04]);
        let end = &zip[zip.len() - 22..];
        assert_eq!(&end[..4], &[0x50, 0x4b, 0x05, 0x06]);
        assert_eq!(u16::from_le_bytes([end[10], end[11]]), 5, "four files and memory.json");
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        let empty = MemoryView {
            notes: Vec::new(),
            saved_model: "auto".into(),
            learning: None,
            controls: false,
        };
        assert_eq!(memory_markdown_files(&empty)[0].0, "notes.md");
    }

    #[test]
    fn a_failure_names_the_memory() {
        assert_eq!(memory_problem(503, None).title, "Memory is not available now");
        assert_eq!(memory_problem(401, None).title, "Sign in again");
        assert_eq!(
            memory_change_problem(400, Some(&json!({ "detail": "Too long." }))).detail,
            "Too long."
        );
        assert_eq!(forgotten_text(false, 3, "Nexus"), "Nexus forgot 3 notes.");
        assert_eq!(forgotten_text(true, 1, "Ada"), "Ada forgot the note.");
    }
}
