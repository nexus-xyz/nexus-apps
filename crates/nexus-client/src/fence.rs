//! A runner's hold on a native session (`/v1/sessions`).
//!
//! The runner that runs a session's agent takes the session ([`Runner::take`]) and gets a fencing token and a 60 s
//! lease. It renews the lease every 20 s ([`Runner::hold`]). Each connector change made inside the session carries the
//! session and the token ([`SESSION_HEADER`], [`TOKEN_HEADER`]), so once another runner took the session over, Gateway
//! refuses the stale runner's change with 409 `lost_runner` before the outside system is called.
//!
//! - [`Fence`]: the session and the token a runner's changes carry, and whether they stopped. It stops on 409
//!   `lost_runner`, 409 `session_ended` or 404 `session_not_found`, from a renewal or from a change. A stopped fence
//!   sends no more changes: each is refused here with the reason. Only changes stop: reads, and the calls that report
//!   and settle what the session spent, go on.
//! - [`Runner`]: the take, the handoff, and the renewals, on the runner's normal Gateway sign-in.
//! - The runner hands the fence to the agent's own processes (the Nexus MCP server, `nexus mcp`) in their environment
//!   ([`SESSION_VAR`], [`TOKEN_VAR`]): a counter and a session id, never a credential. They read it with
//!   [`Fence::from_env`].

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};

use crate::url::enc;
use crate::{Client, CoreError, Error, Method, Result};

/// The header a connector change names its native session in (`cs_…`).
pub const SESSION_HEADER: &str = "x-nexus-cloud-session";
/// The header a connector change carries the runner's fencing token in. Both headers or neither.
pub const TOKEN_HEADER: &str = "x-nexus-fencing-token";
/// The variable a runner hands its fence's session to the agent's processes in.
pub const SESSION_VAR: &str = "NEXUS_CLOUD_SESSION";
/// The variable a runner hands its fence's token to the agent's processes in.
pub const TOKEN_VAR: &str = "NEXUS_FENCING_TOKEN";
/// How often a runner renews its lease: a third of Gateway's 60 s lease, so one renewal may fail.
pub const RENEW_EVERY: Duration = Duration::from_secs(20);

/// Whether a call is a connector change, `POST /v1/connectors/{name}/{operation}`. Its read twin (`…/read`) has one
/// more segment, and path parameters are encoded, so a name holds no `/`.
pub fn is_connector_write(method: &Method, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    *method == Method::POST && path.starts_with("/v1/connectors/") && path.matches('/').count() == 4
}

/// Why a fence stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// 409 `lost_runner`: another runner took the session over.
    LostRunner,
    /// 409 `session_ended`: the session was stopped or failed.
    SessionEnded,
    /// 404 `session_not_found`: the session is not the caller's, or is gone.
    SessionNotFound,
    /// The runner handed an unusable fence (one variable without the other, or a malformed one).
    Invalid(String),
}

impl Stopped {
    /// The stop a Gateway error means, if any.
    pub fn of(error: &Error) -> Option<Stopped> {
        let Error::Core(error) = error else {
            return None;
        };
        match (error.status, error.code.as_deref()) {
            (409, Some("lost_runner")) => Some(Stopped::LostRunner),
            (409, Some("session_ended")) => Some(Stopped::SessionEnded),
            (404, Some("session_not_found")) => Some(Stopped::SessionNotFound),
            _ => None,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Stopped::LostRunner => "lost_runner",
            Stopped::SessionEnded => "session_ended",
            Stopped::SessionNotFound => "session_not_found",
            Stopped::Invalid(_) => "invalid_session_fence",
        }
    }

    /// The refusal a change gets once the fence stopped: Gateway's own status and code, and words for the agent.
    pub fn refusal(&self) -> Error {
        let (status, message) = match self {
            Stopped::LostRunner => (
                409,
                "Another runner took this session over, so this runner makes no more changes in it.".to_string(),
            ),
            Stopped::SessionEnded => (409, "This session ended, so it takes no more changes.".to_string()),
            Stopped::SessionNotFound => (404, "This session is gone, so it takes no more changes.".to_string()),
            Stopped::Invalid(why) => (400, format!("This runner's session fence is unusable: {why}")),
        };
        Error::from(CoreError {
            status,
            message,
            code: Some(self.code().to_string()),
            interval: None,
            details: None,
        })
    }
}

#[derive(Debug)]
struct State {
    session: String,
    token: i64,
    stopped: Option<Stopped>,
}

/// The session and the fencing token a runner's connector changes carry, shared by the runner and its callers.
#[derive(Debug, Clone)]
pub struct Fence(Arc<Mutex<State>>);

