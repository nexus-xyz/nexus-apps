use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::Context;
use crate::error::{CliError, Error, Result};

use super::{
    ACCESS_REFUSED, CALL_WAIT, CONNECT_WAIT, DESKTOP_UNAVAILABLE, NO_ACCESS, NO_ANSWER, NOT_RUNNING, NOT_RUNNING_HINT,
    SESSION_NOT_FOUND, message_of, save_token, saved_token, session_not_found, socket,
};

/// One request line to the app and its one reply line.
pub fn exchange(path: &Path, request: &Value, wait: Duration) -> Result<Value> {
    exchange_with(path, request, wait, false, || {})
}

#[cfg(unix)]
fn exchange_with(path: &Path, request: &Value, wait: Duration, limited: bool, sent: impl FnOnce()) -> Result<Value> {
    use std::io::{BufRead, BufReader, ErrorKind, Write};

    let start = Instant::now();
    let socket_error = |error: std::io::Error| {
        let failure = CliError::hint(NOT_RUNNING, NOT_RUNNING_HINT);
        if limited && matches!(error.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) {
            failure.with_code(DESKTOP_UNAVAILABLE)
        } else {
            failure
        }
    };
    let mut stream = std::os::unix::net::UnixStream::connect(path).map_err(socket_error)?;
    let left = || {
        if limited {
            wait.saturating_sub(start.elapsed())
        } else {
            wait
        }
    };
    if left().is_zero() {
        return Err(CliError::new(NO_ANSWER).into());
    }
    let _ = stream.set_write_timeout(Some(left().min(Duration::from_secs(5))));
    writeln!(stream, "{request}").map_err(socket_error)?;
    sent();
    let wait = left();
    if wait.is_zero() {
        return Err(CliError::new(NO_ANSWER).into());
    }
    let _ = stream.set_read_timeout(Some(wait));
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|_| CliError::new(NO_ANSWER))?;
    serde_json::from_str(&line).map_err(|_| CliError::new("Nexus for desktop's answer did not read.").into())
}

#[cfg(not(unix))]
fn exchange_with(_: &Path, _: &Value, _: Duration, _: bool, _: impl FnOnce()) -> Result<Value> {
    Err(CliError::new("Outside access needs Nexus for desktop on macOS or Linux.").into())
}

/// A connection to the app as one client: its socket, its name, and its token once granted.
pub struct Desktop<'a> {
    ctx: &'a Context,
    path: PathBuf,
    client: String,
    wait_limit: Option<(Instant, Duration)>,
    final_read: Option<Instant>,
    access_requested: Cell<bool>,
}

impl<'a> Desktop<'a> {
    pub fn new(ctx: &'a Context, client: &str) -> Result<Desktop<'a>> {
        let path =
            socket(ctx).ok_or_else(|| CliError::new("Outside access needs Nexus for desktop on macOS or Linux."))?;
        Ok(Desktop {
            ctx,
            path,
            client: client.to_string(),
            wait_limit: None,
            final_read: None,
            access_requested: Cell::new(false),
        })
    }

    /// Bound every request by the time left for the command.
    pub(crate) fn with_wait_limit(mut self, start: Instant, timeout: Duration) -> Self {
        self.wait_limit = Some((start, timeout));
        self
    }

    /// Give the final authenticated read time to query the app after the deadline.
    pub(crate) fn start_final_read(&mut self) {
        self.final_read = Some(Instant::now());
    }

    fn exchange(&self, request: &Value, timeout: Duration) -> Result<Value> {
        let limit = self
            .final_read
            .map(|start| (start, Duration::from_secs(3)))
            .or(self.wait_limit);
        let wait = limit.map_or(timeout, |(start, limit)| {
            timeout.min(limit.saturating_sub(start.elapsed()))
        });
        if wait.is_zero() {
            return Err(CliError::new(NO_ANSWER).into());
        }
        exchange_with(&self.path, request, wait, limit.is_some(), || {})
    }

    /// Asks for access and saves the token if the person approves in the app.
    pub fn connect(&self, scope: &str) -> Result<(String, String)> {
        let prompt = || {
            self.ctx.io.err(&format!(
                "Approve “{}” in Nexus for desktop: it asks you now.",
                self.client
            ));
        };
        let request = json!({"client": self.client, "connect": scope});
        let reply = if let Some((start, limit)) = self.wait_limit {
            if self.access_requested.get() || self.final_read.is_some() {
                return Err(CliError::new(NO_ANSWER).into());
            }
            let wait = CONNECT_WAIT.min(limit.saturating_sub(start.elapsed()));
            if wait.is_zero() {
                return Err(CliError::new(NO_ANSWER).into());
            }
            exchange_with(&self.path, &request, wait, true, || {
                self.access_requested.set(true);
                prompt();
            })
            .map_err(|error| {
                if self.access_requested.get() {
                    self.access_refused(&message_of(&error))
                } else {
                    error
                }
            })?
        } else {
            prompt();
            exchange(&self.path, &request, CONNECT_WAIT)?
        };
        let (Some(token), Some(granted)) = (reply["token"].as_str(), reply["scope"].as_str()) else {
            let why = reply["error"]["message"].as_str().unwrap_or("no answer");
            return Err(self.access_refused(why));
        };
        save_token(self.ctx, &self.client, Some(token))?;
        Ok((token.to_string(), granted.to_string()))
    }

    fn access_refused(&self, why: &str) -> Error {
        self.cli_error(format!("Nexus for desktop did not grant access: {why}"), ACCESS_REFUSED)
            .into()
    }

    pub(super) fn cli_error(&self, message: impl Into<String>, code: &'static str) -> CliError {
        let error = CliError::new(message);
        if self.wait_limit.is_some() {
            error.with_code(code)
        } else {
            error
        }
    }

    pub fn forget(&self) -> Result<()> {
        save_token(self.ctx, &self.client, None)
    }

    /// Sends one MCP message with the saved token, asking for access first when there is none, and once more when the
    /// app no longer knows the token (revoked).
    pub fn send(&self, message: &Value) -> Result<Value> {
        let saved = saved_token(self.ctx, &self.client);
        let fresh = saved.is_none();
        let token = match saved {
            Some(token) => token,
            None => self.connect("act")?.0,
        };
        let reply = self.exchange(&json!({"token": token, "message": message}), CALL_WAIT)?;
        if reply["error"]["code"].as_i64() != Some(NO_ACCESS) {
            return Ok(reply);
        }
        self.forget()?;
        if fresh || self.access_requested.get() || self.final_read.is_some() {
            return Ok(reply);
        }
        let token = self.connect("act")?.0;
        self.exchange(&json!({"token": token, "message": message}), CALL_WAIT)
    }

    /// Calls tool `name`: its structured answer, or its error as the CLI's.
    pub fn call(&self, name: &str, arguments: Value) -> Result<Value> {
        let reply = self.send(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}
        }))?;
        if let Some(message) = reply["error"]["message"].as_str() {
            let error = if reply["error"]["code"].as_i64() == Some(NO_ACCESS) {
                self.cli_error(message, ACCESS_REFUSED)
            } else {
                CliError::new(message)
            };
            return Err(error.into());
        }
        let result = &reply["result"];
        if result["isError"] == true {
            let text = result["content"][0]["text"].as_str().unwrap_or("The tool failed.");
            let error = if name == "show_session"
                && session_not_found(text, arguments["session_id"].as_str().unwrap_or_default())
            {
                self.cli_error(text, SESSION_NOT_FOUND)
            } else {
                CliError::new(text)
            };
            return Err(error.into());
        }
        Ok(result["structuredContent"].clone())
    }
}
