//! The device-code sign-in (`nexus auth login`): Gateway issues a code, a member approves it in Nexus UI, and
//! the client polls `POST /v1/auth/token` until it gets a personal token. This module holds the protocol (the request
//! bodies, what each answer means, the wait between polls) and an async loop for apps; what a person sees while it
//! waits is the caller's.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::error::{Error, Result, Transport};
use crate::url::enc;

/// Where a client starts a device-code sign-in.
pub const START_PATH: &str = "/v1/auth/device";
/// Where a client polls for the personal token.
pub const TOKEN_PATH: &str = "/v1/auth/token";
/// The OAuth 2.0 device grant (RFC 8628) that the token poll names.
pub const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The `nexus` CLI's OAuth client id, as `nexus_types::oauth_clients::CLI`.
pub const CLI_CLIENT_ID: &str = "nxc_cli";
/// The one shared OAuth client id of every client with no id of its own, as
/// `nexus_types::oauth_clients::UNREGISTERED`.
pub const UNREGISTERED_CLIENT_ID: &str = "nxc_unregistered";
/// Nexus Desktop's OAuth client id: `NEXUS_DESKTOP_CLIENT_ID` at build time, else [`UNREGISTERED_CLIENT_ID`].
/// The official id is release config: only Nexus's release pipeline sets it, so a build from source signs in as the
/// shared third-party client.
pub const DESKTOP_CLIENT_ID: &str = match option_env!("NEXUS_DESKTOP_CLIENT_ID") {
    Some(id) if !id.is_empty() => id,
    _ => UNREGISTERED_CLIENT_ID,
};

/// `POST /v1/auth/device`'s body: `label` names the client on the approval page (`nexus CLI on <host>`), and
/// `client_id` names the OAuth client that signs in. Gateway ignores it while `NEXUS_OAUTH_CLIENT_TIERS` is
/// off.
pub fn start_body(label: &str, client_id: Option<&str>) -> Value {
    match client_id {
        Some(id) => json!({ "client": label, "client_id": id }),
        None => json!({ "client": label }),
    }
}

/// `POST /v1/auth/token`'s body.
pub fn token_body(device_code: &str) -> Value {
    json!({ "device_code": device_code, "grant_type": GRANT_TYPE })
}

/// A started login: the code a person approves, where, and how to poll.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceCode {
    pub user_code: String,
    pub device_code: String,
    /// The approval page with the code filled in.
    pub link: String,
    /// Seconds between polls.
    pub interval: f64,
    /// Seconds until the code expires.
    pub expires_in: f64,
}

impl DeviceCode {
    /// Gateway's answer to `POST /v1/auth/device`; the link falls back to Nexus UI's `/device?code=` at `ui_url`.
    pub fn from_answer(started: &Value, ui_url: &str) -> DeviceCode {
        let text = |field: &str| started.get(field).and_then(Value::as_str).unwrap_or("").to_string();
        let number = |field: &str| started.get(field).and_then(Value::as_f64).unwrap_or(0.0);
        let user_code = text("user_code");
        let link = started
            .get("verification_uri_complete")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| format!("{ui_url}/device?code={}", enc(&user_code)));
        DeviceCode {
            device_code: text("device_code"),
            link,
            interval: number("interval"),
            expires_in: number("expires_in"),
            user_code,
        }
    }
}

/// What one poll's answer means.
#[derive(Debug, Clone, PartialEq)]
pub enum Poll {
    /// Approved: Gateway's grant (`access_token`, `tenant`, `user`, `role`, `expires_at`).
    Granted(Value),
    /// Not approved yet: poll again after the interval.
    Pending,
    /// Poll more slowly: the new interval in seconds.
    SlowDown(f64),
    /// Denied in Nexus UI.
    Denied,
    /// The code expired (or the wait passed its deadline) before anyone approved it.
    Expired,
    /// Anything else: stop with this error.
    Failed(Error),
}

