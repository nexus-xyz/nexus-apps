//! The async client for the Nexus API (`kit/src/core-client.ts`): a thin client, not an SDK. Bodies are JSON values
//! passed through as Gateway sends them. One `reqwest::Client` (one connection pool) per [`Client`] and its clones:
//! connections stay open between calls (HTTP/2 where the server offers it, keep-alive pings while idle), so a call
//! after a pause does not pay for a new TCP and TLS handshake.
//!
//! Writes carry a fresh `Idempotency-Key` when [`ClientOptions::idempotency`] is on and are retried with it, up
//! to twice, when the connection drops after sending or Gateway answers 502, 503 (not `cutover_paused`), 504 or 409
//! `idempotency_in_progress`: Gateway applies a write at most once. A read (GET, HEAD) is retried the same way, also
//! when the connection is refused (a restarting Gateway), and waits a 503's `Retry-After` (up to 5 s) when it has one.
//! A stream is sent once, never retried.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::Value;

pub use reqwest::{Method, StatusCode};

use crate::error::{Error, Result, failed_answer, failed_answer_after, parse_answer, retryable, transport};
use crate::etag::{Etags, etag_of, has_if_match, needs_if_match, read_path};
use crate::sse::{ChatOutcome, ChatStream, Decoder};
use crate::url::enc;

/// Waits before the first and second retry of a write or a read.
pub(crate) const RETRY_DELAYS_MS: [u64; 2] = [250, 1_000];

/// The longest `Retry-After` a read waits (a draining Gateway sends 5).
pub(crate) const RETRY_AFTER_MAX: Duration = Duration::from_secs(5);

/// A read (GET, HEAD) is safe to send again.
pub(crate) fn is_read(method: &Method) -> bool {
    *method == Method::GET || *method == Method::HEAD
}

/// How many times a call may be sent: a read, or a write with an idempotency key, up to three times; else once.
pub(crate) fn attempts(read: bool, keyed: bool) -> usize {
    if read || keyed { RETRY_DELAYS_MS.len() + 1 } else { 1 }
}

/// Whether a failed send may be sent again. A timeout never is: the wait is over. A refused connection is for a read
/// only: a write keeps its earlier rule (retried only when the connection drops after sending).
pub(crate) fn retry_transport(read: bool, error: &reqwest::Error) -> bool {
    !error.is_timeout() && (read || !error.is_connect())
}

/// The 503 codes that say this Gateway lacks a feature (billing or Stripe off, no CLI bundle): a retry does not
/// change them.
const NOT_RETRIED: [&str; 2] = ["not_configured", "cli_bundle_unavailable"];

/// Whether an answer may be sent again: [`retryable`], but for a read not a 503 whose `error.code` is in
/// [`NOT_RETRIED`].
pub(crate) fn retry_answer(read: bool, status: u16, text: &str) -> bool {
    if !retryable(status, text) {
        return false;
    }
    let code = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|body| body.pointer("/error/code").and_then(Value::as_str).map(String::from));
    !(read && code.is_some_and(|code| NOT_RETRIED.contains(&code.as_str())))
}

/// An answer's `Retry-After`, whatever the case of the header's name.
pub(crate) fn retry_after(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.as_str())
}

/// The wait before retry `attempt`: for a read, the answer's `Retry-After` in seconds (an HTTP date is ignored) (at most
/// [`RETRY_AFTER_MAX`]) when it has one; else [`RETRY_DELAYS_MS`].
pub(crate) fn retry_delay(attempt: usize, read: bool, retry_after: Option<&str>) -> Duration {
    let seconds = retry_after
        .filter(|_| read)
        .and_then(|value| value.trim().parse::<u64>().ok());
    match seconds {
        Some(seconds) => Duration::from_secs(seconds).min(RETRY_AFTER_MAX),
        None => Duration::from_millis(RETRY_DELAYS_MS[attempt]),
    }
}

