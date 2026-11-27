//! Errands for a native shell: the agent API's errands read strictly, as the kit's `agent-errands.ts`, and the
//! words of the view, as Nexus UI's `lib/agent-errands.ts`. Nexus GPUI draws it
//! (`desktop-gpui/src/pages/assistant_views/errands.rs`).
//!
//! Each errand is its own thread: its goal as the person's first message, its steps as quiet notes, then
//! the replies about it ([`entries`]). `PATCH /agent/errands/{id}` renames, pins, archives or marks it done ([`Edit`]),
//! and `POST /agent/errands/{id}/messages` replies about it ([`message_body`], [`errand_reply_of`]). The sidebar lists
//! the open ones ([`sidebar_errands`]), and the main chat opens with those that need the person
//! ([`errands_needing_you`]).

use serde_json::{Value, json};

use crate::agent::{Message, Problem, errand_id, safe_url};
use crate::agent_approvals::view_problem;

/// Where an errand stands. `NeedsYou`: it waits on the person, and `need` says for what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NeedsYou,
    Running,
    Done,
    Failed,
}

/// What it waits for: a draft to confirm, a write card to decide, or a question the agent asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Confirm,
    Approval,
    Question,
}

/// What the person may do with it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Confirm,
    Done,
    Cancel,
}

impl Action {
    pub fn word(self) -> &'static str {
        match self {
            Action::Confirm => "confirm",
            Action::Done => "done",
            Action::Cancel => "cancel",
        }
    }

    /// Its button.
    pub fn label(self) -> &'static str {
        match self {
            Action::Confirm => "Confirm",
            Action::Done => "Mark done",
            Action::Cancel => "Cancel errand",
        }
    }

    /// `POST /agent/errands/{id}`'s body.
    pub fn body(self) -> String {
        json!({ "action": self.word() }).to_string()
    }
}

/// One row of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Errand {
    /// `i` and a number, such as `i3`.
    pub id: String,
    pub goal: String,
    pub status: Status,
    pub need: Option<Need>,
    /// For an approval: the write's summary, in the agent's words.
    pub detail: Option<String>,
    pub steps: u64,
    /// The last day, `YYYY-MM-DD`.
    pub deadline: Option<String>,
    pub at: String,
    /// The name the person gave it; None: its goal names it ([`errand_name`]).
    pub title: Option<String>,
    /// The person pinned it: it leads its group.
    pub pinned: bool,
    /// The person archived it: it shows only in the history.
    pub archived: bool,
    /// The project the person filed it in. None: it is loose, under the Assistant.
    pub project: Option<String>,
}

/// One turn of an errand's own thread: a reply of the person's about it, or the agent's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// The person wrote it (true), or the agent.
    pub mine: bool,
    pub text: String,
    pub at: String,
    /// The agent's reply as the API rendered it: an answer, an approval card, a refusal. A kind this build does not
    /// know is left out.
    pub messages: Vec<Message>,
}

/// What a `PATCH /agent/errands/{id}` changes: a name (`Some(None)` drops it), the pin, the archive, done, or its
/// project (`Some(None)` makes it loose).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Edit {
    pub title: Option<Option<String>>,
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    pub done: bool,
    pub project: Option<Option<String>>,
}

impl Edit {
    /// Its body, the change alone, as the agent API reads it strictly.
    pub fn body(&self) -> String {
        let mut fields = serde_json::Map::new();
        if let Some(title) = &self.title {
            fields.insert("title".into(), title.clone().map_or(Value::Null, Value::String));
        }
        if let Some(pinned) = self.pinned {
            fields.insert("pinned".into(), Value::Bool(pinned));
        }
        if let Some(archived) = self.archived {
            fields.insert("archived".into(), Value::Bool(archived));
        }
        if self.done {
            fields.insert("done".into(), Value::Bool(true));
        }
        if let Some(project) = &self.project {
            fields.insert("project".into(), project.clone().map_or(Value::Null, Value::String));
        }
        Value::Object(fields).to_string()
    }
}

/// A reply's body: `{"message"}` alone.
pub fn message_body(message: &str) -> String {
    json!({ "message": message }).to_string()
}

/// A new errand's body: the goal, trimmed to what the agent API takes, and the project it starts in, when the
/// name is one the agent API reads.
pub fn goal_body(goal: &str, project: Option<&str>) -> String {
    let goal: String = goal.trim().chars().take(crate::agent::ERRAND_GOAL_CHARS).collect();
    match project.and_then(crate::agent::project_name) {
        Some(project) => json!({ "goal": goal, "project": project }).to_string(),
        None => json!({ "goal": goal }).to_string(),
    }
}

/// `POST /agent/errands/{id}/messages`'s body: the reply's messages, and the errand as it is now.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub messages: Vec<Message>,
    pub errand: Option<ErrandView>,
}

/// One thing the agent did for an errand.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub urls: Vec<String>,
    pub at: String,
}

/// One piece of an outcome's evidence.
#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    Link(String),
    Citation(String),
}

/// How a closed errand ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// `done`, `partly_done`, `failed` or `cancelled`.
    pub status: String,
    pub at: String,
    pub evidence: Vec<Evidence>,
}

