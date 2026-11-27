//! What a session left running past its turn: the one list behind `ThreadSummary::background`, which the sidebar's
//! mark, its hover card and keep-awake read. It has two sources:
//!
//! - its shared-terminal commands ([`crate::terminal`]), which outlive the agent process, and
//! - its agent's background tasks: the task book's running ones, then those the agent's own list names.
//!
//! Antigravity (ACP) reports no tasks and gets no shared terminal, so it never shows any.

use std::collections::HashSet;

use crate::Workbench;
use crate::event::{BACKGROUND_LISTED, BackgroundKind, BackgroundWork};
use crate::linked::results::OWED_GRACE;

/// What a session left running, while no turn runs.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct RunningWork {
    /// Oldest first per source, terminal commands first; at most [`BACKGROUND_LISTED`].
    pub listed: Vec<BackgroundWork>,
    /// How many in all, past the cap too.
    pub total: usize,
    /// Its agent still holds the session: a background sub-agent runs, or it owes a turn (`in_background`).
    pub agent_holds: bool,
}

impl Workbench {
    /// What session `thread_id` left running past its turn. Empty while a turn runs: that work is the turn's.
    pub(crate) fn running_work(&self, thread_id: &str) -> RunningWork {
        let terminals = self.inner.terminals.running(&self.config().scope, thread_id);
        let Some(live) = self.live(thread_id) else {
            // The agent process ended, or never ran: its terminal commands may still run.
            return list(terminals, Vec::new(), false);
        };
        let state = live.state();
        let idle = !state.done && state.turns.is_empty() && state.unsent.is_empty();
        if !idle && !state.done {
            return RunningWork::default();
        }
        let agent = if idle {
            agent_work(&state.book.running_tasks(), &state.background, |task| {
                state.book.known(task)
            })
        } else {
            Vec::new()
        };
        let owed = state.owed > 0 && state.owed_since.elapsed() < OWED_GRACE;
        list(terminals, agent, idle && (state.book.agents_running() || owed))
    }

    /// Whether any session's shared-terminal command still runs: the computer stays awake for it.
    pub(crate) fn terminals_running(&self) -> bool {
        self.inner.terminals.any_running()
    }
}

fn list(terminals: Vec<BackgroundWork>, agent: Vec<BackgroundWork>, agent_holds: bool) -> RunningWork {
    let total = terminals.len() + agent.len();
    RunningWork {
        listed: terminals.into_iter().chain(agent).take(BACKGROUND_LISTED).collect(),
        total,
        agent_holds,
    }
}

/// What an idle agent left running: the book's running tasks, oldest first, then the tasks the agent's own list names
/// that the book does not show running. `known` names one of those from the book: a Codex sub-agent the book saw end
/// still drains past its turn.
fn agent_work<'a>(
    book: &[(&str, &str, &str, i64)],
    listed: &HashSet<String>,
    known: impl Fn(&str) -> Option<(&'a str, &'a str, i64)>,
) -> Vec<BackgroundWork> {
    let running: HashSet<&str> = book.iter().map(|task| task.0).collect();
    let mut others: Vec<&String> = listed.iter().filter(|id| !running.contains(id.as_str())).collect();
    // The set has no order: sort, so the list does not reorder at each read.
    others.sort();
    book.iter()
        .map(|(_, kind, title, at)| BackgroundWork {
            kind: kind_of(kind),
            title: (*title).to_string(),
            started_at: Some(*at),
        })
        .chain(others.into_iter().map(|id| match known(id) {
            Some((kind, title, at)) => BackgroundWork {
                kind: kind_of(kind),
                title: title.to_string(),
                started_at: Some(at),
            },
            None => BackgroundWork {
                kind: BackgroundKind::Shell,
                title: String::new(),
                started_at: None,
            },
        }))
        .collect()
}

