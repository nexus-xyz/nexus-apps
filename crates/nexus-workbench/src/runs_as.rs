//! What a session's agent runs with for its permission mode: the agent's own settings, by the agent's own names, from
//! its harness's mode table ([`crate::harness::Harness::mode_settings`]). A session's `permissionMode` is the mode asked
//! for; this is how the agent runs it. Antigravity's settings depend on this computer's Sandbox switch and on whether
//! agy's last run offered its `auto` mode, so they are read from the workbench that runs the session.

use serde_json::{Map, Value};

use crate::Workbench;
use crate::event::{PermissionMode, SessionAgent, ThreadSummary};
use crate::harness::{ModeSettings, RunFacts, harness};

/// The agent's settings for `mode` on a run with `facts`, as a JSON object of name to value:
/// `{"--permission-mode": "auto"}`. An agent with no setting for the mode gives `{}`.
pub fn runs_as(agent: SessionAgent, mode: PermissionMode, facts: &RunFacts) -> Value {
    let ModeSettings(settings) = harness(agent).mode_settings(mode, facts);
    Value::Object(
        settings
            .into_iter()
            .map(|(name, value)| (name.to_owned(), Value::String(value)))
            .collect::<Map<_, _>>(),
    )
}

impl Workbench {
    /// What this workbench's next agent run reads besides the session: the Sandbox switch and agy's last answer
    /// about `auto`, as the launch reads them.
    pub fn run_facts(&self) -> RunFacts {
        RunFacts {
            sandbox: self.inner.host.antigravity_sandbox(),
            auto_offered: self.agy_auto(),
        }
    }

    /// The settings `thread`'s agent runs with for its permission mode on this workbench ([`runs_as`]), from its next
    /// turn: an agy started before the Sandbox switch or agy's answer about `auto` changed keeps its old arguments
    /// until the next message between turns starts it again (`antigravity_args_changed`).
    pub fn runs_as(&self, thread: &ThreadSummary) -> Value {
        runs_as(thread.agent, thread.permission_mode, &self.run_facts())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::future::BoxFuture;
    use serde_json::json;

    use super::*;
    use crate::event::Usage;
    use crate::store::Store;
    use crate::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd, Result};

    /// A host with the Antigravity Sandbox switch on, which never launches.
    struct Sandboxed;
    impl Host for Sandboxed {
        fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, Result<Launch>> {
            Box::pin(async { panic!("the fixture never launches") })
        }
        fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
            Box::pin(async { true })
        }
        fn price(&self, _: &str, _: &Usage) -> Option<f64> {
            None
        }
        fn antigravity_sandbox(&self) -> bool {
            true
        }
        fn emit(&self, _: Emit) {}
    }

    fn thread(agent: &str, mode: &str) -> ThreadSummary {
        serde_json::from_value(json!({
            "id": "t", "projectId": "p", "title": "t", "agent": agent, "model": null,
            "permissionMode": mode, "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
            "contextTokens": null, "contextWindow": null, "createdAt": 0, "updatedAt": 0, "archived": false,
        }))
        .expect("a summary")
    }

    #[tokio::test]
    async fn a_session_runs_as_its_harness_says_with_this_workbenchs_facts() {
        let store = Store::in_memory().expect("an in-memory store");
        let wb = Workbench::new(store, Arc::new(Sandboxed), Config::default()).expect("a workbench");
        // Compared with the harness's own table, which mode_tests.rs pins: this pins what reaches it.
        let agy = thread("antigravity", "auto");
        for offered in [None, Some(false), Some(true)] {
            *wb.inner.agy_auto.lock().expect("agy_auto") = offered;
            let facts = RunFacts {
                sandbox: true,
                auto_offered: offered,
            };
            assert_eq!(wb.run_facts(), facts, "{offered:?}");
            let ModeSettings(expected) = harness(SessionAgent::Antigravity).mode_settings(PermissionMode::Auto, &facts);
            let expected: Map<String, Value> = expected
                .into_iter()
                .map(|(name, value)| (name.to_owned(), Value::String(value)))
                .collect();
            assert_eq!(wb.runs_as(&agy), Value::Object(expected), "{offered:?}");
        }
        assert_eq!(
            wb.runs_as(&thread("claude", "auto")),
            json!({"--permission-mode": "auto"})
        );
    }
}