/// Read one poll's result. `interval` is the current one (`slow_down` without an interval adds five seconds);
/// `deadline_passed` turns a still-pending code into an expired one. A dropped connection or a slow answer counts as
/// pending until then, so one reset does not abort a login a person is approving. Until then too, a 429 (the
/// address's rate limit) slows the polls as `slow_down` does, and a 502, 503 (`draining` during a deploy) or 504 from
/// Gateway or its proxy (whose body may not be JSON) counts as pending.
pub fn classify(result: Result<Value>, interval: f64, deadline_passed: bool) -> Poll {
    match result {
        Ok(granted) => Poll::Granted(granted),
        Err(Error::Core(error)) if error.status == 429 && !deadline_passed => {
            Poll::SlowDown(error.interval.unwrap_or(interval + 5.0))
        }
        Err(Error::Core(error)) if matches!(error.status, 502..=504) && !deadline_passed => Poll::Pending,
        Err(Error::Core(error)) if error.status == 400 => match error.code.as_deref() {
            Some("slow_down") => Poll::SlowDown(error.interval.unwrap_or(interval + 5.0)),
            Some("access_denied") => Poll::Denied,
            code if code == Some("expired_token") || deadline_passed => Poll::Expired,
            Some("authorization_pending") => Poll::Pending,
            _ => Poll::Failed(Error::Core(error)),
        },
        Err(Error::Transport(Transport::Unreachable(_) | Transport::Timeout)) if !deadline_passed => Poll::Pending,
        Err(error) => Poll::Failed(error),
    }
}

/// A personal token, as the grant carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct Grant {
    pub token: String,
    pub tenant: Value,
    pub user: Value,
    pub role: Option<String>,
    pub expires_at: Option<String>,
}

impl Grant {
    pub fn from_answer(granted: &Value) -> Grant {
        Grant {
            token: granted
                .get("access_token")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            tenant: granted.get("tenant").cloned().unwrap_or(Value::Null),
            user: granted.get("user").cloned().unwrap_or(Value::Null),
            role: granted.get("role").and_then(Value::as_str).map(String::from),
            expires_at: granted.get("expires_at").and_then(Value::as_str).map(String::from),
        }
    }

    /// The organization's segment in Nexus UI's URLs: its slug, else its id.
    pub fn org(&self) -> Option<String> {
        ["slug", "id"]
            .iter()
            .filter_map(|field| self.tenant.get(field).and_then(Value::as_str))
            .find(|value| !value.is_empty())
            .map(String::from)
    }
}