fn kind_of(kind: &str) -> BackgroundKind {
    match kind {
        "agent" => BackgroundKind::Agent,
        _ => BackgroundKind::Shell,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(title: &str, at: i64) -> BackgroundWork {
        BackgroundWork {
            kind: BackgroundKind::Shell,
            title: title.into(),
            started_at: Some(at),
        }
    }

    fn shown(work: &[BackgroundWork]) -> Vec<(BackgroundKind, &str, Option<i64>)> {
        work.iter().map(|w| (w.kind, w.title.as_str(), w.started_at)).collect()
    }

    #[test]
    fn lists_the_books_tasks_then_the_agents_own_list_without_repeats() {
        let book = [
            ("a1", "agent", "Explore the repo", 10),
            ("b1", "shell", "cargo test", 20),
        ];
        let listed: HashSet<String> = ["b1", "z9"].into_iter().map(String::from).collect();
        let work = agent_work(&book, &listed, |_| None);
        assert_eq!(
            shown(&work),
            vec![
                (BackgroundKind::Agent, "Explore the repo", Some(10)),
                (BackgroundKind::Shell, "cargo test", Some(20)),
                (BackgroundKind::Shell, "", None),
            ]
        );
        assert!(agent_work(&[], &HashSet::new(), |_| None).is_empty());
    }

    #[test]
    fn a_draining_codex_child_keeps_its_name() {
        let listed: HashSet<String> = ["child".to_string()].into_iter().collect();
        let work = agent_work(&[], &listed, |id| {
            (id == "child").then_some(("agent", "Review the diff", 7))
        });
        assert_eq!(shown(&work), vec![(BackgroundKind::Agent, "Review the diff", Some(7))]);
    }

    #[test]
    fn terminal_commands_come_first_and_the_total_counts_past_the_cap() {
        let agent: Vec<_> = (0..30).map(|i| shell("agent's", i)).collect();
        let running = list(vec![shell("sleep 60", 1)], agent, true);
        assert_eq!(running.listed.len(), BACKGROUND_LISTED);
        assert_eq!(running.listed[0].title, "sleep 60");
        assert_eq!(running.total, 31);
        assert!(running.agent_holds);
    }
}

#[cfg(all(test, unix))]
mod terminal_tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use futures::future::BoxFuture;
    use serde_json::json;

    use crate::event::{BackgroundKind, ThreadSummary, Usage};
    use crate::process::Shell;
    use crate::store::{Store, ThreadRecord};
    use crate::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd, Workbench};

    /// Keeps each session summary the workbench shows.
    #[derive(Default)]
    struct Shown(Mutex<Vec<ThreadSummary>>);

    impl Host for Shown {
        fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, crate::Result<Launch>> {
            Box::pin(async { panic!("no agent starts here") })
        }
        fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
            Box::pin(async { true })
        }
        fn price(&self, _: &str, _: &Usage) -> Option<f64> {
            None
        }
        fn emit(&self, emit: Emit) {
            if let Emit::Thread(thread) = emit {
                self.0.lock().expect("shown").push(*thread);
            }
        }
    }

    fn workbench(shown: Arc<Shown>) -> Workbench {
        let store = Store::in_memory().expect("a store");
        let project = store.add_project(&std::env::temp_dir(), 1).expect("a project");
        let summary: ThreadSummary = serde_json::from_value(json!({
            "id": "t", "projectId": project.id, "title": "Run it", "agent": "claude", "model": null,
            "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0, "nexusSessions": [],
            "contextTokens": null, "contextWindow": null, "createdAt": 1, "updatedAt": 1, "archived": false,
        }))
        .expect("a summary");
        store
            .insert_thread(&ThreadRecord {
                summary,
                native_id: "native-t".into(),
                native_started: false,
                scope: "scope".into(),
                fork_from: None,
                seed: None,
                automatic_title: false,
                delivered_seq: 0,
                halt: None,
            })
            .expect("a session");
        Workbench::new(
            store,
            shown,
            Config {
                terminal: true,
                scope: "scope".into(),
                shell: Shell::sh(),
                ..Config::default()
            },
        )
        .expect("a workbench")
    }

    /// The `n`th summary shown, waiting up to 10 s for it.
    fn nth(shown: &Shown, n: usize) -> ThreadSummary {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(thread) = shown.0.lock().expect("shown").get(n) {
                return thread.clone();
            }
            assert!(Instant::now() < until, "summary {n} was never shown");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_terminal_command_shows_while_it_runs_and_keeps_the_computer_awake() {
        let shown = Arc::new(Shown::default());
        let wb = workbench(shown.clone());
        assert!(!wb.has_active_work());
        wb.terminal(
            "t",
            "run",
            json!({"command": "sleep 1\necho done", "request_id": "r1"}),
            true,
        )
        .await
        .expect("the command starts");

        let started = nth(&shown, 0);
        assert_eq!(started.id, "t");
        assert_eq!(started.background.len(), 1);
        assert_eq!(started.background[0].kind, BackgroundKind::Shell);
        assert_eq!(started.background[0].title, "sleep 1");
        assert!(started.background[0].started_at.is_some());
        assert_eq!(started.background_total, 1);
        assert!(wb.has_active_work(), "only a terminal command runs");

        let ended = nth(&shown, 1);
        assert!(ended.background.is_empty());
        assert_eq!(ended.background_total, 0);
        assert!(!wb.has_active_work());
        assert_eq!(
            shown.0.lock().expect("shown").len(),
            2,
            "one summary at the start, one at the end"
        );
    }
}
