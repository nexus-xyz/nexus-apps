//! The session's Gateway calls, each on a thread of its own so the session keeps drawing. A prompt: the chat session
//! is made on the first one (as `nexus chat --new` makes one), the answer streams back as events, and the budget is
//! read again when it ends. The lists `/model` and `/resume` open, a session to resume, and `/spend`. The same calls
//! as `nexus chat`, `nexus models` and `nexus spend`.

use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use crate::chat::session_refusal;
use crate::cli::{cli_client, left_text};
use crate::error::{CliError, Result, to_cli_error};
use crate::json::{a, js_string, n, obj, s, t};
use crate::style::{ago, provider_name, provider_of};

use super::list::Item;

/// What a turn sends back, tagged with its number.
#[derive(Debug)]
pub enum TurnEvent {
    /// The chat session the first prompt made.
    Session(String),
    /// Answer text.
    Delta(String),
    /// The stored answer, or why there is none.
    Done(std::result::Result<Value, CliError>),
    /// `$4 of $5 left`, read after the answer.
    Budget(String),
    /// The next prompt the session suggests after an answer: its text (`None`: none), the model that
    /// wrote it, and what it cost.
    Suggested {
        session: String,
        suggestion: Option<String>,
        model: String,
        cost: f64,
    },
    /// The models the person may use, for `/model`.
    Models(std::result::Result<Vec<Item>, CliError>),
    /// Their recent chat sessions, for `/resume`.
    Sessions(std::result::Result<Vec<Item>, CliError>),
    /// A session to resume, with its messages.
    Opened(std::result::Result<Value, CliError>),
    /// An agent conversation to switch to: its title, messages and subagents.
    AgentOpened(std::result::Result<(String, Vec<Value>, Vec<super::agent::sessions::Subagent>), String>),
    /// The agent's conversation as it stands, to draw again on leaving a subagent's transcript.
    MainView(Vec<Value>),
    /// `$4 of $5 left`, asked for with `/spend`.
    Spend(std::result::Result<String, CliError>),
    /// The agent asks before an edit or a command; it waits for the answer.
    Approve(super::agent::tools::Ask),
    /// The agent ran a tool.
    Tool(super::agent::tools::Action),
    /// A subagent (the `agent` tool) started, ran a tool, or finished.
    Task {
        id: String,
        task: super::agent::Task,
    },
    /// The agent's turn ended.
    Finished(std::result::Result<super::agent::Summary, CliError>),
    /// The workspace's files, for `@` mentions.
    Files(Vec<String>),
    /// An image pasted with ⌃v (or as a path), `{data, name}`, or why there is none.
    Image(std::result::Result<Value, String>),
    /// Something to tell the person (a mentioned file that could not be attached).
    Note(String),
    /// `/btw`: the side question's answer as it streams, then its end with its cost.
    BtwDelta(String),
    BtwDone(std::result::Result<f64, CliError>),
    /// What one agent step cost, as it ends.
    Spent(f64),
    /// `/diff`: `git status --short`, then `git diff`, or why there is none.
    Diff(std::result::Result<(Vec<String>, Vec<String>), String>),
    /// `!cmd`: the person's own command, run.
    Shell(super::agent::tools::Action),
    /// `/compact`: the conversation's size before and after (in characters), and what the summary cost.
    Compacted(std::result::Result<(usize, usize, f64), CliError>),
}

/// What to read for a slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Load {
    /// The models, the one in use marked.
    Models {
        current: Option<String>,
    },
    /// The recent sessions, the one in use marked: this folder's agent conversations (`saved`, filled in by the
    /// session's loop), then the chat sessions.
    Sessions {
        current: Option<String>,
        saved: Vec<Item>,
    },
    Session(String),
    Spend,
}

/// Where and as whom the session talks to Gateway, and what each prompt asks for.
#[derive(Debug, Clone)]
pub struct Gateway {
    pub url: String,
    pub api_key: String,
    pub model: Option<String>,
    pub team: Option<String>,
}

