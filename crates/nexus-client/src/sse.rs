//! Server-sent events as Gateway streams a chat answer, decoded exactly as `kit`'s `streamChatMessage`: blocks
//! end at a blank line, a block's `data:` lines are joined, empty data and `[DONE]` are skipped, and every other block
//! is one JSON chunk. A chunk with `error` ends the stream with Gateway's error; the last `nexus.session`,
//! `nexus.question` and `nexus.message` are the outcome. Transport-free: feed it bytes from any client.

use serde_json::Value;

use crate::error::{CoreError, Error, Result, Transport};

/// Splits bytes into events as they arrive. A trailing block without its blank line is never an event (as in
/// TypeScript).
#[derive(Debug, Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    /// Add bytes; answers the complete events among everything received so far, in order. Stops at the first
    /// unreadable or error event (its `Err` is the last item).
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<Value>> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(at) = find(&self.buffer, b"\n\n") {
            let block = String::from_utf8_lossy(&self.buffer[..at]).to_string();
            self.buffer.drain(..at + 2);
            let data = block
                .split('\n')
                .filter(|line| line.starts_with("data:"))
                .map(|line| line[5..].trim_start())
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let parsed: Value = match serde_json::from_str(&data) {
                Ok(parsed) => parsed,
                Err(error) => {
                    events.push(Err(Error::Transport(Transport::Other(format!(
                        "Nexus Gateway sent an unreadable event ({error})"
                    )))));
                    return events;
                }
            };
            if let Some(error) = parsed.get("error").filter(|error| !error.is_null()) {
                events.push(Err(CoreError {
                    status: 502,
                    message: error.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                    code: error.get("code").and_then(Value::as_str).map(String::from),
                    interval: None,
                    details: error.get("nexus").cloned(),
                }
                .into()));
                return events;
            }
            events.push(Ok(parsed));
        }
        events
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// A finished answer: the session (its new title, cost and order), the stored question and the stored answer.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatOutcome {
    pub session: Value,
    pub question: Value,
    pub message: Value,
}

/// Keeps the last `nexus.session`, `nexus.question` and `nexus.message` of a stream's chunks.
#[derive(Debug, Default)]
pub struct ChatStream {
    session: Option<Value>,
    question: Option<Value>,
    message: Option<Value>,
}

impl ChatStream {
    pub fn new() -> ChatStream {
        ChatStream::default()
    }

    /// Note one chunk.
    pub fn observe(&mut self, chunk: &Value) {
        if let Some(nexus) = chunk.get("nexus") {
            for (field, slot) in [
                ("session", &mut self.session),
                ("question", &mut self.question),
                ("message", &mut self.message),
            ] {
                if let Some(value) = nexus.get(field).filter(|value| !value.is_null()) {
                    *slot = Some(value.clone());
                }
            }
        }
    }

    /// The outcome, or the error a stream that ended early is.
    pub fn finish(self) -> Result<ChatOutcome> {
        match (self.session, self.question, self.message) {
            (Some(session), Some(question), Some(message)) => Ok(ChatOutcome {
                session,
                question,
                message,
            }),
            _ => Err(CoreError {
                status: 502,
                message: "the answer's stream ended early; reload the session to see what was kept".into(),
                code: None,
                interval: None,
                details: None,
            }
            .into()),
        }
    }
}

/// The text a chunk adds to the answer (`choices[0].delta.content`), when it adds any.
pub fn delta(chunk: &Value) -> Option<&str> {
    chunk
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
        .filter(|delta| !delta.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_events_split_anywhere() {
        let stream = concat!(
            "data: {\"nexus\":{\"question\":{\"id\":\"q\"}}}\n\n",
            ": comment\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"nexus\":{\"session\":{\"id\":\"s\"},\"message\":{\"id\":\"m\"}}}\n\n",
            "data: [DONE]\n\n",
            "data: {\"never\":true}"
        );
        for split in [1, 3, 7, stream.len()] {
            let mut decoder = Decoder::new();
            let mut outcome = ChatStream::new();
            let mut text = String::new();
            for piece in stream.as_bytes().chunks(split) {
                for event in decoder.push(piece) {
                    let event = event.expect("event");
                    outcome.observe(&event);
                    text.push_str(delta(&event).unwrap_or(""));
                }
            }
            assert_eq!(text, "Hello");
            let outcome = outcome.finish().expect("outcome");
            assert_eq!(outcome.message, json!({ "id": "m" }));
        }
    }

    #[test]
    fn ends_on_errors_and_early_endings() {
        let mut decoder = Decoder::new();
        let events =
            decoder.push(b"data: {\"error\":{\"message\":\"refused\",\"code\":\"policy\",\"nexus\":{\"a\":1}}}\n\nx");
        let Some(Err(Error::Core(error))) = events.last() else {
            panic!("an error event")
        };
        assert_eq!((error.status, error.code.as_deref()), (502, Some("policy")));
        assert!(matches!(
            Decoder::new().push(b"data: {oops\n\n").last(),
            Some(Err(Error::Transport(_)))
        ));
        assert!(ChatStream::new().finish().is_err());
    }
}
