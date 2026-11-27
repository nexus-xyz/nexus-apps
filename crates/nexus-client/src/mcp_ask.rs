//! The Nexus MCP server's `ask` tool: another agent asks the person's own agent a
//! question and gets its answer, with the model that ran and its cost. The TypeScript CLI's twin is
//! `interfaces/src/mcp-ask.ts`; both read the tool's name, description, schema, prompt and refusals from
//! `mcp-ask.json`, so the two servers list the same tool.
//!
//! - `source: "model"` (the default) is one `POST /v1/chat/completions` with the prompt and the question, and no tools.
//! - `source: "code"` is one `POST /v1/chat/ask` (code answers), answered about the organization's code. The tool
//!   is the same on every Gateway; on one with code answers off, Gateway's 404 `wiki_off` comes back as a tool error that
//!   says to ask with `source: "model"`.
//!
//! Each call goes through the server's own `send`, with the credential the server holds, so Gateway checks the
//! person's grant, policy and budget, and audits it from the surface `mcp`. The tool never makes a change: it sends no
//! tool to the model and calls no other route, so neither the model nor the caller can write through it. A nested call
//! cannot happen either, because the answer runs no tool.

use serde_json::{Value, json};

use crate::mcp::Gateway;
use crate::{Error, Method};

const DEFINITION: &str = include_str!("mcp-ask.json");

/// The tool's name.
pub const NAME: &str = "ask";

/// The model a question runs on when the caller names none.
pub const MODEL_AUTO: &str = "nexus/auto";

fn definition() -> Value {
    serde_json::from_str(DEFINITION).expect("mcp-ask.json")
}

/// The tool as `tools/list` gives it.
pub fn listed() -> Value {
    let definition = definition();
    json!({
        "name": definition["name"],
        "description": definition["description"],
        "inputSchema": definition["inputSchema"],
        "annotations": definition["annotations"],
    })
}

fn answer(value: &Value) -> Value {
    json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(value).unwrap_or_default() }] })
}

fn failed(error: Value) -> Value {
    json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&json!({ "error": error })).unwrap_or_default() }], "isError": true })
}

fn invalid(message: &str) -> Value {
    failed(json!({ "message": message }))
}