/// One errand, read whole.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrandView {
    pub errand: Errand,
    pub constraints: Vec<String>,
    pub budget_usd: Option<String>,
    /// Where it started: a surface's slug, such as `slack` or `api`.
    pub surface: String,
    pub created_at: String,
    pub steps: Vec<Step>,
    pub outcome: Option<Outcome>,
    pub actions: Vec<Action>,
    /// Its own thread, oldest first; empty from an agent API that predates it.
    pub thread: Vec<Turn>,
}

/// What an action did. `Discarded`: a draft went, and there is no errand to show.
#[derive(Debug, Clone, PartialEq)]
pub enum Acted {
    Confirmed(Box<ErrandView>),
    Done(Box<ErrandView>),
    Cancelled(Box<ErrandView>),
    Discarded,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(String::from)
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default()
}

fn day(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(at, byte)| match at {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// One row, read strictly.
pub fn errand_of(value: &Value) -> Option<Errand> {
    let id = text(value, "id").filter(|id| errand_id(id))?;
    let status = match value.get("status")?.as_str()? {
        "needs-you" => Status::NeedsYou,
        "running" => Status::Running,
        "done" => Status::Done,
        "failed" => Status::Failed,
        _ => return None,
    };
    let need = match value.get("need").and_then(Value::as_str) {
        Some("confirm") => Some(Need::Confirm),
        Some("approval") => Some(Need::Approval),
        Some("question") => Some(Need::Question),
        _ => None,
    };
    Some(Errand {
        id,
        goal: text(value, "goal")?,
        status,
        need,
        detail: text(value, "detail").filter(|detail| !detail.is_empty()),
        steps: value.get("steps").and_then(Value::as_u64).unwrap_or(0),
        deadline: text(value, "deadline").filter(|deadline| day(deadline)),
        at: text(value, "at")?,
        title: text(value, "title").filter(|title| !title.trim().is_empty()),
        pinned: value.get("pinned") == Some(&Value::Bool(true)),
        archived: value.get("archived") == Some(&Value::Bool(true)),
        project: text(value, "project").filter(|project| !project.trim().is_empty()),
    })
}

/// `GET /agent/errands`'s body: its rows in the API's order.
pub fn errand_list_of(body: &Value) -> Option<Vec<Errand>> {
    Some(body.get("errands")?.as_array()?.iter().filter_map(errand_of).collect())
}

/// `GET /agent/errands`'s history: the closed and archived errands, newest first; empty from an API without one.
pub fn errand_history_of(body: &Value) -> Vec<Errand> {
    body.get("history")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(errand_of).collect())
        .unwrap_or_default()
}

/// The messages of a reply the agent API rendered, read as the chat reads them; a kind this build does not know, or
/// one that is not a message, is left out.
fn messages_of(value: Option<&Value>) -> Vec<Message> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| serde_json::from_value::<Message>(message.clone()).ok())
        .filter(|message| !matches!(message, Message::Unknown))
        .collect()
}

/// One thread turn, read strictly.
fn turn_of(value: &Value) -> Option<Turn> {
    let mine = match value.get("role")?.as_str()? {
        "user" => true,
        "assistant" => false,
        _ => return None,
    };
    Some(Turn {
        mine,
        text: text(value, "text")?,
        at: text(value, "at")?,
        messages: messages_of(value.get("messages")),
    })
}

/// `GET /agent/errands/{id}`'s body.
pub fn errand_view_of(body: &Value) -> Option<ErrandView> {
    let errand = errand_of(body.get("errand")?)?;
    let steps = body
        .get("steps")?
        .as_array()?
        .iter()
        .filter_map(|step| {
            Some(Step {
                id: text(step, "id")?,
                kind: text(step, "kind").unwrap_or_else(|| "tool".into()),
                text: text(step, "text")?,
                urls: strings(step, "urls"),
                at: text(step, "at")?,
            })
        })
        .collect();
    let outcome = body.get("outcome").and_then(|outcome| {
        Some(Outcome {
            status: text(outcome, "status")?,
            at: text(outcome, "at")?,
            evidence: outcome
                .get("evidence")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| match item.get("kind").and_then(Value::as_str) {
                    Some("link") => text(item, "url").map(Evidence::Link),
                    Some("citation") => text(item, "text").map(Evidence::Citation),
                    _ => None,
                })
                .collect(),
        })
    });
    let actions = strings(body, "actions")
        .iter()
        .filter_map(|action| match action.as_str() {
            "confirm" => Some(Action::Confirm),
            "done" => Some(Action::Done),
            "cancel" => Some(Action::Cancel),
            _ => None,
        })
        .collect();
    Some(ErrandView {
        errand,
        constraints: strings(body, "constraints"),
        budget_usd: text(body, "budgetUsd"),
        surface: text(body, "surface")?,
        created_at: text(body, "createdAt")?,
        steps,
        outcome,
        actions,
        thread: body
            .get("thread")
            .and_then(Value::as_array)
            .map(|turns| turns.iter().filter_map(turn_of).collect())
            .unwrap_or_default(),
    })
}

/// `POST /agent/errands`'s body: the new errand, read whole.
pub fn errand_opened_of(body: &Value) -> Option<ErrandView> {
    (body.get("state")?.as_str()? == "opened")
        .then(|| errand_view_of(body.get("errand")?))
        .flatten()
}

/// `POST /agent/errands/{id}/messages`'s body; None when it is not one.
pub fn errand_reply_of(body: &Value) -> Option<Reply> {
    body.get("messages")?.as_array()?;
    Some(Reply {
        messages: messages_of(body.get("messages")),
        errand: body.get("errand").and_then(errand_view_of),
    })
}

