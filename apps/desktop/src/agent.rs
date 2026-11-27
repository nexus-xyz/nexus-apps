//! The Assistant's pane without GPUI: what it holds and how each answer of the agent API
//! changes it. The view is `pages/agent.rs`. The rules, wire types and words are `nexus_desktop_core::agent`'s.
//!
//! The agent API is its own service, not Gateway. The Gateway backend calls it with the person's token
//! ([`crate::gateway::Gateway::agent`]) and hands back its answer as [`Chunk`]s. Without an agent API, or signed out,
//! that stream is a machine-readable 503 or 401, and an API that does not answer is a 502, so the pane always says why.
//!
//! The pane never writes. A write the agent asks for is a `pending_action`, drawn as the approval card. Approve and
//! Decline reach the agent API with the person's own token (`POST /agent/writes/{card}/approve|decline`), and the agent
//! API writes it once Policy has their approval. A write the card cannot approve here opens its page in the browser.
//!
//! It reads like a text thread: under the person's last message, "Delivered", then "Read" and the
//! time once Nexus starts answering, or "Not sent" with Retry when the ask failed first ([`Sent`]). An ask that could
//! not reach the agent service at all also says the app is offline ([`Pane::offline`]), and the view sends it again.
//!
//! The first time the person opens it, the empty chat is the first open, as on the web: the pane asks
//! `GET /agent/intro`, and once it shows tells the API with `POST /agent/intro/seen`. The words and the strict read are
//! `nexus_desktop_core::agent_cards`'s. While the API has it off, or on any failure, the chat opens empty as before.

use std::time::Instant;

use futures::channel::mpsc::UnboundedReceiver;
use nexus_desktop_core::agent::{self as rules, Event, EventReader, Reply};
pub use nexus_desktop_core::agent::{Message, Problem};
use nexus_desktop_core::agent_cards::{self as cards, Intro};
use nexus_desktop_core::agent_composer as composer;
use nexus_desktop_core::agent_proactivity::PROACTIVITY_PATH;
use serde_json::Value;

/// A call the pane makes.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// `POST /agent/ask`, streamed.
    Ask {
        body: String,
    },
    /// `GET /agent/conversations/{id}`; the id is checked when the call is made.
    Read {
        path: String,
    },
    /// `GET /agent/intro`: the first open.
    Intro,
    /// `POST /agent/intro/seen`: the first open showed, so no app shows it again.
    IntroSeen,
    /// `POST /agent/writes/{card}/approve` or `…/decline`, as `agent_cards::write_decision_path` and `_body` make it.
    Decide {
        path: String,
        body: String,
    },
    /// `PATCH /agent/writes/{card}`, as `agent_cards::write_edit_path` and `_body` make it.
    EditWrite {
        path: String,
        body: String,
    },
    /// `GET /agent/memory`, `GET /agent/approvals/settings` or `GET /agent/errands`.
    Memory,
    Approvals,
    Errands,
    /// `GET` one errand, or `POST` one of its actions: its path from [`Call::errand`] or [`Call::act`].
    Errand {
        path: String,
    },
    Act {
        path: String,
        body: String,
    },
    /// `POST /agent/errands` with `{"goal"}`: a new errand.
    OpenErrand {
        body: String,
    },
    /// `PATCH /agent/errands/{id}`: rename, pin, archive or mark done.
    EditErrand {
        path: String,
        body: String,
    },
    /// `POST /agent/errands/{id}/messages` with `{"message"}`: a reply about one errand, in its thread.
    ReplyErrand {
        path: String,
        body: String,
    },
    /// `GET /agent/errands/{id}/handoff`: the errand as a session's prompt.
    ErrandHandoff {
        path: String,
    },
    /// `POST /agent/errands/{id}/handoff` with `{}`: the errand records "Handed off to a session".
    HandedOff {
        path: String,
    },
    /// `DELETE` every note, one note or one rule, or every rule: a path from the constructors below.
    Delete {
        path: String,
    },
    /// `POST /agent/memory/notes` with `{"text"}`.
    AddNote {
        body: String,
    },
    /// `PATCH /agent/memory/notes/{id}` with `{"text"}`.
    EditNote {
        path: String,
        body: String,
    },
    /// `PUT /agent/memory/learning` with `{"paused"}`.
    Learning {
        body: String,
    },
    /// `PUT /agent/approvals/settings` with `{"spendLimitUsd"}`.
    SaveLine {
        body: String,
    },
    /// Privacy and trust: the pause, what Nexus can see, and Revoke all access, each built and checked by
    /// `agent_privacy::Request`'s constructors.
    Privacy(nexus_desktop_core::agent_privacy::Request),
    /// Passwords and payments, each call built and checked by `agent_vault::Request`'s constructors.
    Vault(nexus_desktop_core::agent_vault::Request),
    /// `GET /agent/brief/today`: today's morning brief.
    Brief,
    /// `GET /agent/proactivity`: the dial and quiet hours.
    Proactivity,
    /// `GET /agent/projects/view?…`: one project's page, its path from [`Call::project_view`].
    ProjectView {
        path: String,
    },
    /// `POST /agent/projects/link` with `{"name","projectId"}`: a name-only project binds to its Gateway project.
    LinkProject {
        body: String,
    },
    /// `POST /agent/memory/notes` with `{"text","project"}`: a note in "What Nexus knows here".
    AddProjectNote {
        body: String,
    },
    /// `PUT /agent/proactivity` with a change, from [`Call::save_proactivity`].
    SaveProactivity {
        body: String,
    },
    /// `GET /agent/watches` (the list), or `GET /agent/watches/{id}` (one, with its history).
    Watches {
        path: String,
    },
    /// `POST /agent/watches` with the proposal's body: Keep.
    KeepWatch {
        body: String,
    },
    /// `PATCH /agent/watches/{id}` with a change.
    ChangeWatch {
        path: String,
        body: String,
    },
    /// `DELETE /agent/watches/{id}`.
    DeleteWatch {
        path: String,
    },
    /// `GET /agent/model`: the saved model and the models the policy allows.
    Model,
    /// `PUT /agent/model` with a model, from [`Call::save_model`].
    SaveModel {
        body: String,
    },
    /// `GET /agent/context`: what `@` may pick.
    Context,
}

/// The native views' calls. Each checks its id and body with the core's rules and
/// allowlist, so a call the agent API could not have asked for is never made.
impl Call {
    pub fn errand(id: &str) -> Option<Call> {
        rules::errand_one_path(id).map(|path| Call::Errand { path })
    }

    pub fn act(id: &str, action: nexus_desktop_core::agent_errands::Action) -> Option<Call> {
        let body = action.body();
        let path = rules::errand_one_path(id)?;
        rules::errand_action(Some(&body)).then_some(Call::Act { path, body })
    }

    pub fn open_errand(goal: &str, project: Option<&str>) -> Option<Call> {
        let body = nexus_desktop_core::agent_errands::goal_body(goal, project);
        rules::errand_goal(Some(&body)).then_some(Call::OpenErrand { body })
    }