/// How a [`Client`] or a `blocking::Client` talks to Gateway. Start from [`ClientOptions::new`].
#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub base_url: String,
    pub api_key: Option<String>,
    /// Extra headers on every call.
    pub headers: Vec<(String, String)>,
    /// A fresh `Idempotency-Key` on every write, and safe retries.
    pub idempotency: bool,
    /// Whether a failed read, or a failed write with a key, is sent again (up to twice). Off, every call is sent once
    /// and a write still carries its own fresh key: a heartbeat or a caller that owns its retries.
    pub retry: bool,
    /// `x-nexus-client` (`cli/<version>`, `desktop/<version>`).
    pub client: Option<String>,
    /// Each call's limit; a stream's is [`ClientOptions::stream_timeout`].
    pub timeout: Duration,
    /// How long a streamed answer may take in all.
    pub stream_timeout: Duration,
    /// The `user-agent` header.
    pub user_agent: Option<String>,
}

impl ClientOptions {
    /// Options for `base_url` with the defaults every client starts from: no key, no extra headers, idempotent
    /// writes with retries, two minutes a call, five minutes a stream.
    pub fn new(base_url: &str) -> ClientOptions {
        ClientOptions {
            base_url: base_url.to_string(),
            api_key: None,
            headers: Vec::new(),
            idempotency: true,
            retry: true,
            client: None,
            timeout: Duration::from_secs(120),
            stream_timeout: Duration::from_secs(300),
            user_agent: None,
        }
    }
}

/// A raw answer, for a caller that reads Gateway's answer itself (its status, headers and bytes) instead of as JSON.
#[derive(Debug, Clone)]
pub struct Exchange {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

/// One answer as a client gets it, before it is read as a result: its status, its headers, and its body when it was read
/// whole. A stream's body is still to come, so it is `None` there.
#[derive(Debug, Clone, Copy)]
pub struct Answer<'a> {
    pub path: &'a str,
    pub status: u16,
    pub headers: &'a [(String, String)],
    pub body: Option<&'a [u8]>,
}