/// `POST /agent/errands/{id}`'s body.
pub fn errand_acted_of(body: &Value) -> Option<Acted> {
    let state = body.get("state")?.as_str()?;
    if state == "discarded" {
        return Some(Acted::Discarded);
    }
    let view = Box::new(errand_view_of(body.get("errand")?)?);
    match state {
        "confirmed" => Some(Acted::Confirmed(view)),
        "done" => Some(Acted::Done(view)),
        "cancelled" => Some(Acted::Cancelled(view)),
        _ => None,
    }
}

/// Whether a failed list read means the agent API has no errands to show: off there (503 `errands_off`), or it predates
/// them (404). The view then says so in one line.
pub fn errands_off(status: u16, body: Option<&Value>) -> bool {
    status == 404
        || (status == 503 && body.and_then(|body| body.get("error")).and_then(Value::as_str) == Some("errands_off"))
}

/// A surface, as the person reads it. The agent API's own chat is the app's.
pub fn surface_name(slug: &str) -> String {
    match slug {
        "slack" => "Slack".into(),
        "telegram" => "Telegram".into(),
        "api" => "Nexus".into(),
        other => other.into(),
    }
}

fn steps_text(count: u64) -> String {
    format!("{count} {}", if count == 1 { "step" } else { "steps" })
}

/// What the errand needs, or how it goes, in a few words under its goal.
pub fn errand_line(errand: &Errand) -> String {
    match (errand.need, errand.status) {
        (Some(Need::Confirm), _) => "Confirm it to start".into(),
        (Some(Need::Approval), _) => match &errand.detail {
            Some(detail) => format!("Approve: {detail}"),
            None => "A change waits for your approval".into(),
        },
        (Some(Need::Question), _) => "It asked you something".into(),
        (None, Status::Running) if errand.steps > 0 => format!("{} so far", steps_text(errand.steps)),
        (None, Status::Running) => "Started".into(),
        (None, Status::Failed) => "It did not finish".into(),
        (None, _) if errand.steps > 0 => format!("Done in {}", steps_text(errand.steps)),
        (None, _) => "Done".into(),
    }
}

/// What it needs, in a word beside its status mark.
pub fn need_word(need: Need) -> &'static str {
    match need {
        Need::Confirm => "Draft",
        Need::Approval => "Asks first",
        Need::Question => "Asked you",
    }
}

/// Where the person answers what it needs, when the answer is not here. None when nothing waits, or for a draft.
pub fn where_to_answer(view: &ErrandView) -> Option<String> {
    let place = surface_name(&view.surface);
    let own = view.surface == "api";
    match view.errand.need? {
        Need::Approval if own => Some("The change waits for your approval in Nexus.".into()),
        Need::Approval => Some(format!(
            "The change waits for your approval in {place}. Approve or cancel it there."
        )),
        Need::Question if own => Some("Reply in the chat where you asked.".into()),
        Need::Question => Some(format!("Reply in its {place} thread.")),
        Need::Confirm => None,
    }
}

/// How a closed errand ended, from the record's own word.
pub fn outcome_text(status: &str) -> &'static str {
    match status {
        "partly_done" => "Partly done",
        "failed" => "It did not finish",
        "cancelled" => "Cancelled",
        _ => "Done",
    }
}

/// A deadline (`YYYY-MM-DD`, UTC) as the person reads it. `today` and `tomorrow` are UTC days; `short` is the day as
/// "Nov 5".
pub fn deadline_text(deadline: &str, today: &str, tomorrow: &str, short: &str) -> String {
    if deadline < today {
        "Past its deadline".into()
    } else if deadline == today {
        "Due today".into()
    } else if deadline == tomorrow {
        "Due tomorrow".into()
    } else {
        format!("Due {short}")
    }
}

/// The list in its three groups, each in the API's order. An empty group is left out.
pub fn errand_groups(errands: &[Errand]) -> Vec<(&'static str, Vec<Errand>)> {
    let of = |keep: &dyn Fn(Status) -> bool| {
        errands
            .iter()
            .filter(|errand| keep(errand.status))
            .cloned()
            .collect::<Vec<_>>()
    };
    [
        ("Needs you", of(&|status| status == Status::NeedsYou)),
        ("Running", of(&|status| status == Status::Running)),
        (
            "Done this week",
            of(&|status| matches!(status, Status::Done | Status::Failed)),
        ),
    ]
    .into_iter()
    .filter(|(_, errands)| !errands.is_empty())
    .collect()
}

/// The count over the list: "5 errands · 2 need you".
pub fn count_text(errands: &[Errand]) -> String {
    let needs = errands
        .iter()
        .filter(|errand| errand.status == Status::NeedsYou)
        .count();
    let mut out = format!(
        "{} {}",
        errands.len(),
        if errands.len() == 1 { "errand" } else { "errands" }
    );
    if needs > 0 {
        out.push_str(&format!(" · {needs} {} you", if needs == 1 { "needs" } else { "need" }));
    }
    out
}

/// What the view says once an action is done. `name`: the Assistant's name.
pub fn acted_text(acted: &Acted, name: &str) -> String {
    match acted {
        Acted::Confirmed(_) => format!("Confirmed. {name} works on it now."),
        Acted::Done(_) => "Marked done.".into(),
        Acted::Cancelled(_) => format!("Cancelled. {name} stopped working on it."),
        Acted::Discarded => "The draft is deleted.".into(),
    }
}