    /// A new errand in a Gateway project, by its id.
    pub fn open_errand_in(goal: &str, project_id: &str) -> Option<Call> {
        let body = nexus_desktop_core::agent_projects::goal_body_in(goal, project_id);
        rules::errand_goal(Some(&body)).then_some(Call::OpenErrand { body })
    }

    /// One project's page: by its Gateway id when it has one, else by its name.
    pub fn project_view(id: Option<&str>, name: &str) -> Option<Call> {
        rules::project_view_path(id, name).map(|path| Call::ProjectView { path })
    }

    pub fn link_project(name: &str, project_id: &str) -> Option<Call> {
        let body = nexus_desktop_core::agent_projects::link_body(name, project_id);
        rules::project_link_body(Some(&body)).then_some(Call::LinkProject { body })
    }

    pub fn add_project_note(text: &str, view: &nexus_desktop_core::agent_projects::ProjectView) -> Option<Call> {
        let body = nexus_desktop_core::agent_projects::project_note_body(text, view);
        rules::project_note_body(Some(&body)).then_some(Call::AddProjectNote { body })
    }

    pub fn edit_errand(id: &str, edit: &nexus_desktop_core::agent_errands::Edit) -> Option<Call> {
        let body = edit.body();
        let path = rules::errand_one_path(id)?;
        rules::errand_edit(Some(&body)).then_some(Call::EditErrand { path, body })
    }

    pub fn reply_errand(id: &str, message: &str) -> Option<Call> {
        let body = nexus_desktop_core::agent_errands::message_body(message.trim());
        let path = rules::errand_messages_path(id)?;
        rules::errand_message(Some(&body)).then_some(Call::ReplyErrand { path, body })
    }

    /// Read the errand's hand-off prompt.
    pub fn errand_handoff(id: &str) -> Option<Call> {
        rules::errand_handoff_path(id).map(|path| Call::ErrandHandoff { path })
    }

    /// Record that the errand was handed off to a session. The desktop cannot know the session's link.
    pub fn handed_off(id: &str) -> Option<Call> {
        rules::errand_handoff_path(id).map(|path| Call::HandedOff { path })
    }

    /// Forget one note, or every note with None.
    pub fn forget(note: Option<&str>) -> Option<Call> {
        let path = match note {
            Some(id) => rules::note_path(id)?,
            None => rules::MEMORY_PATH.to_string(),
        };
        Some(Call::Delete { path })
    }

    /// Reset one rule, or every rule with None.
    pub fn revoke(rule: Option<&str>) -> Option<Call> {
        let path = match rule {
            Some(id) => rules::rule_path(id)?,
            None => rules::APPROVALS_RULES_PATH.to_string(),
        };
        Some(Call::Delete { path })
    }

    pub fn add_note(text: &str) -> Option<Call> {
        let body = nexus_desktop_core::agent_memory::note_body(text);
        rules::note_body(Some(&body)).then_some(Call::AddNote { body })
    }

    pub fn edit_note(id: &str, text: &str) -> Option<Call> {
        let body = nexus_desktop_core::agent_memory::note_body(text);
        let path = rules::note_path(id)?;
        rules::note_body(Some(&body)).then_some(Call::EditNote { path, body })
    }

    pub fn learning(paused: bool) -> Call {
        Call::Learning {
            body: serde_json::json!({ "paused": paused }).to_string(),
        }
    }

    /// Set the spend line, or go back to the default with None.
    pub fn save_line(usd: Option<&str>) -> Option<Call> {
        let body = nexus_desktop_core::agent_approvals::line_body(usd);
        rules::line_body(Some(&body)).then_some(Call::SaveLine { body })
    }

    /// The watches' list.
    pub fn watches() -> Call {
        Call::Watches {
            path: nexus_desktop_core::agent_watches::WATCHES_PATH.into(),
        }
    }

    /// One watch with its history.
    pub fn watch(id: &str) -> Option<Call> {
        nexus_desktop_core::agent_watches::watch_path(id).map(|path| Call::Watches { path })
    }

    /// Keep what the chat proposed.
    pub fn keep_watch(proposal: &nexus_desktop_core::agent_watches::Proposal) -> Option<Call> {
        proposal.keep_body().map(|body| Call::KeepWatch { body })
    }

    /// Change one watch.
    pub fn change_watch(id: &str, change: &nexus_desktop_core::agent_watches::Change) -> Option<Call> {
        let path = nexus_desktop_core::agent_watches::watch_path(id)?;
        change.body().map(|body| Call::ChangeWatch { path, body })
    }

    /// Delete one watch.
    pub fn delete_watch(id: &str) -> Option<Call> {
        nexus_desktop_core::agent_watches::watch_path(id).map(|path| Call::DeleteWatch { path })
    }

    /// Change the dial, the kinds turned off or the quiet hours.
    pub fn save_proactivity(change: &nexus_desktop_core::agent_proactivity::Change) -> Option<Call> {
        let body = change.body();
        nexus_desktop_core::agent_proactivity::change_body(Some(&body)).then_some(Call::SaveProactivity { body })
    }
}

impl Call {
    /// Save the person's model (`auto`, `fast`, `best` or a model id); None for an id the API would refuse.
    pub fn save_model(model: &str) -> Option<Call> {
        composer::model_body(model).map(|body| Call::SaveModel { body })
    }
}