impl Gateway {
    fn body(&self, content: Option<&str>) -> Value {
        obj(vec![
            ("content", content.map(|content| json!(content))),
            ("model", self.model.as_ref().map(|model| json!(model))),
            (
                "team",
                self.team
                    .as_ref()
                    .filter(|team| !team.is_empty())
                    .map(|team| json!(team)),
            ),
        ])
    }
}

/// Send `prompt` as turn `turn` on its own thread; its events go to `events`.
pub fn spawn(
    gateway: Gateway,
    session: Option<String>,
    prompt: String,
    images: Vec<Value>,
    suggest: bool,
    turn: u64,
    events: Sender<(u64, TurnEvent)>,
) {
    std::thread::spawn(move || {
        let send = |event: TurnEvent| {
            let _ = events.send((turn, event));
        };
        let client = cli_client(&gateway.url, Some(&gateway.api_key));
        let answer = (|| -> Result<(String, Value)> {
            let id = match session {
                Some(id) => id,
                None => {
                    // Titled by the prompt as typed: Gateway would take the first message, instruction and files too.
                    let mut body = gateway.body(None);
                    let typed = super::mentions::strip_files(super::suggest::strip_instruction(&prompt));
                    let title: String = typed.lines().next().unwrap_or("").trim().chars().take(60).collect();
                    if !title.is_empty() {
                        body["title"] = json!(title);
                    }
                    let made = client.post("/v1/chat/sessions", body)?;
                    let id = js_string(made.get("id"));
                    send(TurnEvent::Session(id.clone()));
                    id
                }
            };
            let (_, _, message) = client
                .stream_chat_message(&id, with_images(gateway.body(Some(&prompt)), &images), &mut |chunk| {
                    if let Some(delta) = chunk
                        .pointer("/choices/0/delta/content")
                        .and_then(Value::as_str)
                        .filter(|delta| !delta.is_empty())
                    {
                        send(TurnEvent::Delta(delta.to_string()));
                    }
                })
                .map_err(|error| session_refusal(error, &id))?;
            Ok((id, message))
        })();
        let session = answer.as_ref().ok().map(|(id, _)| id.clone());
        send(TurnEvent::Done(
            answer
                .map(|(_, message)| message)
                .map_err(|error| to_cli_error(&error, &gateway.url)),
        ));
        // What is left, at once after the answer, and again after a suggestion so it counts that call too.
        let budget = || {
            if let Ok(spend) = client.get("/v1/spend") {
                send(TurnEvent::Budget(left_text(&spend)));
            }
        };
        budget();
        // After a stored answer, the next prompt: a call on the session's cached prompt. One that fails offers none.
        if let Some(session) = session.filter(|_| suggest)
            && let Ok(suggested) = client.post(&format!("/v1/chat/sessions/{session}/suggest"), json!({}))
        {
            send(TurnEvent::Suggested {
                session,
                suggestion: s(&suggested, "suggestion").map(str::to_string),
                model: s(&suggested, "model").unwrap_or_default().to_string(),
                cost: n(&suggested, "costUsd"),
            });
            budget();
        }
    });
}

/// A message body with its images (`images`, as `nexus chat --image` sends them).
fn with_images(mut body: Value, images: &[Value]) -> Value {
    if !images.is_empty()
        && let Value::Object(map) = &mut body
    {
        map.insert("images".into(), Value::Array(images.to_vec()));
    }
    body
}

