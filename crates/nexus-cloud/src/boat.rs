//! [`Provider`] over boat.dev's API (`https://boat.dev/api/v1`, OpenAPI at
//! docs.boat.dev/openapi/boat-v1.yaml). boat calls a machine a "sandbox" (`bx_23456789`).
//!
//! Choices that hold for every call:
//!
//! - **Our own credentials only.** Every machine is created with `noEnv: true`: none of the boat account's secrets,
//!   saved logins or in-sandbox API key reach it. What the agent needs (the Claude token) the app writes over SSH.
//! - **No automatic stop.** `ttlSeconds: null`: boat's stop is a timer from start, not idleness, so it would stop a
//!   machine in the middle of a turn. The app (or the machine itself, [`crate::link`]) puts it to sleep when idle.
//! - **Created once.** Each create carries an `Idempotency-Key`, and a create whose answer was lost is retried with
//!   the same key, so a flaky network never starts (and bills) a second machine.
//! - **The key stays secret.** It goes in the `Authorization` header and nowhere else: not in errors, not in
//!   [`fmt::Debug`], not in logs. Redirects are not followed, so it is never sent to another host.
//! - **Errors in plain words** ([`error`]): the start limit says when the next start is possible, a machine asleep
//!   past 14 days says its disk is gone ([`Error::DataExpired`]), a refused key says so.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use futures::future::BoxFuture;
use reqwest::Method;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER};
use serde_json::{Value, json};

use crate::{Error, ExecOutput, Machine, Provider, Result, Size, Spec, SshAccess, State};

/// boat's API.
pub const DEFAULT_URL: &str = "https://boat.dev/api/v1";

/// How long an ordinary call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// How long [`Boat::desktop`] waits for boat to prepare the stream.
const DESKTOP_WAIT: Duration = Duration::from_secs(30);

/// rustls' ring provider for the process, once, before the first client: reqwest is built with
/// `rustls-no-provider` (the app's pin), which has none of its own. The same call as nexus-client's `install_tls`;
/// whichever runs first wins and the other is a no-op.
fn install_tls() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// boat's word for a machine's state, as one of ours. `provisioning` is [`State::Creating`] for a new machine and
/// [`State::Starting`] for one that has a saved disk (a wake: boat reports the same states while it finds a machine).
/// A state this client does not know yet counts as [`State::Starting`], so a wait goes on rather than failing.
pub fn state(raw: &str, error: Option<&str>, has_disk: bool) -> State {
    match raw {
        "init" | "provisioning" if has_disk => State::Starting,
        "init" | "provisioning" => State::Creating,
        "provisioned" | "cloning" => State::Starting,
        // `idle` and `running` only describe boat's own agent prompts, which we never use: the machine is up.
        "ready" | "idle" | "running" => State::Ready,
        "archiving" => State::Stopping,
        "archived" => State::Sleeping,
        "error" => State::Failed {
            message: error
                .filter(|e| !e.is_empty())
                .unwrap_or("boat reported an error.")
                .to_string(),
        },
        "cancelled" => State::Gone,
        _ => State::Starting,
    }
}

/// A `Sandbox` object as a [`Machine`].
fn machine(sandbox: &Value) -> Result<Machine> {
    let text = |key: &str| sandbox.get(key).and_then(Value::as_str).map(str::to_string);
    let id = text("id").ok_or_else(|| Error::Network("boat answered without a machine.".into()))?;
    let raw = text("state").unwrap_or_default();
    let has_disk = text("snapshotCompletedAt").is_some() || text("stoppedAt").is_some();
    let labels: BTreeMap<String, String> = sandbox
        .get("labels")
        .and_then(Value::as_object)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok(Machine {
        state: state(&raw, sandbox.get("error").and_then(Value::as_str), has_disk),
        name: text("name").unwrap_or_default(),
        provider_state: raw,
        size: text("type").as_deref().and_then(Size::parse),
        labels,
        stopped_at: text("stoppedAt"),
        data_expired_at: text("dataExpiredAt"),
        id,
    })
}