impl Call {
    /// Its method, path and body.
    pub fn request(&self) -> (nexus_client::Method, String, Option<String>) {
        use nexus_client::Method;
        match self {
            Call::Memory => (Method::GET, rules::MEMORY_PATH.into(), None),
            Call::Approvals => (Method::GET, rules::APPROVALS_SETTINGS_PATH.into(), None),
            Call::Errands => (Method::GET, rules::ERRANDS_PATH.into(), None),
            Call::Errand { path } => (Method::GET, path.clone(), None),
            Call::Act { path, body } | Call::EditNote { path, body } => {
                let method = if matches!(self, Call::Act { .. }) {
                    Method::POST
                } else {
                    Method::PATCH
                };
                (method, path.clone(), Some(body.clone()))
            }
            Call::OpenErrand { body } => (Method::POST, rules::ERRANDS_PATH.into(), Some(body.clone())),
            Call::EditErrand { path, body } => (Method::PATCH, path.clone(), Some(body.clone())),
            Call::ReplyErrand { path, body } => (Method::POST, path.clone(), Some(body.clone())),
            Call::ErrandHandoff { path } => (Method::GET, path.clone(), None),
            Call::HandedOff { path } => (Method::POST, path.clone(), Some("{}".into())),
            Call::Delete { path } => (Method::DELETE, path.clone(), None),
            Call::AddNote { body } | Call::AddProjectNote { body } => {
                (Method::POST, rules::MEMORY_ADD_PATH.into(), Some(body.clone()))
            }
            Call::ProjectView { path } => (Method::GET, path.clone(), None),
            Call::LinkProject { body } => (Method::POST, rules::PROJECT_LINK_PATH.into(), Some(body.clone())),
            Call::Learning { body } => (Method::PUT, rules::MEMORY_LEARNING_PATH.into(), Some(body.clone())),
            Call::SaveLine { body } => (Method::PUT, rules::APPROVALS_SETTINGS_PATH.into(), Some(body.clone())),
            Call::Privacy(request) => {
                let (method, path, body) = request.request();
                (method, path.into(), body)
            }
            Call::Vault(request) => request.request(),
            Call::Brief => (Method::GET, rules::BRIEF_PATH.into(), None),
            Call::Proactivity => (Method::GET, PROACTIVITY_PATH.into(), None),
            Call::SaveProactivity { body } => (Method::PUT, PROACTIVITY_PATH.into(), Some(body.clone())),
            Call::Watches { path } => (Method::GET, path.clone(), None),
            Call::KeepWatch { body } => (
                Method::POST,
                nexus_desktop_core::agent_watches::WATCHES_PATH.into(),
                Some(body.clone()),
            ),
            Call::ChangeWatch { path, body } => (Method::PATCH, path.clone(), Some(body.clone())),
            Call::DeleteWatch { path } => (Method::DELETE, path.clone(), None),
            Call::Model => (Method::GET, composer::MODEL_PATH.into(), None),
            Call::SaveModel { body } => (Method::PUT, composer::MODEL_PATH.into(), Some(body.clone())),
            Call::Context => (Method::GET, composer::CONTEXT_PATH.into(), None),
            Call::Ask { body } => (nexus_client::Method::POST, rules::ASK_PATH.into(), Some(body.clone())),
            Call::Read { path } => (nexus_client::Method::GET, path.clone(), None),
            Call::Intro => (nexus_client::Method::GET, rules::INTRO_PATH.into(), None),
            Call::IntroSeen => (nexus_client::Method::POST, rules::INTRO_SEEN_PATH.into(), None),
            Call::Decide { path, body } => (nexus_client::Method::POST, path.clone(), Some(body.clone())),
            Call::EditWrite { path, body } => (Method::PATCH, path.clone(), Some(body.clone())),
        }
    }
}

/// The agent API's answer as it arrives.
#[derive(Debug, Clone, PartialEq)]
pub enum Chunk {
    Head {
        status: u16,
        content_type: String,
    },
    Data(Vec<u8>),
    /// The body broke off.
    Broken,
    End,
}

/// A call in flight: its chunks, and what stops it when dropped.
pub struct Stream {
    pub chunks: UnboundedReceiver<Chunk>,
    pub _stop: Option<Box<dyn Send>>,
}

/// One entry of the conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    User(String),
    Agent(Vec<Message>),
    /// A voice call that ended: its card, in the thread after the turns it asked.
    Voice(Box<crate::voice::VoiceSummary>),
}

/// The answer as it is written.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Draft {
    pub text: String,
    pub model: Option<String>,
    /// The tool the agent runs now, as the stream's latest `step` line ("Searching Linear").
    pub step: Option<String>,
}

impl Draft {
    /// What the thread shows before the answer's first word (Nexus UI's thinking row): the step line, or "Thinking"
    /// until the answer has a model. None once the typing dots take over: a model and no step.
    pub fn thinking_line(&self) -> Option<&str> {
        if !self.text.is_empty() {
            return None;
        }
        match (&self.step, &self.model) {
            (Some(step), _) => Some(step),
            (None, None) => Some("Thinking"),
            (None, Some(_)) => None,
        }
    }
}

/// Where the person's last message stands, as a text thread says it under the message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sent {
    /// Asked; Nexus has not started answering.
    Delivered,
    /// Nexus started answering at this local time.
    Read(chrono::NaiveTime),
    /// The ask failed before Nexus started answering. Retry asks it again.
    NotSent,
}

impl Sent {
    /// The line under the message: "Delivered", "Read 08:02", "Not sent · Retry".
    pub fn line(&self) -> String {
        match self {
            Sent::Delivered => "Delivered".into(),
            Sent::Read(at) => format!("Read {}", at.format("%H:%M")),
            Sent::NotSent => "Not sent · Retry".into(),
        }
    }
}

/// What a finished answer asks the view to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// A notification of this kind (`answer`, `approval`), shown only when no app window has the focus.
    Notify {
        kind: &'static str,
        conversation: Option<String>,
    },
}

/// How a call's answer is being read.
#[derive(Debug, Default)]
struct Reading {
    status: Option<u16>,
    events: Option<EventReader>,
    body: Vec<u8>,
    finished: bool,
}

/// The pane's state.
#[derive(Debug)]
pub struct Pane {
    pub entries: Vec<Entry>,
    /// When each entry of `entries` was sent or landed, on this computer's clock: the thread's time separators read it
    /// (`pages/agent/thread_layout.rs`). None for a turn read back, which the agent API sends without its time.
    pub times: Vec<Option<chrono::DateTime<chrono::Local>>>,
    /// The answer being written; Some while busy.
    pub draft: Option<Draft>,
    pub problem: Option<Problem>,
    pub conversation: Option<String>,
    /// What a screen reader hears: answering, answered, stopped, a new conversation.
    pub status: &'static str,
    /// A conversation is being read back.
    pub loading: bool,
    /// The person's last message: delivered, read or not sent. None for a conversation read back.
    pub sent: Option<Sent>,
    /// The last ask could not reach the agent service: the thread says the app is offline.
    pub offline: bool,
    /// The first open, once the agent API sent one; it goes when the person asks anything.
    pub intro: Option<Intro>,
    /// The surface the asks come from, when it is not the thread's (`desktop` for the Mac chat bar, `chat_bar.rs`).
    pub surface: Option<&'static str>,
    ask: Option<(Reading, Instant)>,
    read: Option<Reading>,
    intro_reading: Option<Reading>,
    /// The last ask's line as the thread shows it and what it sent, so a retry sends the same picks and skill.
    last_ask: Option<(String, composer::Wire)>,
}

impl Pane {
    /// A new pane; with a conversation (a notification's click), the call that reads it back.
    pub fn new(conversation: Option<&str>) -> (Pane, Option<Call>) {
        let read = conversation.and_then(rules::conversation_path);
        let pane = Pane {
            entries: Vec::new(),
            times: Vec::new(),
            draft: None,
            problem: None,
            conversation: read.as_ref().and(conversation.map(String::from)),
            status: "",
            loading: read.is_some(),
            sent: None,
            offline: false,
            intro: None,
            surface: None,
            ask: None,
            read: read.as_ref().map(|_| Reading::default()),
            intro_reading: None,
            last_ask: None,
        };
        (pane, read.map(|path| Call::Read { path }))
    }

