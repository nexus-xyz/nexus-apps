//! The person's own projects and where each chat is filed, for the sidebar's one grouping.
//!
//! The sidebar has one kind of group, the project. A Code project is a folder or a repository the Code store knows. A
//! plain project is only a name the person made, for work with no code ("Q4 hiring"). A chat sits in one project, or
//! in Recent when it is in none. A Code session sits in its own Code project, so it is never filed here.
//!
//! This is a per-device file for now: `nexus-desktop-gpui/project-groups.json` in this build's data folder, one entry
//! per organization. Gateway has no project for a chat yet. When it gets one, this file is the import source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where a chat is filed: a plain project, or a Code project by its Code store id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Home {
    Plain(String),
    Code(String),
}

impl Home {
    fn key(&self) -> String {
        match self {
            Home::Plain(id) => format!("plain:{id}"),
            Home::Code(id) => format!("code:{id}"),
        }
    }

    fn parse(key: &str) -> Option<Home> {
        let (kind, id) = key.split_once(':')?;
        match kind {
            "plain" => Some(Home::Plain(id.to_string())),
            "code" => Some(Home::Code(id.to_string())),
            _ => None,
        }
    }
}

/// A project the person made with a name and no folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlainProject {
    pub id: String,
    pub name: String,
    pub created_ms: i64,
}

/// One organization's projects and filed chats.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Groups {
    #[serde(default)]
    pub projects: Vec<PlainProject>,
    /// Chat id to its home's key (`plain:<id>` or `code:<id>`).
    #[serde(default)]
    filed: BTreeMap<String, String>,
    /// A Code project's sessions in the order the person dragged them, top first, by the project's id (a repository's
    /// lead, `repo_groups.rs`). A project with none keeps the store's order (`session_order.rs`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    session_order: BTreeMap<String, Vec<String>>,
    /// The sidebar's projects in the order the person dragged them, top first, by their keys
    /// (`workspace/project_list.rs`). Empty: pinned first, then the newest added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    project_order: Vec<String>,
}

impl Groups {
    /// The project a chat is filed in, or None for Recent.
    pub fn home_of(&self, chat: &str) -> Option<Home> {
        let home = Home::parse(self.filed.get(chat)?)?;
        // A chat filed in a plain project that is gone is back in Recent.
        match &home {
            Home::Plain(id) if !self.projects.iter().any(|project| &project.id == id) => None,
            _ => Some(home),
        }
    }

    /// Files a chat in `home`, or back in Recent with None.
    pub fn file(&mut self, chat: &str, home: Option<Home>) {
        match home {
            Some(home) => {
                self.filed.insert(chat.to_string(), home.key());
            }
            None => {
                self.filed.remove(chat);
            }
        }
    }

    /// Adds a plain project and returns its id. A name already taken is reused, so there is never two of one name.
    pub fn add(&mut self, name: &str, now_ms: i64) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        if let Some(project) = self
            .projects
            .iter()
            .find(|project| project.name.eq_ignore_ascii_case(name))
        {
            return Some(project.id.clone());
        }
        let id = format!("p{now_ms:x}");
        self.projects.push(PlainProject {
            id: id.clone(),
            name: name.to_string(),
            created_ms: now_ms,
        });
        Some(id)
    }

    /// The order the person gave the sessions of the Code project `project`, top first; empty when they gave none.
    pub fn session_order(&self, project: &str) -> &[String] {
        self.session_order.get(project).map(Vec::as_slice).unwrap_or_default()
    }

    /// Keeps `order` as the sessions' order in the Code project `project`.
    pub fn set_session_order(&mut self, project: &str, order: Vec<String>) {
        self.session_order.insert(project.to_string(), order);
    }

    /// The order the person gave the sidebar's projects, by their keys, top first; empty when they gave none.
    pub fn project_order(&self) -> &[String] {
        &self.project_order
    }

    /// Keeps `order` as the sidebar's project order.
    pub fn set_project_order(&mut self, order: Vec<String>) {
        self.project_order = order;
    }

    /// Removes a plain project. Its chats go back to Recent.
    pub fn remove(&mut self, id: &str) {
        self.projects.retain(|project| project.id != id);
        let key = Home::Plain(id.to_string()).key();
        self.filed.retain(|_, home| *home != key);
    }
}

fn file() -> Option<PathBuf> {
    crate::paths::own_data_dir().map(|dir| dir.join("project-groups.json"))
}

/// Every organization's groups in `file`. No file yet is none; a file that doesn't read or parse is an error, so a save
/// never writes over it with one organization's groups alone.
fn read_all(file: &Path) -> Result<BTreeMap<String, Groups>, String> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.to_string()),
    };
    serde_json::from_slice(&bytes).map_err(|error| {
        // Keep a copy aside, so the projects can be recovered by hand even if the file is replaced later. Only the
        // first: a later, differently broken file must not replace the copy that may hold the person's projects.
        let bad = file.with_extension("json.bad");
        if !bad.exists() {
            let _ = std::fs::copy(file, bad);
        }
        format!("{} doesn't read ({error})", file.display())
    })
}

