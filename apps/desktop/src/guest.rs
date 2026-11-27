//! Contract tests for the Gateway guest routes used by Nexus UI and mobile. The desktop Workspace
//! no longer opens a guest view before sign-in.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::channel::{mpsc, oneshot};
use futures::{Stream, StreamExt};
use nexus_client::{Client, ClientOptions, Error, Method};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The switch: `on` turns guest chat on. Unset or anything else: off.
pub const FLAG: &str = "NEXUS_DESKTOP_GUEST";
const STATUS_PATH: &str = "/v1/guest/chat";
const MESSAGES_PATH: &str = "/v1/guest/chat/messages";
const GUEST_HEADER: &str = "x-nexus-guest";
const ROUNDS_USED_HEADER: &str = "x-nexus-guest-rounds-used";
const ROUNDS_LIMIT_HEADER: &str = "x-nexus-guest-rounds-limit";

/// Whether guest chat is on in this process.
pub fn enabled() -> bool {
    flag_on(std::env::var(FLAG).ok().as_deref())
}

fn flag_on(value: Option<&str>) -> bool {
    value == Some("on")
}

/// A debug build's fixed Turnstile token, used instead of the challenge page.
pub fn token() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("NEXUS_DESKTOP_GUEST_TURNSTILE_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
}

/// Nexus UI's challenge page at `site`: Turnstile's widget alone, empty unless Cloudflare needs the person.
pub fn challenge_url(site: &str) -> String {
    format!("{}/app/guest/challenge", site.trim_end_matches('/'))
}

/// Where the challenge page hands its outcome over: `#<token>`, or `#error=<Turnstile's error code>`.
pub fn handoff_url(site: &str) -> String {
    format!("{}/token", challenge_url(site))
}

/// A token is good for 300 seconds; the app drops one older than four minutes.
pub const TOKEN_MAX_AGE: Duration = Duration::from_secs(4 * 60);

/// How long a round waits for the challenge page, as the mobile app's.
pub const CHALLENGE_WAIT: Duration = Duration::from_secs(15);

/// What the challenge page handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    Token(String),
    /// Turnstile's error code, or the app's own: `not_found` (the page answered 404), `network`, `timeout`.
    Error(String),
}

/// The outcome in the fragment of the handoff address `url`.
pub fn handoff(url: &str) -> Handoff {
    let fragment = url.split_once('#').map_or("", |(_, fragment)| fragment);
    let fragment = percent_encoding::percent_decode_str(fragment).decode_utf8_lossy();
    match fragment.strip_prefix("error=") {
        Some("") => Handoff::Error("unknown".into()),
        Some(code) => Handoff::Error(code.into()),
        None if fragment.is_empty() => Handoff::Error("empty".into()),
        None => Handoff::Token(fragment.into_owned()),
    }
}

/// A token the page gave ahead of its round, and when.
#[derive(Debug)]
pub struct Kept {
    token: String,
    at: Instant,
}

impl Kept {
    pub fn new(token: String, at: Instant) -> Kept {
        Kept { token, at }
    }

    /// Whether it is still young enough to send at `now`.
    pub fn fresh(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) < TOKEN_MAX_AGE
    }

    /// The token, if it is still fresh. A token is good once.
    pub fn take(self, now: Instant) -> Option<String> {
        self.fresh(now).then_some(self.token)
    }
}

/// Why a round did not run, as Nexus UI names it. `Off`: Gateway has no guest chat set up, or no guest route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Off,
    RoundsUsed,
    AddressLimit,
    Paused,
    Turnstile,
    Invalid,
    Failed,
}

/// The refusal a failed call names: Gateway's error code, or `Off` for a Gateway without the route (404).
pub fn refusal(error: &Error) -> Refusal {
    let Error::Core(core) = error else {
        return Refusal::Failed;
    };
    match core.code.as_deref() {
        Some("guest_chat_not_configured") => Refusal::Off,
        Some("guest_rounds_used") => Refusal::RoundsUsed,
        Some("guest_address_limit") => Refusal::AddressLimit,
        Some("guest_chat_paused") => Refusal::Paused,
        Some("turnstile_failed") => Refusal::Turnstile,
        Some("guest_request_invalid") => Refusal::Invalid,
        _ if core.status == 404 => Refusal::Off,
        _ => Refusal::Failed,
    }
}