/// boat's error envelope (`{ ok: false, status, code, message, error: { code, message, status, details } }`) as an
/// [`Error`]. Fields are read from the top level first, then from `error` and `error.details`, since boat's older
/// answers and its 503s put them in different places. `retry_after` is the `Retry-After` header, in seconds.
pub fn error(status: u16, body: &Value, retry_after: Option<u64>) -> Error {
    let inner = body.get("error");
    let details = inner.and_then(|e| e.get("details"));
    let field = |key: &str| {
        body.get(key)
            .or_else(|| inner.and_then(|e| e.get(key)))
            .or_else(|| details.and_then(|d| d.get(key)))
    };
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| inner.and_then(|e| e.get("code")).and_then(Value::as_str))
        .or_else(|| inner.and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let message = field("message").and_then(Value::as_str).unwrap_or("").to_string();
    if code == "snapshot_data_expired" {
        return Error::DataExpired;
    }
    if status == 404 {
        return Error::NotFound;
    }
    if status == 401 || code == "api_key_expired" {
        return Error::Unauthorized(
            "boat did not accept the API key: it may be wrong, revoked or expired. Paste a new one in Settings.".into(),
        );
    }
    if status == 429 && code == "rate_limited" {
        return Error::RateLimited {
            message: if message.is_empty() {
                "boat's limit on machine starts was reached; try again in a few minutes.".into()
            } else {
                format!("boat's limit on machine starts was reached: {message}")
            },
            next_start_at: field("nextStartAt").and_then(Value::as_str).map(str::to_string),
            retry_after_secs: field("retryAfterSeconds").and_then(Value::as_u64).or(retry_after),
        };
    }
    let said = |prefix: &str| {
        if message.is_empty() {
            format!("{prefix}.")
        } else {
            format!("{prefix}: {message}")
        }
    };
    let message = match code.as_str() {
        "limit_reached" | "member_limit_reached" => {
            said("The boat plan's limit on machines running at once was reached; put another machine to sleep first")
        }
        "billing_required" | "credit_required" => said("boat needs a payment method or more credit"),
        "machine_class_plan_required" | "trial_machine_class_not_allowed" => {
            said("This machine size needs a bigger boat plan")
        }
        "out_of_capacity" | "no_ready_machine" => {
            "boat has no free machine right now; try again in a few minutes.".into()
        }
        "api_key_action_forbidden" | "api_key_sandbox_forbidden" => {
            said("The boat API key may not do this (make a key with the `ci` preset)")
        }
        _ if message.is_empty() => format!("boat refused the request ({status} {code})."),
        _ => format!("boat: {message}"),
    };
    Error::Api { status, code, message }
}

/// A boat id as a path segment: letters, digits, `_` and `-` only, so it cannot reach another endpoint.
fn checked(id: &str) -> Result<&str> {
    let ok = !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(id)
    } else {
        Err(Error::Other(format!("{id:?} is not a boat machine id.")))
    }
}

/// `host:port`, `[v6]:port`, or a bare address (port 22).
fn endpoint(value: &str) -> Option<(String, u16)> {
    if let Some(rest) = value.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        return Some((host.to_string(), port.parse().ok()?));
    }
    match value.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Some((host.to_string(), port.parse().ok()?)),
        _ => Some((value.to_string(), 22)),
    }
}

/// boat's API with one key.
#[derive(Clone)]
pub struct Boat {
    http: reqwest::Client,
    base: String,
    key: String,
    org: Option<String>,
}

impl fmt::Debug for Boat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Boat")
            .field("base", &self.base)
            .field("key", &"<redacted>")
            .field("org", &self.org)
            .finish()
    }
}