impl Fence {
    pub fn new(session: &str, token: i64) -> Fence {
        Fence(Arc::new(Mutex::new(State {
            session: session.to_string(),
            token,
            stopped: None,
        })))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The fence a runner handed this process, from `var` (the environment): none when neither variable is set. One
    /// without the other, or a malformed one, is a fence that is already stopped, so no change goes unfenced.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Option<Fence> {
        let set = |name: &str| var(name).filter(|value| !value.trim().is_empty());
        match (set(SESSION_VAR), set(TOKEN_VAR)) {
            (None, None) => None,
            (Some(session), Some(token)) => {
                let session = session.trim();
                match token.trim().parse::<i64>() {
                    Ok(token) if token > 0 && valid_session(session) => Some(Fence::new(session, token)),
                    _ => Some(Fence::stopped(Stopped::Invalid(format!(
                        "{SESSION_VAR} must be a cs_ session id and {TOKEN_VAR} a token above 0"
                    )))),
                }
            }
            _ => Some(Fence::stopped(Stopped::Invalid(format!(
                "{SESSION_VAR} and {TOKEN_VAR} are set together or not at all"
            )))),
        }
    }

    fn stopped(why: Stopped) -> Fence {
        let fence = Fence::new("", 0);
        fence.stop(why);
        fence
    }

    pub fn session(&self) -> String {
        self.state().session.clone()
    }

    pub fn token(&self) -> i64 {
        self.state().token
    }

    /// Why changes stopped, if they did.
    pub fn stopped_by(&self) -> Option<Stopped> {
        self.state().stopped.clone()
    }

    /// Stops the changes; the first reason stays.
    pub fn stop(&self, why: Stopped) {
        let mut state = self.state();
        if state.stopped.is_none() {
            state.stopped = Some(why);
        }
    }

    /// The variables that hand this fence to a child process ([`Fence::from_env`]).
    pub fn env(&self) -> Vec<(String, String)> {
        let state = self.state();
        vec![
            (SESSION_VAR.to_string(), state.session.clone()),
            (TOKEN_VAR.to_string(), state.token.to_string()),
        ]
    }

    /// The headers of `method` `path`: a connector change gets the session and the token in place of any it named
    /// itself, and is refused once the fence stopped. Any other call is left as it is.
    pub fn apply(&self, method: &Method, path: &str, headers: &mut Vec<(String, String)>) -> Result<()> {
        if !is_connector_write(method, path) {
            return Ok(());
        }
        let state = self.state();
        if let Some(stopped) = &state.stopped {
            return Err(stopped.refusal());
        }
        headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case(SESSION_HEADER) && !name.eq_ignore_ascii_case(TOKEN_HEADER));
        headers.push((SESSION_HEADER.to_string(), state.session.clone()));
        headers.push((TOKEN_HEADER.to_string(), state.token.to_string()));
        Ok(())
    }

    /// Stops the fence when `answer` says the runner lost the session or the session ended.
    pub fn observe<T>(&self, answer: &Result<T>) {
        if let Err(error) = answer
            && let Some(why) = Stopped::of(error)
        {
            self.stop(why);
        }
    }

    /// One call through the fence: [`Fence::apply`], `send`, then [`Fence::observe`].
    pub fn send<T>(
        &self,
        method: Method,
        path: &str,
        mut headers: Vec<(String, String)>,
        send: impl FnOnce(Method, &str, Vec<(String, String)>) -> Result<T>,
    ) -> Result<T> {
        let write = is_connector_write(&method, path);
        self.apply(&method, path, &mut headers)?;
        let answer = send(method, path, headers);
        if write {
            self.observe(&answer);
        }
        answer
    }
}

