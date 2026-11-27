//! How a call fails: Gateway answered with an error (`CoreError`, as `kit`'s TypeScript names it), or the request
//! never got an answer (`Transport`). Wording a failure for a person is the caller's job (the CLI's `to_cli_error`),
//! except a paused sign-in ([`CoreError::cutover_paused_message`]), which every client words the same way.

use serde_json::Value;

/// Gateway answered with an error (`CoreError` in `core-client.ts`).
#[derive(Debug, Clone, PartialEq)]
pub struct CoreError {
    pub status: u16,
    pub message: String,
    pub code: Option<String>,
    /// `slow_down` from `POST /v1/auth/token`: the interval to poll at from now on, in seconds. A refused download's,
    /// or a 503 `cutover_paused`'s, `Retry-After`, in seconds.
    pub interval: Option<f64>,
    /// Gateway's `error.nexus` (a refused model's rule, a chat refusal's session and stored message).
    pub details: Option<Value>,
}

/// How a request failed before Gateway answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Transport {
    /// Connection refused, unknown host, reset: Gateway is not there ("fetch failed").
    Unreachable(String),
    Timeout,
    Other(String),
}

/// Why a call to Gateway failed.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Boxed: Gateway's error carries its details, and every `Result` stays small.
    Core(Box<CoreError>),
    Transport(Transport),
}

impl From<CoreError> for Error {
    fn from(error: CoreError) -> Error {
        Error::Core(Box::new(error))
    }
}

impl From<Transport> for Error {
    fn from(error: Transport) -> Error {
        Error::Transport(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Core(error) => match error.cutover_paused_message() {
                Some(message) => write!(f, "{message}"),
                None => write!(f, "{}", error.message),
            },
            Error::Transport(Transport::Unreachable(message)) => write!(f, "Nexus Gateway is unreachable: {message}"),
            Error::Transport(Transport::Timeout) => write!(f, "Nexus Gateway did not answer in time"),
            Error::Transport(Transport::Other(message)) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// Gateway refused an edit because the resource changed since the caller read it (412 `version_mismatch`). Read
    /// it again before another edit.
    pub fn is_version_mismatch(&self) -> bool {
        matches!(self, Error::Core(error) if error.status == 412)
    }
}

/// A call's result: Gateway's answer or why it failed.
pub type Result<T> = std::result::Result<T, Error>;

/// A transport failure as the TypeScript client words it. `Unreachable` keeps the whole cause chain, so the caller can
/// tell a TLS failure ("invalid peer certificate: UnknownIssuer") from a refused connection.
pub(crate) fn transport(error: &reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Transport(Transport::Timeout)
    } else if error.is_connect() || error.is_request() || error.is_body() || error.is_decode() {
        let mut text = error.to_string();
        let mut source = std::error::Error::source(error);
        while let Some(cause) = source {
            text.push_str(&format!(": {cause}"));
            source = cause.source();
        }
        Error::Transport(Transport::Unreachable(text))
    } else {
        Error::Transport(Transport::Other(error.to_string()))
    }
}

/// Gateway's `error.nexus` on a connector write refused with 403 `write_consent_required` (only while
/// `NEXUS_CONNECTOR_WRITE_CONSENT` is on): the connection was consented for reading only.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteConsent {
    pub connector: String,
    pub account: String,
    /// The vendor scopes the write needs.
    pub scopes: Vec<String>,
    /// The connection's holder: `user` or `org`.
    pub scope: String,
}

/// `error.nexus` of a `write_consent_required` refusal, read as strictly as kit's `writeConsentOf`: only the one call
/// the contract names passes, a POST to this connector's own OAuth start asking for writes. None for anything else.
pub fn write_consent_of(nexus: &Value) -> Option<WriteConsent> {
    let connector = nexus.get("connector")?.as_str()?;
    let named = connector.len() <= 64
        && connector.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && connector
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !named {
        return None;
    }
    let account = nexus.get("account")?.as_str()?;
    let scopes = nexus
        .get("scopes")?
        .as_array()?
        .iter()
        .map(|scope| scope.as_str().map(String::from))
        .collect::<Option<Vec<_>>>()?;
    let consent = nexus.get("consent")?;
    if consent.get("method")?.as_str()? != "POST"
        || consent.get("path")?.as_str()? != format!("/v1/connections/{connector}/oauth")
    {
        return None;
    }
    let body = consent.get("body")?;
    let scope = body.get("scope")?.as_str()?;
    if body.get("access")?.as_str()? != "write" || !matches!(scope, "user" | "org") {
        return None;
    }
    Some(WriteConsent {
        connector: connector.to_string(),
        account: account.to_string(),
        scopes,
        scope: scope.to_string(),
    })
}