/// How a login ended without a token.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginError {
    Denied,
    Expired,
    Cancelled,
    Failed(Error),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::Denied => write!(f, "The sign-in was denied in Nexus UI."),
            LoginError::Expired => write!(f, "The sign-in code expired before it was approved."),
            LoginError::Cancelled => write!(f, "The sign-in was cancelled."),
            LoginError::Failed(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LoginError {}

/// Start a login on `client` (no credential needed), for the OAuth client `client_id`.
pub async fn start(client: &crate::Client, label: &str, client_id: Option<&str>, ui_url: &str) -> Result<DeviceCode> {
    let started = client.post(START_PATH, start_body(label, client_id)).await?;
    Ok(DeviceCode::from_answer(&started, ui_url))
}

/// Poll until the code is approved, denied or expired, or `cancelled` says stop (checked before each poll).
pub async fn wait(
    client: &crate::Client,
    code: &DeviceCode,
    cancelled: impl Fn() -> bool,
) -> std::result::Result<Grant, LoginError> {
    let mut interval = code.interval;
    let deadline = Instant::now() + Duration::from_secs_f64(code.expires_in.max(0.0));
    loop {
        tokio::time::sleep(Duration::from_secs_f64(interval.max(0.0))).await;
        if cancelled() {
            return Err(LoginError::Cancelled);
        }
        let result = client.post(TOKEN_PATH, token_body(&code.device_code)).await;
        match classify(result, interval, Instant::now() > deadline) {
            Poll::Granted(granted) => return Ok(Grant::from_answer(&granted)),
            Poll::Pending => {}
            Poll::SlowDown(next) => interval = next,
            Poll::Denied => return Err(LoginError::Denied),
            Poll::Expired => return Err(LoginError::Expired),
            Poll::Failed(error) => return Err(LoginError::Failed(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CoreError;

    fn refused(code: &str, interval: Option<f64>) -> Result<Value> {
        Err(CoreError {
            status: 400,
            message: code.into(),
            code: Some(code.into()),
            interval,
            details: None,
        }
        .into())
    }

    fn answered(status: u16, code: &str) -> Result<Value> {
        Err(CoreError {
            status,
            message: code.into(),
            code: Some(code.into()),
            interval: None,
            details: None,
        }
        .into())
    }

    #[test]
    fn the_start_body_names_the_client_id_only_when_given() {
        assert_eq!(
            start_body("nexus CLI on host", Some(CLI_CLIENT_ID)),
            json!({ "client": "nexus CLI on host", "client_id": "nxc_cli" })
        );
        assert_eq!(start_body("my script", None), json!({ "client": "my script" }));
    }

    #[test]
    fn reads_every_poll_answer() {
        assert_eq!(
            classify(refused("authorization_pending", None), 5.0, false),
            Poll::Pending
        );
        assert_eq!(
            classify(refused("authorization_pending", None), 5.0, true),
            Poll::Expired
        );
        assert_eq!(classify(refused("slow_down", None), 5.0, false), Poll::SlowDown(10.0));
        assert_eq!(
            classify(refused("slow_down", Some(7.0)), 5.0, true),
            Poll::SlowDown(7.0)
        );
        assert_eq!(classify(refused("access_denied", None), 5.0, false), Poll::Denied);
        assert_eq!(classify(refused("expired_token", None), 5.0, false), Poll::Expired);
        assert!(matches!(classify(refused("other", None), 5.0, false), Poll::Failed(_)));
        let reset = || Err(Transport::Unreachable("connection reset".into()).into());
        assert_eq!(classify(reset(), 5.0, false), Poll::Pending);
        assert!(matches!(classify(reset(), 5.0, true), Poll::Failed(_)));
        assert_eq!(
            classify(answered(429, "rate_limited"), 5.0, false),
            Poll::SlowDown(10.0)
        );
        assert!(matches!(
            classify(answered(429, "rate_limited"), 5.0, true),
            Poll::Failed(_)
        ));
        for (status, code) in [(502, "bad_gateway"), (503, "draining"), (504, "gateway_timeout")] {
            assert_eq!(classify(answered(status, code), 5.0, false), Poll::Pending);
            assert!(matches!(classify(answered(status, code), 5.0, true), Poll::Failed(_)));
        }
        assert!(matches!(
            classify(answered(401, "unauthorized"), 5.0, false),
            Poll::Failed(_)
        ));
        assert!(matches!(
            classify(answered(500, "internal"), 5.0, false),
            Poll::Failed(_)
        ));
        // A proxy's HTML page arrives as a `CoreError` with its status (`failed_answer`).
        let html = |status| Err(crate::error::failed_answer(status, "<html>bad gateway</html>").into());
        assert_eq!(classify(html(502), 5.0, false), Poll::Pending);
        assert_eq!(classify(html(429), 5.0, false), Poll::SlowDown(10.0));
        assert!(matches!(classify(html(502), 5.0, true), Poll::Failed(_)));
        assert!(matches!(classify(html(500), 5.0, false), Poll::Failed(_)));
        let granted = json!({ "access_token": "nxc_1", "tenant": { "slug": "", "id": "t_1" }, "role": "owner" });
        let Poll::Granted(answer) = classify(Ok(granted), 5.0, false) else {
            panic!("granted")
        };
        let grant = Grant::from_answer(&answer);
        assert_eq!((grant.token.as_str(), grant.org().as_deref()), ("nxc_1", Some("t_1")));
    }

    #[test]
    fn links_to_the_approval_page() {
        let code = DeviceCode::from_answer(
            &json!({ "user_code": "AB CD", "device_code": "d", "interval": 5, "expires_in": 600 }),
            "http://ui.test/app",
        );
        assert_eq!(code.link, "http://ui.test/app/device?code=AB%20CD");
        assert_eq!((code.interval, code.expires_in), (5.0, 600.0));
    }

    /// The official desktop id is release config. Without the release's `NEXUS_DESKTOP_CLIENT_ID`, the
    /// build signs in as the shared third-party client.
    #[test]
    fn a_build_from_source_signs_in_as_the_shared_third_party_client() {
        match option_env!("NEXUS_DESKTOP_CLIENT_ID") {
            Some(id) if !id.is_empty() => assert_eq!(DESKTOP_CLIENT_ID, id),
            _ => assert_eq!(DESKTOP_CLIENT_ID, "nxc_unregistered"),
        }
        assert_eq!(
            start_body("Nexus for desktop on test", Some(UNREGISTERED_CLIENT_ID)),
            json!({ "client": "Nexus for desktop on test", "client_id": "nxc_unregistered" })
        );
    }
}