/// The sign-in card a refusal ends the chat with: its title and its line. None: the chat goes on after a retry.
pub fn card(reason: Refusal) -> Option<(&'static str, &'static str)> {
    match reason {
        Refusal::RoundsUsed => Some((
            "You used your free rounds",
            "Sign in to keep going with this chat. It is free to create an account.",
        )),
        Refusal::AddressLimit => Some((
            "This network used its free rounds for today",
            "Sign in to keep going. It is free to create an account.",
        )),
        Refusal::Paused => Some((
            "Guest chat is resting for today",
            "Sign in to keep going. It is free to create an account.",
        )),
        Refusal::Off => Some(("Sign in to chat with Nexus", "It is free to create an account.")),
        Refusal::Turnstile | Refusal::Invalid | Refusal::Failed => None,
    }
}

/// A guest id's rounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rounds {
    pub used: u32,
    pub limit: u32,
}

/// `GET /v1/guest/chat`'s answer.
#[derive(Debug, Deserialize)]
struct Status {
    enabled: bool,
    rounds_used: u32,
    rounds_limit: u32,
    model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A turn of the conversation the app holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Serialize)]
struct RoundBody<'a> {
    messages: &'a [Message],
    turnstile: &'a str,
}

/// What the status read says: the rounds and the model, or None when guest chat is off at this Gateway.
pub type StatusRead = Result<Option<(Rounds, String)>, Refusal>;
/// How a round ended: the rounds after it, when Gateway said.
pub type RoundRead = Result<Option<Rounds>, Refusal>;

fn read_status(answer: nexus_client::Result<Value>) -> StatusRead {
    match answer {
        Ok(body) => {
            let status: Status = serde_json::from_value(body).map_err(|_| Refusal::Failed)?;
            // `enabled` is false only for a round limit: this guest id's, or, with rounds left, its address's.
            if !status.enabled && status.rounds_used < status.rounds_limit {
                return Err(Refusal::AddressLimit);
            }
            let rounds = Rounds {
                used: status.rounds_used,
                limit: status.rounds_limit,
            };
            Ok(Some((rounds, status.model)))
        }
        Err(error) => match refusal(&error) {
            Refusal::Off => Ok(None),
            reason => Err(reason),
        },
    }
}

/// Guest calls run on a small runtime of their own: the window's Gateway client is signed in or about to be, and a
/// guest call carries no token.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("nexus-guest")
            .enable_all()
            .build()
            .expect("the guest chat runtime starts")
    })
}

fn client(gateway: &str) -> Client {
    // A round is sent once: it may have counted.
    Client::new(ClientOptions {
        retry: false,
        ..crate::gateway::client_options(gateway, None)
    })
}

/// This computer's guest id: made once and kept, so the count survives a restart.
fn guest_id() -> String {
    kept_id(crate::paths::own_data_dir().map(|dir| dir.join("guest-id")))
}

fn kept_id(file: Option<PathBuf>) -> String {
    if let Some(file) = &file
        && let Ok(kept) = std::fs::read_to_string(file)
        && let Ok(id) = uuid::Uuid::parse_str(kept.trim())
    {
        return id.to_string();
    }
    let made = uuid::Uuid::new_v4().to_string();
    if let Some(file) = file {
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&file, &made));
        if let Err(error) = written {
            eprintln!("nexus-desktop-gpui: the guest id was not kept: {error}");
        }
    }
    made
}

