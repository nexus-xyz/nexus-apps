//! The synchronous client (feature `blocking`), for the `nexus` CLI: the same calls, headers, retries and stream
//! decoding as the async [`crate::Client`], on reqwest's blocking client.

use std::io::Read;

use reqwest::blocking::{Client as Http, Response};
use reqwest::{Method, StatusCode};
use serde_json::Value;

use crate::client::{ClientOptions, attempts, is_read, retry_answer, retry_delay, retry_transport};
use crate::error::{Error, Result, Transport, failed_answer, failed_answer_after, parse_answer, transport};
use crate::etag::{Etags, needs_if_match, read_path};
use crate::sse::{ChatOutcome, ChatStream, Decoder};
use crate::url::enc;

/// The header that names an export, whose audit event holds a CSV file's manifest (`EXPORT_ID_HEADER` in
/// `nexus-types`).
pub const EXPORT_ID_HEADER: &str = "x-nexus-export-id";

/// The header that names the boards a page export did not render (`FAILED_HEADER` in `nexus-state`).
pub const EXPORT_FAILED_HEADER: &str = "x-nexus-export-failed";

/// One exported file: every byte Gateway sent, the export's id, and the boards a page export did not render.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportFile {
    pub body: Vec<u8>,
    pub export_id: Option<String>,
    pub failed: Option<String>,
}

/// How a download ended: the whole file; cut after the headers (the reason, as the connection gave it); or cut by
/// this client's own time limit ([`ClientOptions::stream_timeout`]) part way.
#[derive(Debug, Clone, PartialEq)]
pub enum Download {
    File(ExportFile),
    Cut(String),
    TimedOut,
}

/// The synchronous Nexus API client: the same calls as the async [`crate::Client`].
#[derive(Debug, Clone)]
pub struct Client {
    options: ClientOptions,
    base: String,
    http: Http,
    etags: Etags,
}

/// A blocking HTTP client with this user agent (and the TLS provider installed).
pub fn http_client(user_agent: Option<&str>) -> Http {
    build_http_client(user_agent, reqwest::redirect::Policy::default())
}

fn build_http_client(user_agent: Option<&str>, redirect: reqwest::redirect::Policy) -> Http {
    crate::install_tls();
    let mut builder = Http::builder().redirect(redirect);
    if let Some(agent) = user_agent {
        builder = builder.user_agent(agent);
    }
    builder.build().expect("valid Nexus HTTP client configuration")
}

impl Client {
    pub fn new(options: ClientOptions) -> Client {
        let base = options
            .base_url
            .strip_suffix('/')
            .unwrap_or(&options.base_url)
            .to_string();
        // Download clients may follow redirects; API calls must keep credentials and bodies on this Gateway.
        let http = build_http_client(options.user_agent.as_deref(), reqwest::redirect::Policy::none());
        Client {
            options,
            base,
            http,
            etags: Etags::default(),
        }
    }