impl WriteConsent {
    /// The CLI command that makes the consent call and prints the vendor's link (`nexus connect <connector> --write`).
    pub fn grant_command(&self) -> String {
        let org = if self.scope == "org" { " --org" } else { "" };
        format!("nexus connect {}{org} --write", self.connector)
    }
}

impl CoreError {
    /// A 403 [`WRITE_CONSENT_REQUIRED`] whose details are the contract's ([`write_consent_of`]); None for anything else.
    pub fn write_consent(&self) -> Option<WriteConsent> {
        if self.status != 403 || self.code.as_deref() != Some(WRITE_CONSENT_REQUIRED) {
            return None;
        }
        self.details.as_ref().and_then(write_consent_of)
    }
}

/// The code of a connector write refused until its connection's holder consents to the write scopes.
pub const WRITE_CONSENT_REQUIRED: &str = "write_consent_required";

/// The code of a 503 on sign-in or an organization write while sign-in is paused for maintenance.
/// Nothing changed; it is not retried ([`retryable`]).
pub const CUTOVER_PAUSED: &str = "cutover_paused";

impl CoreError {
    /// A 503 `cutover_paused` as a person reads it, with the wait its `Retry-After` named (rounded up to whole
    /// minutes); None for any other error.
    pub fn cutover_paused_message(&self) -> Option<String> {
        if self.status != 503 || self.code.as_deref() != Some(CUTOVER_PAUSED) {
            return None;
        }
        let wait = match self.interval.filter(|seconds| seconds.is_finite() && *seconds > 0.0) {
            Some(seconds) => match (seconds / 60.0).ceil() as u64 {
                1 => "1 minute".to_string(),
                minutes => format!("{minutes} minutes"),
            },
            None => "a few minutes".to_string(),
        };
        Some(format!("Sign-in is paused for maintenance; try again in {wait}."))
    }
}

/// [`failed_answer`] with the answer's `Retry-After`: a 503 `cutover_paused` keeps its whole seconds as `interval`.
pub(crate) fn failed_answer_after(status: u16, text: &str, retry_after: Option<&str>) -> CoreError {
    let mut error = failed_answer(status, text);
    if error.code.as_deref() == Some(CUTOVER_PAUSED) {
        error.interval = retry_after
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|seconds| seconds as f64);
    }
    error
}

/// Gateway's error body (`{ error: { message, code, nexus } }`, and `interval` for `slow_down`) as a `CoreError`.
pub fn core_error(status: u16, data: &Value) -> CoreError {
    let error = data.get("error");
    CoreError {
        status,
        message: error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| format!("HTTP {status}")),
        code: error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .map(String::from),
        interval: data.get("interval").and_then(Value::as_f64),
        details: error
            .and_then(|error| error.get("nexus"))
            .filter(|value| !value.is_null())
            .cloned(),
    }
}

/// Parse an answer's body as JSON; an empty body is `{}`. A body that is not JSON is a transport failure that names
/// the status, as the CLI words it.
pub(crate) fn parse_body(status: u16, text: &str) -> Result<Value> {
    if text.is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_str(text).map_err(|error| {
        Error::Transport(Transport::Other(format!(
            "Nexus Gateway answered HTTP {status} with a body that is not JSON ({error})"
        )))
    })
}

/// Whether these request headers ask for plain text (`Accept: text/plain`): a page's source. The answer is then its
/// text, as a JSON string, whatever the text looks like (a Markdown page that reads `42` is still a string).
pub(crate) fn wants_text(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("accept") && value.trim().eq_ignore_ascii_case("text/plain"))
}