    /// Add an entry that happens now.
    fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        self.times.push(Some(chrono::Local::now()));
    }

    pub fn busy(&self) -> bool {
        self.draft.is_some()
    }

    /// The empty state shows only with nothing else on screen.
    pub fn empty(&self) -> bool {
        self.entries.is_empty() && !self.busy() && self.problem.is_none() && !self.loading
    }

    /// The call that reads the first open, once: only for a new pane that opens no conversation.
    pub fn intro_call(&mut self) -> Option<Call> {
        if self.loading || self.conversation.is_some() || !self.entries.is_empty() || self.intro_reading.is_some() {
            return None;
        }
        self.intro_reading = Some(Reading::default());
        Some(Call::Intro)
    }

    /// The first open to draw: while the chat is empty.
    pub fn first_open(&self) -> Option<&Intro> {
        self.intro.as_ref().filter(|_| self.empty())
    }

    /// The next piece of the first open's answer. Once it shows, the call that keeps it seen.
    pub fn intro_chunk(&mut self, chunk: Chunk) -> Option<Call> {
        let reading = self.intro_reading.as_mut()?;
        match chunk {
            Chunk::Head { status, .. } => reading.status = Some(status),
            Chunk::Data(bytes) => reading.body.extend_from_slice(&bytes),
            Chunk::Broken => reading.finished = true,
            Chunk::End if !reading.finished => {
                reading.finished = true;
                let body: Option<Value> = serde_json::from_slice(&reading.body).ok();
                let intro = cards::intro_to_show(reading.status.unwrap_or(502), body.as_ref());
                // Asked, or opened a conversation, while it loaded: it no longer applies.
                if intro.is_some() && self.empty() && self.conversation.is_none() {
                    self.intro = intro;
                    return Some(Call::IntroSeen);
                }
            }
            Chunk::End => {}
        }
        None
    }

    /// Ask `text`: None while busy or when it is blank. Longer than the API takes, it is cut, as the textarea's limit.
    pub fn ask(&mut self, text: &str, now: Instant) -> Option<Call> {
        let message: String = text.trim().chars().take(rules::MAX_CHARS).collect();
        let wire = composer::Wire {
            message: message.clone(),
            context: Vec::new(),
            skill: None,
        };
        self.ask_with(&message, wire, now)
    }

    /// Ask `wire`, which the thread shows as `display`: the person's words with the `@` picks and the `/` skill they
    /// chose. None while busy or when there is nothing to send.
    pub fn ask_with(&mut self, display: &str, wire: composer::Wire, now: Instant) -> Option<Call> {
        let message: String = wire.message.trim().chars().take(rules::MAX_CHARS).collect();
        let shown: String = display.trim().chars().take(rules::MAX_CHARS).collect();
        if message.is_empty() || self.busy() {
            return None;
        }
        let wire = composer::Wire {
            message: message.clone(),
            ..wire
        };
        self.problem = None;
        // Never a wizard: the first open goes once the person asks anything.
        self.intro = None;
        self.push(Entry::User(shown.clone()));
        self.sent = Some(Sent::Delivered);
        self.offline = false;
        self.draft = Some(Draft::default());
        self.status = "Nexus is answering.";
        self.ask = Some((Reading::default(), now));
        let body = composer::ask_body_on(
            &message,
            self.conversation.as_deref(),
            &wire.context,
            wire.skill,
            self.surface,
        );
        self.last_ask = Some((shown, wire));
        Some(Call::Ask { body })
    }

    /// A voice call ended: its card, if anything was said, then the writes it asked for, each its approval card.
    /// The call asked in this conversation; the thread does not show its turns as bubbles, as on the web.
    pub fn land_voice(&mut self, summary: Option<crate::voice::VoiceSummary>, writes: Vec<Message>) -> bool {
        let before = self.entries.len();
        if let Some(summary) = summary {
            self.intro = None;
            self.push(Entry::Voice(Box::new(summary)));
        }
        if !writes.is_empty() {
            self.push(Entry::Agent(writes));
        }
        self.entries.len() != before
    }

    /// Ask the last message again after it was not sent: it moves to the end of the thread as a new ask.
    pub fn retry(&mut self, now: Instant) -> Option<Call> {
        if self.sent != Some(Sent::NotSent) || self.busy() {
            return None;
        }
        let Some(Entry::User(text)) = self.entries.last().cloned() else {
            return None;
        };
        self.entries.pop();
        self.times.pop();
        match self.last_ask.clone() {
            Some((shown, wire)) if shown == text => self.ask_with(&shown, wire, now),
            _ => self.ask(&text, now),
        }
    }

    /// The latest reply carries a write that waits for the person's approval, and they have not written since: the
    /// sidebar's row shows Needs you.
    pub fn needs_you(&self) -> bool {
        matches!(self.entries.last(), Some(Entry::Agent(messages))
            if messages.iter().any(|message| matches!(message, Message::PendingAction { .. })))
    }

    /// Stop the answer (Esc, or the stop button). The view drops its stream.
    pub fn stop(&mut self) -> bool {
        if self.ask.take().is_none() {
            return false;
        }
        if self.sent == Some(Sent::Delivered) {
            self.sent = None;
        }
        self.draft = None;
        self.status = "Stopped.";
        true
    }

    /// New conversation: not while an answer is written.
    pub fn start_over(&mut self) -> bool {
        if self.busy() {
            return false;
        }
        self.entries.clear();
        self.times.clear();
        self.conversation = None;
        self.problem = None;
        self.sent = None;
        self.offline = false;
        self.status = "New conversation.";
        true
    }

    /// The next piece of the ask's answer.
    pub fn ask_chunk(&mut self, chunk: Chunk, now: Instant) -> Vec<Effect> {
        let Some((reading, started)) = self.ask.as_mut() else {
            return Vec::new();
        };
        let started = *started;
        let mut events = Vec::new();
        let mut ended = false;
        match chunk {
            Chunk::Head { status, content_type } => {
                reading.status = Some(status);
                if (200..300).contains(&status) && content_type.starts_with("text/event-stream") {
                    reading.events = Some(EventReader::default());
                }
            }
            Chunk::Data(bytes) => match reading.events.as_mut() {
                Some(reader) => events = reader.push(&bytes),
                None => reading.body.extend_from_slice(&bytes),
            },
            Chunk::Broken => {
                // As the Nexus app's catch: the answer did not finish, and the pane says the agent did not answer.
                // Broken before Nexus started: not sent. It is not offline, which sends again by itself: the agent
                // API may have the question already, so only the person's Retry asks it again.
                if self.sent == Some(Sent::Delivered) {
                    self.sent = Some(Sent::NotSent);
                }
                self.ask = None;
                self.draft = None;
                self.problem = Some(rules::problem(502, None, None));
                self.status = "";
                return Vec::new();
            }
            Chunk::End => {
                if let Some(reader) = reading.events.as_mut() {
                    events = reader.finish();
                }
                ended = true;
            }
        }
        let mut effects = Vec::new();
        for event in events {
            effects.extend(self.on_event(event, now.saturating_duration_since(started).as_millis()));
        }
        if ended && let Some((reading, _)) = self.ask.take() {
            if reading.events.is_none() && !reading.finished {
                // Not a stream: the failure's JSON, or an answer in one piece the pane does not read.
                let status = reading.status.unwrap_or(502);
                let status = if (200..300).contains(&status) { 502 } else { status };
                let body: Option<Value> = serde_json::from_slice(&reading.body).ok();
                self.problem = Some(rules::problem_of(status, body.as_ref()));
                // Refused or unreachable before Nexus started: not sent. Unreachable is the app's own answer when the
                // agent service does not answer (`gateway.rs`), so the thread says it is offline.
                if self.sent == Some(Sent::Delivered) {
                    self.sent = Some(Sent::NotSent);
                    self.offline = body.as_ref().and_then(|body| body.get("error")).and_then(Value::as_str)
                        == Some(rules::UNREACHABLE.1);
                }
            } else if !reading.finished {
                self.problem = Some(rules::cut_problem());
            }
            // As the Nexus app's `finished`: a reply, an error event or a failure's answer all count.
            let finished = reading.finished || reading.events.is_none();
            self.draft = None;
            self.status = if finished { "Nexus answered." } else { "" };
        }
        effects
    }

    fn on_event(&mut self, event: Event, elapsed_ms: u128) -> Vec<Effect> {
        // Nexus's first word of any kind: the person's message was read.
        if self.sent == Some(Sent::Delivered) {
            self.sent = Some(Sent::Read(chrono::Local::now().time()));
        }
        match event {
            Event::Started { conversation } => self.conversation = Some(conversation),
            Event::Model { model } => {
                if let Some(draft) = self.draft.as_mut() {
                    draft.model = Some(model);
                }
            }
            Event::Text { delta } => {
                if let Some(draft) = self.draft.as_mut() {
                    draft.text.push_str(&delta);
                }
            }
            Event::Step { line } => {
                if let Some(draft) = self.draft.as_mut() {
                    draft.step = Some(line);
                }
            }
            Event::Done { reply } => {
                self.mark_finished();
                return self.finish(reply, elapsed_ms);
            }
            Event::Error { status, error, detail } => {
                self.mark_finished();
                self.problem = Some(rules::problem(status, Some(&error), Some(&detail)));
            }
        }
        Vec::new()
    }

    fn mark_finished(&mut self) {
        if let Some((reading, _)) = self.ask.as_mut() {
            reading.finished = true;
        }
    }

    /// A finished reply: keep its conversation, show its messages, and say what to announce. A write that waits is the
    /// card in the thread; the notification brings the person back to it.
    fn finish(&mut self, reply: Reply, elapsed_ms: u128) -> Vec<Effect> {
        if !reply.conversation.is_empty() {
            self.conversation = Some(reply.conversation.clone());
        }
        let mut effects = Vec::new();
        if let Some(kind) = rules::notice_for(&reply, elapsed_ms) {
            effects.push(Effect::Notify {
                kind,
                conversation: (!reply.conversation.is_empty()).then(|| reply.conversation.clone()),
            });
        }
        if !reply.messages.is_empty() {
            self.push(Entry::Agent(reply.messages));
        }
        effects
    }

    /// The next piece of the read-back's answer.
    pub fn read_chunk(&mut self, chunk: Chunk) {
        let Some(reading) = self.read.as_mut() else {
            return;
        };
        match chunk {
            Chunk::Head { status, .. } => reading.status = Some(status),
            Chunk::Data(bytes) => reading.body.extend_from_slice(&bytes),
            // Aborted, or the core: the pane still takes a new question.
            Chunk::Broken => {
                self.read = None;
                self.loading = false;
            }
            Chunk::End => {
                let reading = self.read.take().unwrap_or_default();
                self.loading = false;
                let status = reading.status.unwrap_or(502);
                let body: Option<Value> = serde_json::from_slice(&reading.body).ok();
                if status == 404 {
                    self.conversation = None;
                    self.status = "That conversation has ended. Start a new one.";
                } else if !(200..300).contains(&status) {
                    self.problem = Some(rules::problem_of(status, body.as_ref()));
                } else {
                    let turns = body.as_ref().map(rules::turns).unwrap_or_default();
                    let read: Vec<Entry> = turns
                        .iter()
                        .map(|turn| match turn.role.as_str() {
                            "user" => Entry::User(turn.text.clone()),
                            _ => Entry::Agent(vec![turn.answer()]),
                        })
                        .collect();
                    // Anything asked while it loaded stays after it.
                    let times: Vec<_> = turns.iter().map(|turn| turn_time(turn.at.as_deref())).collect();
                    self.times.splice(0..0, times);
                    self.entries.splice(0..0, read);
                }
            }
        }
    }
}