    /// The `If-Match` an edit of an editable resource carries (`etag.rs`): the version last read, or read now.
    fn if_match(&self, method: &Method, path: &str) -> Option<String> {
        if !needs_if_match(method, path) {
            return None;
        }
        if self.etags.get(path).is_none() {
            let _ = self.send(Method::GET, read_path(path), None);
        }
        self.etags.get(path).flatten()
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn request(
        &self,
        method: &Method,
        path: &str,
        body: Option<&Value>,
        key: Option<&str>,
    ) -> reqwest::blocking::RequestBuilder {
        self.etags.touch(method, path);
        let mut request = self
            .http
            .request(method.clone(), format!("{}{path}", self.base))
            .timeout(self.options.timeout);
        for (name, value) in &self.options.headers {
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
            request = request
                .header("content-type", "application/json")
                .body(serde_json::to_vec(body).unwrap_or_default());
        }
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        request
    }

    /// One HTTP exchange; a read, or (with idempotency) a write that carries a key, is retried safely. An edit of an
    /// editable resource carries `If-Match` (`etag.rs`). Answers the status and body.
    pub fn send(&self, method: Method, path: &str, body: Option<&Value>) -> Result<(StatusCode, String)> {
        self.exchange(method, path, body)
            .map(|(status, text, _retry_after)| (status, text))
    }

    /// [`Client::send`], also answering the last answer's `Retry-After`.
    fn exchange(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(StatusCode, String, Option<String>)> {
        let read = is_read(&method);
        let key = (!read && self.options.idempotency).then(|| uuid::Uuid::new_v4().to_string());
        let if_match = self.if_match(&method, path);
        let attempts = if self.options.retry {
            attempts(read, key.is_some())
        } else {
            1
        };
        let mut attempt = 0;
        loop {
            let mut request = self.request(&method, path, body, key.as_deref());
            if let Some(etag) = &if_match {
                request = request.header("if-match", etag);
            }
            let result = request.send().and_then(|response| {
                let status = response.status();
                let header = |name: &str| {
                    response
                        .headers()
                        .get(name)
                        .and_then(|value| value.to_str().ok())
                        .map(String::from)
                };
                let (retry_after, etag) = (header("retry-after"), header("etag"));
                response.text().map(|text| (status, text, retry_after, etag))
            });
            match result {
                Ok((status, text, retry_after, etag)) => {
                    if attempt + 1 < attempts && retry_answer(read, status.as_u16(), &text) {
                        std::thread::sleep(retry_delay(attempt, read, retry_after.as_deref()));
                        attempt += 1;
                        continue;
                    }
                    self.etags.learn(&method, path, status.as_u16(), etag.as_deref());
                    return Ok((status, text, retry_after));
                }
                Err(error) => {
                    if attempt + 1 >= attempts || !retry_transport(read, &error) {
                        return Err(transport(&error));
                    }
                    std::thread::sleep(retry_delay(attempt, read, None));
                    attempt += 1;
                }
            }
        }
    }

    pub fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let (status, text, retry_after) = self.exchange(method, path, body.as_ref())?;
        if !status.is_success() {
            return Err(failed_answer_after(status.as_u16(), &text, retry_after.as_deref()).into());
        }
        parse_answer(status.as_u16(), &text, &self.options.headers)
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        self.call(Method::GET, path, None)
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::POST, path, Some(body))
    }

