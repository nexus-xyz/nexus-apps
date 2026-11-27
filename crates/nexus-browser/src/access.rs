//! An agent's waiting requests for the session browser. A local Code agent starts with control of its browser. Once
//! the person takes over, the agent's next call asks again: the pane shows the request with Allow, Always in this
//! session and Don't allow, and a notification brings the person back to it. Both apps keep their requests here.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

/// How long a request waits for the person.
pub const WAIT: Duration = Duration::from_secs(10 * 60);
/// The notification's text.
pub const NOTICE: &str = "Allow browser control to continue your website task.";
/// What the agent hears when the person declines, the run ends or nobody answers.
pub const DECLINED: &str = "The person declined browser access. Do not use desktop capture as a substitute.";
pub const RUN_ENDED: &str = "The browser request ended with its agent run.";
pub const UNANSWERED: &str = "Browser access was not answered within ten minutes. Ask again when the person returns.";
/// The most requests open at once, so waiting requests leave the socket room for other tools.
const MAX_OPEN: usize = 4;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: String,
    pub thread_id: String,
    pub url: String,
    pub tab: Option<String>,
    pub at: u64,
}

struct Pending {
    request: Request,
    run: String,
}

/// The open requests, one per session at most.
#[derive(Default)]
pub struct Requests(Mutex<HashMap<String, Pending>>);

impl Requests {
    pub fn clear(&self) {
        if let Ok(mut open) = self.0.lock() {
            open.clear();
        }
    }

    /// A new request of the run `run` of `thread`, for `url` (and `tab`, when it named one).
    pub fn ask(&self, thread: &str, run: &str, url: &str, tab: Option<String>) -> Result<Request, String> {
        let mut open = self.0.lock().map_err(|_| "Browser requests unavailable.")?;
        if open.values().any(|pending| pending.request.thread_id == thread) {
            return Err("This session already waits for browser access. Finish that request first.".into());
        }
        if open.len() >= MAX_OPEN {
            return Err("Other sessions wait for browser access. Try again after one is answered.".into());
        }
        let request = Request {
            id: format!("browser-{}", uuid::Uuid::new_v4()),
            thread_id: thread.into(),
            url: url.into(),
            tab,
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };
        open.insert(
            request.id.clone(),
            Pending {
                request: request.clone(),
                run: run.into(),
            },
        );
        Ok(request)
    }

    pub fn open(&self) -> Vec<Request> {
        self.0
            .lock()
            .map(|open| open.values().map(|pending| pending.request.clone()).collect())
            .unwrap_or_default()
    }

    /// The open request of `thread`, if any.
    pub fn of(&self, thread: &str) -> Option<Request> {
        self.open().into_iter().find(|request| request.thread_id == thread)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.0.lock().is_ok_and(|open| open.contains_key(id))
    }

    /// Answered, declined or given up: the request goes.
    pub fn remove(&self, id: &str) {
        if let Ok(mut open) = self.0.lock() {
            open.remove(id);
        }
    }

    pub fn end_run(&self, run: &str) {
        if let Ok(mut open) = self.0.lock() {
            open.retain(|_, pending| pending.run != run);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_use_waits_once_and_cancellation_does_not_grant_access() {
        let requests = Requests::default();
        let one = requests
            .ask("one", "run-one", "https://example.com", None)
            .expect("request");
        assert!(
            requests
                .ask("one", "another-run", "https://elsewhere.com", None)
                .is_err()
        );
        let two = requests
            .ask("two", "run-two", "https://example.com", Some("tab".into()))
            .expect("other session");
        assert_eq!(requests.of("two").map(|request| request.id), Some(two.id.clone()));
        requests.remove(&one.id);
        assert!(!requests.contains(&one.id));
        assert!(requests.contains(&two.id));
        requests.end_run("run-two");
        assert!(requests.open().is_empty());
    }

    #[test]
    fn waiting_requests_leave_socket_capacity_for_other_tools() {
        let requests = Requests::default();
        for index in 0..4 {
            requests
                .ask(&index.to_string(), "run", "https://example.com", None)
                .expect("request");
        }
        assert!(requests.ask("fifth", "run", "https://example.com", None).is_err());
    }
}