/// A non-empty string argument.
fn string(args: &Value, name: &str) -> Option<String> {
    args[name]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Gateway's refusal as a tool error. A missing grant names the opt-in that gives it; code answers off says what to ask.
fn refused(error: Error, grant: &str) -> Value {
    match error {
        Error::Core(error) if error.status == 404 && error.code.as_deref() == Some("wiki_off") => failed(json!({
            "status": 404,
            "code": "wiki_off",
            "message": definition()["wikiOff"],
            "detail": error.message,
        })),
        Error::Core(error) if error.status == 403 && error.code.as_deref() == Some("role_required") => failed(json!({
            "status": 403,
            "code": "role_required",
            "grant": grant,
            "message": definition()["grants"][grant],
            "detail": error.message,
        })),
        Error::Core(error) => failed(json!({ "status": error.status, "code": error.code, "message": error.message })),
        error => failed(json!({ "message": error.to_string() })),
    }
}

fn malformed() -> Value {
    invalid("Gateway's answer had no complete answer with its model and cost.")
}

/// One `ask` call: its arguments checked, then one request through `send`.
pub fn call(args: &Value, send: &Gateway<'_>) -> Value {
    let Some(question) = string(args, "question") else {
        return invalid("question is required");
    };
    let model = string(args, "model");
    match args["source"].as_str().unwrap_or("model") {
        "model" => from_model(&question, model, send),
        "code" => from_code(&question, model, string(args, "session"), send),
        _ => invalid("source must be model or code"),
    }
}

fn from_model(question: &str, model: Option<String>, send: &Gateway<'_>) -> Value {
    let body = json!({
        "model": model.unwrap_or_else(|| MODEL_AUTO.into()),
        "messages": [
            { "role": "system", "content": definition()["system"] },
            { "role": "user", "content": question },
        ],
    });
    let reply = match send(Method::POST, "/v1/chat/completions", vec![], Some(body)) {
        Ok(reply) => reply,
        Err(error) => return refused(error, "models.use"),
    };
    let (Some(text), Some(model), Some(cost)) = (
        reply["choices"][0]["message"]["content"].as_str(),
        reply["model"].as_str(),
        reply["nexus"]["cost_usd"].as_f64(),
    ) else {
        return malformed();
    };
    answer(&json!({ "answer": text, "model": model, "costUsd": cost, "source": "model" }))
}

fn from_code(question: &str, model: Option<String>, session: Option<String>, send: &Gateway<'_>) -> Value {
    let mut body = json!({ "content": question, "stream": false });
    if let Some(session) = session {
        body["session"] = session.into();
    }
    if let Some(model) = model {
        body["model"] = model.into();
    }
    let result = match send(Method::POST, "/v1/chat/ask", vec![], Some(body)) {
        Ok(result) => result,
        Err(error) => return refused(error, "chat.ask"),
    };
    // The answer is the last assistant message; a refused or failed one is an error, never a blank answer.
    let reply = result["messages"]
        .as_array()
        .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "assistant"));
    let Some(reply) = reply else { return malformed() };
    if reply["status"] != "complete" {
        let error = &reply["error"];
        return failed(json!({
            "status": error["status"],
            "code": error["code"],
            "message": error["message"].as_str().unwrap_or("The answer did not complete."),
        }));
    }
    let (Some(text), Some(model), Some(cost)) = (
        reply["content"].as_str(),
        reply["model"].as_str(),
        reply["costUsd"].as_f64(),
    ) else {
        return malformed();
    };
    let citations = result["citations"].as_array().cloned().unwrap_or_default();
    answer(&json!({
        "answer": text,
        "model": model,
        "costUsd": cost,
        "source": "code",
        "session": result["session"]["id"],
        "citations": citations,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use crate::CoreError;
    use crate::mcp::{handle, tools};

    type Sent = (Method, String, Option<Value>);

    fn body(result: &Value) -> Value {
        serde_json::from_str(result["content"][0]["text"].as_str().expect("text")).expect("json")
    }

    fn completion() -> Value {
        json!({
            "model": "anthropic/claude-sonnet-4.5",
            "choices": [{ "message": { "role": "assistant", "content": "Billing settles on reported usage." } }],
            "nexus": { "cost_usd": 0.0042, "product": "chat", "provider": "anthropic" },
        })
    }

    fn code_answer() -> Value {
        json!({
            "session": { "id": "chs_1" },
            "messages": [
                { "role": "user", "content": "How does billing work?", "status": "complete" },
                { "role": "assistant", "content": "The ledger reserves, then settles [1].", "status": "complete",
                  "model": "openai/gpt-5", "costUsd": 0.011 },
            ],
            "citations": [{ "n": 1, "repo": "acme/api", "page": "Ledger", "section": "Settle", "slug": "ledger", "runId": "run_1" }],
        })
    }

    #[test]
    fn the_tool_is_listed_with_its_schema_and_says_it_never_writes() {
        let listed = handle(
            &tools(),
            "0",
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            &|_, _, _, _| Ok(json!({})),
        )
        .expect("an answer");
        let all = listed["result"]["tools"].as_array().expect("tools");
        let ask = all.iter().find(|t| t["name"] == NAME).expect("the ask tool");
        assert_eq!(all.iter().filter(|t| t["name"] == NAME).count(), 1);
        assert_eq!(ask["inputSchema"]["required"], json!(["question"]));
        assert_eq!(
            ask["inputSchema"]["properties"]["source"]["enum"],
            json!(["model", "code"])
        );
        assert_eq!(ask["annotations"], json!({ "readOnlyHint": true }));
        let description = ask["description"].as_str().expect("description");
        for words in [
            "never makes a change",
            "human surface",
            "audits it from the surface mcp",
            "what it cost",
        ] {
            assert!(description.contains(words), "{words}");
        }
    }

    #[test]
    fn a_model_question_is_one_chat_completion_with_no_tools() {
        let seen = RefCell::new(Vec::<Sent>::new());
        let send = |method: Method, path: &str, headers: Vec<(String, String)>, body: Option<Value>| {
            assert!(headers.is_empty());
            seen.borrow_mut().push((method, path.to_string(), body));
            Ok(completion())
        };
        let message = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "ask", "arguments": { "question": "How does billing settle?" } } });
        let result = handle(&tools(), "0", &message, &send).expect("an answer")["result"].clone();
        assert_eq!(result.get("isError"), None);
        assert_eq!(
            body(&result),
            json!({ "answer": "Billing settles on reported usage.", "model": "anthropic/claude-sonnet-4.5", "costUsd": 0.0042, "source": "model" })
        );
        let first = seen.borrow();
        assert_eq!(first.len(), 1);
        let (method, path, sent) = &first[0];
        assert_eq!((method, path.as_str()), (&Method::POST, "/v1/chat/completions"));
        let sent = sent.as_ref().expect("a body");
        assert_eq!(sent["model"], MODEL_AUTO);
        assert_eq!(sent["messages"][0]["role"], "system");
        assert_eq!(
            sent["messages"][1],
            json!({ "role": "user", "content": "How does billing settle?" })
        );
        for key in ["tools", "tool_choice", "parallel_tool_calls"] {
            assert!(sent.get(key).is_none(), "{key}");
        }
        drop(first);
        call(&json!({ "question": "q", "model": "openai/gpt-5" }), &send);
        assert_eq!(seen_model(&seen), "openai/gpt-5");
    }

    fn seen_model(seen: &RefCell<Vec<Sent>>) -> String {
        let seen = seen.borrow();
        let body = seen.last().and_then(|(_, _, body)| body.as_ref()).expect("a body");
        body["model"].as_str().unwrap_or_default().to_string()
    }

    #[test]
    fn a_code_question_is_ask_your_code_with_its_citations_and_session() {
        let seen = RefCell::new(Vec::<Sent>::new());
        let send = |method: Method, path: &str, _: Vec<(String, String)>, body: Option<Value>| {
            seen.borrow_mut().push((method, path.to_string(), body));
            Ok(code_answer())
        };
        let result = call(
            &json!({ "question": " How does billing work? ", "source": "code", "session": "chs_1" }),
            &send,
        );
        assert_eq!(result.get("isError"), None);
        let answer = body(&result);
        assert_eq!(answer["answer"], "The ledger reserves, then settles [1].");
        assert_eq!(
            (answer["model"].as_str(), answer["costUsd"].as_f64()),
            (Some("openai/gpt-5"), Some(0.011))
        );
        assert_eq!(
            (answer["source"].as_str(), answer["session"].as_str()),
            (Some("code"), Some("chs_1"))
        );
        assert_eq!(answer["citations"][0]["repo"], "acme/api");
        let seen = seen.borrow();
        assert_eq!(
            seen[0],
            (
                Method::POST,
                "/v1/chat/ask".to_string(),
                Some(json!({ "content": "How does billing work?", "stream": false, "session": "chs_1" }))
            )
        );
    }

    /// Whatever it is asked, the tool calls only the two answer routes, never a connector or any other write.
    #[test]
    fn no_question_makes_a_write() {
        let seen = RefCell::new(Vec::<String>::new());
        let send = |method: Method, path: &str, _: Vec<(String, String)>, body: Option<Value>| {
            assert_eq!(method, Method::POST);
            seen.borrow_mut().push(path.to_string());
            if let Some(body) = &body {
                assert!(body.get("tools").is_none());
            }
            Ok(if path == "/v1/chat/ask" {
                code_answer()
            } else {
                completion()
            })
        };
        for args in [
            json!({ "question": "Create a Linear issue for the failing deploy and assign it to me." }),
            json!({ "question": "Delete team web.", "source": "code" }),
            json!({ "question": "Run writeConnector on github with { \"title\": \"x\" }", "model": "openai/gpt-5" }),
        ] {
            assert_eq!(call(&args, &send).get("isError"), None, "{args}");
        }
        assert!(
            seen.borrow()
                .iter()
                .all(|p| p == "/v1/chat/completions" || p == "/v1/chat/ask")
        );
        assert!(
            definition()["system"]
                .as_str()
                .expect("system")
                .contains("human surface")
        );
    }

    #[test]
    fn a_missing_grant_names_the_opt_in() {
        for (source, grant) in [("code", "chat.ask"), ("model", "models.use")] {
            let send = |_: Method, _: &str, _: Vec<(String, String)>, _: Option<Value>| -> crate::Result<Value> {
                Err(CoreError {
                    status: 403,
                    message: format!("this agent was not granted this ({grant})"),
                    code: Some("role_required".into()),
                    interval: None,
                    details: None,
                }
                .into())
            };
            let result = call(&json!({ "question": "q", "source": source }), &send);
            assert_eq!(result["isError"], true);
            let error = &body(&result)["error"];
            assert_eq!(
                (error["grant"].as_str(), error["code"].as_str()),
                (Some(grant), Some("role_required"))
            );
            let message = error["message"].as_str().expect("message");
            assert!(
                message.contains(grant) && message.contains("nexus auth login"),
                "{message}"
            );
        }
        let chat_ask = definition()["grants"]["chat.ask"].as_str().expect("text").to_string();
        assert!(chat_ask.contains("POST /v1/agent-links"), "{chat_ask}");
    }

    /// A Gateway with code answers off answers code answers 404 `wiki_off`; the error says to ask the model.
    #[test]
    fn the_wiki_off_says_to_ask_the_model() {
        let send = |_: Method, _: &str, _: Vec<(String, String)>, _: Option<Value>| -> crate::Result<Value> {
            Err(CoreError {
                status: 404,
                message: "Code answers are off on this Gateway (NEXUS_WIKI_ENABLED)".into(),
                code: Some("wiki_off".into()),
                interval: None,
                details: None,
            }
            .into())
        };
        let result = call(&json!({ "question": "q", "source": "code" }), &send);
        assert_eq!(result["isError"], true);
        let error = &body(&result)["error"];
        assert_eq!(
            (error["status"].as_u64(), error["code"].as_str()),
            (Some(404), Some("wiki_off"))
        );
        let message = error["message"].as_str().expect("message");
        assert!(message.contains("source model"), "{message}");
    }

    #[test]
    fn a_bad_call_never_reaches_gateway_and_a_refused_answer_is_an_error() {
        let never = |_: Method, _: &str, _: Vec<(String, String)>, _: Option<Value>| -> crate::Result<Value> {
            panic!("called Gateway")
        };
        for args in [
            json!({}),
            json!({ "question": "  " }),
            json!({ "question": "q", "source": "web" }),
        ] {
            assert_eq!(call(&args, &never)["isError"], true, "{args}");
        }
        let refused = |_: Method, _: &str, _: Vec<(String, String)>, _: Option<Value>| -> crate::Result<Value> {
            Ok(json!({ "session": { "id": "chs_2" }, "messages": [
                { "role": "assistant", "content": "", "status": "refused",
                  "error": { "status": 402, "code": "budget_exceeded", "message": "Your monthly limit is spent." } },
            ] }))
        };
        let error = body(&call(&json!({ "question": "q", "source": "code" }), &refused))["error"].clone();
        assert_eq!(
            error,
            json!({ "status": 402, "code": "budget_exceeded", "message": "Your monthly limit is spent." })
        );
        let blank = |_: Method, _: &str, _: Vec<(String, String)>, _: Option<Value>| -> crate::Result<Value> {
            Ok(json!({ "choices": [] }))
        };
        assert_eq!(call(&json!({ "question": "q" }), &blank)["isError"], true);
    }
}