impl Boat {
    /// A client for `base` (None: [`DEFAULT_URL`]; the app passes `NEXUS_BOAT_URL` when set), with `key`
    /// (`boat_…`) and, for an account key billing an organization, `org` (sent as `X-Boat-Org`). Plain `http` is
    /// accepted only for this computer (the fake server), so the key never crosses a network in the clear.
    pub fn new(base: Option<&str>, key: impl Into<String>, org: Option<String>) -> Result<Boat> {
        let base = base
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .unwrap_or(DEFAULT_URL)
            .trim_end_matches('/')
            .to_string();
        let parsed = reqwest::Url::parse(&base).map_err(|_| Error::Other(format!("{base:?} is not a URL.")))?;
        let loopback = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
            return Err(Error::Other(format!("{base} is not an https URL.")));
        }
        let key = key.into();
        if key.trim().is_empty() || HeaderValue::from_str(&format!("Bearer {key}")).is_err() {
            return Err(Error::Unauthorized("The boat API key is empty or malformed.".into()));
        }
        if let Some(org) = &org
            && HeaderValue::from_str(org).is_err()
        {
            return Err(Error::Other("The boat organization name is malformed.".into()));
        }
        install_tls();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!("nexus-desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Other(format!("Could not create the boat client: {e}")))?;
        Ok(Boat { http, base, key, org })
    }

