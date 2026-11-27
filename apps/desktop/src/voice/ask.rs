//! How a voice turn is asked (Nexus UI's `voice/voice-ask.ts`): the same `POST /agent/ask` the typed chat sends
//! (`crate::agent`), in the same conversation, read to its `done` event. A conversation the ask starts is kept, so the
//! next turn and the thread continue it.

use nexus_desktop_core::agent::{self as rules, Event, EventReader, Reply};
use serde_json::Value;

use super::call::{POOR_CONNECTION, VoiceReply};
use super::spoken::spoken_reply;
use crate::agent::Chunk;

const CUT: &str = "The answer stopped before it finished. Say it again.";

/// One voice ask's answer as it arrives.
#[derive(Debug, Default)]
pub struct AskReading {
    status: Option<u16>,
    events: Option<EventReader>,
    body: Vec<u8>,
    reply: Option<Reply>,
    failed: Option<VoiceReply>,
    broken: bool,
}

fn problem(problem: rules::Problem) -> VoiceReply {
    VoiceReply::Problem {
        text: format!("{}. {}", problem.title, problem.detail),
        offline: false,
    }
}

impl AskReading {
    /// The next chunk. Returns the conversation the ask started, the moment the API names it.
    pub fn chunk(&mut self, chunk: Chunk) -> Option<String> {
        let events = match chunk {
            Chunk::Head { status, content_type } => {
                self.status = Some(status);
                if (200..300).contains(&status) && content_type.starts_with("text/event-stream") {
                    self.events = Some(EventReader::default());
                }
                Vec::new()
            }
            Chunk::Data(bytes) => match self.events.as_mut() {
                Some(reader) => reader.push(&bytes),
                None => {
                    self.body.extend_from_slice(&bytes);
                    Vec::new()
                }
            },
            Chunk::Broken => {
                self.broken = true;
                Vec::new()
            }
            Chunk::End => self.events.as_mut().map(EventReader::finish).unwrap_or_default(),
        };
        let mut kept = None;
        for event in events {
            match event {
                Event::Started { conversation } => kept = Some(conversation),
                Event::Done { reply } => {
                    if !reply.conversation.is_empty() {
                        kept = Some(reply.conversation.clone());
                    }
                    self.reply = Some(reply);
                }
                Event::Error { status, error, detail } => {
                    self.failed = Some(problem(rules::problem(status, Some(&error), Some(&detail))))
                }
                Event::Model { .. } | Event::Text { .. } | Event::Step { .. } => {}
            }
        }
        kept
    }

    /// What the call hears once the answer ended.
    pub fn finish(self) -> VoiceReply {
        if let Some(failed) = self.failed {
            return failed;
        }
        if let Some(reply) = self.reply {
            return spoken_reply(reply.messages);
        }
        if self.events.is_some() || self.broken {
            return VoiceReply::Problem {
                text: CUT.into(),
                offline: false,
            };
        }
        // Not a stream: the failure's JSON. No answer at all from the agent service reads as the network.
        let status = self.status.unwrap_or(502);
        let status = if (200..300).contains(&status) { 502 } else { status };
        let body: Option<Value> = serde_json::from_slice(&self.body).ok();
        let error = body.as_ref().and_then(|body| body.get("error")).and_then(Value::as_str);
        if error == Some(rules::UNREACHABLE.1) {
            return VoiceReply::Problem {
                text: POOR_CONNECTION.into(),
                offline: true,
            };
        }
        problem(rules::problem_of(status, body.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(lines: &str) -> (AskReading, Vec<String>) {
        let mut reading = AskReading::default();
        let mut kept = Vec::new();
        for chunk in [
            Chunk::Head {
                status: 200,
                content_type: "text/event-stream".into(),
            },
            Chunk::Data(lines.as_bytes().to_vec()),
            Chunk::End,
        ] {
            kept.extend(reading.chunk(chunk));
        }
        (reading, kept)
    }

    #[test]
    fn a_streamed_answer_is_said_and_its_conversation_kept() {
        let done = serde_json::json!({
            "status": 200, "conversation": "c-1",
            "messages": [{ "kind": "answer", "text": "**Booked.**", "model": "m" }],
        });
        let (reading, kept) = stream(&format!(
            "event: started\ndata: {{\"conversation\":\"c-1\"}}\n\nevent: done\ndata: {done}\n\n"
        ));
        assert_eq!(kept.first().map(String::as_str), Some("c-1"));
        let VoiceReply::Said { say, needs_you, .. } = reading.finish() else {
            panic!("a reply")
        };
        assert_eq!((say.as_str(), needs_you), ("Booked.", false));
    }

    #[test]
    fn an_agent_service_that_does_not_answer_is_a_poor_connection() {
        let (status, error, detail) = rules::UNREACHABLE;
        let mut reading = AskReading::default();
        reading.chunk(Chunk::Head {
            status,
            content_type: "application/json".into(),
        });
        reading.chunk(Chunk::Data(rules::failure_body(error, detail).into_bytes()));
        reading.chunk(Chunk::End);
        assert_eq!(
            reading.finish(),
            VoiceReply::Problem {
                text: POOR_CONNECTION.into(),
                offline: true
            }
        );
    }

    #[test]
    fn a_stream_cut_short_says_to_say_it_again() {
        let (reading, _) = stream("event: started\ndata: {\"conversation\":\"c-1\"}\n\n");
        assert_eq!(
            reading.finish(),
            VoiceReply::Problem {
                text: CUT.into(),
                offline: false
            }
        );
    }
}