/// The name an errand goes by: the one the person gave it, else its goal.
pub fn errand_name(errand: &Errand) -> &str {
    errand.title.as_deref().unwrap_or(&errand.goal)
}

/// How many errands the sidebar lists under the Assistant; the rest are on the Errands view.
pub const SIDEBAR_ERRANDS: usize = 5;

/// The errands the sidebar lists under the Assistant's row: the open ones, what needs the person first,
/// then what runs, in the API's order, at most [`SIDEBAR_ERRANDS`]. A closed or archived errand is not listed.
pub fn sidebar_errands(errands: &[Errand]) -> Vec<Errand> {
    let of = |status: Status| {
        errands
            .iter()
            .filter(move |errand| errand.status == status && !errand.archived)
    };
    of(Status::NeedsYou)
        .chain(of(Status::Running))
        .take(SIDEBAR_ERRANDS)
        .cloned()
        .collect()
}

/// The errands that wait on the person, pinned first, for the main chat's overview ("2 errands need you").
pub fn errands_needing_you(errands: &[Errand]) -> Vec<Errand> {
    let needs = || {
        errands
            .iter()
            .filter(|errand| errand.status == Status::NeedsYou && !errand.archived)
    };
    needs()
        .filter(|errand| errand.pinned)
        .chain(needs().filter(|errand| !errand.pinned))
        .cloned()
        .collect()
}

/// The most errands the main chat's overview card names; the Errands view has the rest.
pub const OVERVIEW_ERRANDS: usize = 3;

/// The overview card's title: "2 errands need you", or None when none does.
pub fn overview_title(count: usize) -> Option<String> {
    (count > 0).then(|| {
        format!(
            "{count} {} you",
            if count == 1 { "errand needs" } else { "errands need" }
        )
    })
}

/// The history rows whose name or goal holds every word of `query`, ignoring case. An empty query holds them all.
pub fn search_errands(errands: &[Errand], query: &str) -> Vec<Errand> {
    let query = query.to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    errands
        .iter()
        .filter(|errand| {
            let text = format!("{} {}", errand.title.as_deref().unwrap_or(""), errand.goal).to_lowercase();
            words.iter().all(|word| text.contains(word))
        })
        .cloned()
        .collect()
}

/// One entry of an errand's thread, as the view draws it. Oldest first.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// A line where the day changes, with the first entry's time.
    Day(String),
    /// The person's goal as their first message, or a reply of theirs.
    You { text: String, at: String },
    /// A step the agent took, as a quiet note.
    Step {
        text: String,
        urls: Vec<String>,
        at: String,
    },
    /// The agent's reply; `text` when it has no messages.
    Nexus {
        messages: Vec<Message>,
        text: String,
        at: String,
    },
}

impl Entry {
    fn at(&self) -> &str {
        match self {
            Entry::Day(at) | Entry::You { at, .. } | Entry::Step { at, .. } | Entry::Nexus { at, .. } => at,
        }
    }
}

/// The thread of an open errand: its goal, its steps and its turns, by time, with a day line where the day changes.
/// Entries at the same time keep the order goal, steps, turns.
pub fn entries(view: &ErrandView) -> Vec<Entry> {
    let mut timed = vec![Entry::You {
        text: view.errand.goal.clone(),
        at: view.created_at.clone(),
    }];
    timed.extend(view.steps.iter().map(|step| Entry::Step {
        text: step.text.clone(),
        urls: step.urls.clone(),
        at: step.at.clone(),
    }));
    timed.extend(view.thread.iter().map(|turn| {
        if turn.mine {
            Entry::You {
                text: turn.text.clone(),
                at: turn.at.clone(),
            }
        } else {
            Entry::Nexus {
                messages: turn.messages.clone(),
                text: turn.text.clone(),
                at: turn.at.clone(),
            }
        }
    }));
    // A stable sort: entries at the same time keep the order above.
    timed.sort_by(|a, b| a.at().cmp(b.at()));
    let mut out = Vec::with_capacity(timed.len() + 2);
    let mut last_day = String::new();
    for entry in timed {
        let today: String = entry.at().chars().take(10).collect();
        if today != last_day {
            out.push(Entry::Day(entry.at().to_string()));
            last_day = today;
        }
        out.push(entry);
    }
    out
}

/// The composer's placeholder: "Reply about <the errand's name>", cut to a short line.
pub fn reply_placeholder(errand: &Errand) -> String {
    let name = errand_name(errand);
    if name.chars().count() > 48 {
        format!("Reply about {}…", name.chars().take(47).collect::<String>())
    } else {
        format!("Reply about {name}")
    }
}

/// The errand's state in a few words, beside its name in the thread's header.
pub fn state_text(view: &ErrandView) -> &'static str {
    if let Some(outcome) = &view.outcome {
        return outcome_text(&outcome.status);
    }
    if view.errand.archived {
        return "Archived";
    }
    match view.errand.status {
        Status::NeedsYou => "Needs you",
        Status::Running => "Working on it",
        Status::Done => "Done",
        Status::Failed => "It did not finish",
    }
}

/// An item of the thread's ⋯ menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Rename,
    Pin,
    /// Move to project: the projects take the menu's place.
    Move,
    Done,
    Archive,
    /// Hand off to a session: a New session draft with the errand's prompt, never sent.
    HandOff,
}