/// The organization's groups, or none yet. An error leaves the file as it is.
pub fn load(org: &str) -> Result<Groups, String> {
    load_at(&file().ok_or("No data folder")?, org)
}

pub(crate) fn load_at(file: &Path, org: &str) -> Result<Groups, String> {
    Ok(read_all(file)?.remove(org).unwrap_or_default())
}

/// Saves the organization's groups. A failure keeps the change on screen until the app closes, and a file that
/// doesn't read stays untouched.
pub fn save(org: &str, groups: &Groups) -> Result<(), String> {
    save_at(&file().ok_or("No data folder")?, org, groups)
}

pub(crate) fn save_at(file: &Path, org: &str, groups: &Groups) -> Result<(), String> {
    let mut all = read_all(file)?;
    all.insert(org.to_string(), groups.clone());
    let bytes = serde_json::to_vec_pretty(&all).map_err(|error| error.to_string())?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    }
    nexus_desktop_core::atomic_file::write_atomic(file, &bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_lives_in_one_place() {
        let mut groups = Groups::default();
        let hiring = groups.add("Q4 hiring", 1).unwrap();
        groups.file("c1", Some(Home::Plain(hiring.clone())));
        groups.file("c1", Some(Home::Code("repo".into())));
        assert_eq!(groups.home_of("c1"), Some(Home::Code("repo".into())));
        groups.file("c1", None);
        assert_eq!(groups.home_of("c1"), None);
    }

    #[test]
    fn a_dragged_order_saves_and_reads_back_and_an_older_file_reads_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("project-groups.json");
        let mut groups = Groups::default();
        assert!(groups.session_order("repo").is_empty());
        groups.set_session_order("repo", vec!["t_2".into(), "t_1".into()]);
        save_at(&file, "org", &groups).unwrap();
        assert_eq!(load_at(&file, "org").unwrap().session_order("repo"), ["t_2", "t_1"]);
        std::fs::write(&file, r#"{"org":{"projects":[],"filed":{}}}"#).unwrap();
        assert!(load_at(&file, "org").unwrap().session_order("repo").is_empty());
    }

    #[test]
    fn a_dragged_project_order_saves_and_reads_back_and_an_older_file_reads_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("project-groups.json");
        let mut groups = Groups::default();
        assert!(groups.project_order().is_empty());
        groups.set_project_order(vec!["plain:p1".into(), "code:repo".into()]);
        save_at(&file, "org", &groups).unwrap();
        assert_eq!(
            load_at(&file, "org").unwrap().project_order(),
            ["plain:p1", "code:repo"]
        );
        std::fs::write(&file, r#"{"org":{"projects":[],"filed":{}}}"#).unwrap();
        assert!(load_at(&file, "org").unwrap().project_order().is_empty());
    }

    #[test]
    fn one_name_one_project() {
        let mut groups = Groups::default();
        let first = groups.add("Launch", 1).unwrap();
        assert_eq!(groups.add(" launch ", 2), Some(first));
        assert_eq!(groups.projects.len(), 1);
        assert_eq!(groups.add("  ", 3), None);
    }

    #[test]
    fn removing_a_project_sends_its_chats_to_recent() {
        let mut groups = Groups::default();
        let id = groups.add("Launch", 1).unwrap();
        groups.file("c1", Some(Home::Plain(id.clone())));
        groups.remove(&id);
        assert_eq!(groups.home_of("c1"), None);
        assert!(groups.filed.is_empty());
    }

    #[test]
    fn round_trips_through_json() {
        let mut groups = Groups::default();
        let id = groups.add("Launch", 1).unwrap();
        groups.file("c1", Some(Home::Plain(id)));
        let json = serde_json::to_string(&groups).unwrap();
        assert_eq!(serde_json::from_str::<Groups>(&json).unwrap(), groups);
    }

    #[test]
    fn a_file_that_does_not_read_is_never_written_over() {
        let dir = std::env::temp_dir().join(format!("nexus-project-groups-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("project-groups.json");
        let _ = std::fs::remove_file(&file);
        let mut groups = Groups::default();
        groups.add("Launch", 1);
        // No file yet: an empty load, and the first save writes it.
        assert_eq!(load_at(&file, "org-a").unwrap(), Groups::default());
        save_at(&file, "org-a", &groups).unwrap();
        save_at(&file, "org-b", &groups).unwrap();
        assert_eq!(load_at(&file, "org-a").unwrap(), groups);
        // A partial write or a newer format: load and save both fail, and the file keeps every organization's bytes.
        std::fs::write(&file, b"{\"org-a\": {\"projects\": [").unwrap();
        assert!(load_at(&file, "org-a").is_err());
        assert!(save_at(&file, "org-b", &Groups::default()).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"{\"org-a\": {\"projects\": [");
        let bad = file.with_extension("json.bad");
        assert_eq!(std::fs::read(&bad).unwrap(), b"{\"org-a\": {\"projects\": [");
        // Broken a second way: the first copy stays, as it may be the one with the projects.
        std::fs::write(&file, b"not json").unwrap();
        assert!(load_at(&file, "org-a").is_err());
        assert_eq!(std::fs::read(&bad).unwrap(), b"{\"org-a\": {\"projects\": [");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
