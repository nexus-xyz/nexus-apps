//! This host as the runner of native sessions (`/v1/sessions`, served by Gateway).
//!
//! Gateway starts a native session on a cloud machine through the host's own calls (`wb_thread_create_once`, then
//! `wb_send`), and names the session in them (`cloudSession`, `cs_…`). The host then runs the session's agent: before
//! each start it takes the session (`nexus_client::fence::Runner`), or keeps the hold it has, and renews it every 20 s in
//! the background. The agent starts with the session and its fencing token in its environment, so the Nexus MCP server
//! it runs (`nexus mcp`, the desktop's `--mcp`) sends both on each connector change.
//!
//! When a renewal says another runner took the session over (409 `lost_runner`) or the session ended, the hold stops:
//! no more renewals, and the agent's changes are refused, here and by Gateway. The agent's Nexus session still ends and
//! settles what it spent as any other (`ended`). A later start takes the session again, and is refused while another
//! runner holds it.
//!
//! The runner id is this machine's (`nexus-host:<machine id>`), so a restarted host resumes its own hold within the
//! lease. It is a name, not a credential: the host takes and renews with its own Gateway sign-in.
//!
//! Every agent of the session starts with the fence, not only those Gateway's calls start:
//! - The threads Gateway named are kept in the host's folder (`native-sessions.json`), so after a restart (a machine
//!   that slept) an agent the workbench or the desktop app starts on one still takes the session.
//! - A crew member a session's agent started (its thread's `parent_thread_id`, up the chain) runs in its lead's
//!   session, with the same fence.
//!
//! Gateway names a session only while `NEXUS_SESSIONS_API` is on, so with it off nothing is noted or saved.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use nexus_client::Client;
use nexus_client::fence::{Fence, Runner, Stopped, conflict_token, valid_session};
use serde_json::Value;

/// The argument Gateway names a native session in, on `wb_thread_create_once` and `wb_send`.
pub const CLOUD_SESSION_ARG: &str = "cloudSession";

/// The file in the host's folder that keeps the threads Gateway named, across restarts.
pub const SAVED: &str = "native-sessions.json";

/// How many parents up a crew member's lead is looked for. A crew is a few levels deep; this only bounds a cycle.
const MAX_LINEAGE: usize = 16;

/// A thread's parent thread (`ThreadSummary::parent_thread_id`), if it has one.
pub type Parent = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The native sessions this host runs, by thread, and its hold on each.
pub struct SessionRunners {
    runner_id: String,
    threads: Mutex<HashMap<String, String>>,
    held: tokio::sync::Mutex<HashMap<String, Runner>>,
    saved: Option<PathBuf>,
    parent: Option<Parent>,
}

impl SessionRunners {
    pub fn new(runner_id: String) -> SessionRunners {
        SessionRunners {
            runner_id,
            threads: Mutex::default(),
            held: tokio::sync::Mutex::default(),
            saved: None,
            parent: None,
        }
    }

    /// Keeps the threads Gateway named in `path`, and starts with those kept there before. An unreadable file is
    /// started over: Gateway names the session again on its next call.
    pub fn saved_in(mut self, path: PathBuf) -> SessionRunners {
        let kept: HashMap<String, String> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let kept = kept.into_iter().filter(|(_, session)| valid_session(session));
        self.threads
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(kept);
        self.saved = Some(path);
        self
    }

    /// Looks a thread's parent up with `parent`, so a crew member runs in its lead's session.
    pub fn with_lineage(mut self, parent: Parent) -> SessionRunners {
        self.parent = Some(parent);
        self
    }

    pub fn runner_id(&self) -> &str {
        &self.runner_id
    }

