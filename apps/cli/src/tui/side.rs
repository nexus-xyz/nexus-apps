//! `/btw`: a side question about the conversation so far, as Claude Code asks one. It goes with what the
//! conversation holds (the agent's messages, or the chat session's from Gateway) and the earlier side questions, and
//! the model answers with no tools. Neither the question nor the answer joins the conversation. It can run while a
//! turn runs: it reads the conversation as it stands, without the answer still being written.

use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use crate::cli::cli_client;
use crate::error::to_cli_error;
use crate::json::{a, js_string, s};

use super::turn::{Gateway, TurnEvent};

/// What a side question goes with: the conversation, and whether it is the agent's (its tools are named, as a
/// conversation with tool calls needs, and none may be called).
pub enum Context {
    Agent {
        messages: Vec<Value>,
        tools: Value,
    },
    /// A chat session's id, read from Gateway; `None`: no session yet.
    Chat(Option<String>),
}

/// How the question is put to the model, after the conversation.
fn framed(question: &str) -> String {
    format!(
        "(A side question about our conversation so far. Answer it briefly from what the conversation already holds. \
         You can't use tools now: don't call any or write tool calls out.)\n\n{question}"
    )
}

/// Ask `question` on its own thread; its text comes as `BtwDelta` events, then `BtwDone` with its cost.
pub fn ask(
    gateway: Gateway,
    model: String,
    context: Context,
    earlier: Vec<(String, String)>,
    question: String,
    events: Sender<(u64, TurnEvent)>,
) {
    std::thread::spawn(move || {
        let send = |event: TurnEvent| {
            let _ = events.send((0, event));
        };
        let client = cli_client(&gateway.url, Some(&gateway.api_key));
        let (mut messages, tools) = match context {
            Context::Agent { messages, tools } => (super::agent::model_history(&messages), Some(tools)),
            Context::Chat(session) => {
                let messages = session
                    .and_then(|id| {
                        client
                            .get(&format!("/v1/chat/sessions/{}", crate::client::enc(&id)))
                            .ok()
                    })
                    .map(|detail| {
                        // Whether the prompt before an answer asked for a `<next>` line (an earlier session's).
                        let mut asked = false;
                        a(&detail, "messages")
                            .iter()
                            .filter_map(|message| {
                                let role = s(message, "role").filter(|role| matches!(*role, "user" | "assistant"))?;
                                let content = js_string(message.get("content"));
                                let content = match role {
                                    "user" => {
                                        asked = super::suggest::asked_for_next(&content);
                                        super::mentions::strip_files(super::suggest::strip_instruction(&content))
                                            .to_string()
                                    }
                                    _ => super::suggest::strip_suggestion(&content, asked).to_string(),
                                };
                                Some(json!({ "role": role, "content": content }))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (messages, None)
            }
        };
        for (question, answer) in earlier {
            messages.push(json!({ "role": "user", "content": framed(&question) }));
            messages.push(json!({ "role": "assistant", "content": answer }));
        }
        messages.push(json!({ "role": "user", "content": framed(&question) }));
        let mut body = json!({ "model": model, "messages": messages });
        if let Some(tools) = tools {
            body["tools"] = tools;
            body["tool_choice"] = json!("none");
        }
        let mut cost = 0.0;
        let result = client
            .stream_completion(body, &mut |chunk| {
                if let Some(spent) = super::agent::cost_of(chunk) {
                    cost += spent;
                }
                if let Some(text) = chunk
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    send(TurnEvent::BtwDelta(text.to_string()));
                }
                true
            })
            .map_err(|error| to_cli_error(&error, &gateway.url));
        send(TurnEvent::BtwDone(result.map(|()| cost)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_side_question_says_what_it_is() {
        let framed = framed("what was that file?");
        assert!(framed.starts_with("(A side question") && framed.ends_with("what was that file?"));
    }
}