/// [`parse_body`], except a call that asked for plain text ([`wants_text`]) gets the text as it is.
pub(crate) fn parse_answer(status: u16, text: &str, headers: &[(String, String)]) -> Result<Value> {
    if wants_text(headers) {
        return Ok(Value::String(text.to_string()));
    }
    parse_body(status, text)
}

/// A failed answer's body as a `CoreError`, whatever it holds (a body that is not JSON reads as `{}`).
pub fn failed_answer(status: u16, text: &str) -> CoreError {
    let data = serde_json::from_str::<Value>(if text.is_empty() { "{}" } else { text })
        .unwrap_or(Value::Object(Default::default()));
    core_error(status, &data)
}

/// Worth retrying a write with the same idempotency key: Gateway or a proxy could not finish it, or the first try is
/// still running. A 503 `cutover_paused` is not: the pause lasts minutes, and the person is told how long.
pub(crate) fn retryable(status: u16, text: &str) -> bool {
    (matches!(status, 502..=504) && !(status == 503 && text.contains(CUTOVER_PAUSED)))
        || (status == 409 && text.contains("idempotency_in_progress"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_gateway_errors() {
        let error = core_error(
            400,
            &json!({ "error": { "message": "slow down", "code": "slow_down", "nexus": null }, "interval": 10 }),
        );
        assert_eq!(error.code.as_deref(), Some("slow_down"));
        assert_eq!(error.interval, Some(10.0));
        assert_eq!(error.details, None);
        assert_eq!(failed_answer(502, "<html>").message, "HTTP 502");
        assert!(retryable(503, ""));
        assert!(retryable(409, r#"{"error":{"code":"idempotency_in_progress"}}"#));
        assert!(!retryable(409, "conflict"));
        assert!(matches!(
            parse_body(200, "nope"),
            Err(Error::Transport(Transport::Other(_)))
        ));
        assert_eq!(parse_body(204, "").ok(), Some(json!({})));
        let text = [("Accept".to_string(), "text/plain".to_string())];
        assert_eq!(parse_answer(200, "42", &text).ok(), Some(json!("42")));
        assert_eq!(parse_answer(200, "# Page\n", &text).ok(), Some(json!("# Page\n")));
        assert_eq!(parse_answer(200, "42", &[]).ok(), Some(json!(42)));
    }

    const PAUSED: &str = r#"{"error":{"message":"sign-in, account deletion and organization changes are paused","type":"server_error","code":"cutover_paused"}}"#;

    #[test]
    fn a_paused_sign_in_names_the_wait_and_is_not_retried() {
        let error = failed_answer_after(503, PAUSED, Some("600"));
        assert_eq!(error.interval, Some(600.0));
        assert_eq!(
            Error::from(error).to_string(),
            "Sign-in is paused for maintenance; try again in 10 minutes."
        );
        let error = failed_answer_after(503, PAUSED, Some("61"));
        assert_eq!(
            error.cutover_paused_message().as_deref(),
            Some("Sign-in is paused for maintenance; try again in 2 minutes.")
        );
        let error = failed_answer_after(503, PAUSED, Some("30"));
        assert_eq!(
            error.cutover_paused_message().as_deref(),
            Some("Sign-in is paused for maintenance; try again in 1 minute.")
        );
        let error = failed_answer_after(503, PAUSED, None);
        assert_eq!(
            error.cutover_paused_message().as_deref(),
            Some("Sign-in is paused for maintenance; try again in a few minutes.")
        );
        assert!(!retryable(503, PAUSED));
    }

    #[test]
    fn other_503s_read_and_retry_as_before() {
        let body = r#"{"error":{"message":"draining","code":"unavailable"}}"#;
        let error = failed_answer_after(503, body, Some("5"));
        assert_eq!(error.interval, None);
        assert_eq!(error.cutover_paused_message(), None);
        assert_eq!(Error::from(error).to_string(), "draining");
        assert!(retryable(503, body));
        assert!(retryable(503, "<html>"));
    }
}