/// Read this computer's rounds at `gateway`.
pub fn status(gateway: &str) -> impl Future<Output = StatusRead> + 'static {
    let client = client(gateway);
    let (done, answer) = oneshot::channel();
    runtime().spawn(async move {
        let header = [(GUEST_HEADER.to_string(), guest_id())];
        let read = read_status(client.call_with(Method::GET, STATUS_PATH, &header, None).await);
        // The window may have closed meanwhile; nobody waits then.
        done.send(read).ok();
    });
    async move { answer.await.unwrap_or(Err(Refusal::Failed)) }
}

/// A round's news: a piece of the answer, then how it ended.
#[derive(Debug, PartialEq)]
pub enum RoundEvent {
    Text(String),
    Done(RoundRead),
}

/// Send one round: the whole conversation, its last message the person's, and a Turnstile token.
pub fn send_round(gateway: &str, messages: Vec<Message>, token: String) -> mpsc::UnboundedReceiver<RoundEvent> {
    let client = client(gateway);
    let (out, events) = mpsc::unbounded();
    runtime().spawn(async move {
        let body = RoundBody {
            messages: &messages,
            turnstile: &token,
        };
        let body = Bytes::from(serde_json::to_vec(&body).unwrap_or_default());
        let extra = [
            (GUEST_HEADER.to_string(), guest_id()),
            ("accept".to_string(), "text/event-stream".to_string()),
        ];
        let read = match client.stream(Method::POST, MESSAGES_PATH, &extra, Some(body)).await {
            Ok((status, headers, stream)) => {
                read_round(status, &headers, stream, |text| {
                    out.unbounded_send(RoundEvent::Text(text)).ok();
                })
                .await
            }
            Err(error) => Err(refusal(&error)),
        };
        out.unbounded_send(RoundEvent::Done(read)).ok();
    });
    events
}

/// Read a round's answer as it streams, as the kit's `streamGuestChatMessage`: a refusal before the stream, an `error`
/// event, or a stream that ends before its last chunk fails the round.
async fn read_round(
    status: u16,
    headers: &[(String, String)],
    stream: impl Stream<Item = nexus_client::Result<Bytes>>,
    mut on_text: impl FnMut(String),
) -> RoundRead {
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };
    let mut stream = std::pin::pin!(stream);
    let events = header("content-type").is_some_and(|kind| kind.contains("text/event-stream"));
    if !(200..300).contains(&status) || !events {
        let mut text = Vec::new();
        while let Some(chunk) = stream.next().await {
            text.extend_from_slice(&chunk.map_err(|error| refusal(&error))?);
        }
        let error = nexus_client::error::failed_answer(status, &String::from_utf8_lossy(&text));
        return Err(refusal(&error.into()));
    }
    let mut decoder = nexus_client::sse::Decoder::new();
    let mut finished = false;
    while let Some(chunk) = stream.next().await {
        for event in decoder.push(&chunk.map_err(|error| refusal(&error))?) {
            let event = event.map_err(|error| refusal(&error))?;
            let choice = &event["choices"][0];
            if let Some(text) = choice["delta"]["content"].as_str().filter(|text| !text.is_empty()) {
                on_text(text.to_string());
            }
            finished |= !choice["finish_reason"].is_null();
        }
    }
    if !finished {
        return Err(Refusal::Failed);
    }
    let rounds = |name| header(name).and_then(|value| value.trim().parse::<u32>().ok());
    Ok(rounds(ROUNDS_USED_HEADER)
        .zip(rounds(ROUNDS_LIMIT_HEADER))
        .map(|(used, limit)| Rounds { used, limit }))
}

/// What the guest screen shows before Gateway answers, when guest chat is off there, or the rounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    Loading,
    Off,
    Rounds { rounds: Rounds, model: String },
}

/// The guest chat: the conversation, the rounds, and why it ended. Nexus UI's `GuestHome`, without the drawing.
#[derive(Debug)]
pub struct Chat {
    pub loaded: Loaded,
    pub thread: Vec<Message>,
    /// The answer arriving: Some while a round runs.
    pub answer: Option<String>,
    ended: Option<Refusal>,
    problem: Option<&'static str>,
    /// This window can get a Turnstile token. Without one, guest chat shows as off.
    token: bool,
}

