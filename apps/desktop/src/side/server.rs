//! A server's session or project for the side pane's Changes and Files, read
//! through the server's own workbench (`wb_changes`, `wb_changed_turns`, `wb_diff`, `wb_diff_targets`,
//! `wb_git_state`, `wb_files`, `wb_file_read`) over its relay (`machines.rs`), since its folder is not on this
//! computer. Its terminal runs there too (`side/terminal/server_shell.rs`), when its host opens shells.

use std::collections::HashMap;

use nexus_workbench::changes_source::ChangesSource as Source;
use nexus_workbench::diff_source::DiffTargets;
use nexus_workbench::git;
use serde_json::{Value, json};

/// What the terminal tool says for a server's session whose host opens no shells: an older host, or none connected.
pub const NO_TERMINAL: &str =
    "This server opens no terminal yet. Update its Nexus host in Settings › Servers, or wait for it to connect.";

/// A server's session, or a project there (the new-session screen).
#[derive(Clone)]
pub struct ServerPlace {
    /// The app's servers (`machines::get`), or a test's.
    pub machines: std::sync::Arc<crate::machines::Machines>,
    pub machine: String,
    pub thread: Option<String>,
    pub project: Option<String>,
}

impl ServerPlace {
    async fn call(&self, call: &str, args: Value) -> Result<Value, String> {
        self.machines
            .call(&self.machine, call, args)
            .await
            .map_err(|_| "The server's answer was lost.".to_string())?
            .map_err(|error| crate::machine_view::plain_error(&error))
    }

    fn thread(&self) -> Result<&str, String> {
        self.thread
            .as_deref()
            .ok_or_else(|| "A project's changes on a server show in its sessions.".to_string())
    }

    /// The patch for `source`, and whether it was cut: the session's and a turn's from `wb_changes`, the rest from
    /// `wb_diff`.
    pub async fn diff(&self, source: &Source) -> Result<(String, bool), String> {
        let thread = self.thread()?;
        let answer = match source {
            Source::Session => {
                self.call("wb_changes", json!({ "threadId": thread, "turn": null }))
                    .await?
            }
            Source::Turn(seq) => {
                self.call("wb_changes", json!({ "threadId": thread, "turn": seq }))
                    .await?
            }
            Source::Branch => {
                self.call("wb_diff", diff_args(thread, json!({ "kind": "branch" })))
                    .await?
            }
            Source::Commit(id) => {
                self.call("wb_diff", diff_args(thread, json!({ "kind": "commit", "id": id })))
                    .await?
            }
            Source::Uncommitted => {
                self.call("wb_diff", diff_args(thread, json!({ "kind": "uncommitted" })))
                    .await?
            }
        };
        Ok(diff_of(&answer))
    }

