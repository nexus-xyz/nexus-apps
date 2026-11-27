//! A setup fix's command in a session's Commands (`crate::setup`, `Fix::RunInTerminal`): it runs as the person's own
//! command, where they see it and answer its prompts, and its end goes to the workspace once, as
//! [`SharedTerminalsEvent::FixEnded`].
use super::*;

/// How a watched command ended: well only when it exited 0 and nobody stopped it.
fn ended_well(entry: &Entry) -> bool {
    entry.exit_code == Some(0) && !matches!(entry.status.as_str(), "interrupted" | "cancelled" | "failed")
}

impl SharedTerminals {
    /// Run `command` as [`SharedTerminals::run`] does, and report its end under `token`. False when the pane can't run
    /// it now (busy, read-only, or no longer the session's).
    pub fn run_watched(&mut self, command: String, name: Option<String>, token: u64, cx: &mut Context<Self>) -> bool {
        if self.busy || self.read_only || !self.is_current() {
            return false;
        }
        self.watch_next = Some(token);
        self.run(command, name, cx);
        // `perform` took it; a run that never reached it must not hand it to the next one.
        self.watch_next = None;
        true
    }

    /// The runner answered a watched run: follow its command, or report that it did not start.
    pub(super) fn watch_started(&mut self, token: u64, result: &Result<Value, String>, cx: &mut Context<Self>) {
        match result {
            Ok(value) => match value["id"].as_str() {
                Some(id) => {
                    self.watched.insert(id.to_owned(), token);
                }
                None => cx.emit(SharedTerminalsEvent::FixEnded {
                    token,
                    ok: false,
                    code: None,
                    last_line: None,
                }),
            },
            Err(error) => cx.emit(SharedTerminalsEvent::FixEnded {
                token,
                ok: false,
                code: None,
                last_line: Some(error.clone()),
            }),
        }
    }

    /// The watched commands that ended in this read, as events, each once.
    pub(super) fn ended_watches(&mut self, entries: &[Entry], cx: &App) -> Vec<SharedTerminalsEvent> {
        if self.watched.is_empty() {
            return Vec::new();
        }
        let mut ended = Vec::new();
        for entry in entries.iter().filter(|entry| !entry.active()) {
            let Some(token) = self.watched.remove(&entry.id) else {
                continue;
            };
            let last_line = self
                .outputs
                .get(&entry.id)
                .and_then(|output| output.view.read(cx).last_line());
            ended.push(SharedTerminalsEvent::FixEnded {
                token,
                ok: ended_well(entry),
                code: entry.exit_code,
                last_line,
            });
        }
        ended
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn entry(status: &str, exit: Option<u32>) -> Entry {
        serde_json::from_value(
            json!({"id":"c1","name":"gh","command":"gh auth login --web","reason":null,
            "cwd":"/work","status":status,"private":false,"exitCode":exit}),
        )
        .expect("an entry")
    }

    #[test]
    fn only_a_zero_exit_that_nobody_stopped_ends_well() {
        assert!(ended_well(&entry("completed", Some(0))));
        assert!(!ended_well(&entry("completed", Some(1))));
        assert!(!ended_well(&entry("interrupted", Some(0))));
        assert!(!ended_well(&entry("completed", None)));
    }
}