/// Sees every answer a client gets ([`Client::observed`]). Nexus Desktop watches for Gateway's refusal of its version
/// (426 `desktop_revoked`) here, so no call site has to.
#[derive(Clone)]
pub struct Observer(Arc<dyn Fn(&Answer<'_>) + Send + Sync>);

impl Observer {
    pub fn new(observe: impl Fn(&Answer<'_>) + Send + Sync + 'static) -> Observer {
        Observer(Arc::new(observe))
    }
}

impl std::fmt::Debug for Observer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Observer")
    }
}

/// The async Nexus API client. Clones share one connection pool.
#[derive(Debug, Clone)]
pub struct Client {
    options: ClientOptions,
    base: String,
    http: reqwest::Client,
    observer: Option<Observer>,
    etags: Etags,
}

/// The connection pool every async client shares unless given its own: idle connections are kept (and pinged over
/// HTTP/2) so the next call reuses one.
pub fn http_client(user_agent: Option<&str>) -> reqwest::Client {
    crate::install_tls();
    let mut builder = reqwest::Client::builder()
        // A redirect can forward a POST body (provider keys, prompts, device codes) to another origin.
        .redirect(reqwest::redirect::Policy::none())
        .pool_idle_timeout(None)
        .pool_max_idle_per_host(8)
        .tcp_nodelay(true)
        .tcp_keepalive(Duration::from_secs(30))
        .http2_keep_alive_interval(Duration::from_secs(20))
        .http2_keep_alive_while_idle(true)
        .http2_adaptive_window(true)
        .connect_timeout(Duration::from_secs(10));
    if let Some(agent) = user_agent {
        builder = builder.user_agent(agent);
    }
    builder.build().expect("valid Nexus API HTTP client configuration")
}

impl Client {
    pub fn new(options: ClientOptions) -> Client {
        let http = http_client(options.user_agent.as_deref());
        Client::with_http(options, http)
    }

    /// A client on an existing connection pool (another client's [`Client::http`]).
    pub fn with_http(options: ClientOptions, http: reqwest::Client) -> Client {
        let base = options
            .base_url
            .strip_suffix('/')
            .unwrap_or(&options.base_url)
            .to_string();
        Client {
            options,
            base,
            http,
            observer: None,
            etags: Etags::default(),
        }
    }

    /// This client, with `observer` shown each answer of a call or a stream. Its clones and
    /// [`Client::with_api_key`] keep it.
    pub fn observed(mut self, observer: Observer) -> Client {
        self.observer = Some(observer);
        self
    }

    fn observe(&self, answer: Answer<'_>) {
        if let Some(observer) = &self.observer {
            (observer.0)(&answer);
        }
    }

    /// The same Gateway and connection pool with another credential (`None`: signed out).
    pub fn with_api_key(&self, api_key: Option<String>) -> Client {
        Client {
            options: ClientOptions {
                api_key,
                ..self.options.clone()
            },
            base: self.base.clone(),
            http: self.http.clone(),
            observer: self.observer.clone(),
            // Another person's versions are not this one's.
            etags: Etags::default(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn options(&self) -> &ClientOptions {
        &self.options
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn signed_in(&self) -> bool {
        self.options.api_key.is_some()
    }

    fn request(
        &self,
        method: &Method,
        path: &str,
        extra: &[(String, String)],
        body: Option<Bytes>,
        key: Option<&str>,
    ) -> reqwest::RequestBuilder {
        self.etags.touch(method, path);
        let mut request = self
            .http
            .request(method.clone(), format!("{}{path}", self.base))
            .timeout(self.options.timeout);
        for (name, value) in self.options.headers.iter().chain(extra) {
            request = request.header(name, value);
        }
        if let Some(api_key) = &self.options.api_key {
            request = request.header("authorization", format!("Bearer {api_key}"));
        }
        if let Some(client) = &self.options.client {
            request = request.header(crate::CLIENT_HEADER, client);
        }
        if path == "/v1/agents/access" {
            request = request.header(nexus_agents::AGENT_LAUNCHERS_HEADER, nexus_agents::Agent::commands(","));
        }
        if let Some(body) = body {
            if !extra.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-type")) {
                request = request.header("content-type", "application/json");
            }
            request = request.body(body);
        }
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        request
    }

    /// One HTTP exchange, raw: `extra` headers on top of the client's, the body as given. With idempotency, a write
    /// carries a key and is retried safely. An edit of an editable resource carries `If-Match` (`etag.rs`). Never
    /// fails on Gateway's error statuses: they are in the answer.
    pub async fn exchange(
        &self,
        method: Method,
        path: &str,
        extra: &[(String, String)],
        body: Option<Bytes>,
    ) -> Result<Exchange> {
        let mut headers = extra.to_vec();
        if needs_if_match(&method, path) && !has_if_match(extra) {
            if self.etags.get(path).is_none()
                && let Ok(read) = self.send(Method::GET, read_path(path), &[], None).await
            {
                self.etags
                    .learn(&Method::GET, read_path(path), read.status, etag_of(&read.headers));
            }
            if let Some(etag) = self.etags.get(path).flatten() {
                headers.push(("if-match".to_string(), etag));
            }
        }
        let exchange = self.send(method.clone(), path, &headers, body).await?;
        self.etags
            .learn(&method, path, exchange.status, etag_of(&exchange.headers));
        Ok(exchange)
    }

    /// [`Client::exchange`] without `If-Match`: the send, its retries and the observer.
    async fn send(
        &self,
        method: Method,
        path: &str,
        extra: &[(String, String)],
        body: Option<Bytes>,
    ) -> Result<Exchange> {
        let read = is_read(&method);
        let key = (!read && self.options.idempotency).then(|| uuid::Uuid::new_v4().to_string());
        let attempts = if self.options.retry {
            attempts(read, key.is_some())
        } else {
            1
        };
        let mut attempt = 0;
        loop {
            let result = async {
                let response = self
                    .request(&method, path, extra, body.clone(), key.as_deref())
                    .send()
                    .await?;
                let status = response.status().as_u16();
                let headers = header_pairs(response.headers());
                let bytes = response.bytes().await?;
                Ok::<_, reqwest::Error>(Exchange {
                    status,
                    headers,
                    body: bytes,
                })
            }
            .await;
            match result {
                Ok(exchange) => {
                    if attempt + 1 < attempts
                        && retry_answer(read, exchange.status, &String::from_utf8_lossy(&exchange.body))
                    {
                        let wait = retry_delay(attempt, read, retry_after(&exchange.headers));
                        tokio::time::sleep(wait).await;
                        attempt += 1;
                        continue;
                    }
                    self.observe(Answer {
                        path,
                        status: exchange.status,
                        headers: &exchange.headers,
                        body: Some(&exchange.body),
                    });
                    return Ok(exchange);
                }
                Err(error) => {
                    if attempt + 1 >= attempts || !retry_transport(read, &error) {
                        return Err(transport(&error));
                    }
                    tokio::time::sleep(retry_delay(attempt, read, None)).await;
                    attempt += 1;
                }
            }
        }
    }

    /// A JSON call: Gateway's answer, or its error as `Error::Core`.
    pub async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        self.call_with(method, path, &[], body).await
    }

    /// [`Client::call`] with `extra` headers on this one call.
    pub async fn call_with(
        &self,
        method: Method,
        path: &str,
        extra: &[(String, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        let body = body.map(|body| Bytes::from(serde_json::to_vec(&body).unwrap_or_default()));
        let exchange = self.exchange(method, path, extra, body).await?;
        let text = String::from_utf8_lossy(&exchange.body);
        if !(200..300).contains(&exchange.status) {
            return Err(failed_answer_after(exchange.status, &text, retry_after(&exchange.headers)).into());
        }
        let headers: Vec<(String, String)> = self.options.headers.iter().chain(extra).cloned().collect();
        parse_answer(exchange.status, &text, &headers)
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.call(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::POST, path, Some(body)).await
    }

    pub async fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::PUT, path, Some(body)).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::PATCH, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.call(Method::DELETE, path, None).await
    }

    /// Send a request and hand back the answer's status, headers and body as it arrives (server-sent events, a
    /// download). Sent once: a stream is never retried, though a write carries an `Idempotency-Key`.
    pub async fn stream(
        &self,
        method: Method,
        path: &str,
        extra: &[(String, String)],
        body: Option<Bytes>,
    ) -> Result<(
        u16,
        Vec<(String, String)>,
        impl Stream<Item = Result<Bytes>> + Send + use<>,
    )> {
        let write = method != Method::GET && method != Method::HEAD;
        let key = (write && self.options.idempotency).then(|| uuid::Uuid::new_v4().to_string());
        let response = self
            .request(&method, path, extra, body, key.as_deref())
            .timeout(self.options.stream_timeout)
            .send()
            .await
            .map_err(|error| transport(&error))?;
        let status = response.status().as_u16();
        let headers = header_pairs(response.headers());
        self.observe(Answer {
            path,
            status,
            headers: &headers,
            body: None,
        });
        let body = response
            .bytes_stream()
            .map(|chunk| chunk.map_err(|error| transport(&error)));
        Ok((status, headers, body))
    }

    /// Post a chat message and read the answer as it streams. `on_chunk` gets every event as it arrives (the
    /// first carries the stored question, the deltas the text, the last the stored answer). A refusal (403, 402) or a
    /// failure mid-way is Gateway's error, with `details` naming the session and the stored message.
    pub async fn stream_chat_message(
        &self,
        id: &str,
        body: Value,
        mut on_chunk: impl FnMut(&Value),
    ) -> Result<ChatOutcome> {
        let mut body = body;
        if let Value::Object(map) = &mut body {
            map.insert("stream".into(), Value::Bool(true));
        }
        let path = format!("/v1/chat/sessions/{}/messages", enc(id));
        let bytes = Bytes::from(serde_json::to_vec(&body).unwrap_or_default());
        let accept = [("accept".to_string(), "text/event-stream".to_string())];
        let (status, headers, mut stream) = self.stream(Method::POST, &path, &accept, Some(bytes)).await?;
        let event_stream = headers
            .iter()
            .any(|(name, value)| name == "content-type" && value.contains("text/event-stream"));
        if !(200..300).contains(&status) || !event_stream {
            let mut text = Vec::new();
            while let Some(chunk) = stream.next().await {
                text.extend_from_slice(&chunk?);
            }
            let mut error = failed_answer(status, &String::from_utf8_lossy(&text));
            error.interval = None;
            return Err(error.into());
        }
        let mut decoder = Decoder::new();
        let mut outcome = ChatStream::new();
        while let Some(chunk) = stream.next().await {
            for event in decoder.push(&chunk?) {
                let event = event?;
                outcome.observe(&event);
                on_chunk(&event);
            }
        }
        outcome.finish()
    }

    /// Open a connection to Gateway ahead of the first real call (`GET /healthz`); answers how long it took.
    pub async fn warm(&self) -> Result<Duration> {
        let started = Instant::now();
        let response = self
            .http
            .get(format!("{}/healthz", self.base))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|error| transport(&error))?;
        let _ = response.bytes().await;
        Ok(started.elapsed())
    }
}

fn header_pairs(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| Some((name.as_str().to_string(), value.to_str().ok()?.to_string())))
        .collect()
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Error {
        transport(&error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_launchers_on_agent_access_requests() {
        let client = Client::new(ClientOptions::new("http://localhost"));
        for method in [Method::GET, Method::PUT] {
            let request = client
                .request(&method, "/v1/agents/access", &[], None, None)
                .build()
                .expect("build agent access request");
            assert_eq!(
                request.headers()[nexus_agents::AGENT_LAUNCHERS_HEADER],
                "claude,codex,agy,cursor-agent,grok"
            );
        }
        let request = client
            .request(&Method::GET, "/v1/agents/sessions", &[], None, None)
            .build()
            .expect("build agent sessions request");
        assert!(!request.headers().contains_key(nexus_agents::AGENT_LAUNCHERS_HEADER));
    }

    #[test]
    fn retries_reads_and_keyed_writes_only() {
        assert_eq!(attempts(is_read(&Method::GET), false), 3);
        assert_eq!(attempts(is_read(&Method::HEAD), false), 3);
        assert_eq!(attempts(is_read(&Method::POST), true), 3);
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert_eq!(attempts(is_read(&method), false), 1, "{method}");
        }
    }

    #[test]
    fn a_read_waits_retry_after_up_to_five_seconds() {
        let ms = |attempt, read, after| retry_delay(attempt, read, after).as_millis();
        assert_eq!(ms(0, true, None), 250);
        assert_eq!(ms(1, true, None), 1_000);
        assert_eq!(ms(0, true, Some("2")), 2_000);
        assert_eq!(ms(0, true, Some("3600")), 5_000);
        // A date or junk is not seconds: the usual wait.
        assert_eq!(ms(0, true, Some("Wed, 21 Oct 2026 07:28:00 GMT")), 250);
        // A write keeps its waits.
        assert_eq!(ms(0, false, Some("5")), 250);
    }

    #[test]
    fn a_read_is_not_retried_when_gateway_lacks_the_feature() {
        let off = r#"{"error":{"message":"billing is off","code":"not_configured"}}"#;
        assert!(!retry_answer(true, 503, off));
        assert!(retry_answer(true, 503, r#"{"error":{"code":"draining"}}"#));
        assert!(retry_answer(true, 502, "<html>"));
        assert!(!retry_answer(true, 500, ""));
        let no_bundle = r#"{"error":{"message":"no bundle","code":"cli_bundle_unavailable"}}"#;
        assert!(!retry_answer(true, 503, no_bundle));
        // Only the code counts, not the words in a message.
        assert!(retry_answer(
            true,
            503,
            r#"{"error":{"message":"not_configured?","code":"draining"}}"#
        ));
    }

    #[test]
    fn finds_retry_after_whatever_its_case() {
        let headers = |name: &str| {
            vec![
                ("content-type".to_string(), "x".to_string()),
                (name.to_string(), "3".to_string()),
            ]
        };
        for name in ["retry-after", "Retry-After", "RETRY-AFTER"] {
            assert_eq!(retry_after(&headers(name)), Some("3"), "{name}");
        }
        assert_eq!(retry_after(&headers("x-retry")), None);
    }
}