    /// The session's turns with changes there (`wb_changed_turns`); none when it cannot tell.
    pub async fn turns(&self) -> Vec<i64> {
        let Ok(thread) = self.thread() else {
            return Vec::new();
        };
        match self.call("wb_changed_turns", json!({ "threadId": thread })).await {
            Ok(answer) => serde_json::from_value(answer).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// The base branch and the newest commits there (`wb_diff_targets`), for the picker.
    pub async fn targets(&self) -> Option<DiffTargets> {
        let answer = self
            .call("wb_diff_targets", json!({ "threadId": self.thread().ok()? }))
            .await
            .ok()?;
        serde_json::from_value(answer).ok()
    }

    /// What a commit would take there.
    pub async fn uncommitted(&self) -> Option<git::Stat> {
        let state = self
            .call("wb_git_state", json!({ "threadId": self.thread.as_deref()? }))
            .await
            .ok()?;
        serde_json::from_value(state.get("uncommitted")?.clone()).ok()
    }

    /// The folder's files, and whether the list was cut.
    pub async fn files(&self) -> Result<(Vec<String>, bool), String> {
        let args = match (&self.thread, &self.project) {
            (Some(thread), _) => json!({ "threadId": thread }),
            (None, Some(project)) => json!({ "projectId": project }),
            (None, None) => return Err("No folder.".into()),
        };
        files_of(&self.call("wb_files", args).await?)
    }

    /// Search the session's files on its host, using the shared workbench limits.
    pub async fn search(
        &self,
        query: &str,
        case_sensitive: bool,
    ) -> Result<nexus_workbench::project_files::SearchResult, String> {
        let answer = self
            .call(
                "wb_file_search",
                json!({
                    "threadId": self.thread()?, "query": query, "caseSensitive": case_sensitive
                }),
            )
            .await?;
        search_of(answer)
    }

    /// Each changed file's git status letter, from the session's changes.
    pub async fn status(&self) -> HashMap<String, char> {
        match self.diff(&Source::Session).await {
            Ok((patch, _)) => status_of_diff(&patch),
            Err(_) => HashMap::new(),
        }
    }

    /// File `path` for the viewer's preview (`wb_file_preview`), and whether the host previews images and PDFs. A
    /// host with `file-preview-pieces` sends an image or PDF in pieces; one without `file-previews` reads it as text
    /// (`wb_file_read`), as the Nexus app's `file-view.tsx` reads it.
    pub async fn preview(&self, path: &str) -> Result<(nexus_workbench::output_files::Preview, bool), String> {
        let thread = self.thread()?;
        if !self
            .machines
            .has_feature(&self.machine, nexus_workbench::output_files::FEATURE)
        {
            let file = self
                .call("wb_file_read", json!({ "threadId": thread, "path": path }))
                .await?;
            let file = serde_json::from_value(file).map_err(|error| error.to_string())?;
            return Ok((nexus_workbench::output_files::Preview::from_text(file), false));
        }
        let pieces = self
            .machines
            .has_feature(&self.machine, nexus_workbench::output_files::PIECES_FEATURE);
        let preview = nexus_workbench::output_files::read_whole(
            |offset| async move {
                let mut args = json!({ "threadId": thread, "path": path });
                if let Some(offset) = offset {
                    args["offset"] = offset.into();
                }
                let answer = self.call("wb_file_preview", args).await?;
                serde_json::from_value(answer).map_err(|error| error.to_string())
            },
            pieces,
        )
        .await?;
        Ok((preview, true))
    }
}

impl ServerPlace {
    /// Whether the server's host reads an answer's images and videos (`wb_file_media`); one without it leaves them as
    /// links, as the Nexus app does (`useInlineMedia`).
    pub fn shows_media(&self) -> bool {
        self.machines
            .has_feature(&self.machine, nexus_workbench::output_media::FEATURE)
    }

    /// The piece of media file `path` from `offset` (`wb_file_media`).
    pub async fn media(&self, path: &str, offset: u64) -> Result<nexus_workbench::output_media::MediaChunk, String> {
        let thread = self.thread()?;
        let answer = self
            .call(
                "wb_file_media",
                json!({ "threadId": thread, "path": path, "offset": offset }),
            )
            .await?;
        serde_json::from_value(answer).map_err(|error| error.to_string())
    }
}

fn diff_args(thread: &str, source: Value) -> Value {
    json!({ "threadId": thread, "source": source })
}

/// A `wb_changes` or `wb_diff` answer: the patch, and whether it was cut.
pub fn diff_of(answer: &Value) -> (String, bool) {
    (
        answer["diff"].as_str().unwrap_or_default().to_string(),
        answer["truncated"].as_bool().unwrap_or(false),
    )
}

/// A `wb_file_search` answer with required fields checked before drawing it.
fn search_of(answer: Value) -> Result<nexus_workbench::project_files::SearchResult, String> {
    #[derive(serde::Deserialize)]
    struct Match {
        path: String,
        line: u32,
        column: u32,
        start: u32,
        text: String,
    }
    #[derive(serde::Deserialize)]
    struct Result {
        matches: Vec<Match>,
        truncated: bool,
    }
    let found: Result =
        serde_json::from_value(answer).map_err(|_| "The server returned an invalid search result.".to_string())?;
    Ok(nexus_workbench::project_files::SearchResult {
        matches: found
            .matches
            .into_iter()
            .map(|hit| nexus_workbench::project_files::SearchMatch {
                path: hit.path,
                line: hit.line,
                column: hit.column,
                start: hit.start,
                text: hit.text,
            })
            .collect(),
        truncated: found.truncated,
    })
}

/// A `wb_files` answer: the files, and whether the list was cut.
pub fn files_of(answer: &Value) -> Result<(Vec<String>, bool), String> {
    let files = serde_json::from_value(answer["files"].clone()).map_err(|error| error.to_string())?;
    Ok((files, answer["truncated"].as_bool().unwrap_or(false)))
}

/// The status letter of each file a unified diff touches: `A` added, `D` deleted, `R` renamed, else `M`.
pub fn status_of_diff(patch: &str) -> HashMap<String, char> {
    let mut status = HashMap::new();
    let mut current: Option<(String, char)> = None;
    let mut flush = |current: &mut Option<(String, char)>| {
        if let Some((path, letter)) = current.take() {
            status.insert(path, letter);
        }
    };
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            flush(&mut current);
            let path = rest.split_once(" b/").map_or(rest, |(_, b)| b);
            current = Some((path.to_string(), 'M'));
        } else if let Some((_, letter)) = &mut current {
            if line.starts_with("new file mode") {
                *letter = 'A';
            } else if line.starts_with("deleted file mode") {
                *letter = 'D';
            } else if line.starts_with("rename to ") {
                *letter = 'R';
            }
        }
    }
    flush(&mut current);
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_results_preserve_remote_line_coordinates_and_truncation() {
        let found = search_of(json!({ "matches": [
            { "path": "src/a.rs", "line": 42, "column": 105, "start": 100, "text": "some match" }
        ], "truncated": true }))
        .unwrap();
        assert_eq!(found.matches[0].line, 42);
        assert_eq!(found.matches[0].column, 105);
        assert_eq!(found.matches[0].start, 100);
        assert!(found.truncated);
        assert!(search_of(json!({ "matches": [{ "path": "a" }], "truncated": false })).is_err());
        assert!(search_of(json!({ "matches": [], "truncated": "yes" })).is_err());
        assert!(
            search_of(json!({ "matches": [], "truncated": false }))
                .unwrap()
                .matches
                .is_empty()
        );
    }

    #[test]
    fn reads_the_status_letters_a_diff_says() {
        let patch = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                     diff --git a/new.txt b/new.txt\nnew file mode 100644\n\
                     diff --git a/old.txt b/old.txt\ndeleted file mode 100644\n\
                     diff --git a/x.rs b/y.rs\nsimilarity index 90%\nrename from x.rs\nrename to y.rs\n";
        let status = status_of_diff(patch);
        assert_eq!(status.get("src/a.rs"), Some(&'M'));
        assert_eq!(status.get("new.txt"), Some(&'A'));
        assert_eq!(status.get("old.txt"), Some(&'D'));
        assert_eq!(status.get("y.rs"), Some(&'R'));
        assert_eq!(status.len(), 4);
    }
}