impl Chat {
    pub fn new(token: bool) -> Chat {
        Chat {
            loaded: Loaded::Loading,
            thread: Vec::new(),
            answer: None,
            ended: None,
            problem: None,
            token,
        }
    }

    /// The challenge page answered 404: Nexus UI's guest mode is off, so no token comes, and guest chat shows as off.
    pub fn no_token(&mut self) {
        self.token = false;
    }

    /// Gateway's status arrived. A refusal with a card ends the chat; any other failure shows it as off.
    pub fn status_read(&mut self, read: StatusRead) {
        match read {
            Ok(Some((rounds, model))) => self.loaded = Loaded::Rounds { rounds, model },
            Ok(None) => self.loaded = Loaded::Off,
            Err(reason) if card(reason).is_some() => self.ended = Some(reason),
            Err(_) => self.loaded = Loaded::Off,
        }
    }

    /// Why the chat is over, if it is: the sign-in card then takes the composer's place.
    pub fn end(&self) -> Option<Refusal> {
        self.ended.or(match &self.loaded {
            Loaded::Loading => None,
            Loaded::Off => Some(Refusal::Off),
            Loaded::Rounds { .. } if !self.token => Some(Refusal::Off),
            Loaded::Rounds { rounds, .. } if rounds.used >= rounds.limit => Some(Refusal::RoundsUsed),
            Loaded::Rounds { .. } => None,
        })
    }

    pub fn busy(&self) -> bool {
        self.answer.is_some()
    }

    /// The person asks `question`: the conversation to send, or None when no round may start now.
    pub fn ask(&mut self, question: &str) -> Option<Vec<Message>> {
        let question = question.trim();
        if question.is_empty() || self.busy() || self.end().is_some() || self.loaded == Loaded::Loading {
            return None;
        }
        self.thread.push(Message {
            role: Role::User,
            content: question.to_string(),
        });
        self.problem = None;
        self.answer = Some(String::new());
        Some(self.thread.clone())
    }

    /// A piece of the answer arrived.
    pub fn text(&mut self, piece: &str) {
        if let Some(answer) = &mut self.answer {
            answer.push_str(piece);
        }
    }

    /// The round ended. A refusal with a card ends the chat; another failure takes the question back, to try again.
    pub fn finish(&mut self, read: RoundRead) {
        let answer = self.answer.take().unwrap_or_default();
        match read {
            Ok(counted) => {
                self.thread.push(Message {
                    role: Role::Assistant,
                    content: answer,
                });
                if let (Some(counted), Loaded::Rounds { rounds, .. }) = (counted, &mut self.loaded) {
                    *rounds = counted;
                }
            }
            Err(reason) if card(reason).is_some() => self.ended = Some(reason),
            Err(reason) => {
                self.thread.pop();
                self.problem = Some(if reason == Refusal::Turnstile {
                    "The check that you are a person did not pass. Try again."
                } else {
                    "The answer did not arrive. Try again."
                });
            }
        }
    }

    /// The line under the composer, while the chat goes on.
    pub fn note(&self) -> Option<String> {
        if self.end().is_some() {
            return None;
        }
        if let Some(problem) = self.problem {
            return Some(problem.to_string());
        }
        let Loaded::Rounds { rounds, model } = &self.loaded else {
            return None;
        };
        let left = rounds.limit.saturating_sub(rounds.used);
        Some(if self.thread.is_empty() {
            format!(
                "You are signed out. You have {} rounds on {model}, and this chat is not saved.",
                rounds.limit
            )
        } else if left <= 2 {
            format!(
                "{left} {} left. Sign in to keep going with this chat.",
                if left == 1 { "round" } else { "rounds" }
            )
        } else {
            format!(
                "{} of {} rounds used. This chat disappears when you close the app. Sign in to keep it.",
                rounds.used, rounds.limit
            )
        })
    }
}

#[cfg(test)]
#[path = "guest_tests.rs"]
mod tests;