/// Read what `load` asks for on its own thread; the answer comes back as an event of turn 0.
pub fn load(gateway: Gateway, load: Load, events: Sender<(u64, TurnEvent)>) {
    std::thread::spawn(move || {
        let client = cli_client(&gateway.url, Some(&gateway.api_key));
        let cli_error = |error| to_cli_error(&error, &gateway.url);
        let event = match load {
            Load::Models { current } => TurnEvent::Models(
                client
                    .get("/v1/models?status=all")
                    .map(|response| model_items(a(&response, "data"), current.as_deref()))
                    .map_err(cli_error),
            ),
            Load::Sessions { current, saved } => {
                let chat = client
                    .get("/v1/chat/sessions?limit=30&archived=exclude")
                    .map(|page| session_items(a(&page, "sessions"), current.as_deref()));
                TurnEvent::Sessions(match chat {
                    // Under their headings when there are both; the agent's own when Gateway can't be read.
                    Ok(chat) if !saved.is_empty() => Ok(saved.into_iter().chain(group(chat, "Chats")).collect()),
                    Ok(chat) => Ok(chat),
                    Err(_) if !saved.is_empty() => Ok(saved),
                    Err(error) => Err(cli_error(error)),
                })
            }
            Load::Session(id) => TurnEvent::Opened(
                client
                    .get(&format!("/v1/chat/sessions/{}", crate::client::enc(&id)))
                    .map_err(cli_error),
            ),
            Load::Spend => TurnEvent::Spend(
                client
                    .get("/v1/spend")
                    .map(|spend| left_text(&spend))
                    .map_err(cli_error),
            ),
        };
        let _ = events.send((0, event));
    });
}

/// Auto first, then each model the person may use, by provider, with its price. Gateway lists a provider's models
/// newest first, so a model is older when one before it is of the same family and has the same name but for its
/// version numbers (Claude Opus 4.8 after Claude Opus 5.5; GPT-6 Sol after GPT-6.1 Sol). Other tiers of a family
/// (GPT-6 Luna) and variants (DeepSeek Flash Fast) are not.
fn model_items(models: &[Value], current: Option<&str>) -> Vec<Item> {
    let mut items = vec![Item {
        id: "nexus/auto".into(),
        label: "Auto".into(),
        detail: "chosen by Nexus Gateway within your policies".into(),
        current: current.is_none_or(|current| current == "nexus/auto"),
        group: Some("Nexus".into()),
        older: false,
    }];
    let usable: Vec<&Value> = models
        .iter()
        .filter(|model| s(model, "status") == Some("supported") && t(model, "allowed"))
        .collect();
    // Providers in the order Gateway lists them, each one's models together.
    let mut providers: Vec<String> = Vec::new();
    for model in &usable {
        let provider = provider_of(&js_string(model.get("id")));
        if !providers.contains(&provider) {
            providers.push(provider);
        }
    }
    let mut lines: Vec<(String, String)> = Vec::new();
    for provider in &providers {
        for model in usable
            .iter()
            .filter(|model| provider_of(&js_string(model.get("id"))) == *provider)
        {
            let id = js_string(model.get("id"));
            let label = s(model, "name").map(String::from).unwrap_or_else(|| id.clone());
            let line = (js_string(model.get("family")), without_versions(&label));
            let older = lines.contains(&line);
            if !older {
                lines.push(line);
            }
            items.push(Item {
                current: Some(id.as_str()) == current,
                detail: price(model.get("pricing")),
                group: Some(provider_name(provider)),
                older,
                label,
                id,
            });
        }
    }
    items
}

/// A model's name without its version numbers: `Claude Opus 4.8` and `Claude Opus 5.5` are one line of models.
fn without_versions(name: &str) -> String {
    name.split(|c: char| c.is_whitespace() || c == '-')
        .map(|word| word.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == 'v' || c == 'V'))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// `$3 · $15 per 1M tokens`: the input and output prices, as short as they go.
fn price(pricing: Option<&Value>) -> String {
    let Some(pricing) = pricing.filter(|pricing| pricing.is_object()) else {
        return String::new();
    };
    let short = |value: f64| {
        let text = format!("{value:.2}");
        format!("${}", text.trim_end_matches('0').trim_end_matches('.'))
    };
    let (input, output) = (n(pricing, "input"), n(pricing, "output"));
    if input == 0.0 && output == 0.0 {
        return String::new();
    }
    format!("{} in · {} out per 1M tokens", short(input), short(output))
}