/// A turn's time read back (RFC 3339), on this computer's clock. None when the API sent none or one it cannot read.
fn turn_time(at: Option<&str>) -> Option<chrono::DateTime<chrono::Local>> {
    let at = chrono::DateTime::parse_from_rfc3339(at?).ok()?;
    Some(at.with_timezone(&chrono::Local))
}

/// The placeholder, as the design writes it. The same before and during a conversation.
pub fn placeholder(_pane: &Pane) -> &'static str {
    composer::PLACEHOLDER
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sse(event: &str, data: &str) -> Chunk {
        Chunk::Data(format!("event: {event}\ndata: {data}\n\n").into_bytes())
    }

    fn head(status: u16, content_type: &str) -> Chunk {
        Chunk::Head {
            status,
            content_type: content_type.into(),
        }
    }

    const ANSWER: &str = r#"{"kind":"answer","text":"Done.","model":"nexus/auto","costUsd":0.01,"sources":[{"title":"Runbook","url":"https://wiki.test/runbook"}]}"#;
    const PENDING: &str = r##"{"kind":"pending_action","card":"c1","approveUrl":"https://nexus.test/app/approvals/a1","detail":"Approve it in Nexus.","write":{"connector":"slack","operation":"post_message","summary":"Post in #general","effect":"irreversible","arguments":{}}}"##;

    /// Feed the recorded stream of an answer, as the agent API sends it.
    fn answered(pane: &mut Pane, messages: &str, started: Instant, took: Duration) -> Vec<Effect> {
        let now = started + took;
        let mut effects = Vec::new();
        for chunk in [
            head(200, "text/event-stream; charset=utf-8"),
            sse("started", r#"{"conversation":"c1"}"#),
            sse("model", r#"{"model":"anthropic/claude-sonnet"}"#),
            sse("text", r#"{"delta":"Do"}"#),
            sse("text", r#"{"delta":"ne."}"#),
            sse(
                "done",
                &format!(r#"{{"status":200,"conversation":"c1","messages":[{messages}]}}"#),
            ),
            Chunk::End,
        ] {
            effects.extend(pane.ask_chunk(chunk, now));
        }
        effects
    }

    /// The pane shows only with an agent API: no setting and no `NEXUS_AGENTS_API_URL` hides it, and a URL that would
    /// send the token in the clear does too. It is a place of its own, at `agent` (`NEXUS_OPEN_PAGE=agent`).
    #[test]
    fn the_pane_shows_only_with_an_agent_api() {
        use nexus_desktop_core::agent::agents_url;
        assert_eq!(agents_url(None, None), None);
        assert_eq!(agents_url(None, Some("http://agents.example")), None);
        assert!(agents_url(None, Some("https://agents.example")).is_some());
        let id = crate::pages::PageId::from_web_path("agent");
        assert_eq!(id, Some(crate::pages::PageId::Agent));
        assert!(crate::pages::PageId::Agent.is_place());
    }

    #[test]
    fn a_new_pane_is_empty_and_reads_back_a_notified_conversation() {
        let (pane, call) = Pane::new(None);
        assert!(pane.empty());
        assert_eq!(call, None);
        assert_eq!(placeholder(&pane), "Hand off anything, / for skills, @ for context");
        let (pane, call) = Pane::new(Some("c1"));
        assert!(pane.loading && !pane.empty());
        assert_eq!(
            call,
            Some(Call::Read {
                path: "/agent/conversations/c1".into()
            })
        );
        // An id the agent API could not have written is not read.
        let (pane, call) = Pane::new(Some("../v1/keys"));
        assert_eq!((pane.loading, call, pane.conversation), (false, None, None));
    }

    #[test]
    fn an_ask_streams_its_answer_and_keeps_the_conversation() {
        let (mut pane, _) = Pane::new(None);
        let started = Instant::now();
        let Some(Call::Ask { body }) = pane.ask("  hi  ", started) else {
            panic!("asks")
        };
        assert_eq!(body, r#"{"message":"hi","stream":true}"#);
        assert!(pane.busy());
        assert_eq!(pane.ask("again", started), None, "one at a time");
        pane.ask_chunk(head(200, "text/event-stream"), started);
        pane.ask_chunk(sse("model", r#"{"model":"m/x"}"#), started);
        pane.ask_chunk(sse("text", r#"{"delta":"Do"}"#), started);
        assert_eq!(
            pane.draft,
            Some(Draft {
                text: "Do".into(),
                model: Some("m/x".into()),
                step: None,
            })
        );
        let effects = answered(&mut pane, ANSWER, started, Duration::from_secs(1));
        assert!(effects.is_empty(), "a short answer is not announced: {effects:?}");
        assert!(!pane.busy());
        assert_eq!(pane.status, "Nexus answered.");
        assert_eq!(pane.conversation.as_deref(), Some("c1"));
        assert!(matches!(&pane.entries[..], [Entry::User(text), Entry::Agent(messages)]
            if text == "hi" && matches!(&messages[0], Message::Answer { text, .. } if text == "Done.")));
        assert_eq!(placeholder(&pane), "Hand off anything, / for skills, @ for context");
        // The follow-up asks in the same conversation.
        let Some(Call::Ask { body }) = pane.ask("and?", started) else {
            panic!("asks")
        };
        assert_eq!(body, r#"{"message":"and?","stream":true,"conversation":"c1"}"#);
    }

    #[test]
    fn a_long_answer_is_announced_and_a_write_is_announced() {
        let (mut pane, _) = Pane::new(None);
        let started = Instant::now();
        pane.ask("hi", started);
        let effects = answered(&mut pane, ANSWER, started, Duration::from_secs(11));
        assert_eq!(
            effects,
            vec![Effect::Notify {
                kind: "answer",
                conversation: Some("c1".into())
            }]
        );
        pane.ask("post it", started);
        let effects = answered(&mut pane, PENDING, started, Duration::ZERO);
        // No native confirm opens by itself: the write is the card in the thread.
        assert_eq!(
            effects,
            vec![Effect::Notify {
                kind: "approval",
                conversation: Some("c1".into())
            }]
        );
    }

    #[test]
    fn the_shells_own_failures_and_the_apis_show_as_the_nexus_app_words_them() {
        // The titles are the core's words (`nexus_desktop_core::agent::problem`), so the core may reword them.
        for (status, error) in [
            (503, "agents_api_unconfigured"),
            (502, "agents_api_unreachable"),
            (401, "unauthenticated"),
            (429, "rate_limited"),
        ] {
            let title = rules::problem(status, Some(error), None).title;
            let (mut pane, _) = Pane::new(None);
            let now = Instant::now();
            pane.ask("hi", now);
            pane.ask_chunk(head(status, "application/json"), now);
            pane.ask_chunk(
                Chunk::Data(nexus_desktop_core::agent::failure_body(error, "d").into_bytes()),
                now,
            );
            pane.ask_chunk(Chunk::End, now);
            assert_eq!(
                pane.problem.as_ref().map(|p| p.title.as_str()),
                Some(title.as_str()),
                "{status}"
            );
            assert!(!pane.busy() && !pane.empty());
        }
    }

    #[test]
    fn an_error_event_a_cut_stream_and_a_broken_body_each_say_why() {
        let now = Instant::now();
        let (mut pane, _) = Pane::new(None);
        pane.ask("hi", now);
        pane.ask_chunk(head(200, "text/event-stream"), now);
        pane.ask_chunk(sse("error", r#"{"status":504,"error":"timeout"}"#), now);
        pane.ask_chunk(Chunk::End, now);
        assert_eq!(pane.problem.unwrap().title, "The answer took too long");

        let (mut pane, _) = Pane::new(None);
        pane.ask("hi", now);
        pane.ask_chunk(head(200, "text/event-stream"), now);
        pane.ask_chunk(sse("text", r#"{"delta":"Do"}"#), now);
        pane.ask_chunk(Chunk::End, now);
        assert_eq!(pane.problem.unwrap().title, "The answer stopped before it finished");

        let (mut pane, _) = Pane::new(None);
        pane.ask("hi", now);
        pane.ask_chunk(head(200, "text/event-stream"), now);
        pane.ask_chunk(Chunk::Broken, now);
        assert_eq!(
            pane.problem.as_ref().unwrap().title,
            rules::problem(502, None, None).title
        );
        assert!(!pane.busy());
    }

    #[test]
    fn stop_and_new_conversation() {
        let now = Instant::now();
        let (mut pane, _) = Pane::new(None);
        assert!(!pane.stop(), "nothing to stop");
        pane.ask("hi", now);
        assert!(!pane.start_over(), "not while it answers");
        assert!(pane.stop());
        assert_eq!((pane.busy(), pane.status), (false, "Stopped."));
        // What the stopped stream still sends is dropped.
        assert!(pane.ask_chunk(sse("text", r#"{"delta":"late"}"#), now).is_empty());
        assert_eq!(pane.entries, vec![Entry::User("hi".into())]);
        pane.conversation = Some("c1".into());
        assert!(pane.start_over());
        assert!(pane.empty());
        assert_eq!((pane.conversation.as_deref(), pane.status), (None, "New conversation."));
    }

    /// A text thread's line under the person's last message: Delivered, then Read and the time once
    /// Nexus starts answering; Not sent when the ask fails first, with the offline line when nothing answered.
    #[test]
    fn the_last_message_reads_delivered_then_read_or_not_sent() {
        let now = Instant::now();
        let (mut pane, _) = Pane::new(None);
        pane.ask("hi", now);
        assert_eq!(pane.sent, Some(Sent::Delivered));
        assert_eq!(pane.sent.unwrap().line(), "Delivered");
        // The head alone is not Nexus answering.
        pane.ask_chunk(head(200, "text/event-stream"), now);
        assert_eq!(pane.sent, Some(Sent::Delivered));
        pane.ask_chunk(sse("started", r#"{"conversation":"c1"}"#), now);
        let Some(Sent::Read(at)) = pane.sent else {
            panic!("read once Nexus starts: {:?}", pane.sent)
        };
        assert_eq!(Sent::Read(at).line(), format!("Read {}", at.format("%H:%M")));
        // A failure after Nexus started leaves it read.
        pane.ask_chunk(Chunk::Broken, now);
        assert!(matches!(pane.sent, Some(Sent::Read(_))));
        assert!(!pane.offline);

        // Refused before Nexus started: not sent, and not offline.
        let (mut pane, _) = Pane::new(None);
        pane.ask("hi", now);
        pane.ask_chunk(head(429, "application/json"), now);
        pane.ask_chunk(
            Chunk::Data(nexus_desktop_core::agent::failure_body("rate_limited", "d").into_bytes()),
            now,
        );
        pane.ask_chunk(Chunk::End, now);
        assert_eq!(pane.sent, Some(Sent::NotSent));
        assert_eq!(pane.sent.unwrap().line(), "Not sent · Retry");
        assert!(!pane.offline);

        // Nothing answered: not sent, and the thread says the app is offline.
        let (mut pane, _) = Pane::new(None);
        pane.ask("hi", now);
        let (status, error, detail) = rules::UNREACHABLE;
        pane.ask_chunk(head(status, "application/json"), now);
        pane.ask_chunk(
            Chunk::Data(nexus_desktop_core::agent::failure_body(error, detail).into_bytes()),
            now,
        );
        pane.ask_chunk(Chunk::End, now);
        assert_eq!((pane.sent, pane.offline), (Some(Sent::NotSent), true));

        // The connection broke before Nexus started: not sent, but not offline, which would send it again by itself.
        let (mut pane, _) = Pane::new(None);
        pane.ask("hi", now);
        pane.ask_chunk(head(200, "text/event-stream"), now);
        pane.ask_chunk(Chunk::Broken, now);
        assert_eq!((pane.sent, pane.offline), (Some(Sent::NotSent), false));
    }

    #[test]
    fn an_ask_carries_its_picks_and_skill_and_a_retry_sends_them_again() {
        use nexus_desktop_core::agent_composer::{ContextKind, Wire};
        let now = Instant::now();
        let (mut pane, _) = Pane::new(None);
        let wire = Wire {
            message: "plan dinner".into(),
            context: vec![(ContextKind::Connector, "gmail".into())],
            skill: Some("errand"),
        };
        let Some(Call::Ask { body }) = pane.ask_with("/errand @Gmail plan dinner", wire, now) else {
            panic!("asks")
        };
        assert_eq!(
            body,
            r#"{"message":"plan dinner","stream":true,"context":[{"kind":"connector","id":"gmail"}],"skill":"errand"}"#
        );
        assert_eq!(pane.entries, vec![Entry::User("/errand @Gmail plan dinner".into())]);
        pane.ask_chunk(head(200, "text/event-stream"), now);
        pane.ask_chunk(Chunk::Broken, now);
        let Some(Call::Ask { body: again }) = pane.retry(now) else {
            panic!("retries")
        };
        assert_eq!(again, body, "the retry keeps the picks and the skill");
        assert_eq!(pane.entries.len(), 1, "the message is not doubled");
    }

    #[test]
    fn retry_asks_the_message_that_was_not_sent_again() {
        let now = Instant::now();
        let (mut pane, _) = Pane::new(None);
        assert_eq!(pane.retry(now), None, "nothing to retry");
        pane.ask("make it 8", now);
        assert_eq!(pane.retry(now), None, "not while it is delivered");
        pane.ask_chunk(head(200, "text/event-stream"), now);
        pane.ask_chunk(Chunk::Broken, now);
        let Some(Call::Ask { body }) = pane.retry(now) else {
            panic!("retries")
        };
        assert_eq!(body, r#"{"message":"make it 8","stream":true}"#);
        assert_eq!(
            pane.entries,
            vec![Entry::User("make it 8".into())],
            "the message is not doubled"
        );
        assert_eq!((pane.sent, pane.offline), (Some(Sent::Delivered), false));
        // Stopped while delivered, the line goes; a new conversation clears it all.
        assert!(pane.stop());
        assert_eq!(pane.sent, None);
        assert!(pane.start_over());
        assert_eq!((pane.sent, pane.offline), (None, false));
    }

    /// Needs you: the latest reply carries a write that waits, and the person has not written since.
    #[test]
    fn a_waiting_write_needs_the_person_until_they_write_again() {
        let (mut pane, _) = Pane::new(None);
        let started = Instant::now();
        assert!(!pane.needs_you());
        pane.ask("post it", started);
        answered(&mut pane, PENDING, started, Duration::ZERO);
        assert!(pane.needs_you());
        pane.ask("never mind", started);
        assert!(!pane.needs_you());
        answered(&mut pane, ANSWER, started, Duration::ZERO);
        assert!(!pane.needs_you());
    }

    const INTRO: &str = r#"{"firstOpen":true,"line":"Hand me anything.","starters":["Summarise my inbox","Track my packages"],"reads":["Gmail"],"accounts":true,"discover":true}"#;

    /// First open: a new pane with no conversation reads the first open once; it shows while the chat is empty, the API
    /// keeps it seen, and the person's first question ends it.
    #[test]
    fn the_first_open_shows_once_and_goes_with_the_first_question() {
        let (mut pane, _) = Pane::new(None);
        assert_eq!(pane.intro_call(), Some(Call::Intro));
        assert_eq!(pane.intro_call(), None, "read once");
        assert_eq!(
            Call::Intro.request(),
            (nexus_client::Method::GET, "/agent/intro".to_string(), None)
        );
        assert_eq!(
            Call::IntroSeen.request(),
            (nexus_client::Method::POST, "/agent/intro/seen".to_string(), None)
        );
        assert_eq!(pane.intro_chunk(head(200, "application/json")), None);
        assert_eq!(pane.intro_chunk(Chunk::Data(INTRO.as_bytes().to_vec())), None);
        assert_eq!(pane.intro_chunk(Chunk::End), Some(Call::IntroSeen));
        assert_eq!(pane.intro_chunk(Chunk::End), None, "seen once");
        let intro = pane.first_open().expect("the first open");
        assert_eq!(intro.starters, ["Summarise my inbox", "Track my packages"]);
        assert!(intro.accounts && intro.discover);
        pane.ask("Summarise my inbox", Instant::now());
        assert_eq!(pane.first_open(), None);
        assert!(pane.stop() && pane.start_over());
        assert_eq!(pane.first_open(), None, "never again in this pane");
    }

    #[test]
    fn the_first_open_stays_away_when_it_does_not_apply() {
        // A conversation read back opens on it.
        let (mut pane, _) = Pane::new(Some("c1"));
        assert_eq!(pane.intro_call(), None);
        // Off (404), seen, or failed: the empty chat as before.
        for (status, body) in [
            (404, r#"{"error":"not_found"}"#),
            (200, r#"{"firstOpen":false}"#),
            (502, ""),
        ] {
            let (mut pane, _) = Pane::new(None);
            pane.intro_call();
            pane.intro_chunk(head(status, "application/json"));
            pane.intro_chunk(Chunk::Data(body.as_bytes().to_vec()));
            assert_eq!(pane.intro_chunk(Chunk::End), None, "{status} {body}");
            assert_eq!(pane.first_open(), None);
        }
        // Asked while it loaded: it no longer applies, and is not marked seen.
        let (mut pane, _) = Pane::new(None);
        pane.intro_call();
        pane.ask("hi", Instant::now());
        pane.intro_chunk(head(200, "application/json"));
        pane.intro_chunk(Chunk::Data(INTRO.as_bytes().to_vec()));
        assert_eq!(pane.intro_chunk(Chunk::End), None);
        assert_eq!(pane.intro, None);
        // A broken body shows nothing.
        let (mut pane, _) = Pane::new(None);
        pane.intro_call();
        pane.intro_chunk(head(200, "application/json"));
        pane.intro_chunk(Chunk::Broken);
        assert_eq!(pane.intro_chunk(Chunk::End), None);
    }

    /// An answer keeps its Connect entries and a waiting write its approval card through the stream.
    #[test]
    fn an_answer_keeps_its_connect_cards_and_a_write_its_approval_card() {
        let (mut pane, _) = Pane::new(None);
        let started = Instant::now();
        pane.ask("my issues", started);
        let connect = r#"{"kind":"answer","text":"Connect Linear first.","connect":[{"connector":"linear","label":"Linear","url":"https://nexus.test/app/acme/settings/connectors#linear","summary":"List your issues"}]}"#;
        answered(&mut pane, connect, started, Duration::ZERO);
        let Some(Entry::Agent(messages)) = pane.entries.last() else {
            panic!("an answer")
        };
        let cards = nexus_desktop_core::agent_cards::answer_connects(&messages[0]);
        assert_eq!(cards.len(), 1);
        assert_eq!(
            nexus_desktop_core::agent_cards::connect_line(&cards[0], "Nexus"),
            "List your issues."
        );

        pane.ask("tell them", started);
        let pending = r##"{"kind":"pending_action","card":"c2","approveUrl":"https://nexus.test/app/approvals/a2","detail":"d","expiresAt":"2026-11-09T12:30:00Z","write":{"connector":"slack","operation":"post_message","summary":"Text Ana","effect":"irreversible"},"asks":{"tier":"always-asks","to":["Ana R."],"text":"Dinner at 7:30.","why":{"said":"tell them","at":"2026-11-09T08:24:00Z"},"always":false}}"##;
        answered(&mut pane, pending, started, Duration::ZERO);
        let Some(Entry::Agent(messages)) = pane.entries.last() else {
            panic!("a write")
        };
        let ask = nexus_desktop_core::agent_cards::write_ask(&messages[0]).expect("an approval card");
        assert!(ask.always_asks);
        assert_eq!(ask.to, ["Ana R."]);
        assert!(pane.needs_you());
    }

    #[test]
    fn a_read_back_shows_the_turns_or_says_the_conversation_ended() {
        let (mut pane, _) = Pane::new(Some("c1"));
        pane.read_chunk(head(200, "application/json"));
        pane.read_chunk(Chunk::Data(
            br#"{"turns":[{"role":"user","text":"hi","at":"t"},{"role":"assistant","text":"Done.","at":"t","model":"m/x","costUsd":0.01}]}"#.to_vec(),
        ));
        pane.read_chunk(Chunk::End);
        assert!(!pane.loading);
        assert_eq!(pane.entries.len(), 2);
        assert_eq!(pane.conversation.as_deref(), Some("c1"));
        assert_eq!(pane.times, vec![None, None], "a time it cannot read is none");

        let (mut pane, _) = Pane::new(Some("c1"));
        pane.read_chunk(head(404, "application/json"));
        pane.read_chunk(Chunk::End);
        assert_eq!(pane.conversation, None);
        assert_eq!(pane.status, "That conversation has ended. Start a new one.");
        assert!(pane.empty());

        let (mut pane, _) = Pane::new(Some("c1"));
        pane.read_chunk(head(502, "application/json"));
        pane.read_chunk(Chunk::Data(br#"{"error":"agents_api_unreachable"}"#.to_vec()));
        pane.read_chunk(Chunk::End);
        assert_eq!(
            pane.problem.unwrap().title,
            rules::problem(502, Some("agents_api_unreachable"), None).title
        );
    }

    /// Each entry keeps when it was said, for the thread's time lines: now for a live one, the API's `at`
    /// for a turn read back, and the two stay in step through Retry and New conversation.
    #[test]
    fn each_entry_keeps_its_time_through_read_back_retry_and_start_over() {
        let (mut pane, _) = Pane::new(Some("c1"));
        pane.read_chunk(head(200, "application/json"));
        pane.read_chunk(Chunk::Data(
            br#"{"turns":[{"role":"user","text":"hi","at":"2026-11-16T16:02:00Z"},{"role":"assistant","text":"Done."}]}"#
                .to_vec(),
        ));
        pane.read_chunk(Chunk::End);
        let read = chrono::DateTime::parse_from_rfc3339("2026-11-16T16:02:00Z").expect("a time");
        assert_eq!(pane.times.len(), 2);
        assert_eq!(pane.times[0].map(|at| at.timestamp()), Some(read.timestamp()));
        assert_eq!(pane.times[1], None);

        let started = Instant::now();
        pane.ask("again", started).expect("an ask");
        answered(&mut pane, ANSWER, started, Duration::from_millis(10));
        assert_eq!(pane.entries.len(), 4);
        assert_eq!(pane.times.len(), 4);
        assert!(pane.times[2..].iter().all(Option::is_some));

        pane.start_over();
        assert!(pane.entries.is_empty() && pane.times.is_empty());
    }

    /// While Nexus thinks, the thread shows "Thinking", then each step line as it comes; once the
    /// answer has a model and no step, the typing dots; then the words. A bad step line changes nothing.
    #[test]
    fn the_thinking_line_follows_the_stream_steps() {
        let (mut pane, _) = Pane::new(None);
        let started = Instant::now();
        pane.ask("plan my day", started).expect("an ask");
        let line = |pane: &Pane| pane.draft.as_ref().and_then(Draft::thinking_line).map(String::from);
        assert_eq!(line(&pane).as_deref(), Some("Thinking"));
        pane.ask_chunk(head(200, "text/event-stream"), started);
        pane.ask_chunk(sse("model", r#"{"model":"m/x"}"#), started);
        assert_eq!(line(&pane), None, "a model and no step: the typing dots");
        pane.ask_chunk(sse("step", r#"{"line":"Reading your calendar"}"#), started);
        assert_eq!(line(&pane).as_deref(), Some("Reading your calendar"));
        pane.ask_chunk(sse("step", r#"{"line":"two\nlines"}"#), started);
        pane.ask_chunk(sse("step", r#"{"line":"Searching Linear"}"#), started);
        assert_eq!(line(&pane).as_deref(), Some("Searching Linear"));
        pane.ask_chunk(sse("text", r#"{"delta":"Here"}"#), started);
        assert_eq!(line(&pane), None, "the words replace it");
    }
}