/// A session id as Gateway takes one in the header: `cs_` and 1 to 64 letters, digits, `_` and `-`.
pub fn valid_session(id: &str) -> bool {
    id.strip_prefix("cs_").is_some_and(|tail| {
        (1..=64).contains(&tail.len())
            && tail
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// The token Gateway says the session holds now, from a 409 `takeover_conflict` (`error.nexus.currentFencingToken`).
pub fn conflict_token(error: &Error) -> Option<i64> {
    match error {
        Error::Core(error) if error.status == 409 && error.code.as_deref() == Some("takeover_conflict") => {
            error.details.as_ref()?.get("currentFencingToken")?.as_i64()
        }
        _ => None,
    }
}

/// A runner's hold on a native session: the take, a handoff, and the renewals. The fence it hands out stops when a
/// renewal says the runner lost the session or the session ended.
#[derive(Debug, Clone)]
pub struct Runner {
    client: Client,
    runner_id: String,
    fence: Fence,
}

impl Runner {
    /// Takes `session` as `runner_id` (`POST /v1/sessions/{id}/runner`). Without `seen`, only when no other runner's
    /// lease is live; with it, a handoff from whoever holds the token `seen`. Another runner's live lease is 409
    /// `takeover_conflict` ([`conflict_token`] reads the session's token from it).
    pub async fn take(client: &Client, session: &str, runner_id: &str, seen: Option<i64>) -> Result<Runner> {
        let mut body = json!({ "runnerId": runner_id });
        if let Some(seen) = seen {
            body["seenToken"] = json!(seen);
        }
        let taken = client
            .post(&format!("/v1/sessions/{}/runner", enc(session)), body)
            .await?;
        let token = token_of(&taken)?;
        Ok(Runner {
            client: client.clone(),
            runner_id: runner_id.to_string(),
            fence: Fence::new(session, token),
        })
    }

    /// [`Runner::take`], and when another lease is live, a handoff only from this same runner (`runner_id`), as after
    /// the runner's process restarted within its lease. Another runner's session stays theirs: 409
    /// `takeover_conflict`.
    pub async fn take_or_resume(client: &Client, session: &str, runner_id: &str) -> Result<Runner> {
        match Runner::take(client, session, runner_id, None).await {
            Err(error) if conflict_token(&error).is_some() => {
                let read = client.get(&format!("/v1/sessions/{}", enc(session))).await?;
                let holder = read["runner"]["runnerId"].as_str().unwrap_or_default();
                let current = read["fencingToken"].as_i64();
                match current {
                    Some(current) if holder == runner_id => {
                        Runner::take(client, session, runner_id, Some(current)).await
                    }
                    _ => Err(error),
                }
            }
            other => other,
        }
    }

    pub fn fence(&self) -> Fence {
        self.fence.clone()
    }

    pub fn runner_id(&self) -> &str {
        &self.runner_id
    }

    /// One renewal (`POST /v1/sessions/{id}/runner/renew`). 409 `lost_runner`, 409 `session_ended` and 404
    /// `session_not_found` stop the fence; a stopped fence renews no more.
    pub async fn renew(&self) -> Result<()> {
        if let Some(stopped) = self.fence.stopped_by() {
            return Err(stopped.refusal());
        }
        let path = format!("/v1/sessions/{}/runner/renew", enc(&self.fence.session()));
        let renewed = self
            .client
            .post(&path, json!({ "fencingToken": self.fence.token() }))
            .await
            .map(|_| ());
        self.fence.observe(&renewed);
        renewed
    }

    /// Renews every [`RENEW_EVERY`] until the fence stops, and says why. A renewal that fails for another reason (no
    /// answer, a 5xx) is tried again at the next turn: the lease outlasts two of them.
    pub async fn hold(&self) -> Stopped {
        self.hold_every(RENEW_EVERY).await
    }

    /// [`Runner::hold`] at `every`.
    pub async fn hold_every(&self, every: Duration) -> Stopped {
        loop {
            if let Some(stopped) = self.fence.stopped_by() {
                return stopped;
            }
            tokio::time::sleep(every).await;
            let _ = self.renew().await;
        }
    }
}

fn token_of(answer: &Value) -> Result<i64> {
    answer["fencingToken"]
        .as_i64()
        .filter(|token| *token > 0)
        .ok_or_else(|| {
            Error::from(crate::Transport::Other(
                "Nexus Gateway took the session but sent no fencing token".into(),
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(status: u16, code: &str) -> Error {
        Error::from(CoreError {
            status,
            message: code.into(),
            code: Some(code.into()),
            interval: None,
            details: None,
        })
    }

    const WRITE: &str = "/v1/connectors/github/pull_review";

    #[test]
    fn only_a_connector_change_is_fenced() {
        assert!(is_connector_write(&Method::POST, WRITE));
        assert!(is_connector_write(
            &Method::POST,
            "/v1/connectors/github/pull_review?x=1"
        ));
        assert!(!is_connector_write(
            &Method::POST,
            "/v1/connectors/github/pull_review/read"
        ));
        assert!(!is_connector_write(&Method::GET, WRITE));
        assert!(!is_connector_write(&Method::POST, "/v1/connectors/github"));
        let fence = Fence::new("cs_abc", 3);
        let mut headers = vec![("accept".to_string(), "text/plain".to_string())];
        fence
            .apply(&Method::POST, "/v1/connectors/github/pull_review/read", &mut headers)
            .expect("a read");
        assert_eq!(headers.len(), 1);
    }

    #[test]
    fn a_change_carries_the_session_and_the_token_in_place_of_its_own() {
        let fence = Fence::new("cs_abc", 3);
        let mut headers = vec![
            (SESSION_HEADER.to_uppercase(), "cs_forged".to_string()),
            (TOKEN_HEADER.to_string(), "99".to_string()),
            ("x-nexus-approval".to_string(), "apr_1".to_string()),
        ];
        fence.apply(&Method::POST, WRITE, &mut headers).expect("fenced");
        assert_eq!(
            headers,
            vec![
                ("x-nexus-approval".to_string(), "apr_1".to_string()),
                (SESSION_HEADER.to_string(), "cs_abc".to_string()),
                (TOKEN_HEADER.to_string(), "3".to_string()),
            ]
        );
    }

    #[test]
    fn lost_runner_stops_every_later_change_and_nothing_else() {
        let fence = Fence::new("cs_abc", 3);
        let sent = std::cell::RefCell::new(Vec::new());
        let send = |answer: Result<Value>| {
            let sent = &sent;
            move |_: Method, path: &str, _: Vec<(String, String)>| {
                sent.borrow_mut().push(path.to_string());
                answer
            }
        };
        assert!(fence.send(Method::POST, WRITE, vec![], send(Ok(json!({})))).is_ok());
        // A read that fails with the same code does not stop the changes: only a change's answer or a renewal does.
        let _ = fence.send(
            Method::GET,
            "/v1/sessions/cs_abc",
            vec![],
            send(Err(core(409, "lost_runner"))),
        );
        assert_eq!(fence.stopped_by(), None);
        let lost = fence.send(Method::POST, WRITE, vec![], send(Err(core(409, "lost_runner"))));
        assert!(matches!(&lost, Err(Error::Core(e)) if e.code.as_deref() == Some("lost_runner")));
        assert_eq!(fence.stopped_by(), Some(Stopped::LostRunner));
        let refused = fence.send(Method::POST, WRITE, vec![], send(Ok(json!({}))));
        let Err(Error::Core(refused)) = refused else {
            panic!("refused here")
        };
        assert_eq!((refused.status, refused.code.as_deref()), (409, Some("lost_runner")));
        assert!(refused.message.contains("Another runner took this session over"));
        assert_eq!(sent.borrow().len(), 3, "the refused change never left");
        // Reads and settling go on.
        assert!(
            fence
                .send(Method::POST, "/v1/agents/sessions/s_1/end", vec![], send(Ok(json!({}))))
                .is_ok()
        );
    }

    #[test]
    fn session_ended_and_not_found_stop_too_and_the_first_reason_stays() {
        for (status, code, why) in [
            (409, "session_ended", Stopped::SessionEnded),
            (404, "session_not_found", Stopped::SessionNotFound),
        ] {
            let fence = Fence::new("cs_abc", 1);
            fence.observe::<()>(&Err(core(status, code)));
            assert_eq!(fence.stopped_by(), Some(why.clone()));
            fence.observe::<()>(&Err(core(409, "lost_runner")));
            assert_eq!(fence.stopped_by(), Some(why));
        }
        let fence = Fence::new("cs_abc", 1);
        fence.observe::<()>(&Err(core(409, "takeover_conflict")));
        fence.observe::<()>(&Err(core(503, "unavailable")));
        assert_eq!(fence.stopped_by(), None);
    }

    #[test]
    fn the_environment_hands_the_fence_over_whole_or_not_at_all() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
        };
        assert!(Fence::from_env(env(&[])).is_none());
        let fence = Fence::from_env(env(&[(SESSION_VAR, "cs_abc"), (TOKEN_VAR, "7")])).expect("a fence");
        assert_eq!(
            (fence.session(), fence.token(), fence.stopped_by()),
            ("cs_abc".into(), 7, None)
        );
        let handed = Fence::from_env(|name| fence.env().into_iter().find(|(k, _)| k == name).map(|(_, v)| v));
        assert_eq!(handed.map(|f| (f.session(), f.token())), Some(("cs_abc".into(), 7)));
        for broken in [
            Fence::from_env(env(&[(SESSION_VAR, "cs_abc")])),
            Fence::from_env(env(&[(TOKEN_VAR, "7")])),
            Fence::from_env(env(&[(SESSION_VAR, "cs_abc"), (TOKEN_VAR, "0")])),
            Fence::from_env(env(&[(SESSION_VAR, "sess_abc"), (TOKEN_VAR, "7")])),
        ] {
            let broken = broken.expect("a stopped fence");
            assert!(matches!(broken.stopped_by(), Some(Stopped::Invalid(_))));
            assert!(
                broken.apply(&Method::POST, WRITE, &mut vec![]).is_err(),
                "no change goes unfenced"
            );
        }
    }

    #[test]
    fn a_takeover_conflict_names_the_current_token() {
        let mut error = CoreError {
            status: 409,
            message: "held".into(),
            code: Some("takeover_conflict".into()),
            interval: None,
            details: Some(json!({ "currentFencingToken": 4 })),
        };
        assert_eq!(conflict_token(&Error::from(error.clone())), Some(4));
        error.code = Some("lost_runner".into());
        assert_eq!(conflict_token(&Error::from(error)), None);
    }
}