    /// Notes the native session a host call names, and takes `cloudSession` out of its arguments: the workbench's
    /// commands do not know it. `wb_thread_create_once`'s thread is `t_<requestId>` ([`nexus_workbench::creation_thread_id`]).
    pub fn note(&self, call: &str, args: &mut Value) {
        let Some(session) = args
            .as_object_mut()
            .and_then(|args| args.remove(CLOUD_SESSION_ARG))
            .and_then(|session| session.as_str().map(String::from))
        else {
            return;
        };
        if !valid_session(&session) {
            return;
        }
        let thread = match call {
            "wb_thread_create_once" => args["requestId"]
                .as_str()
                .and_then(|request| nexus_workbench::creation_thread_id(request).ok()),
            "wb_send" => args["threadId"].as_str().map(String::from),
            _ => None,
        };
        let Some(thread) = thread else {
            return;
        };
        let mut threads = self.threads.lock().unwrap_or_else(PoisonError::into_inner);
        if threads.get(&thread) == Some(&session) {
            return;
        }
        threads.insert(thread, session);
        if let Some(path) = &self.saved {
            let text = serde_json::to_string(&*threads).unwrap_or_default();
            if let Err(error) = crate::files::save_private(path, &text) {
                eprintln!("nexus-host: native sessions are not kept across a restart: {error}");
            }
        }
    }

    /// The native session `thread` runs in, if any: its own, or its crew lead's.
    pub fn session_of(&self, thread: &str) -> Option<String> {
        let threads = self.threads.lock().unwrap_or_else(PoisonError::into_inner);
        let mut thread = thread.to_string();
        for _ in 0..=MAX_LINEAGE {
            if let Some(session) = threads.get(&thread) {
                return Some(session.clone());
            }
            thread = self.parent.as_ref()?(&thread)?;
        }
        None
    }

    /// The fence an agent of `thread` starts with: none outside a native session. The hold this host has, while it
    /// stands, else a new take, renewed in the background. Refused, with words for the person, while another runner
    /// holds the session or once it ended.
    pub async fn fence_for(&self, client: &Client, thread: &str) -> Result<Option<Fence>, String> {
        let Some(session) = self.session_of(thread) else {
            return Ok(None);
        };
        let mut held = self.held.lock().await;
        if let Some(runner) = held.get(&session)
            && runner.fence().stopped_by().is_none()
        {
            return Ok(Some(runner.fence()));
        }
        let runner = Runner::take_or_resume(client, &session, &self.runner_id)
            .await
            .map_err(|error| refusal(&error))?;
        let fence = runner.fence();
        eprintln!(
            "nexus-host: native session {session}: taken, fencing token {}",
            fence.token()
        );
        let holding = runner.clone();
        tokio::spawn(async move {
            let stopped = holding.hold().await;
            let session = holding.fence().session();
            eprintln!(
                "nexus-host: native session {session}: no longer renewed ({})",
                stopped.code()
            );
        });
        held.insert(session, runner);
        Ok(Some(fence))
    }
}