    pub fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::PUT, path, Some(body))
    }

    pub fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::PATCH, path, Some(body))
    }

    pub fn delete(&self, path: &str) -> Result<Value> {
        self.call(Method::DELETE, path, None)
    }

    /// `GET` a text answer (the audit CSV).
    pub fn get_text(&self, path: &str) -> Result<String> {
        let (status, text) = self.send(Method::GET, path, None)?;
        if !status.is_success() {
            let mut error = failed_answer(status.as_u16(), &text);
            error.interval = None;
            error.details = None;
            return Err(error.into());
        }
        Ok(text)
    }

    /// `GET` one exported file, its bytes as Gateway sent them and the export's id
    /// ([`EXPORT_ID_HEADER`]). Sent once: each try would be a new export, with its own audit event and a place in the
    /// organization's hourly limit. A refusal comes before the first byte, as an error; a body that breaks after the
    /// headers is [`Download::Cut`], because Gateway closes the connection when a file fails part way; the time limit
    /// reached part way is [`Download::TimedOut`]. A refusal's `Retry-After`, in whole seconds, is its `interval`
    /// (skills delivery waits a 429 out).
    pub fn download(&self, path: &str) -> Result<Download> {
        let mut response = self
            .request(&Method::GET, path, None, None)
            .timeout(self.options.stream_timeout)
            .send()
            .map_err(|error| transport(&error))?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok());
            let text = response.text().map_err(|error| transport(&error))?;
            let mut error = failed_answer(status.as_u16(), &text);
            error.interval = retry_after.map(|seconds| seconds as f64);
            error.details = None;
            return Err(error.into());
        }
        let export_id = response
            .headers()
            .get(EXPORT_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(String::from);
        let failed = response
            .headers()
            .get(EXPORT_FAILED_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(String::from);
        let mut body = Vec::new();
        match response.read_to_end(&mut body) {
            Ok(_) => Ok(Download::File(ExportFile {
                body,
                export_id,
                failed,
            })),
            Err(error) if timed_out(&error) => Ok(Download::TimedOut),
            Err(error) => Ok(Download::Cut(error.to_string())),
        }
    }

    /// Post a chat message and read the answer as it streams (server-sent events). `on_chunk` gets every event;
    /// answers the last `nexus.session`, `nexus.question` and `nexus.message`. Sent once: a stream is never retried.
    pub fn stream_chat_message(&self, id: &str, body: Value, on_chunk: &mut dyn FnMut(&Value)) -> Result<ChatOutcome> {
        let mut body = body;
        if let Value::Object(map) = &mut body {
            map.insert("stream".into(), Value::Bool(true));
        }
        let key = self.options.idempotency.then(|| uuid::Uuid::new_v4().to_string());
        let path = format!("/v1/chat/sessions/{}/messages", enc(id));
        let response = self
            .request(&Method::POST, &path, Some(&body), key.as_deref())
            .header("accept", "text/event-stream")
            .timeout(self.options.stream_timeout)
            .send()
            .map_err(|error| transport(&error))?;
        let event_stream = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"));
        if !response.status().is_success() || !event_stream {
            let status = response.status().as_u16();
            let text = response.text().map_err(|error| transport(&error))?;
            let mut error = failed_answer(status, &text);
            error.interval = None;
            return Err(error.into());
        }
        read_events(response, on_chunk)
    }

    /// `POST /v1/chat/completions` with `stream: true` (the `nexus` session's agent): each `chat.completion.chunk` to
    /// `on_chunk`, until the stream ends or `on_chunk` answers false (the person stopped it: the connection closes).
    /// Sent once: a stream is never retried.
    pub fn stream_completion(&self, body: Value, on_chunk: &mut dyn FnMut(&Value) -> bool) -> Result<()> {
        let mut body = body;
        if let Value::Object(map) = &mut body {
            map.insert("stream".into(), Value::Bool(true));
        }
        let key = self.options.idempotency.then(|| uuid::Uuid::new_v4().to_string());
        let mut response = self
            .request(&Method::POST, "/v1/chat/completions", Some(&body), key.as_deref())
            .header("accept", "text/event-stream")
            .timeout(self.options.stream_timeout)
            .send()
            .map_err(|error| transport(&error))?;
        let event_stream = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"));
        if !response.status().is_success() || !event_stream {
            let status = response.status().as_u16();
            let text = response.text().map_err(|error| transport(&error))?;
            let mut error = failed_answer(status, &text);
            error.interval = None;
            return Err(error.into());
        }
        let mut decoder = Decoder::new();
        let mut chunk = [0u8; 8192];
        loop {
            let read = response.read(&mut chunk).map_err(|error| {
                if error.kind() == std::io::ErrorKind::TimedOut {
                    Error::Transport(Transport::Timeout)
                } else {
                    Error::Transport(Transport::Unreachable(error.to_string()))
                }
            })?;
            for event in decoder.push(&chunk[..read]) {
                if !on_chunk(&event?) {
                    return Ok(());
                }
            }
            if read == 0 {
                return Ok(());
            }
        }
    }
}

/// Whether a body read failed on the time limit: reqwest wraps its own error in an `io::Error` of kind `Other`.
fn timed_out(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::TimedOut
        || error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
            .is_some_and(reqwest::Error::is_timeout)
}

fn read_events(mut response: Response, on_chunk: &mut dyn FnMut(&Value)) -> Result<ChatOutcome> {
    let mut decoder = Decoder::new();
    let mut outcome = ChatStream::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = response.read(&mut chunk).map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut {
                Error::Transport(Transport::Timeout)
            } else {
                Error::Transport(Transport::Unreachable(error.to_string()))
            }
        })?;
        for event in decoder.push(&chunk[..read]) {
            let event = event?;
            outcome.observe(&event);
            on_chunk(&event);
        }
        if read == 0 {
            break;
        }
    }
    outcome.finish()
}
