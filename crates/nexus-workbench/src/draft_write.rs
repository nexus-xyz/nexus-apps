//! "Write it"'s call: the commit message or the pull request's text drafted from a [`draft::Context`] by a model
//! through Nexus Gateway, with the person's token. Nexus GPUI's commit and pull request
//! forms make it. The app's own, never a remote host's: a host has no token to call
//! Gateway with.

use nexus_client::agents;
use serde_json::Value;

use crate::draft;
use crate::event::Via;

/// The model a session's draft asks for: the session's, when it runs on
/// Gateway's route; None lets Gateway pick. A session on the person's own plan names its provider's model, which Gateway
/// may not serve, so its draft lets Gateway pick too: the draft always goes through Gateway, never to a provider.
pub fn asked(via: Via, model: Option<&str>) -> Option<&str> {
    match via {
        Via::Nexus => model,
        Via::Subscription => None,
    }
}

/// The model a draft asks Gateway for: the session's, when the person picked one on Gateway's route; else (none, or
/// `auto`) `nexus/auto`, which Gateway resolves by the organization's policy, as `nexus chat` does with no model.
pub fn model(asked: Option<&str>) -> String {
    match asked.map(str::trim).filter(|model| !model.is_empty()) {
        Some(model) if !agents::is_auto_name(Some(model)) => model.to_string(),
        _ => "nexus/auto".to_string(),
    }
}

/// The `POST /v1/chat/completions` body for `context`'s draft, on the model [`model`] picks from `asked`.
pub fn body(context: &draft::Context, asked: Option<&str>) -> Value {
    draft::request(&model(asked), context)
}

/// Gateway's `answer` to [`body`], read back into the draft.
pub fn answered(kind: draft::Kind, answer: &Value) -> Result<draft::Draft, String> {
    let text = draft::answer_text(answer).ok_or_else(|| "Nexus Gateway answered without any text.".to_string())?;
    draft::read(kind, &text)
}

/// One `POST /v1/chat/completions` for `context`'s draft, read back into its text. Gateway's error as it said it.
pub async fn write(
    client: &nexus_client::Client,
    context: &draft::Context,
    asked: Option<&str>,
) -> Result<draft::Draft, String> {
    let answer = client
        .post("/v1/chat/completions", body(context, asked))
        .await
        .map_err(|e| e.to_string())?;
    answered(context.kind, &answer)
}

#[cfg(test)]
mod tests {
    use super::{Via, answered, asked, draft, model, write};
    use serde_json::Value;
    use std::io::{Read, Write};

    /// A Gateway on this computer that answers one request with `status` and `body`, and hands back what it was sent.
    fn gateway(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connection");
            let mut request = Vec::new();
            let mut chunk = [0; 65_536];
            // The head, then as many bytes as its content-length says.
            loop {
                let n = stream.read(&mut chunk).expect("read");
                request.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|n| n.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if request.len() >= head_end + 4 + length || n == 0 {
                        break;
                    }
                }
            }
            stream.write_all(response.as_bytes()).expect("response");
            String::from_utf8_lossy(&request).into_owned()
        });
        (url, server)
    }

    fn client(url: &str) -> nexus_client::Client {
        nexus_client::Client::new(nexus_client::ClientOptions {
            api_key: Some("nxs_test".into()),
            ..nexus_client::ClientOptions::new(url)
        })
    }

    fn context() -> draft::Context {
        draft::Context {
            kind: draft::Kind::PullRequest,
            branch: Some("ada/eng-1-two".into()),
            base: Some("origin/main".into()),
            subjects: vec!["feat: one".into()],
            commits: vec!["feat: two".into()],
            template: None,
            diff: "diff --git a/a.txt b/a.txt".into(),
            cut: false,
            untracked: vec![],
        }
    }

    #[test]
    fn a_session_asks_for_its_model_only_on_gateways_route() {
        assert_eq!(
            asked(Via::Nexus, Some("anthropic/claude-haiku-5.5")),
            Some("anthropic/claude-haiku-5.5")
        );
        assert_eq!(asked(Via::Nexus, None), None);
        assert_eq!(asked(Via::Subscription, Some("claude-opus-5-5")), None);
    }

    #[test]
    fn the_model_is_the_sessions_unless_it_is_auto_or_none() {
        assert_eq!(model(Some("anthropic/claude-haiku-5.5")), "anthropic/claude-haiku-5.5");
        for asked in [None, Some(""), Some("auto"), Some("nexus/default")] {
            assert_eq!(model(asked), "nexus/auto", "{asked:?}");
        }
    }

    #[tokio::test]
    async fn a_draft_is_one_chat_completion_through_gateway() {
        let answer = r#"{"choices":[{"message":{"role":"assistant","content":"Two, at last\n\nWhy it matters."}}]}"#;
        let (url, server) = gateway("200 OK", answer);
        let draft = write(&client(&url), &context(), None).await;
        let request = server.join().expect("server");
        assert_eq!(
            draft,
            Ok(draft::Draft {
                title: "Two, at last".into(),
                body: "Why it matters.".into()
            })
        );
        assert!(request.starts_with("POST /v1/chat/completions "), "{request}");
        assert!(
            request.to_ascii_lowercase().contains("authorization: bearer nxs_test"),
            "{request}"
        );
        let body: Value =
            serde_json::from_str(&request[request.find("\r\n\r\n").expect("a body") + 4..]).expect("json");
        assert_eq!(body, draft::request("nexus/auto", &context()));
    }

    #[tokio::test]
    async fn gateways_error_is_what_the_person_sees() {
        let (url, server) = gateway(
            "403 Forbidden",
            r#"{"error":{"code":"model_not_allowed","message":"Your team may not use this model."}}"#,
        );
        let draft = write(&client(&url), &context(), Some("openai/gpt-6")).await;
        let request = server.join().expect("server");
        assert_eq!(draft, Err("Your team may not use this model.".to_string()));
        assert!(request.contains(r#""model":"openai/gpt-6""#), "{request}");
    }

    #[test]
    fn an_answer_without_text_says_so() {
        let empty = serde_json::json!({ "choices": [] });
        assert_eq!(
            answered(draft::Kind::Commit, &empty),
            Err("Nexus Gateway answered without any text.".to_string())
        );
    }
}