/// The ⋯ menu's items for an errand, in the board's order, with their words. Mark done only while it runs, and Hand off
/// to a session last, while it runs too: only an open, confirmed errand records the step.
pub fn menu(view: &ErrandView) -> Vec<(MenuItem, &'static str)> {
    let mut items = vec![
        (MenuItem::Rename, "Rename"),
        (MenuItem::Pin, if view.errand.pinned { "Unpin" } else { "Pin" }),
        (MenuItem::Move, "Move to project"),
    ];
    if view.actions.contains(&Action::Done) {
        items.push((MenuItem::Done, "Mark done"));
    }
    items.push((
        MenuItem::Archive,
        if view.errand.archived {
            "Move out of the archive"
        } else {
            "Archive"
        },
    ));
    if view.actions.contains(&Action::Done) {
        items.push((MenuItem::HandOff, "Hand off to a session"));
    }
    items
}

/// The longest hand-off prompt: the desktop's own bound on a draft link's prompt ([`crate::prompt_link::MAX_PROMPT`]),
/// which the kit's `AGENT_ERRAND_HANDOFF_CHARS` matches.
pub const HANDOFF_CHARS: usize = 8_000;

/// What the view says once a hand-off opened its draft: where the prompt waits, never sent.
pub const HANDED_OFF_TEXT: &str = "Your New session draft has the errand in it. Send it when it reads right.";

/// An errand as a coding session's prompt, as `GET /agent/errands/{id}/handoff` answers it. The kit's
/// `errandHandoff` builds it, so every app hands an errand off the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    pub prompt: String,
    /// `owner/name` on GitHub, when the errand or its project names one.
    pub repo: Option<String>,
}

/// `owner/name` as the kit reads one: an owner GitHub could have, a slash, and a name of letters, digits, `.`, `_`, `-`.
fn repo_name(text: &str) -> bool {
    let Some((owner, name)) = text.split_once('/') else {
        return false;
    };
    let owner_ok = (1..=39).contains(&owner.len())
        && owner.starts_with(|c: char| c.is_ascii_alphanumeric())
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let name_ok = (1..=100).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    owner_ok && name_ok
}

/// `GET /agent/errands/{id}/handoff`'s body, read strictly as the kit's `errandHandoffOf`: a prompt within the bound,
/// and a repository only when it is one.
pub fn handoff_of(body: &Value) -> Option<Handoff> {
    let prompt = body.get("prompt")?.as_str()?;
    if prompt.trim().is_empty() || prompt.encode_utf16().count() > HANDOFF_CHARS {
        return None;
    }
    let repo = body
        .get("repo")
        .and_then(Value::as_str)
        .filter(|repo| repo_name(repo))
        .map(str::to_string);
    Some(Handoff {
        prompt: prompt.to_string(),
        repo,
    })
}

/// The New session draft's link for a hand-off: `nexus://new?prompt=…&repo=owner/name`, as the session card's. It
/// fills a draft and never sends it.
pub fn handoff_link(handoff: &Handoff) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("prompt", &handoff.prompt);
    if let Some(repo) = &handoff.repo {
        query.append_pair("repo", repo);
    }
    format!("nexus://new?{}", query.finish().replace('+', "%20"))
}

/// What the view says once an edit is done.
pub fn edited_text(edit: &Edit) -> String {
    match &edit.project {
        Some(Some(project)) if !edit.done && edit.archived.is_none() && edit.pinned.is_none() => {
            format!("Moved to {project}.")
        }
        _ => edited_line(edit).to_string(),
    }
}

fn edited_line(edit: &Edit) -> &'static str {
    if edit.done {
        "Marked done."
    } else if edit.archived == Some(true) {
        "Archived. It is in the history now."
    } else if edit.archived == Some(false) {
        "Back in your errands."
    } else if edit.pinned == Some(true) {
        "Pinned."
    } else if edit.pinned == Some(false) {
        "Unpinned."
    } else if edit.project == Some(None) {
        "Loose again, under the Assistant."
    } else if edit.title == Some(None) {
        "The name is gone."
    } else {
        "Renamed."
    }
}

/// The rename's change: the goal again, or nothing, drops the name. None when the name stays as it is.
pub fn rename(errand: &Errand, typed: &str) -> Option<Edit> {
    let next = typed.trim();
    if next == errand_name(errand) {
        return None;
    }
    let title = (!next.is_empty() && next != errand.goal).then(|| next.to_string());
    Some(Edit {
        title: Some(title),
        ..Edit::default()
    })
}

/// What the view says when a new errand does not start: the agent API's own words for its limit.
pub fn open_problem(status: u16, body: Option<&Value>, name: &str) -> Problem {
    let error = body.and_then(|body| body.get("error")).and_then(Value::as_str);
    let detail = body
        .and_then(|body| body.get("detail"))
        .and_then(Value::as_str)
        .filter(|detail| !detail.is_empty());
    match (status, error) {
        (409, Some("too_many")) => Problem {
            title: format!("{name} has too many errands open"),
            detail: detail.unwrap_or("Finish or cancel one first.").into(),
        },
        (400, _) => Problem {
            title: "That errand did not start".into(),
            detail: "Say what it is for, in 500 characters or fewer.".into(),
        },
        _ => errands_problem(status, body),
    }
}

