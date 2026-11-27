//! The terminal's agent conversations, so `/resume` can switch between them as Claude Code's does. Each is
//! the same file `nexus __agent --session` keeps (`<id>.json`, the messages), beside the CLI's config, with a note of
//! its own (`<id>.meta.json`: the folder, a title, the model, when it last changed). Only the terminal writes the note,
//! so the desktop's conversations stay out of its list and are never open in two places.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::stdio::session_file;

/// A conversation as `/resume` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub id: String,
    pub title: String,
    pub model: Option<String>,
    /// ISO time it last changed.
    pub updated: String,
    pub messages: usize,
}

fn meta_file(config_path: &Path, id: &str) -> Result<PathBuf, String> {
    Ok(session_file(config_path, id)?.with_extension("meta.json"))
}

/// A new conversation's id.
pub fn new_id() -> String {
    format!("t-{}", uuid::Uuid::new_v4())
}

/// Saves conversation `id` of `workspace`: its messages, and its note.
pub fn save(
    config_path: &Path,
    id: &str,
    workspace: &Path,
    history: &[Value],
    model: Option<&str>,
) -> Result<(), String> {
    let file = session_file(config_path, id)?;
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string(history).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_millis() as f64);
    let meta = json!({
        "workspace": workspace.display().to_string(),
        "title": title(history),
        "model": model,
        "updated": crate::style::iso(now),
        "messages": history.len(),
    });
    std::fs::write(meta_file(config_path, id)?, meta.to_string()).map_err(|error| error.to_string())
}

/// This folder's conversations, newest first.
pub fn list(config_path: &Path, workspace: &Path) -> Vec<Saved> {
    let Ok(file) = session_file(config_path, "x") else {
        return Vec::new();
    };
    let Some(folder) = file.parent() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let here = workspace.display().to_string();
    let mut saved: Vec<Saved> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let id = name.strip_suffix(".meta.json")?.to_string();
            let meta: Value = serde_json::from_str(&std::fs::read_to_string(entry.path()).ok()?).ok()?;
            let text = |key: &str| meta.get(key).and_then(Value::as_str).map(String::from);
            (text("workspace").as_deref() == Some(here.as_str())).then(|| Saved {
                id,
                title: text("title").unwrap_or_else(|| "Untitled".into()),
                model: text("model"),
                updated: text("updated").unwrap_or_default(),
                messages: meta.get("messages").and_then(Value::as_u64).unwrap_or(0) as usize,
            })
        })
        .collect();
    // ISO times sort as text.
    saved.sort_by(|a, b| b.updated.cmp(&a.updated));
    saved
}

/// A subagent of a conversation: what it was asked, its own conversation, and how it ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Subagent {
    /// The `agent` tool call that started it.
    pub call: String,
    pub description: String,
    pub prompt: String,
    pub messages: Vec<Value>,
    pub result: String,
    pub error: bool,
}

fn agents_file(config_path: &Path, id: &str) -> Result<PathBuf, String> {
    Ok(session_file(config_path, id)?.with_extension("agents.json"))
}

/// Saves conversation `id`'s subagents beside it.
pub fn save_agents(config_path: &Path, id: &str, agents: &[Subagent]) -> Result<(), String> {
    let saved: Vec<Value> = agents
        .iter()
        .map(|agent| {
            json!({ "call": agent.call, "description": agent.description, "prompt": agent.prompt,
                "messages": agent.messages, "result": agent.result, "error": agent.error })
        })
        .collect();
    std::fs::write(agents_file(config_path, id)?, Value::Array(saved).to_string()).map_err(|error| error.to_string())
}

/// Conversation `id`'s subagents; none when it had none.
pub fn load_agents(config_path: &Path, id: &str) -> Vec<Subagent> {
    let Some(saved) = agents_file(config_path, id)
        .ok()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok())
    else {
        return Vec::new();
    };
    saved
        .iter()
        .map(|agent| {
            let text = |key: &str| agent.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            Subagent {
                call: text("call"),
                description: text("description"),
                prompt: text("prompt"),
                messages: agent
                    .get("messages")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                result: text("result"),
                error: agent.get("error").and_then(Value::as_bool).unwrap_or(false),
            }
        })
        .collect()
}

/// Conversation `id`'s messages.
pub fn load(config_path: &Path, id: &str) -> Result<Vec<Value>, String> {
    let text = std::fs::read_to_string(session_file(config_path, id)?).map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// A message's text: its string, or its text parts (a prompt with images).
pub fn text_of(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The first prompt's first line, as the conversation's title.
pub fn title(history: &[Value]) -> String {
    let first = history
        .iter()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .map(text_of)
        .unwrap_or_default();
    let line = crate::tui::mentions::strip_files(&first)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let title: String = line.chars().take(80).collect();
    if title.is_empty() { "Untitled".into() } else { title }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_lists_this_folders_conversations_newest_first_and_loads_them() {
        let root = std::env::temp_dir().join(format!("nexus-sessions-{}", uuid::Uuid::new_v4()));
        let config = root.join("config.json");
        let (here, there) = (root.join("here"), root.join("there"));
        let prompt = |text: &str| {
            vec![
                json!({ "role": "user", "content": text }),
                json!({ "role": "assistant", "content": "ok" }),
            ]
        };
        save(
            &config,
            "t-1",
            &here,
            &prompt("Fix the login\nwith detail"),
            Some("anthropic/claude-haiku-5.5"),
        )
        .expect("saved");
        std::thread::sleep(std::time::Duration::from_millis(5));
        save(&config, "t-2", &here, &prompt("Add tests"), None).expect("saved");
        save(&config, "t-3", &there, &prompt("Elsewhere"), None).expect("saved");
        // A conversation of `nexus __agent` (the desktop's) has no note: it is not listed; nor is a subagents' file.
        std::fs::write(session_file(&config, "desktop").expect("a path"), "[]").expect("written");

        let listed = list(&config, &here);
        let titles: Vec<&str> = listed.iter().map(|saved| saved.title.as_str()).collect();
        assert_eq!(titles, ["Add tests", "Fix the login"]);
        assert_eq!(
            (listed[1].messages, listed[1].model.as_deref()),
            (2, Some("anthropic/claude-haiku-5.5"))
        );
        assert_eq!(load(&config, "t-1").map(|messages| messages.len()), Ok(2));
        assert_eq!(
            text_of(&json!({ "content": [{ "type": "text", "text": "look" }, { "type": "image_url" }] })),
            "look"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