/// Why this host may not run the session, for the person.
fn refusal(error: &nexus_client::Error) -> String {
    if conflict_token(error).is_some() {
        return "Another runner holds this session, so its agent does not start here.".into();
    }
    match Stopped::of(error) {
        Some(Stopped::SessionEnded) => "This session ended, so its agent does not start again.".into(),
        Some(Stopped::SessionNotFound) => {
            "Nexus Gateway does not know this session, so its agent does not start.".into()
        }
        _ => format!("Nexus Gateway did not hand this host the session: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const REQUEST: &str = "6f1c2a1e-5b0e-4c3a-9d8e-2f4b1a7c9e10";

    #[test]
    fn a_host_call_names_its_session_and_the_workbench_never_sees_it() {
        let runners = SessionRunners::new("nexus-host:cm_1".into());
        let mut create = json!({ "requestId": REQUEST, "projectId": "p_1", CLOUD_SESSION_ARG: "cs_abc" });
        runners.note("wb_thread_create_once", &mut create);
        assert_eq!(create.get(CLOUD_SESSION_ARG), None);
        assert_eq!(runners.session_of(&format!("t_{REQUEST}")).as_deref(), Some("cs_abc"));

        let mut send = json!({ "threadId": "t_other", "text": "hi", CLOUD_SESSION_ARG: "cs_def" });
        runners.note("wb_send", &mut send);
        assert_eq!(send, json!({ "threadId": "t_other", "text": "hi" }));
        assert_eq!(runners.session_of("t_other").as_deref(), Some("cs_def"));

        // Not a session id, another call, or no session: nothing is noted.
        let mut forged = json!({ "threadId": "t_x", CLOUD_SESSION_ARG: "cs_a/../b" });
        runners.note("wb_send", &mut forged);
        let mut other = json!({ "threadId": "t_y", CLOUD_SESSION_ARG: "cs_ghi" });
        runners.note("wb_stop", &mut other);
        let mut plain = json!({ "threadId": "t_z" });
        runners.note("wb_send", &mut plain);
        assert_eq!(plain, json!({ "threadId": "t_z" }));
        for thread in ["t_x", "t_y", "t_z"] {
            assert_eq!(runners.session_of(thread), None, "{thread}");
        }
    }

    #[test]
    fn a_restarted_host_still_knows_the_threads_gateway_named() {
        let dir = std::env::temp_dir().join(format!("nh-native-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(SAVED);

        let before = SessionRunners::new("nexus-host:cm_1".into()).saved_in(path.clone());
        let mut send = json!({ "threadId": "t_lead", "text": "hi", CLOUD_SESSION_ARG: "cs_abc" });
        before.note("wb_send", &mut send);
        drop(before);

        // After the restart, an agent the workbench or the desktop app starts there still runs in the session.
        let after = SessionRunners::new("nexus-host:cm_1".into()).saved_in(path.clone());
        assert_eq!(after.session_of("t_lead").as_deref(), Some("cs_abc"));
        assert_eq!(after.session_of("t_other"), None);

        // A kept entry that is not a session id is dropped; an unreadable file starts over.
        std::fs::write(&path, r#"{"t_lead":"cs_abc","t_bad":"cs_a/../b"}"#).expect("write");
        let read = SessionRunners::new("nexus-host:cm_1".into()).saved_in(path.clone());
        assert_eq!(read.session_of("t_lead").as_deref(), Some("cs_abc"));
        assert_eq!(read.session_of("t_bad"), None);
        std::fs::write(&path, "not json").expect("write");
        let unreadable = SessionRunners::new("nexus-host:cm_1".into()).saved_in(path.clone());
        assert_eq!(unreadable.session_of("t_lead"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crew_member_runs_in_its_leads_session() {
        let parents: HashMap<&str, &str> = HashMap::from([
            ("t_member", "t_lead"),
            ("t_sub", "t_member"),
            ("t_a", "t_b"),
            ("t_b", "t_a"),
        ]);
        let parent: Parent = Arc::new(move |thread: &str| parents.get(thread).map(|p| p.to_string()));
        let runners = SessionRunners::new("nexus-host:cm_1".into()).with_lineage(parent);
        let mut create = json!({ "threadId": "t_lead", CLOUD_SESSION_ARG: "cs_abc" });
        runners.note("wb_send", &mut create);

        assert_eq!(runners.session_of("t_member").as_deref(), Some("cs_abc"));
        assert_eq!(runners.session_of("t_sub").as_deref(), Some("cs_abc"));
        // Outside a native session, and a parent chain that loops, run in none.
        assert_eq!(runners.session_of("t_desktop"), None);
        assert_eq!(runners.session_of("t_a"), None);
    }

    #[tokio::test]
    async fn an_agent_outside_a_native_session_starts_with_no_fence_and_no_call() {
        let runners = SessionRunners::new("nexus-host:cm_1".into());
        // Nothing listens there: a call would fail.
        let client = Client::new(nexus_client::ClientOptions::new("http://127.0.0.1:9"));
        assert!(
            runners
                .fence_for(&client, "t_desktop")
                .await
                .expect("no session")
                .is_none()
        );
    }

    #[test]
    fn a_refusal_says_why_the_agent_does_not_start() {
        let error = |status: u16, code: &str, details: Option<Value>| {
            nexus_client::Error::from(nexus_client::CoreError {
                status,
                message: code.into(),
                code: Some(code.into()),
                interval: None,
                details,
            })
        };
        let conflict = error(409, "takeover_conflict", Some(json!({ "currentFencingToken": 3 })));
        assert!(refusal(&conflict).contains("Another runner holds this session"));
        assert!(refusal(&error(409, "session_ended", None)).contains("ended"));
        assert!(refusal(&error(404, "session_not_found", None)).contains("does not know"));
        assert!(refusal(&error(503, "unavailable", None)).contains("did not hand this host the session"));
    }
}