/// Recent sessions, newest first: title, then when they were last used.
fn group(items: Vec<Item>, heading: &str) -> Vec<Item> {
    items
        .into_iter()
        .map(|item| Item {
            group: Some(heading.to_string()),
            ..item
        })
        .collect()
}

/// The terminal's saved agent conversations as `/resume` lists them, under the folder's name; ids start with
/// `agent:`.
pub fn saved_items(saved: &[super::agent::sessions::Saved], current: Option<&str>, folder: &str) -> Vec<Item> {
    saved
        .iter()
        .map(|conversation| {
            let noun = if conversation.messages == 1 {
                "message"
            } else {
                "messages"
            };
            let mut detail = format!(
                "{} · {} {noun} · agent",
                ago(&conversation.updated),
                conversation.messages
            );
            if let Some(model) = &conversation.model {
                detail.push_str(&format!(" · {}", model.rsplit('/').next().unwrap_or(model)));
            }
            Item {
                id: format!("{AGENT_PREFIX}{}", conversation.id),
                label: conversation.title.clone(),
                detail,
                current: Some(conversation.id.as_str()) == current,
                group: Some(folder.to_string()),
                ..Item::default()
            }
        })
        .collect()
}

/// What starts the id of an agent conversation in `/resume`'s list.
pub const AGENT_PREFIX: &str = "agent:";

fn session_items(sessions: &[Value], current: Option<&str>) -> Vec<Item> {
    sessions
        .iter()
        .map(|session| {
            let id = js_string(session.get("id"));
            let title = s(session, "title")
                .map(super::suggest::title_without_instruction)
                .filter(|title| !title.is_empty())
                .unwrap_or("Untitled");
            Item {
                current: Some(id.as_str()) == current,
                label: title.to_string(),
                detail: {
                    let count = js_string(session.get("messageCount"));
                    let noun = if count == "1" { "message" } else { "messages" };
                    format!("{} · {count} {noun}", ago(s(session, "updatedAt").unwrap_or("")))
                },
                id,
                ..Item::default()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn newer_models_hide_older_ones_of_their_line() {
        let model = |id: &str, name: &str, family: &str| {
            json!({ "id": id, "name": name, "family": family, "status": "supported", "allowed": true,
                    "pricing": { "input": 3.0, "output": 15.0 } })
        };
        let models = [
            model("anthropic/claude-opus-5.5", "Claude Opus 5.5", "claude-opus"),
            model("anthropic/claude-sonnet-5.5", "Claude Sonnet 5.5", "claude-sonnet"),
            model("anthropic/claude-opus-4.8", "Claude Opus 4.8", "claude-opus"),
            model("openai/gpt-6.1-sol", "GPT-6.1 Sol", "gpt-6"),
            model("openai/gpt-6-luna", "GPT-6 Luna", "gpt-6"),
            model("openai/gpt-6-sol", "GPT-6 Sol", "gpt-6"),
            model(
                "deepseek/deepseek-v4.1-flash-1017",
                "DeepSeek V4.1 Flash 1017",
                "deepseek-v4.1-flash",
            ),
            model(
                "deepseek/deepseek-v4.1-flash-1017-fast",
                "DeepSeek V4.1 Flash 1017 Fast",
                "deepseek-v4.1-flash",
            ),
        ];
        let items = model_items(&models, Some("anthropic/claude-opus-4.8"));
        let older: Vec<&str> = items
            .iter()
            .filter(|item| item.older)
            .map(|item| item.id.as_str())
            .collect();
        assert_eq!(older, ["anthropic/claude-opus-4.8", "openai/gpt-6-sol"]);
        assert!(
            items
                .iter()
                .any(|item| item.current && item.id == "anthropic/claude-opus-4.8")
        );
        assert_eq!(items[1].group.as_deref(), Some("Anthropic"));
        assert_eq!(items[1].detail, "$3 in · $15 out per 1M tokens");
    }
}
