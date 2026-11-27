//! The worktree sweep on a server: its host sweeps the sessions it owns by the app's rules
//! (`wb_worktree_cleanup`), and the app gathers each machine's report into one [`LastCleanup`]. Nexus GPUI's runner
//! (`code_run.rs`) uses it.

use std::time::Duration;

use nexus_workbench::cleanup::{Entry, Report, Rules};
use serde::Serialize;
use serde_json::{Value, json};

use crate::machine::Remote;

/// A server's sweep is given up on after this long (AGENTS.md rule 11): each pull request it asks GitHub about is
/// bounded, but it may ask about [`nexus_workbench::cleanup::MAX_LOOKUPS`] of them.
pub const REMOTE_LIMIT: Duration = Duration::from_secs(10 * 60);

/// A session a rule matched, and the server it is on (None: this computer).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Swept {
    pub machine: Option<String>,
    #[serde(flatten)]
    pub entry: Entry,
}

/// What the last sweep did, everywhere it ran.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastCleanup {
    /// When it ran (ms since the epoch).
    pub at: i64,
    pub removed: Vec<Swept>,
    pub kept: Vec<Swept>,
    /// A machine it could not sweep, and why, in words for the person.
    pub problems: Vec<String>,
}

impl LastCleanup {
    /// Adds machine `machine`'s report (None: this computer's).
    pub fn add(&mut self, machine: Option<&str>, report: Report) {
        let swept = |entry| Swept {
            machine: machine.map(str::to_string),
            entry,
        };
        self.removed.extend(report.removed.into_iter().map(swept));
        self.kept.extend(report.kept.into_iter().map(swept));
    }

    /// Adds machine `name`'s sweep: its report, or why there is none.
    pub fn add_answer(&mut self, machine: Option<&str>, swept: Result<Report, String>) {
        match swept {
            Ok(report) => self.add(machine, report),
            Err(problem) => self.problems.push(problem),
        }
    }
}

/// A server's answer to `wb_worktree_cleanup` (None: it took longer than [`REMOTE_LIMIT`]), or why there is none, in
/// words for the person.
pub fn report_of(name: &str, answer: Option<Result<Value, String>>) -> Result<Report, String> {
    match answer {
        None => Err(format!(
            "{name}: its cleanup took longer than {} minutes.",
            REMOTE_LIMIT.as_secs() / 60
        )),
        Some(Err(error)) if error.starts_with("Unknown command") => Err(format!(
            "{name}: its Nexus host is older than this app. Update it to clean up there."
        )),
        Some(Err(error)) => Err(format!("{name}: {error}")),
        Some(Ok(value)) => serde_json::from_value(value).map_err(|error| format!("{name}: {error}")),
    }
}

/// Asks machine `name`'s host, over `remote`, to sweep its sessions by `rules`, within [`REMOTE_LIMIT`].
pub async fn sweep(name: &str, remote: &Remote, rules: &Rules) -> Result<Report, String> {
    let call = remote.call("wb_worktree_cleanup", json!({ "rules": rules }));
    report_of(name, tokio::time::timeout(REMOTE_LIMIT, call).await.ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_servers_answer_or_its_problem() {
        let report = json!({ "at": 5, "removed": [{ "threadId": "t", "title": "T", "path": "/w", "branch": "nexus/t",
                             "rule": "archived" }], "kept": [] });
        let read = report_of("devbox", Some(Ok(report))).unwrap_or_default();
        assert_eq!(read.removed.len(), 1);
        assert_eq!(
            report_of("devbox", Some(Err("Unknown command wb_worktree_cleanup.".into()))).err(),
            Some("devbox: its Nexus host is older than this app. Update it to clean up there.".into())
        );
        assert_eq!(
            report_of("devbox", None).err(),
            Some("devbox: its cleanup took longer than 10 minutes.".into())
        );
        let mut last = LastCleanup::default();
        last.add(Some("devbox"), read);
        last.add_answer(Some("old"), Err("old: its Nexus host is older than this app.".into()));
        let shown = serde_json::to_value(&last).unwrap_or_default();
        assert_eq!(shown["removed"][0]["machine"], "devbox");
        assert_eq!(shown["removed"][0]["rule"], "archived");
        assert_eq!(shown["removed"][0]["threadId"], "t");
        assert_eq!(
            last.problems,
            vec!["old: its Nexus host is older than this app.".to_string()]
        );
    }
}
