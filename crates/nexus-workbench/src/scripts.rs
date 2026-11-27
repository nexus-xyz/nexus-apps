//! Project commands: named shell commands a project keeps (`npm run dev`, `cargo test`), each run on request in a new
//! terminal of a session's folder, from the top bar, the ⌘K palette or its own key (desktop's project-scripts.ts).
//! Like T3 Code's `t3.json` scripts, but kept here beside the project's setup script ([`crate::setup`]), not read from
//! a file in the repository: a cloned repository's file would put someone else's commands behind a button and a key.
//!
//! This module only keeps them. Running one is the shared terminal's `run` ([`crate::terminal`]), which the person's
//! click starts. On a server or cloud machine whose host opens shells, Nexus Desktop runs it in a shell there
//! ([`crate::shell_pty`]).

use serde::{Deserialize, Serialize};

use crate::{Result, Workbench};

/// The most commands a project keeps.
pub const MAX_SCRIPTS: usize = 32;
/// The longest name, as the terminal's list shows it (terminal/mod.rs keeps 48 characters of a terminal's name).
pub const MAX_NAME: usize = 48;
/// The longest command, as the shared terminal takes one.
pub const MAX_COMMAND: usize = 32_000;
const MAX_ID: usize = 64;
/// A key as the app writes it (`Mod+Alt+Shift+KeyR`); its form is the app's to check (script-keys.ts).
const MAX_SHORTCUT: usize = 64;

/// One command of a project. `id` is the app's, stable across renames, so a key stays with its command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectScript {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
}

/// The commands as they are kept: names and commands trimmed, a blank key dropped. Refused, with a reason the
/// Settings row shows, when one has no name or no command, two share an id, a name or a key, or any is too long.
pub fn normalize(scripts: Vec<ProjectScript>) -> Result<Vec<ProjectScript>> {
    if scripts.len() > MAX_SCRIPTS {
        return Err(format!("A project keeps at most {MAX_SCRIPTS} commands."));
    }
    let mut kept: Vec<ProjectScript> = Vec::with_capacity(scripts.len());
    for script in scripts {
        let id = script.id.trim().to_string();
        let name = script.name.trim().to_string();
        let command = script.command.trim().to_string();
        let shortcut = script
            .shortcut
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        if id.is_empty() || id.len() > MAX_ID || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("A command has no usable id.".into());
        }
        if name.is_empty() {
            return Err("Give each command a name.".into());
        }
        if name.chars().count() > MAX_NAME {
            return Err(format!("“{name}”: a name has at most {MAX_NAME} characters."));
        }
        if command.is_empty() {
            return Err(format!("“{name}” has no command to run."));
        }
        if command.len() > MAX_COMMAND {
            return Err(format!("“{name}”: the command is too long."));
        }
        if shortcut.as_ref().is_some_and(|key| key.len() > MAX_SHORTCUT) {
            return Err(format!("“{name}”: that key is not one the app knows."));
        }
        if kept.iter().any(|other| other.id == id) {
            return Err("Two commands share an id.".into());
        }
        if kept
            .iter()
            .any(|other| other.name.to_lowercase() == name.to_lowercase())
        {
            return Err(format!("Two commands are named “{name}”."));
        }
        if let Some(key) = &shortcut
            && let Some(other) = kept.iter().find(|other| other.shortcut.as_ref() == Some(key))
        {
            return Err(format!("“{name}” and “{}” have the same key.", other.name));
        }
        kept.push(ProjectScript {
            id,
            name,
            command,
            shortcut,
        });
    }
    Ok(kept)
}

/// Why a session cannot run project commands (desktop's `project-scripts.ts`): a server's or cloud machine's session
/// has no terminal on this computer, and its host opens none there ([`crate::shell_pty`]).
pub const REMOTE_REASON: &str = "Remote sessions have no terminal yet.";

/// Why the session cannot run project commands, or None when it can. `remote`: it runs on another machine.
pub fn unavailable(remote: bool) -> Option<&'static str> {
    unavailable_on(remote, false)
}

/// [`unavailable`] for an app that runs a remote session's commands in a shell on its machine: they run when the
/// machine's host opens shells (`server_shells`, its hello names [`crate::shell_pty::FEATURE`]).
pub fn unavailable_on(remote: bool, server_shells: bool) -> Option<&'static str> {
    (remote && !server_shells).then_some(REMOTE_REASON)
}

/// The commands as a store row keeps them (JSON). A row that no longer reads (written by a later app) is no commands,
/// not a broken Settings page.
pub fn read(saved: Option<&str>) -> Vec<ProjectScript> {
    saved
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default()
}

/// What a menu or the palette shows under a command's name: its first line.
pub fn first_line(command: &str) -> &str {
    command.lines().next().unwrap_or_default()
}

impl Workbench {
    /// The project's commands, in the order the person put them.
    pub fn project_scripts(&self, project_id: &str) -> Result<Vec<ProjectScript>> {
        let saved = self.store().project_scripts(project_id)?;
        Ok(read(saved.as_deref()))
    }

