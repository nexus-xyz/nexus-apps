//! The HTTP client for the Nexus API: `nexus-client`'s blocking client (shared with Nexus for desktop and, later, the
//! Apple apps), with the CLI's error type. Bodies are JSON values passed through as Gateway sends them, so `--json`
//! prints exactly Gateway's answer, unknown fields included.
//!
//! Every call the CLI makes carries `x-nexus-client: cli/<version>` (the audit log's surface) and
//! `x-nexus-cli-version` (an internal decision: Gateway refuses a CLI older than it supports with 426 `cli_too_old`). Writes carry a fresh
//! `Idempotency-Key` and are retried with it, up to twice, when the connection drops after sending or Gateway
//! answers 502, 503, 504 or 409 `idempotency_in_progress`: Gateway applies a write at most once. Reads (GET, HEAD) are
//! retried the same way, also on a refused connection, and wait a 503's `Retry-After` (at most 5 s).

use reqwest::{Method, StatusCode};
use serde_json::Value;

pub use nexus_client::ClientOptions;
pub use nexus_client::error::core_error;
pub use nexus_client::url::{enc, search};

use crate::error::Result;

pub const CLI_VERSION_HEADER: &str = nexus_client::CLI_VERSION_HEADER;

/// The CLI's user agent: `nexus-cli/<version>`.
pub const USER_AGENT: &str = concat!("nexus-cli/", env!("NEXUS_CLI_VERSION"));

/// A blocking HTTP client with the CLI's user agent (`nexus update`'s downloads).
pub fn http_client() -> reqwest::blocking::Client {
    nexus_client::blocking::http_client(Some(USER_AGENT))
}

#[derive(Debug, Clone)]
pub struct Client(nexus_client::blocking::Client);

impl Client {
    /// A client with these options and the CLI's user agent.
    pub fn new(options: ClientOptions) -> Client {
        Client(nexus_client::blocking::Client::new(ClientOptions {
            user_agent: Some(USER_AGENT.to_string()),
            ..options
        }))
    }

    pub fn base(&self) -> &str {
        self.0.base()
    }

    /// One HTTP exchange; with idempotency, a write carries a key and is retried safely. Answers the status and body.
    pub fn send(&self, method: Method, path: &str, body: Option<&Value>) -> Result<(StatusCode, String)> {
        Ok(self.0.send(method, path, body)?)
    }

    pub fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        Ok(self.0.call(method, path, body)?)
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        Ok(self.0.get(path)?)
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        Ok(self.0.post(path, body)?)
    }

    pub fn put(&self, path: &str, body: Value) -> Result<Value> {
        Ok(self.0.put(path, body)?)
    }

    pub fn patch(&self, path: &str, body: Value) -> Result<Value> {
        Ok(self.0.patch(path, body)?)
    }

    pub fn delete(&self, path: &str) -> Result<Value> {
        Ok(self.0.delete(path)?)
    }

    /// `GET` a text answer (the audit CSV).
    pub fn get_text(&self, path: &str) -> Result<String> {
        Ok(self.0.get_text(path)?)
    }

    /// `GET` one exported file, sent once (`nexus spend`, `nexus analytics` and `nexus agents` `--csv`).
    pub fn download(&self, path: &str) -> Result<nexus_client::blocking::Download> {
        Ok(self.0.download(path)?)
    }

    /// Post a chat message and read the answer as it streams (server-sent events). `on_chunk` gets every event;
    /// answers the last `nexus.session`, `nexus.question` and `nexus.message`. Sent once: a stream is never retried.
    pub fn stream_chat_message(
        &self,
        id: &str,
        body: Value,
        on_chunk: &mut dyn FnMut(&Value),
    ) -> Result<(Value, Value, Value)> {
        let outcome = self.0.stream_chat_message(id, body, on_chunk)?;
        Ok((outcome.session, outcome.question, outcome.message))
    }

    /// `POST /v1/chat/completions`, streamed: each chunk to `on_chunk` until it answers false or the stream ends.
    pub fn stream_completion(&self, body: Value, on_chunk: &mut dyn FnMut(&Value) -> bool) -> Result<()> {
        Ok(self.0.stream_completion(body, on_chunk)?)
    }
}