    /// One call: JSON in, JSON out, boat's errors as [`Error`].
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&'static str, String)],
        timeout: Duration,
    ) -> Result<Value> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header(AUTHORIZATION, format!("Bearer {}", self.key))
            .header(ACCEPT, "application/json")
            .timeout(timeout);
        if let Some(org) = &self.org {
            request = request.header("X-Boat-Org", org);
        }
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if let Some(body) = body {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(&body).unwrap_or_default());
        }
        // reqwest's errors name the URL (a machine id at most), never the headers.
        let response = request
            .send()
            .await
            .map_err(|e| Error::Network(format!("Could not reach boat: {e}")))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok());
        let bytes = response
            .bytes()
            .await
            .map_err(|e| Error::Network(format!("boat's answer was cut off: {e}")))?;
        let json: Option<Value> = if bytes.is_empty() {
            Some(Value::Null)
        } else {
            serde_json::from_slice(&bytes).ok()
        };
        match json {
            Some(json) if (200..300).contains(&status) => Ok(json),
            Some(json) => Err(error(status, &json, retry_after)),
            None if (500..600).contains(&status) => Err(Error::Api {
                status,
                code: String::new(),
                message: format!("boat is having trouble ({status}); try again in a moment."),
            }),
            None => Err(Error::Network(format!(
                "boat answered something that is not its API ({status})."
            ))),
        }
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::POST, path, Some(body), &[], CALL_TIMEOUT).await
    }

    async fn create_machine(&self, spec: Spec) -> Result<String> {
        let body = json!({
            "name": spec.name,
            "type": spec.size.as_str(),
            "labels": spec.labels,
            "noEnv": true,
            "ttlSeconds": null,
        });
        let key = uuid::Uuid::new_v4().to_string();
        let mut tries = 0;
        loop {
            tries += 1;
            let answer = self
                .call(
                    Method::POST,
                    "/sandboxes",
                    Some(body.clone()),
                    &[("Idempotency-Key", key.clone())],
                    CALL_TIMEOUT,
                )
                .await;
            match answer {
                Ok(answer) => {
                    return answer
                        .get("sandbox")
                        .and_then(|s| s.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| Error::Network("boat answered the create without a machine.".into()));
                }
                // Lost answers and boat's own hiccups are safe to repeat with the same key: boat returns the
                // machine the first try made. Its out-of-machines answer is not: the person should hear it.
                Err(error)
                    if tries < 3
                        && (matches!(error, Error::Network(_))
                            || matches!(&error, Error::Api { status, code, .. }
                                if code == "idempotency_in_progress" || (*status >= 500 && code != "out_of_capacity" && code != "no_ready_machine"))) =>
                {
                    tokio::time::sleep(Duration::from_secs(tries)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn get_machine(&self, id: &str) -> Result<Machine> {
        let path = format!("/sandboxes/{}", checked(id)?);
        match self.call(Method::GET, &path, None, &[], CALL_TIMEOUT).await {
            Ok(answer) => machine(answer.get("sandbox").unwrap_or(&Value::Null)),
            Err(Error::NotFound) => Ok(Machine {
                id: id.to_string(),
                name: String::new(),
                state: State::Gone,
                provider_state: "deleted".into(),
                size: None,
                labels: BTreeMap::new(),
                stopped_at: None,
                data_expired_at: None,
            }),
            Err(error) => Err(error),
        }
    }

    async fn sleep_machine(&self, id: &str) -> Result<()> {
        let path = format!("/sandboxes/{}/stop", checked(id)?);
        match self.post(&path, json!({})).await {
            Ok(_) => Ok(()),
            // Already asleep or on its way: what was asked for.
            Err(Error::Api { status: 409, .. }) | Err(Error::Api { status: 400, .. })
                if matches!(
                    self.get_machine(id).await.map(|m| m.state),
                    Ok(State::Sleeping | State::Stopping)
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn wake_machine(&self, id: &str, size: Option<Size>) -> Result<()> {
        let path = format!("/sandboxes/{}/resume", checked(id)?);
        // The TTL (none) and noEnv carry over from the create when left out.
        let mut body = json!({});
        if let Some(size) = size {
            body["type"] = json!(size.as_str());
        }
        match self.post(&path, body).await {
            Ok(_) => Ok(()),
            // Already up or on its way: what was asked for (unless a new size was asked for, which needs a sleep).
            Err(Error::Api { status: 409, .. })
                if size.is_none()
                    && matches!(
                        self.get_machine(id).await.map(|m| m.state),
                        Ok(State::Ready | State::Starting | State::Creating)
                    ) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn delete_machine(&self, id: &str) -> Result<()> {
        let path = format!("/sandboxes/{}", checked(id)?);
        // boat asks for the id again in a header, so a wrong path never deletes the wrong machine.
        match self
            .call(
                Method::DELETE,
                &path,
                None,
                &[("X-Ascii-Confirm-Delete", id.to_string())],
                CALL_TIMEOUT,
            )
            .await
        {
            Ok(_) | Err(Error::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn authorize(&self, id: &str, public_key: &str) -> Result<SshAccess> {
        let public_key = public_key.trim();
        if !public_key.starts_with("ssh-") || public_key.contains("PRIVATE KEY") {
            return Err(Error::Other("That is not an SSH public key.".into()));
        }
        let path = format!("/sandboxes/{}/sshkey", checked(id)?);
        let answer = self.post(&path, json!({ "key": public_key })).await?;
        let text = |key: &str| {
            answer
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        // `sshEndpoint` is set when the machine has no public IPv4 of its own: connect there instead.
        let (host, port) = match (text("sshEndpoint"), text("machineIp")) {
            (Some(at), _) => {
                endpoint(&at).ok_or_else(|| Error::Network(format!("boat gave an odd SSH address: {at}")))?
            }
            (None, Some(ip)) => (ip, 22),
            (None, None) => return Err(Error::Network("boat did not say where the machine is.".into())),
        };
        let host_key = text("hostKey").ok_or_else(|| Error::Api {
            status: 503,
            code: "ssh_host_key_unavailable".into(),
            message: "boat did not give the machine's host key yet.".into(),
        })?;
        Ok(SshAccess {
            host,
            port,
            user: text("sshUser").unwrap_or_else(|| "user".into()),
            host_key,
        })
    }

    async fn host_port(&self, id: &str, port: u16, public: bool) -> Result<String> {
        if port == 0 {
            return Err(Error::Other("Port 0 cannot be previewed.".into()));
        }
        let path = format!("/sandboxes/{}/host", checked(id)?);
        // `localhost: true` has boat relay to 127.0.0.1, which reaches servers bound to loopback only (Vite's
        // default) and those bound to every address alike.
        let answer = self
            .post(&path, json!({ "port": port, "public": public, "localhost": true }))
            .await?;
        answer
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::Network("boat did not give a preview link.".into()))
    }

    async fn desktop_url(&self, id: &str) -> Result<String> {
        let path = format!("/sandboxes/{}/desktop", checked(id)?);
        let started = std::time::Instant::now();
        loop {
            let answer = self.post(&path, json!({})).await;
            match answer {
                Ok(answer) => {
                    if let Some(url) = answer.get("desktopUrl").and_then(Value::as_str) {
                        return Ok(url.to_string());
                    }
                }
                Err(Error::Api { code, .. }) if code == "desktop_not_ready" || code == "browser_not_ready" => {}
                Err(error) => return Err(error),
            }
            if started.elapsed() >= DESKTOP_WAIT {
                return Err(Error::Other(
                    "The machine's desktop is not ready yet; try again in a minute.".into(),
                ));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn run(&self, id: &str, command: &str, timeout: Duration) -> Result<ExecOutput> {
        let path = format!("/sandboxes/{}/commands", checked(id)?);
        let seconds = timeout.as_secs().clamp(1, 600);
        let answer = self
            .call(
                Method::POST,
                &path,
                Some(json!({ "command": command, "timeoutSeconds": seconds })),
                &[],
                Duration::from_secs(seconds + 30),
            )
            .await?;
        let text = |key: &str| answer.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        Ok(ExecOutput {
            exit_code: answer
                .get("exitCode")
                .and_then(Value::as_i64)
                .and_then(|c| i32::try_from(c).ok()),
            stdout: text("stdout"),
            stderr: text("stderr"),
            timed_out: answer.get("timedOut").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    async fn stop_key(&self, id: &str) -> Result<Option<String>> {
        let id = checked(id)?;
        let body = json!({
            "name": format!("nexus-cloud-stop-{id}"),
            "ttl": "365d",
            "actions": ["sandbox.read", "sandbox.stop"],
            "sandboxIds": [id],
        });
        match self.post("/api-keys/scoped", body).await {
            Ok(answer) => Ok(answer
                .get("secret")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)),
            // A key that may not make keys (a `ci` key), or scoped keys not switched on for the account: the app
            // sleeps the machine itself instead.
            Err(Error::Api { status: 403 | 503, .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl Provider for Boat {
    fn create(&self, spec: Spec) -> BoxFuture<'_, Result<String>> {
        Box::pin(self.create_machine(spec))
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Machine>> {
        Box::pin(self.get_machine(id))
    }

    fn sleep<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.sleep_machine(id))
    }

    fn wake<'a>(&'a self, id: &'a str, size: Option<Size>) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.wake_machine(id, size))
    }

    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.delete_machine(id))
    }

    fn authorize_ssh<'a>(&'a self, id: &'a str, public_key: &'a str) -> BoxFuture<'a, Result<SshAccess>> {
        Box::pin(self.authorize(id, public_key))
    }

    fn preview<'a>(&'a self, id: &'a str, port: u16, public: bool) -> BoxFuture<'a, Result<String>> {
        Box::pin(self.host_port(id, port, public))
    }

    fn desktop<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(self.desktop_url(id))
    }

    fn exec<'a>(&'a self, id: &'a str, command: &'a str, timeout: Duration) -> BoxFuture<'a, Result<ExecOutput>> {
        Box::pin(self.run(id, command, timeout))
    }

    fn mint_stop_key<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(self.stop_key(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_boat_state_has_a_place() {
        let cases = [
            ("init", false, State::Creating),
            ("provisioning", false, State::Creating),
            ("provisioning", true, State::Starting),
            ("provisioned", false, State::Starting),
            ("cloning", false, State::Starting),
            ("ready", false, State::Ready),
            ("idle", false, State::Ready),
            ("running", false, State::Ready),
            ("archiving", false, State::Stopping),
            ("archived", false, State::Sleeping),
            ("cancelled", false, State::Gone),
            ("something-new", false, State::Starting),
        ];
        for (raw, disk, want) in cases {
            assert_eq!(state(raw, None, disk), want, "{raw}");
        }
        assert_eq!(
            state("error", Some("disk full"), false),
            State::Failed {
                message: "disk full".into()
            }
        );
        assert!(matches!(state("error", None, false), State::Failed { .. }));
    }

    #[test]
    fn errors_from_the_envelope() {
        let expired = json!({ "ok": false, "status": 409, "code": "snapshot_data_expired", "message": "gone",
            "error": { "code": "snapshot_data_expired", "message": "gone", "status": 409 } });
        assert_eq!(error(409, &expired, None), Error::DataExpired);
        let limited = json!({ "ok": false, "status": 429, "code": "rate_limited", "message": "5 starts per minute",
            "nextStartAt": "2026-11-01T12:00:00Z", "retryAfterSeconds": 30,
            "error": { "code": "rate_limited", "message": "5 starts per minute", "status": 429 } });
        assert_eq!(
            error(429, &limited, None),
            Error::RateLimited {
                message: "boat's limit on machine starts was reached: 5 starts per minute".into(),
                next_start_at: Some("2026-11-01T12:00:00Z".into()),
                retry_after_secs: Some(30),
            }
        );
        // Fields only under `error.details`, and the header as the fallback.
        let nested = json!({ "error": { "code": "rate_limited", "message": "m", "status": 429,
            "details": { "nextStartAt": "2026-11-01T13:00:00Z" } } });
        assert!(matches!(
            error(429, &nested, Some(12)),
            Error::RateLimited { next_start_at: Some(at), retry_after_secs: Some(12), .. } if at == "2026-11-01T13:00:00Z"
        ));
        // The 503 of scoped keys has `error` as a string.
        let disabled = json!({ "error": "scoped_api_key_creation_disabled", "message": "not yet",
            "scopedCreationEnabled": false });
        assert!(matches!(
            error(503, &disabled, None),
            Error::Api { status: 503, code, .. } if code == "scoped_api_key_creation_disabled"
        ));
        assert!(matches!(error(401, &json!({}), None), Error::Unauthorized(_)));
        assert_eq!(error(404, &json!({}), None), Error::NotFound);
        let billing = error(
            402,
            &json!({ "code": "billing_required", "message": "Add a card." }),
            None,
        );
        assert_eq!(
            billing.to_string(),
            "boat needs a payment method or more credit: Add a card."
        );
        assert_eq!(
            error(418, &json!({}), None).to_string(),
            "boat refused the request (418 )."
        );
    }

    #[test]
    fn endpoints_and_ids() {
        assert_eq!(endpoint("203.0.113.10:22001"), Some(("203.0.113.10".into(), 22001)));
        assert_eq!(endpoint("203.0.113.10"), Some(("203.0.113.10".into(), 22)));
        assert_eq!(endpoint("[2001:db8::1]:2222"), Some(("2001:db8::1".into(), 2222)));
        assert_eq!(endpoint("2001:db8::1"), Some(("2001:db8::1".into(), 22)));
        assert_eq!(endpoint("host:notaport"), None);
        assert!(checked("bx_23456789").is_ok());
        assert!(checked("../api-keys").is_err());
        assert!(checked("bx_1?x=1").is_err());
        assert!(checked("").is_err());
    }

    #[test]
    fn the_key_stays_out_of_debug_and_http_needs_loopback() {
        let boat = Boat::new(Some("http://127.0.0.1:9/api/v1/"), "boat_secret", None).unwrap_or_else(|e| panic!("{e}"));
        assert!(!format!("{boat:?}").contains("boat_secret"));
        assert_eq!(boat.base, "http://127.0.0.1:9/api/v1");
        assert!(Boat::new(Some("http://boat.example/api/v1"), "k", None).is_err());
        assert!(Boat::new(None, "", None).is_err());
        assert!(Boat::new(None, "k\nX: y", None).is_err());
        assert_eq!(
            Boat::new(None, "k", None).map(|b| b.base).unwrap_or_default(),
            DEFAULT_URL
        );
    }
}