    /// Replaces the project's commands ([`normalize`]d; an empty list takes them away) and answers with what was saved.
    pub fn set_project_scripts(&self, project_id: &str, scripts: Vec<ProjectScript>) -> Result<Vec<ProjectScript>> {
        let scripts = normalize(scripts)?;
        let store = self.store();
        if store.project(project_id)?.is_none() {
            return Err("That project is no longer open.".into());
        }
        let json = if scripts.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&scripts).map_err(|e| e.to_string())?)
        };
        store.set_project_scripts(project_id, json.as_deref())?;
        Ok(scripts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(id: &str, name: &str, command: &str, shortcut: Option<&str>) -> ProjectScript {
        ProjectScript {
            id: id.into(),
            name: name.into(),
            command: command.into(),
            shortcut: shortcut.map(String::from),
        }
    }

    #[test]
    fn trims_what_it_keeps_and_drops_a_blank_key() {
        let kept = normalize(vec![
            script(" dev ", "  Dev server ", " npm run dev\n", Some(" Mod+Alt+KeyD ")),
            script("test", "Test", "cargo test", Some("  ")),
        ]);
        assert_eq!(
            kept,
            Ok(vec![
                script("dev", "Dev server", "npm run dev", Some("Mod+Alt+KeyD")),
                script("test", "Test", "cargo test", None),
            ])
        );
    }

    #[test]
    fn refuses_what_could_not_run_or_would_be_ambiguous() {
        let refused = |scripts: Vec<ProjectScript>| normalize(scripts).err().unwrap_or_default();
        assert_eq!(refused(vec![script("a", " ", "ls", None)]), "Give each command a name.");
        assert_eq!(
            refused(vec![script("a", "List", "  ", None)]),
            "“List” has no command to run."
        );
        assert_eq!(
            refused(vec![script("", "List", "ls", None)]),
            "A command has no usable id."
        );
        assert_eq!(
            refused(vec![script("a b", "List", "ls", None)]),
            "A command has no usable id."
        );
        assert_eq!(
            refused(vec![script("a", "List", "ls", None), script("a", "Other", "pwd", None)]),
            "Two commands share an id."
        );
        assert_eq!(
            refused(vec![
                script("a", "List", "ls", None),
                script("b", "list", "ls -a", None)
            ]),
            "Two commands are named “list”."
        );
        assert_eq!(
            refused(vec![
                script("a", "List", "ls", Some("Mod+Alt+KeyL")),
                script("b", "All", "ls -a", Some("Mod+Alt+KeyL")),
            ]),
            "“All” and “List” have the same key."
        );
        assert!(refused(vec![script("a", &"n".repeat(MAX_NAME + 1), "ls", None)]).contains("at most 48"));
        assert!(refused(vec![script("a", "Big", &"x".repeat(MAX_COMMAND + 1), None)]).contains("too long"));
        let many = (0..=MAX_SCRIPTS)
            .map(|n| script(&n.to_string(), &format!("Command {n}"), "ls", None))
            .collect();
        assert!(refused(many).contains("at most 32"));
        // A name at the limit, counted in characters rather than bytes, is fine.
        assert!(normalize(vec![script("a", &"é".repeat(MAX_NAME), "ls", None)]).is_ok());
    }

    #[test]
    fn a_remote_session_runs_none_and_says_why() {
        assert_eq!(unavailable(true), Some(REMOTE_REASON));
        assert_eq!(unavailable(false), None);
    }

    #[test]
    fn a_remote_session_runs_them_where_its_host_opens_shells() {
        assert_eq!(unavailable_on(true, true), None);
        assert_eq!(unavailable_on(true, false), Some(REMOTE_REASON));
        assert_eq!(unavailable_on(false, false), None);
    }

    #[test]
    fn a_row_that_does_not_read_is_no_commands() {
        let saved = r#"[{"id":"dev","name":"Dev","command":"npm run dev\nnpm test"}]"#;
        let read = read(Some(saved));
        assert_eq!(read, vec![script("dev", "Dev", "npm run dev\nnpm test", None)]);
        assert_eq!(first_line(&read[0].command), "npm run dev");
        assert!(super::read(Some("not json")).is_empty());
        assert!(super::read(None).is_empty());
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn reads_back_in_the_wire_shape() {
        let json = serde_json::to_value(script("dev", "Dev", "npm run dev", None)).unwrap_or_default();
        assert_eq!(
            json,
            serde_json::json!({ "id": "dev", "name": "Dev", "command": "npm run dev" })
        );
        let back: ProjectScript = serde_json::from_value(serde_json::json!({
            "id": "dev", "name": "Dev", "command": "npm run dev", "shortcut": "Mod+Shift+KeyR"
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back.shortcut.as_deref(), Some("Mod+Shift+KeyR"));
    }
}