/// An http(s) link of a step or of the evidence, with its host for the label.
pub fn link_of(url: &str) -> Option<(String, String)> {
    let safe = safe_url(url)?;
    let host = url::Url::parse(&safe).ok()?.host_str()?.to_string();
    Some((safe, host))
}

/// What the view says when a read or an action fails.
pub fn errands_problem(status: u16, body: Option<&Value>) -> Problem {
    view_problem(status, body, |detail| {
        let said = |title: &str, fallback: &str| Problem {
            title: title.into(),
            detail: if detail.is_empty() {
                fallback.into()
            } else {
                detail.clone()
            },
        };
        match status {
            404 => Problem {
                title: "That errand is gone".into(),
                detail: "It was deleted, or it is not yours. Open another one.".into(),
            },
            409 => said("That was not done", "The errand changed. Open it again."),
            503 => said("Your errands are not available now", "Try again in a moment."),
            _ => said(
                "Your errands did not open",
                "Nexus could not reach the Assistant. Try again in a moment.",
            ),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Value {
        json!({
            "errand": { "id": "i3", "goal": "Dinner Friday", "status": "needs-you", "need": "approval", "detail": "Book Nopa", "steps": 4, "deadline": "2026-11-20", "at": "2026-11-17T08:02:00Z" },
            "constraints": ["Near Divisadero", 3],
            "budgetUsd": "120.00",
            "surface": "slack",
            "createdAt": "2026-11-17T08:02:00Z",
            "steps": [
                { "id": "s1", "kind": "web", "text": "Compared 6 places", "urls": ["https://nopa.test/menu", "javascript:alert(1)"], "at": "2026-11-17T08:10:00Z" },
                { "id": "s2", "text": 7, "at": "x" }
            ],
            "outcome": { "status": "partly_done", "at": "2026-11-17T09:00:00Z", "evidence": [{ "kind": "link", "url": "https://nopa.test" }, { "kind": "citation", "text": "Booked" }, { "kind": "x" }] },
            "actions": ["done", "cancel", "delete"]
        })
    }

    #[test]
    fn reads_an_errand_strictly() {
        let view = errand_view_of(&view()).expect("reads");
        assert_eq!(view.errand.need, Some(Need::Approval));
        assert_eq!(view.constraints, ["Near Divisadero"]);
        assert_eq!(view.steps.len(), 1);
        assert_eq!(view.steps[0].kind, "web");
        assert_eq!(view.outcome.as_ref().map(|outcome| outcome.evidence.len()), Some(2));
        assert_eq!(view.actions, [Action::Done, Action::Cancel]);
        assert_eq!(
            where_to_answer(&view).as_deref(),
            Some("The change waits for your approval in Slack. Approve or cancel it there.")
        );
        assert_eq!(
            link_of("https://nopa.test/menu"),
            Some(("https://nopa.test/menu".into(), "nopa.test".into()))
        );
        assert_eq!(link_of("javascript:alert(1)"), None);
        assert_eq!(
            errand_of(&json!({ "id": "x3", "goal": "g", "status": "done", "at": "a" })),
            None
        );
        assert_eq!(Action::Cancel.body(), r#"{"action":"cancel"}"#);
    }

    #[test]
    fn the_list_groups_and_counts_what_needs_you() {
        let body = json!({ "errands": [
            { "id": "i1", "goal": "A", "status": "running", "steps": 1, "at": "a" },
            { "id": "i2", "goal": "B", "status": "needs-you", "need": "confirm", "at": "a" },
            { "id": "i3", "goal": "C", "status": "failed", "at": "a" },
            { "id": "i4", "goal": "D", "status": "lost", "at": "a" }
        ]});
        let errands = errand_list_of(&body).expect("reads");
        assert_eq!(errands.len(), 3);
        let groups = errand_groups(&errands);
        assert_eq!(
            groups.iter().map(|group| group.0).collect::<Vec<_>>(),
            ["Needs you", "Running", "Done this week"]
        );
        assert_eq!(count_text(&errands), "3 errands · 1 needs you");
        assert_eq!(errand_line(&errands[0]), "1 step so far");
        assert_eq!(errand_line(&errands[1]), "Confirm it to start");
        assert_eq!(errand_line(&errands[2]), "It did not finish");
    }

    #[test]
    fn an_action_reads_what_it_did() {
        let confirmed = json!({ "state": "confirmed", "errand": view() });
        assert!(matches!(errand_acted_of(&confirmed), Some(Acted::Confirmed(_))));
        assert_eq!(
            errand_acted_of(&json!({ "state": "discarded" })),
            Some(Acted::Discarded)
        );
        assert_eq!(errand_acted_of(&json!({ "state": "done" })), None);
        assert_eq!(acted_text(&Acted::Discarded, "Ada"), "The draft is deleted.");
    }

    #[test]
    fn errands_off_and_failures_read_plainly() {
        assert!(errands_off(404, None));
        assert!(errands_off(503, Some(&json!({ "error": "errands_off" }))));
        assert!(!errands_off(503, None));
        assert_eq!(errands_problem(404, None).title, "That errand is gone");
        assert_eq!(errands_problem(401, None).title, "Sign in again");
        assert_eq!(
            deadline_text("2026-11-16", "2026-11-17", "2026-11-18", "Nov 16"),
            "Past its deadline"
        );
        assert_eq!(
            deadline_text("2026-11-18", "2026-11-17", "2026-11-18", "Nov 18"),
            "Due tomorrow"
        );
        assert_eq!(
            deadline_text("2026-11-20", "2026-11-17", "2026-11-18", "Nov 20"),
            "Due Nov 20"
        );
        assert_eq!(outcome_text("partly_done"), "Partly done");
    }

    fn row(id: &str, status: &str, extra: Value) -> Errand {
        let mut value =
            json!({ "id": id, "goal": format!("Goal {id}"), "status": status, "at": "2026-11-17T08:00:00Z" });
        if let (Value::Object(row), Value::Object(extra)) = (&mut value, extra) {
            row.extend(extra);
        }
        errand_of(&value).expect("a row")
    }

    /// The name, the pin and the archive read strictly, with the history.
    #[test]
    fn a_row_reads_its_name_pin_and_archive_and_the_history_beside_it() {
        let named = row(
            "i1",
            "running",
            json!({ "title": "Dinner", "pinned": true, "archived": "yes" }),
        );
        assert_eq!(errand_name(&named), "Dinner");
        assert!(named.pinned && !named.archived);
        let blank = row("i2", "done", json!({ "title": "  ", "archived": true }));
        assert_eq!(errand_name(&blank), "Goal i2");
        assert!(blank.archived);
        let body = json!({
            "errands": [{ "id": "i1", "goal": "a", "status": "running", "at": "x" }],
            "history": [{ "id": "i2", "goal": "b", "status": "done", "at": "x" }, { "id": "bad" }]
        });
        assert_eq!(errand_history_of(&body).len(), 1);
        assert!(errand_history_of(&json!({ "errands": [] })).is_empty());
    }

    #[test]
    fn the_sidebar_and_the_overview_pick_what_is_open_and_what_needs_you() {
        let errands = [
            row("i1", "running", json!({})),
            row("i2", "needs-you", json!({})),
            row("i3", "done", json!({})),
            row("i4", "needs-you", json!({ "pinned": true })),
            row("i5", "needs-you", json!({ "archived": true })),
        ];
        let ids = |list: Vec<Errand>| list.into_iter().map(|errand| errand.id).collect::<Vec<_>>();
        assert_eq!(ids(sidebar_errands(&errands)), ["i2", "i4", "i1"]);
        assert_eq!(ids(errands_needing_you(&errands)), ["i4", "i2"]);
        assert_eq!(overview_title(2).as_deref(), Some("2 errands need you"));
        assert_eq!(overview_title(1).as_deref(), Some("1 errand needs you"));
        assert_eq!(overview_title(0), None);
        let many: Vec<Errand> = (1..=8).map(|n| row(&format!("i{n}"), "running", json!({}))).collect();
        assert_eq!(sidebar_errands(&many).len(), SIDEBAR_ERRANDS);
        let found = search_errands(
            &[
                row("i1", "done", json!({ "title": "Dinner Friday" })),
                row("i2", "done", json!({})),
            ],
            "friday DIN",
        );
        assert_eq!(ids(found), ["i1"]);
        assert_eq!(search_errands(&errands, "  ").len(), errands.len());
    }

    /// The thread: the goal first, the steps and the turns by time, a day line where the day changes.
    #[test]
    fn an_errand_reads_as_its_thread() {
        let mut body = view();
        body["thread"] = json!([
            { "role": "user", "text": "book it", "at": "2026-11-18T07:00:00Z" },
            { "role": "assistant", "text": "Booked.", "at": "2026-11-18T07:01:00Z",
              "messages": [{ "kind": "answer", "text": "Booked." }, { "kind": "later" }, 3] },
            { "role": "system", "text": "x", "at": "y" }
        ]);
        let view = errand_view_of(&body).expect("reads");
        assert_eq!(view.thread.len(), 2);
        assert_eq!(view.thread[1].messages.len(), 1);
        let kinds: Vec<&str> = entries(&view)
            .iter()
            .map(|entry| match entry {
                Entry::Day(_) => "day",
                Entry::You { .. } => "you",
                Entry::Step { .. } => "step",
                Entry::Nexus { .. } => "nexus",
            })
            .collect();
        assert_eq!(kinds, ["day", "you", "step", "day", "you", "nexus"]);
        assert_eq!(reply_placeholder(&view.errand), "Reply about Dinner Friday");
        let long = row("i9", "running", json!({ "title": "x".repeat(60) }));
        assert_eq!(reply_placeholder(&long).chars().count(), "Reply about ".len() + 48);
        assert_eq!(state_text(&view), "Partly done");
    }

    /// The hand-off reads strictly, and its draft link reads back as the desktop's prompt link.
    #[test]
    fn a_hand_off_reads_strictly_and_opens_a_draft_that_never_sends() {
        let body = json!({ "prompt": "Continue my errand.\nGoal: fix a+b", "repo": "acme/cortex-sim" });
        let handoff = handoff_of(&body).expect("reads");
        assert_eq!(handoff.repo.as_deref(), Some("acme/cortex-sim"));
        let link = handoff_link(&handoff);
        assert_eq!(
            link,
            "nexus://new?prompt=Continue%20my%20errand.%0AGoal%3A%20fix%20a%2Bb&repo=acme%2Fcortex-sim"
        );
        let read = crate::prompt_link::read(&link).expect("a draft link");
        assert_eq!(read.prompt, handoff.prompt);
        assert_eq!(read.repo.map(|repo| repo.path).as_deref(), Some("acme/cortex-sim"));
        assert!(read.notes.is_empty());
        let loose = handoff_of(&json!({ "prompt": "Go", "repo": "a/b/c" })).expect("reads");
        assert_eq!(loose.repo, None);
        assert_eq!(handoff_link(&loose), "nexus://new?prompt=Go");
        assert_eq!(handoff_of(&json!({ "prompt": " " })), None);
        assert_eq!(handoff_of(&json!({ "prompt": "x".repeat(HANDOFF_CHARS + 1) })), None);
        assert_eq!(handoff_of(&json!({ "repo": "a/b" })), None);
    }

    #[test]
    fn the_menu_and_the_edits_say_what_they_do() {
        let mut view = errand_view_of(&view()).expect("reads");
        let words = |view: &ErrandView| menu(view).into_iter().map(|(_, word)| word).collect::<Vec<_>>();
        assert_eq!(
            words(&view),
            [
                "Rename",
                "Pin",
                "Move to project",
                "Mark done",
                "Archive",
                "Hand off to a session"
            ]
        );
        view.errand.pinned = true;
        view.errand.archived = true;
        view.actions.clear();
        assert_eq!(
            words(&view),
            ["Rename", "Unpin", "Move to project", "Move out of the archive"]
        );
        let pin = Edit {
            pinned: Some(true),
            ..Edit::default()
        };
        assert_eq!(pin.body(), r#"{"pinned":true}"#);
        assert_eq!(edited_text(&pin), "Pinned.");
        let done = Edit {
            done: true,
            ..Edit::default()
        };
        assert_eq!(done.body(), r#"{"done":true}"#);
        let errand = &view.errand;
        assert_eq!(rename(errand, " Dinner Friday "), None);
        let renamed = rename(errand, "Nopa").expect("a change");
        assert_eq!(renamed.body(), r#"{"title":"Nopa"}"#);
        assert_eq!(edited_text(&renamed), "Renamed.");
        let named = row("i1", "running", json!({ "title": "Nopa" }));
        let dropped = rename(&named, "").expect("drops it");
        assert_eq!(dropped.body(), r#"{"title":null}"#);
        assert_eq!(edited_text(&dropped), "The name is gone.");
        assert_eq!(
            rename(&named, "Goal i1").map(|edit| edit.body()).as_deref(),
            Some(r#"{"title":null}"#)
        );
        for body in [pin.body(), done.body(), renamed.body(), dropped.body()] {
            assert!(crate::agent::errand_edit(Some(&body)), "{body}");
        }
        assert!(!crate::agent::errand_edit(Some(&Edit::default().body())));

        // Move to project files it, and Keep loose takes it out.
        let moved = Edit {
            project: Some(Some("Fundraise".into())),
            ..Edit::default()
        };
        assert_eq!(moved.body(), r#"{"project":"Fundraise"}"#);
        assert_eq!(edited_text(&moved), "Moved to Fundraise.");
        let loose = Edit {
            project: Some(None),
            ..Edit::default()
        };
        assert_eq!(loose.body(), r#"{"project":null}"#);
        assert_eq!(edited_text(&loose), "Loose again, under the Assistant.");
        for body in [moved.body(), loose.body()] {
            assert!(crate::agent::errand_edit(Some(&body)), "{body}");
        }
        let filed = |project: Value| row("i1", "running", json!({ "project": project })).project;
        assert_eq!(filed(json!("Fundraise")).as_deref(), Some("Fundraise"));
        assert_eq!(filed(json!("  ")), None);
        assert_eq!(filed(json!(3)), None);
    }

    #[test]
    fn a_reply_and_a_new_errand_read_strictly() {
        let body = message_body("ask Lattice for Thursday");
        assert!(crate::agent::errand_message(Some(&body)));
        let reply = errand_reply_of(&json!({ "messages": [{ "kind": "notice", "text": "On it." }], "errand": view() }))
            .expect("a reply");
        assert_eq!(reply.messages.len(), 1);
        assert!(reply.errand.is_some());
        assert_eq!(errand_reply_of(&json!({ "errand": view() })), None);
        let opened = errand_opened_of(&json!({ "state": "opened", "errand": view() }));
        assert_eq!(opened.map(|view| view.errand.id).as_deref(), Some("i3"));
        assert_eq!(errand_opened_of(&json!({ "state": "done", "errand": view() })), None);
        let goal = goal_body(&format!("  {}  ", "g".repeat(600)), None);
        assert!(crate::agent::errand_goal(Some(&goal)));
        // Ask Nexus on a project's page starts it there; a name the API would refuse is left out.
        let filed = goal_body("Book the venue", Some("  Fundraise  "));
        assert_eq!(filed, r#"{"goal":"Book the venue","project":"Fundraise"}"#);
        assert!(crate::agent::errand_goal(Some(&filed)));
        assert_eq!(goal_body("Book it", Some("   ")), r#"{"goal":"Book it"}"#);
        assert_eq!(
            open_problem(409, Some(&json!({ "error": "too_many", "detail": "" })), "Ada").title,
            "Ada has too many errands open"
        );
        assert_eq!(open_problem(400, None, "Ada").title, "That errand did not start");
        assert_eq!(
            acted_text(
                &Acted::Confirmed(Box::new(errand_view_of(&view()).expect("reads"))),
                "Ada"
            ),
            "Confirmed. Ada works on it now."
        );
    }
}
