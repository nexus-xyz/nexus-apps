//! What a Code session shows under its transcript while it runs here, as the Nexus app's session dock
//! (`session.tsx`):
//!
//! - A request for a secret (`request_secret`, `secret-request-card.tsx`), first: `secret_card.rs`.
//! - A question from the agent (`AskUserQuestion`, `question.tsx`): its options, one or many, and an answer of your
//!   own. Submit answer sends them; Skip tells the agent you skipped it.
//! - A plan it wants to start on (`ExitPlanMode`): the plan and the canon's four answers (`PLAN_ANSWERS`): three yeses
//!   that approve it and set the session's mode, and No, keep planning, with what to change. Implement in a new session
//!   stays as a fifth, quieter answer.
//! - Any other request to use a tool (`permission_queue.rs`): Allow, Allow for this session, Deny; the agent's rules
//!   behind Allow always…, and a reason behind Deny with a reason….
//! - Tool requests form an expandable queue above the composer. Each decision targets one request.
//! - Otherwise the composer, with the messages queued behind the running turn above it (`queued-chips.tsx`), and the
//!   session's model, effort and permission mode as chips in its toolbar (`effort-menu.tsx`).

use std::collections::{HashMap, HashSet};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::api::UpdateArgs;
use nexus_workbench::event::{Decision, Effort, LimitStop, PermissionMode, PermissionSuggestion, SessionAgent};
use nexus_workbench::limit_stop;
use serde_json::{Map, Value};

use super::code_view::CodeView;
use super::queue_drag::{QueuedDrag, grip};
use super::{View, Workspace, glyph, transcript};
use crate::code::{Block, CodeSession, Todo, TodoStatus};
use crate::i18n::tr;
use crate::motion;
use crate::palette::Palette;

#[path = "effort.rs"]
pub(super) mod effort;

/// The tool whose approval carries the agent's questions.
pub(super) const QUESTION_TOOL: &str = "AskUserQuestion";
/// The tool whose approval asks to leave plan mode with a plan.
pub(super) const PLAN_TOOL: &str = "ExitPlanMode";
/// The canon's answers to a plan (`canon_transcript.py`, Claude's plan approval), each with the mode it sets. The first
/// is the primary one.
pub(super) const PLAN_ANSWERS: [(&str, PermissionMode); 3] = [
    ("Yes, and use auto mode", PermissionMode::Auto),
    ("Yes, auto-accept edits", PermissionMode::AcceptEdits),
    ("Yes, manually approve edits", PermissionMode::Default),
];
/// The plan's last answer: deny it, with what to change.
pub(super) const KEEP_PLANNING: &str = "No, keep planning";
/// How many questions one request carries at most (Claude Code's limit): one answer field each.
const QUESTIONS: usize = 4;
/// What a skipped question tells the agent (`question.tsx`).
const SKIPPED: &str = "The person skipped the question.";

/// The permission modes in the menu's order (`effort-menu.tsx`'s `SHIELD_ORDER`), with the Nexus app's names and
/// what each does (`contract.ts`'s `PERMISSION_MODES`).
pub(crate) const MODES: [(PermissionMode, &str, &str); 5] = [
    (PermissionMode::Default, "Ask first", "Asks before edits and commands"),
    (
        PermissionMode::AcceptEdits,
        "Accept edits",
        "Edits without asking, asks before commands",
    ),
    (PermissionMode::Auto, "Auto", "Runs commands and tools without asking"),
    (
        PermissionMode::BypassPermissions,
        "Full access",
        "Never asks. Best in a sandbox or worktree",
    ),
    (PermissionMode::Plan, "Plan only", "Reads and plans, changes nothing"),
];

pub(crate) fn permission_copy(agent: SessionAgent, mode: PermissionMode) -> (&'static str, &'static str) {
    agent
        .permission_settings()
        .map(|settings| (settings, "The agent decides when to ask"))
        .unwrap_or_else(|| {
            let (_, label, hint) = MODES
                .iter()
                .copied()
                .find(|(each, ..)| *each == mode)
                .unwrap_or(MODES[0]);
            (label, hint)
        })
}

/// The reasoning levels (`contract.ts`'s `EFFORTS`), without the model's default.
pub(super) const EFFORTS: [(Effort, &str); 8] = [
    (Effort::Disabled, "None"),
    (Effort::Minimal, "Minimal"),
    (Effort::Low, "Low"),
    (Effort::Medium, "Medium"),
    (Effort::High, "High"),
    (Effort::Xhigh, "XHigh"),
    (Effort::Max, "Max"),
    (Effort::Ultra, "Ultra"),
];

/// The dock's menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Menu {
    /// The approval's "Always allow" rules.
    Always,
    /// The session's model.
    Model,
    /// The session's effort and permission mode.
    Effort,
}

/// The dock's own state, kept with the open session: the text fields, a question's picks, the open menu.
pub(super) struct Dock {
    pub(super) tray_layout: crate::command_tray::Layout,
    /// The reason sent with Deny.
    pub(super) reason: Entity<InputState>,
    /// What to change in a plan, sent with No, keep planning.
    change: Entity<InputState>,
    /// Each question's answer of your own, by the question's place.
    others: Vec<Entity<InputState>>,
    /// The question request whose fields have been cleared and masked.
    questions_for: Option<String>,
    /// The rewrite of a queued message.
    edit: Entity<InputState>,
    /// The queued message being rewritten (its `seq`).
    editing: Option<i64>,
    /// Queued messages sent now or taken back whose request has not answered: a second press does nothing. A failed
    /// request clears them (`Workspace::on_code_events`).
    pub(super) acting: HashSet<i64>,
    /// The options picked, by question.
    picked: HashMap<String, Vec<String>>,
    /// The questions answered (also) with your own words.
    custom: HashMap<String, bool>,
    /// The request the picks are for: another request starts them again.
    draft_for: Option<String>,
    /// The question on the card when there are several; `questions.len()` is the review.
    step: usize,
    /// The step changed: the keys go back to the card, off the field the last step hid.
    refocus: bool,
    pub(super) permissions: super::permission_queue::PermissionQueue,
    /// The plan waiting for an answer, as Markdown: its request and its view.
    plan: Option<(String, Entity<TextViewState>)>,
    /// The plan bar above the composer shows its whole list.
    todos_open: bool,
    pub(super) menu: Option<Menu>,
    /// The agent's request for a secret (`secret_card.rs`).
    pub(super) secret: super::secret_card::SecretCard,
    /// The keys that answer a tool approval (`approval_keys.rs`).
    pub(super) keys: super::approval_keys::ApprovalKeys,
    _subscriptions: Vec<Subscription>,
}

impl Dock {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Dock {
        let reason =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("Tell the agent what to do instead (optional)")));
        let change =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("Tell the agent what to change (optional)")));
        let edit = cx.new(|cx| InputState::new(window, cx));
        let others: Vec<_> = (0..QUESTIONS)
            .map(|_| cx.new(|cx| InputState::new(window, cx).placeholder(tr("Write your answer…"))))
            .collect();
        let mut subscriptions = vec![
            cx.subscribe_in(&reason, window, |this, _, event: &InputEvent, window, cx| {
                // Enter in the reason denies, but not in the moment after the card appears (`approval_keys.rs`).
                if let InputEvent::PressEnter { shift: false, .. } = event
                    && !this.approval_arming()
                {
                    this.deny_with_reason(window, cx);
                }
            }),
            cx.subscribe_in(&change, window, |this, state, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event
                    && !state.read(cx).value().trim().is_empty()
                {
                    this.keep_planning(window, cx);
                }
            }),
            cx.subscribe_in(&edit, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.save_queued(window, cx);
                }
            }),
        ];
        let (secret, typed) = super::secret_card::SecretCard::new(window, cx);
        subscriptions.push(typed);
        for (ix, other) in others.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(
                other,
                window,
                move |this, _, event: &InputEvent, window, cx| match event {
                    // Typing an answer of your own picks it.
                    InputEvent::Change => this.pick_custom(ix, true, cx),
                    // Enter goes on: to the next question, or from the last one it submits.
                    InputEvent::PressEnter { shift: false, .. } => this.question_enter(window, cx),
                    _ => {}
                },
            ));
        }
        Dock {
            tray_layout: crate::command_tray::Layout::default(),
            reason,
            change,
            others,
            questions_for: None,
            edit,
            editing: None,
            acting: HashSet::new(),
            picked: HashMap::new(),
            custom: HashMap::new(),
            draft_for: None,
            step: 0,
            refocus: false,
            permissions: Default::default(),
            plan: None,
            todos_open: false,
            menu: None,
            secret,
            keys: super::approval_keys::ApprovalKeys::new(cx),
            _subscriptions: subscriptions,
        }
    }

    /// Follow the request the session waits on: a new one starts the picks again, and a plan gets its Markdown view.
    pub fn follow(&mut self, pending: Option<&Block>, cx: &mut App) {
        let Some(Block::Approval {
            request_id,
            tool,
            input,
            ..
        }) = pending
        else {
            self.draft_for = None;
            self.plan = None;
            self.keys.gone();
            return;
        };
        if self.draft_for.as_ref() != Some(request_id) {
            self.draft_for = Some(request_id.clone());
            self.picked.clear();
            self.custom.clear();
            self.step = 0;
            self.menu = None;
            // A question replaces the composer: the card takes the keys, so its number keys work at once.
            self.refocus = tool == QUESTION_TOOL;
            if tool != QUESTION_TOOL && tool != PLAN_TOOL {
                self.keys.shown();
            } else {
                self.keys.gone();
            }
        }
        if tool == PLAN_TOOL {
            if self.plan.as_ref().is_none_or(|(id, _)| id != request_id) {
                let body = plan_body(plan_of(input));
                self.plan = Some((request_id.clone(), cx.new(|cx| TextViewState::markdown(&body, cx))));
            }
        } else {
            self.plan = None;
        }
    }
}

/// One question the agent asks (`questions.ts`'s `Question`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Question {
    /// Codex's stable id; Claude Code's questions have none and are keyed by their text.
    pub(super) id: Option<String>,
    pub(super) question: String,
    /// Claude Code's short tab label for the question.
    pub(super) header: Option<String>,
    pub(super) multi: bool,
    pub(super) secret: bool,
    pub(super) options: Vec<(String, Option<String>)>,
}

impl Question {
    fn key(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.question)
    }
}

/// The questions in a request's input (`questionsOf`).
pub(super) fn questions_of(input: &Value) -> Vec<Question> {
    let Some(list) = input.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| {
            let question = item.get("question")?.as_str()?.to_string();
            let options = item
                .get("options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .filter_map(|option| {
                            let label = option.get("label")?.as_str()?.to_string();
                            let description = option.get("description").and_then(Value::as_str).map(String::from);
                            Some((label, description))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Question {
                id: item.get("id").and_then(Value::as_str).map(String::from),
                question,
                header: item.get("header").and_then(Value::as_str).map(String::from),
                multi: item.get("multiSelect").and_then(Value::as_bool) == Some(true),
                secret: item.get("isSecret").and_then(Value::as_bool) == Some(true),
                options,
            })
        })
        .collect()
}

/// A question's answer: the options picked, then your own words (`questionValues`). One pick alone drops the options
/// when your own words are picked.
fn question_values(question: &Question, picked: &[String], custom: bool, other: &str) -> Vec<String> {
    let mut values = if !question.multi && custom {
        Vec::new()
    } else {
        picked.to_vec()
    };
    let other = other.trim();
    if custom && !other.is_empty() {
        values.push(other.to_string());
    }
    values
}

/// The answers the agent reads (`questionAnswers`): Claude Code's by question text with the labels joined, Codex's by
/// id as a list.
pub(super) fn question_answers(questions: &[Question], values: &[Vec<String>]) -> Map<String, Value> {
    questions
        .iter()
        .zip(values)
        .map(|(question, values)| {
            let value = match question.id {
                None => Value::String(values.join(", ")),
                Some(_) => Value::Array(values.iter().cloned().map(Value::String).collect()),
            };
            (question.key().to_string(), value)
        })
        .collect()
}

/// The receipt the log keeps (`questionReceipt`).
fn question_receipt(questions: &[Question], values: &[Vec<String>]) -> String {
    questions
        .iter()
        .zip(values)
        .map(|(question, values)| {
            let answer = if question.secret {
                "Private answer submitted".to_string()
            } else {
                values.join(", ")
            };
            if questions.len() == 1 {
                answer
            } else {
                format!("{}\n{answer}", question.question)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Next goes to the first unanswered question after `from`, else to the review (`nextStep`).
fn next_step(values: &[Vec<String>], from: usize) -> usize {
    (from + 1..values.len())
        .find(|ix| values[*ix].is_empty())
        .unwrap_or(values.len())
}

/// The receipt read back as one row per question (`receiptRows`), or None when it does not match the questions.
pub(super) fn receipt_rows(questions: &[Question], message: &str) -> Option<Vec<(String, String)>> {
    match questions {
        [] => None,
        [only] => Some(vec![(only.question.clone(), message.to_string())]),
        _ => {
            let blocks: Vec<&str> = message.split("\n\n").collect();
            if blocks.len() != questions.len() {
                return None;
            }
            questions
                .iter()
                .zip(blocks)
                .map(|(question, block)| {
                    let answer = block.strip_prefix(question.question.as_str())?.strip_prefix('\n')?;
                    Some((question.question.clone(), answer.to_string()))
                })
                .collect()
        }
    }
}

/// The plan's Markdown in a call's input (`planOf`).
pub(super) fn plan_of(input: &Value) -> &str {
    input.get("plan").and_then(Value::as_str).unwrap_or("")
}

/// The plan's first heading, or its first line, without the Markdown (`planTitle`).
pub(super) fn plan_title(plan: &str) -> Option<String> {
    let lines: Vec<&str> = plan.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let line = lines.iter().find(|line| heading(line).is_some()).or(lines.first())?;
    let mut title = heading(line).unwrap_or(line).trim();
    for prefix in ["proposed plan", "plan"] {
        if title.len() > prefix.len()
            && title.is_char_boundary(prefix.len())
            && title[..prefix.len()].eq_ignore_ascii_case(prefix)
        {
            let rest = title[prefix.len()..].trim_start();
            if let Some(rest) = rest.strip_prefix([':', '-', '–', '—']) {
                title = rest.trim();
            }
        }
    }
    let title: String = title.chars().filter(|c| !matches!(c, '*' | '_' | '`')).collect();
    let title = title.trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// A Markdown heading's text (`# Title`), or None for another line.
fn heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    ((1..=6).contains(&hashes) && line[hashes..].starts_with(char::is_whitespace)).then(|| line[hashes..].trim())
}

/// The plan under its title: without the heading the title came from, when that heading leads (`planBody`).
fn plan_body(plan: &str) -> String {
    let lines: Vec<&str> = plan.lines().collect();
    let Some(first) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return plan.to_string();
    };
    if heading(lines[first].trim()).is_none() {
        return plan.to_string();
    }
    lines[first + 1..]
        .iter()
        .skip_while(|line| line.trim().is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// One "Always allow" rule in plain words (`suggestions.ts`'s `describe`): what it allows, and for how long.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SuggestionView {
    /// Its index in the request, sent back when picked.
    pub index: usize,
    pub title: String,
    /// The words of the title to show in code type: a command.
    pub code: Option<String>,
    pub scope: &'static str,
}

/// The request's rules, described, with deny rules and repeats left out (`describeAll`): picking a row allows this
/// request, so a deny rule would say the opposite of what it does.
pub(super) fn describe_all(suggestions: &[PermissionSuggestion]) -> Vec<SuggestionView> {
    let mut seen = Vec::new();
    suggestions
        .iter()
        .enumerate()
        .filter(|(_, suggestion)| suggestion.raw.get("behavior").and_then(Value::as_str) != Some("deny"))
        .map(|(index, suggestion)| describe(suggestion, index))
        .filter(|view| {
            let key = (view.title.clone(), view.code.clone(), view.scope);
            let new = !seen.contains(&key);
            seen.push(key);
            new
        })
        .collect()
}

fn describe(suggestion: &PermissionSuggestion, index: usize) -> SuggestionView {
    let raw = &suggestion.raw;
    let scope = match raw.get("destination").and_then(Value::as_str) {
        Some("session") => "For the rest of this session",
        Some("localSettings") => "In this project, from now on, just for you",
        Some("projectSettings") => "In this project, from now on, for everyone",
        Some("userSettings") => "In every project, from now on",
        _ => "From now on",
    };
    let kind = raw.get("type").and_then(Value::as_str).unwrap_or(&suggestion.kind);
    let view = |title: String, code: Option<String>| SuggestionView {
        index,
        title,
        code,
        scope,
    };
    match kind {
        "addRules" | "replaceRules" => {
            let rules = raw.get("rules").and_then(Value::as_array).cloned().unwrap_or_default();
            match rules.as_slice() {
                [rule] => {
                    let (title, code) = rule_view(rule);
                    return view(title, code);
                }
                [_, _, ..] => {
                    let title = rules
                        .iter()
                        .map(|rule| match rule_view(rule) {
                            (title, Some(code)) => format!("{title} {code}"),
                            (title, None) => title,
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    return view(title, None);
                }
                [] => {}
            }
        }
        "setMode" => {
            let title = match raw.get("mode").and_then(Value::as_str) {
                Some("acceptEdits") => "Every file edit".to_string(),
                Some("auto") => "Auto mode".to_string(),
                Some("bypassPermissions") => "Everything, without asking".to_string(),
                Some(mode) => format!("{mode} mode"),
                None => "Another mode".to_string(),
            };
            return view(title, None);
        }
        "addDirectories" => {
            let dirs: Vec<String> = raw
                .get("directories")
                .and_then(Value::as_array)
                .map(|dirs| dirs.iter().filter_map(Value::as_str).map(tidy_home).collect())
                .unwrap_or_default();
            let title = if dirs.is_empty() {
                "Working in another folder".to_string()
            } else {
                format!("Working in {}", dirs.join(", "))
            };
            return view(title, None);
        }
        _ => {}
    }
    SuggestionView {
        index,
        title: suggestion.label.clone(),
        code: None,
        scope: "",
    }
}

/// One rule (`ruleView`): `Commands starting with npm test`, `Reading ~/src`, `Fetching from example.com`.
fn rule_view(rule: &Value) -> (String, Option<String>) {
    let tool = rule.get("toolName").and_then(Value::as_str).unwrap_or("");
    let content = rule.get("ruleContent").and_then(Value::as_str).unwrap_or("").trim();
    match tool {
        "Bash" if content.is_empty() => ("Every command".into(), None),
        "Bash" => match content.strip_suffix(":*") {
            Some(prefix) => ("Commands starting with".into(), Some(prefix.to_string())),
            None => ("The command".into(), Some(content.to_string())),
        },
        "WebFetch" => match content.trim_start_matches("domain:") {
            "" => ("Fetching any page".into(), None),
            domain => (format!("Fetching from {domain}"), None),
        },
        "WebSearch" => ("Searching the web".into(), None),
        "Read" | "Glob" | "Grep" | "LS" | "NotebookRead" => match content {
            "" => ("Reading any file".into(), None),
            path => (format!("Reading {}", tidy_path(path)), None),
        },
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => match content {
            "" => ("Editing any file".into(), None),
            path => (format!("Editing {}", tidy_path(path)), None),
        },
        tool if tool.starts_with("mcp__") => {
            let mut parts = tool.split("__").skip(1);
            let server = parts.next().unwrap_or("");
            match parts.next() {
                Some(name) => (format!("{} on {server}", name.replace('_', " ")), None),
                None => (format!("Every {server} tool"), None),
            }
        }
        tool => {
            let tool = if tool.is_empty() { "tool" } else { tool };
            if content.is_empty() {
                (format!("Every {tool} request"), None)
            } else {
                (format!("{tool} on {content}"), None)
            }
        }
    }
}

/// A path with the home folder as `~` (`tidyHome`).
fn tidy_home(path: &str) -> String {
    for root in ["/Users/", "/home/"] {
        if let Some(rest) = path.strip_prefix(root) {
            return match rest.find('/') {
                Some(slash) => format!("~{}", &rest[slash..]),
                None => "~".into(),
            };
        }
    }
    if path.is_empty() { "/".into() } else { path.into() }
}

/// A path in Claude Code's rule syntax as a person reads it (`tidyPath`): `//abs/**` is absolute, `/rel` is relative.
fn tidy_path(content: &str) -> String {
    let bare = content.trim_end_matches("/**").trim_end_matches("/*");
    if let Some(absolute) = bare.strip_prefix("//") {
        return tidy_home(&format!("/{absolute}"));
    }
    if let Some(relative) = bare.strip_prefix('/') {
        return if relative.is_empty() {
            ".".into()
        } else {
            relative.into()
        };
    }
    if bare.is_empty() { "/".into() } else { bare.into() }
}

/// The agent's name on the cards.
pub(super) fn agent_name(agent: SessionAgent) -> &'static str {
    crate::agents::ui(agent).short_name()
}

/// The Stop button's tooltip while a turn runs (`interrupt.ts`'s `interruptTitle`).
pub(super) fn interrupt_title(agent: SessionAgent) -> &'static str {
    crate::agents::ui(agent).interrupt_title()
}

/// A model id without its maker's prefix (`anthropic/claude-opus-5-5` as `claude-opus-5-5`).
fn raw_id(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}

/// The chip label for a session's model: match the agent's catalog by id or resolved id so a routed provider id like
/// `claude-opus-5-5` reads "Opus 5.5" even when the catalog entry's id is the alias `opus`.
fn session_model_label(catalog: &[nexus_workbench::agent_models::Model], id: &str) -> String {
    let target = raw_id(id);
    let matches = |model: &&nexus_workbench::agent_models::Model| {
        raw_id(&model.id) == target
            || model
                .resolved_id
                .as_deref()
                .is_some_and(|resolved| raw_id(resolved) == target)
    };
    catalog
        .iter()
        .filter(|model| !crate::code::is_default_entry(model))
        .find(matches)
        .map(|model| model.name.clone())
        .or_else(|| {
            catalog
                .iter()
                .filter(|model| crate::code::is_default_entry(model))
                .find(matches)
                .and_then(|_| crate::code::default_model_name(catalog))
        })
        .unwrap_or_else(|| crate::code::model_label(target))
}

impl Workspace {
    /// Clear before unmasking a reused field, so a previous private answer never flashes in another question.
    pub(super) fn prepare_question_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, request, tool, input)) = self.pending_request() else {
            return;
        };
        if tool != QUESTION_TOOL {
            return;
        }
        let Some(dock) = self.dock_mut() else { return };
        if std::mem::take(&mut dock.refocus) {
            let card = dock.keys.card.clone();
            window.defer(cx, move |window, cx| window.focus(&card, cx));
        }
        if dock.questions_for.as_deref() == Some(&request) {
            return;
        }
        dock.questions_for = Some(request);
        let others = dock.others.clone();
        let questions = questions_of(&input);
        for (ix, other) in others.into_iter().enumerate() {
            let secret = questions.get(ix).is_some_and(|question| question.secret);
            other.update(cx, |state, cx| {
                state.set_value("", window, cx);
                state.set_masked(secret, window, cx);
            });
        }
    }

    /// The open Code session's summary.
    pub(super) fn code_session(&self) -> Option<&CodeSession> {
        let View::Code(view) = &self.view else {
            return None;
        };
        self.session_by_id(&view.id)
    }

    /// Marks queued message `seq` as being sent now or taken back. False when a request for it is already on its way.
    fn begin_queue_action(&mut self, seq: i64) -> bool {
        self.dock_mut().is_some_and(|dock| dock.acting.insert(seq))
    }

    fn dock_mut(&mut self) -> Option<&mut Dock> {
        match &mut self.view {
            View::Code(view) => Some(&mut view.dock),
            _ => None,
        }
    }

    /// The request the open session waits on: its thread, its id, its tool and input.
    fn pending_request(&self) -> Option<(String, String, String, Value)> {
        let View::Code(view) = &self.view else {
            return None;
        };
        let pending = view.dock.permissions.current(&view.blocks);
        match pending {
            Some(Block::Approval {
                request_id,
                tool,
                input,
                ..
            }) => Some((view.id.clone(), request_id.clone(), tool.clone(), input.clone())),
            _ => None,
        }
    }

    /// Put the keyboard in the dock's request: a question's own answer, a plan's change, a tool's reason to deny.
    /// With no request waiting, the composer.
    pub(super) fn focus_pending_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tool = self.pending_request().map(|(_, _, tool, _)| tool);
        let field = match (tool, self.dock_mut()) {
            // The card, not a field: only the step on screen has its field, and the card's keys pick and go on.
            (Some(tool), Some(dock)) if tool == QUESTION_TOOL => {
                let card = dock.keys.card.clone();
                window.focus(&card, cx);
                return;
            }
            (Some(tool), Some(dock)) if tool == PLAN_TOOL => Some(dock.change.clone()),
            // A tool request: its Allow, so Enter answers it; the reason is a second step (Deny with a reason…).
            (Some(_), Some(dock)) => {
                let allow = dock.keys.allow.clone();
                window.focus(&allow, cx);
                return;
            }
            _ => None,
        };
        match field {
            Some(field) => field.update(cx, |state, cx| state.focus(window, cx)),
            None => self.focus_composer(window, cx),
        }
    }

    /// Answer the open session's request.
    pub(super) fn answer(
        &mut self,
        decision: Decision,
        message: Option<String>,
        suggestion: Option<usize>,
        answers: Option<Map<String, Value>>,
        cx: &mut Context<Self>,
    ) {
        let Some((thread, request, ..)) = self.pending_request() else {
            return;
        };
        if let View::Code(view) = &self.view
            && view
                .blocks
                .iter()
                .any(|block| super::permission_queue::tool_request(block) == Some(request.as_str()))
        {
            self.answer_tool_request_with_message(request, decision, suggestion, message, cx);
            return;
        }
        let Some(runs) = self.runs(&thread) else {
            return;
        };
        runs.approve(thread, request, decision, message, suggestion, answers);
        self.composer_note = None;
        if let Some(dock) = self.dock_mut() {
            dock.menu = None;
        }
        cx.notify();
    }

    /// Deny, with the reason typed, if any; the field empties.
    pub(super) fn deny_with_reason(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock) = self.dock_mut() else {
            return;
        };
        let reason = dock.reason.clone();
        let message = reason.read(cx).value().trim().to_string();
        reason.update(cx, |state, cx| state.set_value("", window, cx));
        self.answer(Decision::Deny, Some(message).filter(|m| !m.is_empty()), None, None, cx);
    }

    /// No, keep planning: deny the plan, with what to change, if anything was typed.
    fn keep_planning(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock) = self.dock_mut() else {
            return;
        };
        let change = dock.change.clone();
        let message = change.read(cx).value().trim().to_string();
        change.update(cx, |state, cx| state.set_value("", window, cx));
        self.answer(Decision::Deny, Some(message).filter(|m| !m.is_empty()), None, None, cx);
    }

    /// One of the plan's yeses: approve it, then leave plan mode for `mode`.
    fn implement_plan(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        let agent = self
            .code_session()
            .map(|session| session.agent)
            .unwrap_or(SessionAgent::Claude);
        let Some((thread, request, ..)) = self.pending_request() else {
            return;
        };
        let Some(runs) = self.runs(&thread) else {
            return;
        };
        runs.implement_plan(
            thread,
            request,
            mode,
            super::code_title::code_meter::permissions(agent, mode),
        );
        self.composer_note = None;
        cx.notify();
    }

    /// Pick or unpick an option of question `ix`.
    fn pick_option(&mut self, ix: usize, label: String, cx: &mut Context<Self>) {
        let Some((_, _, _, input)) = self.pending_request() else {
            return;
        };
        let Some(question) = questions_of(&input).into_iter().nth(ix) else {
            return;
        };
        let Some(dock) = self.dock_mut() else {
            return;
        };
        let key = question.key().to_string();
        let picked = dock.picked.entry(key.clone()).or_default();
        if question.multi {
            if let Some(at) = picked.iter().position(|picked| *picked == label) {
                picked.remove(at);
            } else {
                picked.push(label);
            }
        } else {
            *picked = vec![label];
            dock.custom.insert(key, false);
            // A single choice moves on by itself; a multiple choice waits for Next.
            self.question_next(ix, cx);
        }
        cx.notify();
    }

    /// From question `from`, go to the next unanswered one, or to the review.
    fn question_next(&mut self, from: usize, cx: &mut Context<Self>) {
        let Some((_, _, _, input)) = self.pending_request() else {
            return;
        };
        let questions = questions_of(&input);
        if questions.len() < 2 {
            return;
        }
        let values = self.question_values(&questions, cx);
        let to = next_step(&values, from);
        self.question_step(to, cx);
    }

    /// Show question `to`, or the review when `to` is the number of questions.
    fn question_step(&mut self, to: usize, cx: &mut Context<Self>) {
        let Some((_, _, _, input)) = self.pending_request() else {
            return;
        };
        let count = questions_of(&input).len();
        let Some(dock) = self.dock_mut() else {
            return;
        };
        dock.step = to.min(count);
        dock.refocus = true;
        cx.notify();
    }

    /// Enter on the question card: the next step once this one has an answer, and from the review it submits.
    fn question_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, _, _, input)) = self.pending_request() else {
            return;
        };
        let questions = questions_of(&input);
        let step = self.question_step_now(questions.len());
        if questions.len() < 2 || step >= questions.len() {
            self.submit_answers(window, cx);
            return;
        }
        let values = self.question_values(&questions, cx);
        if !values[step].is_empty() {
            self.question_next(step, cx);
        }
    }

    /// The step on the card, within `count` questions and their review.
    fn question_step_now(&self, count: usize) -> usize {
        match &self.view {
            View::Code(view) if count > 1 => view.dock.step.min(count),
            _ => 0,
        }
    }

    /// A plain key while a question shows and no text field has it: a number picks that option (the one after the
    /// options writes your own), Enter goes on, Left and Right move between steps. True when the card took the key.
    pub(super) fn question_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, _, tool, input)) = self.pending_request() else {
            return false;
        };
        if tool != QUESTION_TOOL || self.open_secret_request().is_some() {
            return false;
        }
        let View::Code(view) = &self.view else {
            return false;
        };
        let card =
            view.dock.keys.card.is_focused(window) || self.focus.is_focused(window) || window.focused(cx).is_none();
        if !card {
            return false;
        }
        let questions = questions_of(&input);
        let step = self.question_step_now(questions.len());
        match key {
            "enter" => self.question_enter(window, cx),
            "left" if questions.len() > 1 => self.question_step(step.saturating_sub(1), cx),
            "right" if questions.len() > 1 => self.question_step(step + 1, cx),
            _ => {
                let Some(question) = questions.get(step) else {
                    return false;
                };
                let Some(n) = key.parse::<usize>().ok().filter(|n| *n >= 1) else {
                    return false;
                };
                if let Some((label, _)) = question.options.get(n - 1) {
                    self.pick_option(step, label.clone(), cx);
                } else if n == question.options.len() + 1 && step < QUESTIONS {
                    let View::Code(view) = &self.view else {
                        return false;
                    };
                    let other = view.dock.others[step].clone();
                    other.update(cx, |state, cx| state.focus(window, cx));
                } else {
                    return false;
                }
            }
        }
        true
    }

    /// Pick question `ix`'s own answer (`on`), or toggle it for a question with many answers.
    fn pick_custom(&mut self, ix: usize, typed: bool, cx: &mut Context<Self>) {
        let Some((_, _, _, input)) = self.pending_request() else {
            return;
        };
        let Some(question) = questions_of(&input).into_iter().nth(ix) else {
            return;
        };
        let Some(dock) = self.dock_mut() else {
            return;
        };
        let custom = dock.custom.entry(question.key().to_string()).or_default();
        *custom = typed || !question.multi || !*custom;
        cx.notify();
    }

    /// Each question's answer as picked and typed now.
    fn question_values(&self, questions: &[Question], cx: &App) -> Vec<Vec<String>> {
        let View::Code(view) = &self.view else {
            return Vec::new();
        };
        let dock = &view.dock;
        questions
            .iter()
            .enumerate()
            .map(|(ix, question)| {
                let key = question.key();
                let picked = dock.picked.get(key).map(Vec::as_slice).unwrap_or(&[]);
                let custom = dock.custom.get(key).copied().unwrap_or(false);
                let other = dock
                    .others
                    .get(ix)
                    .map(|other| other.read(cx).value().to_string())
                    .unwrap_or_default();
                question_values(question, picked, custom, &other)
            })
            .collect()
    }

    /// Submit the answers to the agent's questions, once each has one.
    fn submit_answers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, _, _, input)) = self.pending_request() else {
            return;
        };
        let questions = questions_of(&input);
        let values = self.question_values(&questions, cx);
        if questions.is_empty() || values.iter().any(Vec::is_empty) {
            return;
        }
        let answers = question_answers(&questions, &values);
        let receipt = question_receipt(&questions, &values);
        self.clear_others(window, cx);
        self.answer(Decision::Allow, Some(receipt), None, Some(answers), cx);
    }

    fn skip_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_others(window, cx);
        self.answer(Decision::Deny, Some(SKIPPED.into()), None, None, cx);
    }

    fn clear_others(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock) = self.dock_mut() else {
            return;
        };
        for other in dock.others.clone() {
            other.update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    /// Open a menu, or close it. Its chip calls this through `menu_trigger`.
    pub(super) fn toggle_dock_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        let Some(dock) = self.dock_mut() else {
            return;
        };
        dock.menu = if dock.menu == Some(menu) { None } else { Some(menu) };
        cx.notify();
    }

    /// Whether the open session's dock shows this menu.
    pub(super) fn dock_menu_is(&self, menu: Menu) -> bool {
        matches!(&self.view, View::Code(view) if view.dock.menu == Some(menu))
    }

    /// Open a menu from the slash menu (`/model`, `/effort`): as its chip does, the model's with its search ready.
    pub(super) fn open_dock_menu(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock) = self.dock_mut() else {
            return;
        };
        dock.menu = Some(menu);
        if menu == Menu::Model {
            self.prepare_model_search(window, cx);
        }
        cx.notify();
    }

    pub(super) fn close_dock_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(dock) = self.dock_mut()
            && dock.menu.take().is_some()
        {
            cx.notify();
        }
    }

    /// Change the open session's model, effort or mode.
    pub(super) fn update_session(
        &mut self,
        model: Option<Option<String>>,
        effort: Option<Option<Effort>>,
        mode: Option<PermissionMode>,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.code_session().map(|session| session.id.clone())
            && let Some(runs) = self.runs(&id)
        {
            if let Some(session) = self.code_session() {
                let values = session.picker_values();
                let agent = session.agent;
                let pair = (model.is_some() || effort.is_some())
                    .then(|| (model.clone().unwrap_or(values.model), effort.unwrap_or(values.effort)));
                self.remember_session_pick(agent, pair, mode);
            }
            runs.update(id, model, effort, mode);
        }
        if let Some(dock) = self.dock_mut() {
            dock.menu = None;
        }
        cx.notify();
    }

    /// Turn the open session's fast mode on or off (`wb_thread_update`). A running agent takes it at its next model call.
    fn set_session_fast(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(id) = self.code_session().map(|session| session.id.clone())
            && let Some(runs) = self.runs(&id)
        {
            runs.update_session(UpdateArgs {
                thread_id: id,
                fast: Some(on),
                ..UpdateArgs::default()
            });
        }
        if let Some(dock) = self.dock_mut() {
            dock.menu = None;
        }
        cx.notify();
    }

    /// Whether the session's model has fast mode: the model picked, else the one the agent reported running, else the one
    /// Default runs. Only Claude Code has it (`AgentUi::fast_mode`).
    pub(super) fn session_has_fast(&self, session: &CodeSession) -> bool {
        if !crate::agents::ui(session.agent).fast_mode() {
            return false;
        }
        let model = session
            .picker_values()
            .model
            .or_else(|| {
                session
                    .connection
                    .as_ref()
                    .and_then(|connection| connection.model.clone())
            })
            .or_else(|| {
                self.agent_catalog(session.agent)
                    .iter()
                    .find(|model| crate::code::is_default_entry(model))
                    .and_then(|model| model.resolved_id.clone())
            });
        model.is_some_and(|model| nexus_workbench::claude::supports_fast(&model))
    }

    /// Start rewriting queued message `seq` in place.
    fn edit_queued(&mut self, seq: i64, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock) = self.dock_mut() else {
            return;
        };
        dock.editing = Some(seq);
        let edit = dock.edit.clone();
        edit.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// Save the rewrite of the queued message; an empty one is not saved (take it back instead).
    fn save_queued(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let runs = match &self.view {
            View::Code(view) => self.runs(&view.id),
            _ => None,
        };
        let View::Code(view) = &mut self.view else {
            return;
        };
        let Some(seq) = view.dock.editing.take() else {
            return;
        };
        let text = view.dock.edit.read(cx).value().to_string();
        let before = view.blocks.iter().find_map(|block| match block {
            Block::Queued { seq: queued, text } if *queued == seq => Some(text.clone()),
            _ => None,
        });
        if !text.trim().is_empty()
            && before.is_some_and(|before| before.trim() != text.trim())
            && let Some(runs) = runs
        {
            runs.queue_edit(view.id.clone(), seq, text);
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Under the session while nothing waits for an answer: the composer, with what waits for the agent at its top
    /// ([`Self::render_code_on_queue`]).
    pub(super) fn render_code_composer(&self, view: &CodeView, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let plan = view.blocks.iter().find_map(|block| match block {
            Block::Plan(todos) => Some(todos.as_slice()),
            _ => None,
        });
        v_flex()
            .w_full()
            .gap(px(8.))
            .when_some(plan, |dock, todos| {
                dock.child(render_todo_bar(todos, view.dock.todos_open, palette, cx))
            })
            .children(self.render_crew_line(&view.id, palette, cx))
            .when_some(self.code_session().and_then(|session| session.limited), |dock, stop| {
                dock.child(self.render_limit_stop(view, stop, palette, cx))
            })
            .children(self.render_continue_bar(view, palette, cx))
            .children(self.render_waiting_messages(palette, cx))
            .children(self.render_usage_warning(palette, cx))
            .child(self.render_composer(palette, cx))
            .into_any_element()
    }

    /// The row over the composer when the person's plan stopped the turn at its usage limit (`composer.tsx`'s
    /// `LimitBar`): what happened and when it resets, Continue now, and Continue at reset (or Turn off) when the plan
    /// said when. The text and when it shows are `nexus_workbench::limit_stop`'s, shared with the Nexus app.
    fn render_limit_stop(
        &self,
        view: &CodeView,
        stop: LimitStop,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = limit_stop::text(stop, chrono::Utc::now().timestamp_millis(), &chrono::Local);
        let (now, schedule) = (view.id.clone(), view.id.clone());
        let scheduling = stop.resets_at.map(|_| {
            let on = !stop.scheduled;
            let button = if on {
                action_button(
                    "limit-schedule",
                    Some(Lucide::Clock),
                    tr("Continue at reset"),
                    false,
                    palette,
                )
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(limit_stop::SCHEDULE_HINT).build(window, cx)
                })
            } else {
                action_button("limit-schedule", None, tr("Turn off"), false, palette)
            };
            button.on_click(cx.listener(move |this, _, _, cx| {
                if let Some(runs) = this.runs(&schedule) {
                    runs.update_session(UpdateArgs {
                        thread_id: schedule.clone(),
                        continue_at_reset: Some(on),
                        ..UpdateArgs::default()
                    });
                }
                cx.notify();
            }))
        });
        super::interrupt_bars::bar_row("limit-stop", palette)
            .child(div().flex_1().min_w(px(0.)).child(text))
            .child(h_flex().flex_none().gap(px(8.)).children(scheduling).child(
                action_button("limit-continue", Some(Lucide::Play), tr("Continue now"), true, palette).on_click(
                    cx.listener(move |this, _, _, cx| {
                        // As the Nexus app's Continue: the composer's draft stays where it is.
                        if let Some(runs) = this.runs(&now) {
                            runs.send(
                                now.clone(),
                                nexus_workbench::CONTINUE.to_string(),
                                Default::default(),
                                false,
                            );
                        }
                        cx.notify();
                    }),
                ),
            ))
            .into_any_element()
    }

    /// What waits for the agent, at the composer's top, one "On queue" row each: first the messages sent into the
    /// running turn, which the agent reads at its next step and which can no longer be taken back, then the ones
    /// queued behind the turn, which can go in now, be rewritten or be taken back. Each leaves for the transcript
    /// where the agent reads it, with its Read.
    pub(super) fn render_code_on_queue(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let View::Code(view) = &self.view else {
            return None;
        };
        let sent = self.sent_on_queue();
        let queued: Vec<(i64, &str)> = view
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Queued { seq, text } => Some((*seq, text.as_str())),
                _ => None,
            })
            .collect();
        if sent.is_empty() && queued.is_empty() {
            return None;
        }
        let agent: SharedString = self
            .code_session()
            .map(|session| nexus_workbench::driver::label(session.agent).into())
            .unwrap_or_else(|| tr("The agent"));
        let next_step: SharedString = tr("Sent: {agent} reads it at its next step")
            .replace("{agent}", &agent)
            .into();
        let sent_rows = sent.iter().map(|message| {
            let tip = next_step.clone();
            on_queue_row(
                ("on-queue-sent", message.seq as u64),
                &message.chip_text(),
                None,
                palette,
            )
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            .into_any_element()
        });
        let thread = view.id.clone();
        let queued_rows = queued.iter().enumerate().map(|(ix, &(seq, text))| {
            if view.dock.editing == Some(seq) {
                return h_flex()
                    .id(("queued", ix))
                    .w_full()
                    .min_h(px(32.))
                    .gap(px(8.))
                    .text_size(px(13.5))
                    .child(on_queue_tag(palette))
                    // The rewrite's field: a hairline box, so the row reads as one being edited.
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .px(px(8.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(palette.faint)
                            .child(Input::new(&view.dock.edit).appearance(false)),
                    )
                    .child(
                        text_button(("queued-cancel", ix), tr("Cancel"), false, palette).on_click(cx.listener(
                            |this, _, window, cx| {
                                if let Some(dock) = this.dock_mut() {
                                    dock.editing = None;
                                }
                                this.focus_composer(window, cx);
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        text_button(("queued-save", ix), tr("Save"), true, palette)
                            .on_click(cx.listener(|this, _, window, cx| this.save_queued(window, cx))),
                    )
                    .into_any_element();
            }
            let (steer, cancel, edit) = (thread.clone(), thread.clone(), text.to_string());
            let acting = view.dock.acting.contains(&seq);
            let send_now: SharedString = tr("Send now: {agent} reads it at its next step")
                .replace("{agent}", &agent)
                .into();
            // A queue of two or more reorders: drag a row's grip onto another row.
            let drag = (queued.len() > 1 && !acting).then(|| {
                let drag = QueuedDrag::new(thread.clone(), seq, queue_line(text).into(), palette);
                grip(("queued-grip", ix), drag, palette).tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tr("Drag to reorder")).build(window, cx)
                })
            });
            let target = palette.hover;
            on_queue_row(("queued", ix), text, drag, palette)
                .when(acting, |row| row.opacity(0.5))
                // The row a drop would land on lights up; the dragged row itself does not.
                .drag_over::<QueuedDrag>(move |style, drag, _, _| {
                    if drag.seq() == seq {
                        style
                    } else {
                        style.rounded(px(8.)).bg(target)
                    }
                })
                .on_drop(cx.listener(move |this, drag: &QueuedDrag, _, cx| this.drop_queued(drag, seq, cx)))
                .child(
                    icon_button(("queued-steer", ix), Lucide::ArrowUp, palette)
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(send_now.clone()).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.begin_queue_action(seq) {
                                return;
                            }
                            if let Some(runs) = this.runs(&steer) {
                                runs.queue_steer(steer.clone(), seq);
                            }
                            cx.notify();
                        })),
                )
                .child(
                    icon_button(("queued-edit", ix), Lucide::Pencil, palette)
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(tr("Edit")).build(window, cx))
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.edit_queued(seq, edit.clone(), window, cx)),
                        ),
                )
                .child(
                    icon_button(("queued-take-back", ix), Lucide::X, palette)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tr("Take back")).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if !this.begin_queue_action(seq) {
                                return;
                            }
                            if let Some(runs) = this.runs(&cancel) {
                                runs.queue_cancel(cancel.clone(), seq);
                            }
                            this.focus_composer(window, cx);
                            cx.notify();
                        })),
                )
                .into_any_element()
        });
        // A hairline under the rows parts them from the text. A long queue scrolls (about five rows show), as chat's.
        Some(
            v_flex()
                .id("code-on-queue")
                .mx(px(12.))
                .pt(px(8.))
                .pb(px(4.))
                .border_b_1()
                .border_color(palette.border)
                .child(
                    v_flex()
                        .id("code-on-queue-rows")
                        .w_full()
                        .max_h(px(168.))
                        .overflow_y_scroll()
                        .children(sent_rows)
                        .children(queued_rows),
                )
                .into_any_element(),
        )
    }

    /// The messages sent while a cloud session starts, as Queued chips: each waits behind the first message, and can
    /// be taken back until the session is made.
    fn render_waiting_messages(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let texts = self.starting_cloud()?.waiting.texts();
        if texts.is_empty() {
            return None;
        }
        let rows = texts.iter().enumerate().map(|(ix, (id, text))| {
            let id = *id;
            queued_chip(("waiting", id as usize), ix, text, palette).child(
                icon_button(("waiting-take-back", id as usize), Lucide::X, palette)
                    .on_click(cx.listener(move |this, _, window, cx| this.take_back_waiting(id, window, cx))),
            )
        });
        Some(
            v_flex()
                .w_full()
                .gap(px(4.))
                .child(queued_header(
                    texts.len(),
                    "sent in order when the machine is ready",
                    palette,
                ))
                .children(rows)
                .into_any_element(),
        )
    }

    /// The session's model and effort chips, in the composer's toolbar (`effort-menu.tsx`, the model chip beside it).
    pub(super) fn render_code_chips(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let (Some(session), View::Code(view)) = (self.code_session(), &self.view) else {
            return div().into_any_element();
        };
        let shown = session.picker_values();
        let catalog = self.agent_catalog(session.agent);
        let model_label: SharedString = match &shown.model {
            Some(id) => session_model_label(&catalog, id).into(),
            // The model Default runs, else the agent while its models are read: never a bare "Default model".
            None => crate::code::default_model_name(&catalog)
                .unwrap_or_else(|| nexus_workbench::driver::label(session.agent).to_string())
                .into(),
        };
        // The model's maker, else the agent's (Default, or a model id that does not say).
        let maker = shown
            .model
            .as_deref()
            .and_then(|id| super::answer_footer::maker_of(raw_id(id)))
            .unwrap_or_else(|| super::new_code::agent_maker(session.agent));
        let efforts = self.session_efforts(session);
        let (mode_label, _) = permission_copy(session.agent, shown.mode);
        let effort_label: SharedString = if efforts.is_empty() {
            mode_label.into()
        } else {
            match shown.effort {
                Some(effort) => EFFORTS
                    .iter()
                    .find(|(level, _)| *level == effort)
                    .map(|(_, label)| *label)
                    .unwrap_or("Default")
                    .into(),
                None => "Default".into(),
            }
        };
        let effort_label: SharedString = if session.fast_shown() && self.session_has_fast(session) {
            format!("{effort_label} · Fast").into()
        } else {
            effort_label
        };
        let open = view.dock.menu;
        // As a new session's: "Opus 5.5 High", the model bright and the effort faint, no chevrons.
        h_flex()
            .min_w_0()
            .flex_shrink(1.)
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .relative()
                    .child(
                        chip(
                            "code-model-chip",
                            maker,
                            model_label.clone(),
                            open == Some(Menu::Model),
                            palette,
                        )
                        .min_w_0()
                        .flex_shrink(1.)
                        .when(shown.waiting_model, |chip| chip.opacity(0.6))
                        .tooltip(move |window, cx| {
                            let text = if shown.waiting_model {
                                format!("{model_label}. {}", nexus_workbench::picker_values::APPLIES_LATER)
                            } else {
                                model_label.to_string()
                            };
                            gpui_kit::component::tooltip::Tooltip::new(text).build(window, cx)
                        })
                        .menu_trigger(
                            open == Some(Menu::Model),
                            cx,
                            |this| this.dock_menu_is(Menu::Model),
                            |this, window, cx| {
                                this.toggle_dock_menu(Menu::Model, cx);
                                if this.dock_menu_is(Menu::Model) {
                                    this.prepare_model_search(window, cx);
                                } else {
                                    this.focus_composer(window, cx);
                                }
                            },
                        ),
                    )
                    .when_some(
                        motion::presence("dock-model-menu", open == Some(Menu::Model), cx),
                        |frame, phase| {
                            // The same menu as a new session's, its rail narrowed to this session's agent.
                            let menu = phase.menu("dock-model-menu", 4., self.render_launch_model_menu(palette, cx));
                            frame.child(menu_layer(menu, Anchor::BottomLeft))
                        },
                    ),
            )
            .child(
                div()
                    .relative()
                    .child(
                        effort_chip(
                            "code-effort-chip",
                            effort_label.clone(),
                            open == Some(Menu::Effort),
                            self.composer_layout.control_icons,
                            if efforts.is_empty() {
                                super::new_code::mode_icon(shown.mode)
                            } else {
                                Lucide::Brain
                            },
                            palette,
                        )
                        .when(shown.waiting_effort || shown.waiting_mode, |chip| chip.opacity(0.6))
                        .tooltip({
                            let text = if shown.waiting_effort || shown.waiting_mode {
                                format!(
                                    "Effort: {effort_label}; permissions: {mode_label}. {}",
                                    nexus_workbench::picker_values::APPLIES_LATER
                                )
                            } else {
                                format!("Effort: {effort_label}; permissions: {mode_label}")
                            };
                            move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
                        })
                        .menu_trigger(
                            open == Some(Menu::Effort),
                            cx,
                            |this| this.dock_menu_is(Menu::Effort),
                            |this, _, cx| this.toggle_dock_menu(Menu::Effort, cx),
                        ),
                    )
                    .when_some(
                        motion::presence("dock-effort-menu", open == Some(Menu::Effort), cx),
                        |frame, phase| {
                            let menu = self.render_effort_menu(session, &efforts, palette, cx);
                            frame.child(menu_layer(phase.menu("dock-effort-menu", 4., menu), Anchor::BottomLeft))
                        },
                    ),
            )
            .into_any_element()
    }

    /// The agent's models, as it reported them: (id, name).
    pub(super) fn session_models(&self, session: &CodeSession) -> Vec<(String, String)> {
        self.code_agents
            .iter()
            .find(|info| info.agent == session.agent)
            .and_then(|info| info.models.as_ref())
            .map(|models| {
                models
                    .iter()
                    .filter(|model| !crate::code::is_default_entry(model))
                    .map(|model| (model.id.clone(), model.name.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The levels the session's model takes (`effortOptions`): its own when the agent reported them, else the usual
    /// ones. Empty when it takes none.
    pub(super) fn session_efforts(&self, session: &CodeSession) -> Vec<(Effort, &'static str)> {
        let shown = session.picker_values();
        effort::options(
            session.agent,
            &self.agent_catalog(session.agent),
            shown.model.as_deref(),
        )
    }

    fn render_effort_menu(
        &self,
        session: &CodeSession,
        efforts: &[(Effort, &'static str)],
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shown = session.picker_values();
        let mut menu = menu_panel("code-effort-menu", 300., palette, cx);
        if !efforts.is_empty() {
            menu = menu
                .child(menu_label("Reasoning", palette))
                .child(
                    menu_row(
                        "code-effort-default",
                        "Default".into(),
                        None,
                        shown.effort.is_none(),
                        palette,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.update_session(None, Some(None), None, cx))),
                )
                .children(efforts.iter().map(|&(effort, label)| {
                    menu_row(
                        SharedString::from(format!("code-effort-{}", effort.as_str())),
                        label.into(),
                        None,
                        shown.effort == Some(effort),
                        palette,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.update_session(None, Some(Some(effort)), None, cx)),
                    )
                }))
                .child(div().h(px(8.)));
        }
        if self.session_has_fast(session) {
            let on = session.fast_shown();
            let off = session
                .connection
                .as_ref()
                .and_then(|connection| connection.fast_off.as_deref());
            menu = menu
                .child(menu_label("Speed", palette))
                .child(
                    menu_row("code-speed-standard", "Standard".into(), None, !on, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.set_session_fast(false, cx))),
                )
                .child(
                    menu_row(
                        "code-speed-fast",
                        "Fast".into(),
                        Some(fast_hint(on.then_some(off).flatten()).into()),
                        on,
                        palette,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.set_session_fast(true, cx))),
                )
                .child(div().h(px(8.)));
        }
        let offered = nexus_workbench::harness::harness(session.agent).offered_modes();
        menu.child(menu_label("Permissions", palette))
            .children(
                MODES
                    .iter()
                    .filter(|(mode, ..)| offered.contains(mode))
                    .map(|&(mode, ..)| {
                        let (label, hint) = permission_copy(session.agent, mode);
                        menu_row(
                            SharedString::from(format!("code-mode-{}", mode.as_str())),
                            label.into(),
                            Some(hint.into()),
                            shown.mode == mode,
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.update_session(None, None, Some(mode), cx)))
                    }),
            )
            .into_any_element()
    }

    /// The agent's questions (`question.tsx`): one at a time with a tab for each and a review when there are several,
    /// each with its options under number keys and a field for your own answer.
    pub(super) fn render_question(
        &self,
        view: &CodeView,
        input: &Value,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let questions = questions_of(input);
        let values = self.question_values(&questions, cx);
        let answered = values.iter().filter(|values| !values.is_empty()).count();
        let complete = !questions.is_empty() && answered == questions.len();
        let name: SharedString = self
            .code_session()
            .map(|session| agent_name(session.agent).into())
            .unwrap_or_else(|| tr("The agent"));
        let blocks = input.get("isBlocking").and_then(Value::as_bool) != Some(false);
        let stepped = questions.len() > 1;
        let step = self.question_step_now(questions.len());
        let reviewing = stepped && step == questions.len();
        let title = if stepped {
            tr("{name} has {count} questions")
                .replace("{name}", &name)
                .replace("{count}", &questions.len().to_string())
        } else {
            tr("{name} has a question").replace("{name}", &name)
        };
        let dock = &view.dock;
        let field = questions.get(step).filter(|_| !reviewing).map(|question| {
            let qx = step;
            let key = question.key();
            let picked = dock.picked.get(key).cloned().unwrap_or_default();
            let custom = dock.custom.get(key).copied().unwrap_or(false);
            let options = question.options.iter().enumerate().map(|(ox, (label, description))| {
                let on = picked.contains(label) && (question.multi || !custom);
                let clicked = label.clone();
                h_flex()
                    .id(SharedString::from(format!("question-{qx}-{ox}")))
                    .w_full()
                    .items_start()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .map(|row| {
                        if on {
                            row.bg(palette.selected).border_1().border_color(palette.border)
                        } else {
                            row.bg(palette.hover)
                        }
                    })
                    .hover(|style| style.bg(palette.selected))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_option(qx, clicked.clone(), cx)))
                    .child(keycap(ox + 1, on, question.multi, palette))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(label.clone()),
                            )
                            .when_some(description.clone(), |column, description| {
                                column.child(div().text_size(px(12.5)).text_color(palette.muted).child(description))
                            }),
                    )
            });
            let other = h_flex()
                .w_full()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(4.))
                .rounded(px(8.))
                .bg(palette.hover)
                .child(
                    div()
                        .id(SharedString::from(format!("question-{qx}-own")))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_custom(qx, false, cx)))
                        .child(keycap(question.options.len() + 1, custom, question.multi, palette)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(13.))
                        .when_some(dock.others.get(qx), |field, other| {
                            field.child(Input::new(other).appearance(false))
                        }),
                );
            v_flex()
                .w_full()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(question.question.clone()),
                )
                .when(question.multi, |column| {
                    column.child(
                        div()
                            .mt(px(-4.))
                            .text_size(px(12.))
                            .text_color(palette.muted)
                            .child(tr("Choose one or more.")),
                    )
                })
                .children(options)
                .when(qx < QUESTIONS, |column| column.child(other))
        });
        let tabs = stepped.then(|| {
            let tab = |id: SharedString, ix: usize, label: String, done: bool, on: bool, cx: &mut Context<Self>| {
                h_flex()
                    .id(id)
                    .flex_none()
                    .max_w(px(180.))
                    .gap(px(6.))
                    .pl(px(4.))
                    .pr(px(10.))
                    .py(px(3.))
                    .rounded_full()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(if on { palette.foreground } else { palette.muted })
                    .when(on, |tab| {
                        tab.bg(palette.selected).border_1().border_color(palette.border)
                    })
                    .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                    .on_click(cx.listener(move |this, _, _, cx| this.question_step(ix, cx)))
                    .child(step_mark(ix, done, ix == questions.len(), palette))
                    .child(div().truncate().child(label))
            };
            let mut row = h_flex().w_full().flex_wrap().gap(px(4.));
            for (ix, question) in questions.iter().enumerate() {
                let label = question
                    .header
                    .clone()
                    .unwrap_or_else(|| tr("Question {number}").replace("{number}", &(ix + 1).to_string()));
                row = row.child(tab(
                    SharedString::from(format!("question-tab-{ix}")),
                    ix,
                    label,
                    !values[ix].is_empty(),
                    ix == step,
                    cx,
                ));
            }
            row.child(tab(
                "question-tab-review".into(),
                questions.len(),
                tr("Review").to_string(),
                complete,
                reviewing,
                cx,
            ))
        });
        let review = reviewing.then(|| {
            let rows = questions
                .iter()
                .zip(&values)
                .enumerate()
                .map(|(ix, (question, values))| {
                    let answer = if values.is_empty() {
                        None
                    } else if question.secret {
                        Some(tr("Private answer").to_string())
                    } else {
                        Some(values.join(", "))
                    };
                    receipt_row(ix, &question.question, answer, true, palette)
                        .id(SharedString::from(format!("question-review-{ix}")))
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.hover))
                        .on_click(cx.listener(move |this, _, _, cx| this.question_step(ix, cx)))
                });
            v_flex().w_full().gap(px(2.)).children(rows)
        });
        let can_next = field.is_some() && values.get(step).is_some_and(|values| !values.is_empty());
        card(palette)
            .track_focus(&dock.keys.card)
            .child(
                h_flex()
                    .w_full()
                    .child(card_head(Lucide::CircleQuestionMark, title, palette))
                    .child(div().flex_1())
                    .when(stepped, |head| {
                        head.child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.muted)
                                .child(format!("{answered} of {} answered", questions.len())),
                        )
                    }),
            )
            .children(tabs)
            .child(
                div().id("question-body").max_h(px(380.)).overflow_y_scroll().child(
                    v_flex()
                        .gap(px(14.))
                        .children(field)
                        .children(review)
                        .when(questions.is_empty(), |body| {
                            body.child(div().text_size(px(13.)).text_color(palette.faint).child(tr(
                                "The question could not be displayed. Skip it to ask the agent to try again.",
                            )))
                        }),
                ),
            )
            .child(self.render_card_problem(palette, cx))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.5))
                            .text_color(palette.faint)
                            .child(if blocks {
                                tr("This agent is waiting for your answer.")
                            } else {
                                tr("Work can continue while you answer.")
                            }),
                    )
                    .child(
                        answer_button("question-skip", tr("Skip"), false, true, palette)
                            .on_click(cx.listener(|this, _, window, cx| this.skip_question(window, cx))),
                    )
                    .child(if stepped && !reviewing {
                        answer_button("question-next", tr("Next"), true, can_next, palette)
                            .child(enter_hint(palette))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_next {
                                    this.question_next(step, cx);
                                }
                            }))
                    } else {
                        answer_button("question-submit", tr("Submit answer"), true, complete, palette)
                            .child(enter_hint(palette))
                            .on_click(cx.listener(|this, _, window, cx| this.submit_answers(window, cx)))
                    }),
            )
            .into_any_element()
    }

    /// The plan the agent wants to start on (`plan-card.tsx`'s `PlanPanel`): the plan, and the canon's four answers.
    pub(super) fn render_plan(
        &self,
        view: &CodeView,
        input: &Value,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name: SharedString = self
            .code_session()
            .map(|session| agent_name(session.agent).into())
            .unwrap_or_else(|| tr("The agent"));
        let plan = plan_of(input);
        let title = plan_title(plan);
        card(palette)
            .child(card_head(
                Lucide::Map,
                tr("{name} has a plan and wants to start on it").replace("{name}", &name),
                palette,
            ))
            .when_some(title, |card, title| {
                card.child(div().text_size(px(15.)).font_weight(FontWeight::MEDIUM).child(title))
            })
            .when_some(view.dock.plan.as_ref(), |card, (_, body)| {
                card.child(
                    div()
                        .id("plan-body")
                        .max_h(px(340.))
                        .overflow_y_scroll()
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(10.))
                        .bg(palette.background)
                        .text_size(px(13.5))
                        .line_height(px(21.))
                        .child(transcript::prose(
                            crate::external_link::text_view(body).selectable(true),
                            palette,
                        )),
                )
            })
            .when(plan.trim().is_empty(), |card| {
                card.child(
                    div()
                        .text_size(px(13.))
                        .text_color(palette.faint)
                        .child(tr("The plan came with no text.")),
                )
            })
            .child(self.render_card_problem(palette, cx))
            // What to change gets its own line: the answers beside it left it too narrow to read.
            .child(h_flex().w_full().child(reason_field(&view.dock.change, palette)))
            .child({
                let mut row = h_flex().flex_wrap().gap(px(8.));
                if crate::canon_preview::transcript_actions() {
                    row = row
                        .children(PLAN_ANSWERS.into_iter().enumerate().map(|(ix, (label, mode))| {
                            answer_button(("plan-yes", ix), label, ix == 0, true, palette)
                                .on_click(cx.listener(move |this, _, _, cx| this.implement_plan(mode, cx)))
                        }))
                        .child(
                            answer_button("plan-keep", KEEP_PLANNING, false, true, palette)
                                .border_color(gpui_kit::transparent_black())
                                .tooltip({
                                    let text = tr("{name} stops and waits; what you typed goes with it")
                                        .replace("{name}", &name);
                                    move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
                                    }
                                })
                                .on_click(cx.listener(|this, _, window, cx| this.keep_planning(window, cx))),
                        )
                        .child(
                            text_button("plan-elsewhere", "Implement in a new session", false, palette)
                                .ml_auto()
                                .when(plan.trim().is_empty(), |button| button.opacity(0.4))
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        "Start a new session on this plan, in auto-accept edits",
                                    )
                                    .build(window, cx)
                                })
                                .when(!plan.trim().is_empty(), |button| {
                                    button.on_click(
                                        cx.listener(|this, _, window, cx| this.implement_in_new_session(window, cx)),
                                    )
                                }),
                        );
                } else {
                    row = row
                        .child(
                            answer_button("plan-keep", tr("Keep planning"), false, true, palette)
                                .tooltip({
                                    let text = tr("{name} stops and waits; what you typed goes with it")
                                        .replace("{name}", &name);
                                    move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
                                    }
                                })
                                .on_click(cx.listener(|this, _, window, cx| this.keep_planning(window, cx))),
                        )
                        .child(
                            answer_button(
                                "plan-elsewhere",
                                tr("Implement in a new session"),
                                false,
                                !plan.trim().is_empty(),
                                palette,
                            )
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tr(
                                    "Start a new session on this plan, in auto-accept edits",
                                ))
                                .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.implement_in_new_session(window, cx))),
                        )
                        .child(
                            answer_button("plan-implement", tr("Implement"), true, true, palette)
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(tr(
                                        "Approve the plan and let it edit files (auto-accept edits)",
                                    ))
                                    .build(window, cx)
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.implement_plan(PermissionMode::AcceptEdits, cx)),
                                ),
                        );
                }
                row
            })
            .into_any_element()
    }

    /// The last command's error, on a card (the composer, where it shows otherwise, is hidden under the card). A used-up
    /// budget is its amber line, never red text (`limit_line.rs`).
    fn render_card_problem(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if let Some(line) = self.render_card_limit(palette, cx) {
            return line;
        }
        match &self.composer_note {
            Some(note) => div()
                .text_size(px(13.))
                .text_color(palette.destructive)
                .child(super::message_text::MessageText::new("card-problem", note.clone()))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }
}

/// A dock card (`.composer.wb-approval`).
pub(super) fn card(palette: Palette) -> Div {
    v_flex()
        .w_full()
        .p(px(16.))
        .gap(px(10.))
        .rounded(px(16.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.faint)
}

pub(super) fn card_head(icon: Lucide, title: String, palette: Palette) -> impl IntoElement {
    h_flex()
        .gap(px(8.))
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .child(glyph(icon, palette.foreground))
        .child(title)
}

/// The optional text sent with Deny or No, keep planning (`.wb-reason`).
pub(super) fn reason_field(state: &Entity<InputState>, palette: Palette) -> impl IntoElement {
    div()
        .flex_1()
        .min_w(px(0.))
        .h(px(30.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .border_1()
        .border_color(palette.border)
        .bg(palette.background)
        .text_size(px(13.))
        .child(Input::new(state).appearance(false))
}

/// An option's number key, which is also its mark: filled when picked, round for one choice, square for many.
fn keycap(n: usize, on: bool, multi: bool, palette: Palette) -> impl IntoElement {
    div()
        .mt(px(1.))
        .min_w(px(18.))
        .h(px(18.))
        .px(px(4.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .map(|cap| if multi { cap.rounded(px(4.)) } else { cap.rounded_full() })
        .bg(if on { palette.primary } else { palette.selected })
        .text_color(if on { palette.primary_foreground } else { palette.muted })
        .font_family("Geist Mono")
        .text_size(px(10.5))
        .child(n.to_string())
}

/// A step's mark in the tab row: its number, or a check once it has an answer (the review always shows a check).
fn step_mark(ix: usize, done: bool, review: bool, palette: Palette) -> impl IntoElement {
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(if done { palette.primary } else { palette.selected })
        .font_family("Geist Mono")
        .text_size(px(10.))
        .map(|mark| {
            if done || review {
                mark.child(Icon::new(Lucide::Check).size(px(11.)).text_color(if done {
                    palette.primary_foreground
                } else {
                    palette.muted
                }))
            } else {
                mark.text_color(palette.muted).child((ix + 1).to_string())
            }
        })
}

/// The ↵ after a card button that Enter presses.
fn enter_hint(palette: Palette) -> impl IntoElement {
    div()
        .ml(px(2.))
        .text_size(px(11.))
        .opacity(0.6)
        .text_color(palette.primary_foreground)
        .child("↵")
}

/// "1. Question → answer", wrapping when long (the review and the answered receipt).
pub(super) fn receipt_row(ix: usize, question: &str, answer: Option<String>, numbered: bool, palette: Palette) -> Div {
    // The number keeps its own narrow column. Beside it, the question wraps inside the card, and "→ answer" follows on
    // its line or under it. Every box may shrink below its text's width: GPUI measures text unwrapped, so a box at its
    // automatic minimum keeps the whole line and the card clips it.
    let answer = match answer {
        Some(answer) => div()
            .min_w_0()
            .text_color(palette.strong)
            .font_weight(FontWeight::MEDIUM)
            .child(answer),
        None => div()
            .min_w_0()
            .text_color(palette.destructive)
            .child(tr("Not answered")),
    };
    h_flex()
        .w_full()
        .items_start()
        .gap_x(px(8.))
        .px(px(8.))
        .py(px(5.))
        .rounded(px(8.))
        .text_size(px(13.))
        .when(numbered, |row| {
            row.child(
                div()
                    .flex_none()
                    .text_color(palette.faint)
                    .child(format!("{}.", ix + 1)),
            )
        })
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .flex_wrap()
                .items_baseline()
                .gap_x(px(8.))
                .gap_y(px(2.))
                .child(
                    div()
                        .debug_selector(|| "receipt-question".into())
                        .min_w_0()
                        .text_color(palette.muted)
                        .child(question.to_string()),
                )
                .child(
                    h_flex()
                        .debug_selector(|| "receipt-answer".into())
                        .min_w_0()
                        .items_baseline()
                        .gap_x(px(8.))
                        .child(glyph(Lucide::ArrowRight, palette.faint).flex_none().size(px(13.)))
                        .child(answer),
                ),
        )
}

/// A card's button (`.head-action`): plain, or filled for the main answer; faded while it cannot answer.
pub(super) fn answer_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
    palette: Palette,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    h_flex()
        .id(id)
        .flex_none()
        .h(px(30.))
        .px(px(12.))
        .gap(px(4.))
        .rounded(px(8.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .map(|button| {
            if primary {
                button.bg(palette.primary).text_color(palette.primary_foreground)
            } else {
                button
                    .border_1()
                    .border_color(palette.border)
                    .text_color(palette.foreground)
            }
        })
        .map(|button| {
            if !enabled {
                button.opacity(0.4)
            } else if primary {
                button
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.88))
                    .active(|style| style.opacity(0.72))
            } else {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .active(|style| style.opacity(0.72))
            }
        })
        .child(label)
}

/// A row of the composer's queue: its grip when it can move, the "On queue" tag and the message's first words, or
/// "Image". Its actions follow.
fn on_queue_row(id: impl Into<ElementId>, text: &str, grip: Option<Stateful<Div>>, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .w_full()
        .h(px(32.))
        .gap(px(8.))
        .text_size(px(13.5))
        .text_color(palette.muted)
        .children(grip)
        .child(on_queue_tag(palette))
        .child(div().flex_1().min_w(px(0.)).truncate().child(queue_line(text)))
}

/// A queued message's first words on one line, or "Image" when it has no text.
fn queue_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() { tr("Image").to_string() } else { line }
}

/// "On queue": a small hairline pill, so the row reads as waiting, not as typed text.
fn on_queue_tag(palette: Palette) -> Div {
    div()
        .flex_none()
        .h(px(20.))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded_full()
        .border_1()
        .border_color(palette.border)
        .bg(palette.hover)
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.muted)
        .child(tr("On queue"))
}

/// "Queued · 2", then when they go.
pub(super) fn queued_header(count: usize, when: &'static str, palette: Palette) -> Div {
    h_flex()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(glyph(Lucide::ListOrdered, palette.faint))
        .child(tr("Queued · {count}").replace("{count}", &count.to_string()))
        .child(div().text_color(palette.faint).child(when))
}

/// A queued message's chip: its place in the queue and its first words. Its actions follow.
pub(super) fn queued_chip(id: impl Into<ElementId>, ix: usize, text: &str, palette: Palette) -> Stateful<Div> {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = if line.is_empty() { tr("Image").to_string() } else { line };
    h_flex()
        .id(id)
        .w_full()
        .h(px(32.))
        .px(px(10.))
        .gap(px(8.))
        .rounded(px(10.))
        .border_1()
        .border_color(palette.border)
        .bg(palette.raised)
        .text_size(px(13.))
        .child(
            div()
                .flex_none()
                .font_family("Geist Mono")
                .text_size(px(11.5))
                .text_color(palette.faint)
                .child((ix + 1).to_string()),
        )
        .child(div().flex_1().min_w(px(0.)).truncate().child(line))
}

/// A small text button on a queued chip.
pub(super) fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    palette: Palette,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex_none()
        .px(px(6.))
        .py(px(2.))
        .rounded(px(6.))
        .text_size(px(12.5))
        .text_color(if primary { palette.foreground } else { palette.muted })
        .when(primary, |button| button.font_weight(FontWeight::MEDIUM))
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
        .child(label)
}

/// A labelled button with an optional icon before it, as the Nexus app's `head-action`; `primary` is its strong one.
pub(super) fn action_button(
    id: impl Into<ElementId>,
    icon: Option<Lucide>,
    label: impl Into<SharedString>,
    primary: bool,
    palette: Palette,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    h_flex()
        .id(id)
        .flex_none()
        .gap(px(5.))
        .px(px(8.))
        .py(px(3.))
        .rounded(px(6.))
        .text_size(px(12.5))
        .cursor_pointer()
        .when(primary, |button| {
            button
                .bg(palette.primary)
                .text_color(palette.primary_foreground)
                .font_weight(FontWeight::MEDIUM)
                .hover(|style| style.opacity(0.9))
        })
        .when(!primary, |button| {
            button
                .text_color(palette.muted)
                .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
        })
        .when_some(icon, |button, icon| {
            button.child(Icon::new(icon).size(px(13.)).text_color(if primary {
                palette.primary_foreground
            } else {
                palette.muted
            }))
        })
        .child(label)
}

pub(super) fn icon_button(id: impl Into<ElementId>, icon: Lucide, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .child(Icon::new(icon).size(px(13.)).text_color(palette.muted))
}

/// The model in the composer's toolbar: the `maker`'s mark, then the name as plain text in the bright ink, no chevron,
/// as a new session's. The effort follows it, fainter, so the two read as one phrase ("Opus 5.5 Medium").
pub(super) fn chip(id: &'static str, maker: &str, label: SharedString, open: bool, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .h(px(26.))
        .px(px(3.))
        .gap(px(5.))
        .child(
            div()
                .flex_none()
                .child(super::model_menu::maker_mark(maker, 14., palette)),
        )
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(palette.foreground)
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .when(open, |chip| chip.bg(palette.hover))
        .child(div().min_w_0().max_w(px(160.)).truncate().child(label))
}

/// The effort after the model: its word alone, in the faint ink.
pub(super) fn effort_chip(
    id: &'static str,
    label: SharedString,
    open: bool,
    compact: bool,
    icon: Lucide,
    palette: Palette,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(26.))
        .px(px(3.))
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(if open { palette.foreground } else { palette.faint })
        .cursor_pointer()
        .hover(|style| style.text_color(palette.foreground))
        .role(Role::Button)
        .aria_label(label.clone())
        .when(compact, |chip| {
            chip.w(px(28.)).justify_center().child(Icon::new(icon).size(px(13.)))
        })
        .when(!compact, |chip| {
            chip.child(div().max_w(px(160.)).truncate().child(label))
        })
}

/// A menu, above everything, from its button: its bottom 8px over the button's top.
pub(super) fn menu_layer(menu: AnyElement, anchor: Anchor) -> impl IntoElement {
    let corner = div().absolute().top_0();
    let corner = if anchor == Anchor::BottomRight {
        corner.right_0()
    } else {
        corner.left_0()
    };
    corner.child(
        deferred(
            anchored()
                .anchor(anchor)
                .offset(point(px(0.), px(-8.)))
                .snap_to_window_with_margin(px(12.))
                .child(menu),
        )
        .with_priority(1),
    )
}

/// A menu's box (`.wb-pop`). A click outside closes it.
fn menu_panel(id: &'static str, width: f32, palette: Palette, cx: &mut Context<Workspace>) -> Stateful<Div> {
    menu_box(id, width, palette)
        .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_dock_menu(cx)))
}

/// The button that opens and closes a menu: a press on it toggles the menu.
///
/// A menu closes itself on a mouse down outside it (`on_mouse_down_out`, in the capture phase). A press on the
/// trigger of an open menu has closed it before the trigger hears it, so a toggle on click would open it again. So
/// the trigger acts on mouse down, and only while the menu is as it was drawn (`open`): open and not closed yet, or
/// closed. `is_open` reads that state when the press comes; `toggle` opens the menu, or closes it.
pub(crate) trait MenuTrigger: InteractiveElement + Sized {
    fn menu_trigger<V: 'static>(
        self,
        open: bool,
        cx: &Context<V>,
        is_open: impl Fn(&V) -> bool + 'static,
        toggle: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Self {
        self.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                if is_open(this) == open {
                    toggle(this, window, cx);
                }
            }),
        )
    }
}

impl<E: InteractiveElement> MenuTrigger for E {}

/// A menu's box (`.wb-pop`), for its owner to close.
pub(crate) fn menu_box(id: impl Into<ElementId>, width: f32, palette: Palette) -> Stateful<Div> {
    // Never narrower than a sidebar row (`tokens::menu::MIN_WIDTH`), so a menu opened from a row lines up with it.
    // Its owner gives the width its longest row needs; a fixed width keeps a card's wrapping text in its column.
    v_flex()
        .id(id)
        .occlude()
        .w(px(width.max(crate::tokens::menu::MIN_WIDTH)))
        .max_h(px(420.))
        .overflow_y_scroll()
        .p(px(4.))
        .gap(px(1.))
        .rounded(px(crate::tokens::radius::CARD))
        .bg(palette.menu)
        // The outline sits inside the box, so a 240 menu is 240 with it.
        .border_1()
        .border_color(palette.menu_ring())
        .shadow(palette.pop_drop())
        .text_color(palette.foreground)
}

/// The hover of an item in a menu (`menu_box`): the `--nx-fill-strong` fill of Nexus UI's menu items. Use it for
/// every hover on a menu's background. `palette.hover` is the menu's own color in the dark theme, so it shows nothing.
pub(crate) fn menu_item_hover(style: StyleRefinement, palette: Palette) -> StyleRefinement {
    style.bg(palette.menu_hover)
}

pub(crate) fn menu_label(label: &'static str, palette: Palette) -> impl IntoElement {
    div()
        .px(px(10.))
        .pt(px(6.))
        .pb(px(2.))
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.faint)
        .child(label)
}

/// A menu row: its words, a hint under them, and a check when it is the current one.
/// The Fast row's hint: what it costs, or, when Claude Code reports fast mode is not serving, why (its
/// `fast_mode_disabled_reason`, `claude::fast_off`).
fn fast_hint(off: Option<&str>) -> String {
    match off {
        None => "Faster output at twice the price".into(),
        // Claude Code reports these until the next model call takes the change.
        Some("sdk_opt_in_required" | "pending") => nexus_workbench::picker_values::APPLIES_LATER.into(),
        Some("preference") => "Off: your Claude organization turned fast mode off".into(),
        Some("free" | "extra_usage_disabled") => "Off: your Claude plan does not include fast mode".into(),
        Some("model_not_allowed") => "Off: this model has no fast mode".into(),
        Some("cooldown") => "Paused after a rate limit; it comes back by itself".into(),
        Some(other) => format!("Off: Claude Code reports {other}"),
    }
}

pub(crate) fn menu_row(
    id: impl Into<ElementId>,
    label: SharedString,
    hint: Option<SharedString>,
    picked: bool,
    palette: Palette,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .w_full()
        .items_start()
        .px(px(10.))
        .py(px(6.))
        .gap(px(8.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|style| menu_item_hover(style, palette))
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .child(div().text_size(px(13.5)).child(label))
                .when_some(hint, |column, hint| {
                    column.child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(palette.muted)
                            .child(hint),
                    )
                }),
        )
        .when(picked, |row| {
            row.child(Icon::new(Lucide::Check).size(px(14.)).text_color(palette.foreground))
        })
}

// A menu over the art has an absolute frosted layer that covers its padding box (`launch_box`). The patched GPUI
// (`vendor/README.md`) must not add the padding to it again: the menu scrolled past its end, and a strip with no
// background opened under its last row. A layer that reaches past the box still scrolls it.
#[cfg(test)]
mod menu_box_scroll {
    use gpui_kit::{
        AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
        ScrollHandle, StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, WindowAppearance, div, px,
    };

    struct Menu {
        scroll: ScrollHandle,
        reach: Pixels,
    }

    impl Render for Menu {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette =
                crate::palette::Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
            div().size_full().child(
                super::menu_box("test-menu", 300., palette)
                    .relative()
                    .h(px(120.))
                    .track_scroll(&self.scroll)
                    .child(div().absolute().inset_0())
                    .child(div().absolute().top_0().left_0().w(px(10.)).h(self.reach))
                    .child(div().flex_none().h(px(40.))),
            )
        }
    }

    fn max_scroll(reach: f32, cx: &mut TestAppContext) -> Pixels {
        cx.update(gpui_kit::init);
        let scroll = ScrollHandle::new();
        let (_, cx) = cx.add_window_view({
            let scroll = scroll.clone();
            move |_, _| Menu {
                scroll,
                reach: px(reach),
            }
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        scroll.max_offset().y
    }

    #[gpui_kit::test]
    fn a_layer_over_the_padding_box_does_not_scroll_the_menu(cx: &mut TestAppContext) {
        assert_eq!(max_scroll(10., cx), px(0.));
    }

    #[gpui_kit::test]
    fn a_layer_past_the_box_scrolls_it_as_far_as_it_reaches(cx: &mut TestAppContext) {
        // The layer starts inside the 1px border: 1 + 149 ends 30px past the 120px box.
        assert_eq!(max_scroll(149., cx), px(30.));
    }
}

#[cfg(test)]
mod tests;

/// The agent's plan above the composer: a 28 chip, "2 of 5 done" and, faint,
/// what it does now; a click opens the whole plan in a box over the chip: done steps checked and muted, the current
/// one dotted and medium, the rest hollow.
fn render_todo_bar(todos: &[Todo], open: bool, palette: Palette, cx: &mut Context<Workspace>) -> impl IntoElement {
    let done = todos.iter().filter(|todo| todo.status == TodoStatus::Completed).count();
    let now = todos
        .iter()
        .find(|todo| todo.status == TodoStatus::InProgress)
        .map(|todo| todo.active_form.clone().unwrap_or_else(|| todo.content.clone()));
    let chip = h_flex()
        .id("todo-bar")
        .flex_none()
        .max_w_full()
        .min_w(px(0.))
        .h(px(28.))
        .px(px(8.))
        .gap(px(6.))
        .rounded(px(6.))
        .bg(if open { palette.selected } else { palette.hover })
        .text_size(px(13.))
        .line_height(px(18.))
        .cursor_pointer()
        .hover(|style| style.bg(palette.selected))
        .on_click(cx.listener(|this, _, _, cx| {
            if let View::Code(view) = &mut this.view {
                view.dock.todos_open = !view.dock.todos_open;
                cx.notify();
            }
        }))
        .child(Icon::new(Lucide::ListChecks).size(px(14.)).text_color(palette.icon))
        .child(
            div()
                .flex_none()
                .text_color(palette.foreground)
                .child(format!("{done} of {} done", todos.len())),
        )
        .children(now.map(|now| div().min_w(px(0.)).truncate().text_color(palette.faint).child(now)));
    let list = todos.iter().map(|todo| {
        let (icon, color, weight) = match todo.status {
            TodoStatus::Completed => (Lucide::CircleCheck, palette.muted, FontWeight::NORMAL),
            TodoStatus::InProgress => (Lucide::CircleDot, palette.foreground, FontWeight::MEDIUM),
            TodoStatus::Pending => (Lucide::Circle, palette.foreground, FontWeight::NORMAL),
        };
        let mark = if todo.status == TodoStatus::InProgress {
            palette.foreground
        } else {
            palette.faint
        };
        h_flex()
            .min_h(px(30.))
            .px(px(8.))
            .gap(px(8.))
            .text_size(px(13.))
            .text_color(color)
            .font_weight(weight)
            .child(Icon::new(icon).size(px(14.)).text_color(mark))
            .child(div().min_w(px(0.)).child(todo.content.clone()))
    });
    // The plan opens over its chip in the layout: its height grows from the chip and folds back into it as it fades
    // (`motion::reveal`), so what sits above it slides instead of jumping. The 8px under it is its own, to fold with it.
    v_flex()
        .w_full()
        .child(motion::reveal(
            "todo-list-area",
            open,
            Some(
                div()
                    .pb(px(8.))
                    .child(
                        v_flex()
                            .id("todo-list")
                            .w(px(360.))
                            .max_w_full()
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .p(px(4.))
                            .rounded(px(10.))
                            .bg(palette.menu)
                            .border_1()
                            .border_color(palette.border)
                            .shadow(palette.pop_shadow(palette.pop_ring()))
                            .child(
                                div()
                                    .h(px(24.))
                                    .px(px(8.))
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.))
                                    .text_color(palette.faint)
                                    .child(format!("Plan · {done} of {} done", todos.len())),
                            )
                            .children(list),
                    )
                    .into_any_element(),
            ),
        ))
        .child(h_flex().w_full().child(chip))
}

#[cfg(test)]
mod receipt_layout_tests {
    use super::receipt_row;
    use crate::palette::{Palette, ThemeChoice};
    // GPUI's prelude has a `test` attribute of its own; `#[gpui_kit::test]` expands to the standard one.
    use core::prelude::v1::test;
    use gpui_kit::*;

    struct Card {
        width: f32,
        question: &'static str,
        answer: &'static str,
    }

    impl Render for Card {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(ThemeChoice::Dark, WindowAppearance::Dark);
            div()
                .debug_selector(|| "receipt-card".into())
                .w(px(self.width))
                .overflow_hidden()
                .child(receipt_row(
                    1,
                    self.question,
                    Some(self.answer.to_string()),
                    true,
                    palette,
                ))
        }
    }

    fn layout(cx: &mut TestAppContext, card: Card) -> (Bounds<Pixels>, Bounds<Pixels>, Bounds<Pixels>) {
        cx.update(gpui_kit::init);
        let (_view, cx) = cx.add_window_view(|_, _| card);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (
            cx.debug_bounds("receipt-card").unwrap(),
            cx.debug_bounds("receipt-question").unwrap(),
            cx.debug_bounds("receipt-answer").unwrap(),
        )
    }

    #[gpui_kit::test]
    fn a_long_question_wraps_inside_the_card_with_its_answer_under_it(cx: &mut TestAppContext) {
        let question = "Which of the three deployment targets should this change go to first, given that staging is \
                        still on the old schema and production has the migration already applied?";
        let (card, asked, answered) = layout(
            cx,
            Card {
                width: 320.,
                question,
                answer: "Staging",
            },
        );
        assert!(
            asked.right() <= card.right(),
            "the question must stay inside the card: {asked:?} in {card:?}"
        );
        assert!(
            asked.size.height > px(20.),
            "a long question wraps to more than one line: {asked:?}"
        );
        assert!(
            asked.left() > card.left() + px(16.),
            "the number keeps its own column: {asked:?}"
        );
        assert!(
            answered.top() >= asked.bottom(),
            "the answer goes under the wrapped question"
        );
        assert!(answered.right() <= card.right());
    }

    #[gpui_kit::test]
    fn a_short_question_and_answer_read_on_one_line(cx: &mut TestAppContext) {
        let (card, asked, answered) = layout(
            cx,
            Card {
                width: 320.,
                question: "Which branch?",
                answer: "main",
            },
        );
        assert!(answered.right() <= card.right());
        assert!(
            answered.top() < asked.bottom(),
            "a short answer stays on the question's line"
        );
        assert!(answered.left() >= asked.right());
    }

    #[test]
    fn session_model_label_matches_resolved_id_for_routed_auto_models() {
        let catalog = vec![
            nexus_workbench::agent_models::Model {
                id: "default".into(),
                name: "Default".into(),
                efforts: Vec::new(),
                resolved_id: Some("claude-opus-5-5".into()),
            },
            nexus_workbench::agent_models::Model {
                id: "opus".into(),
                name: "Opus 5.5".into(),
                efforts: Vec::new(),
                resolved_id: Some("claude-opus-5-5".into()),
            },
            nexus_workbench::agent_models::Model {
                id: "gpt-6.1-sol".into(),
                name: "GPT-6.1 Sol".into(),
                efforts: Vec::new(),
                resolved_id: None,
            },
        ];
        assert_eq!(super::session_model_label(&catalog, "claude-opus-5-5"), "Opus 5.5");
        assert_eq!(
            super::session_model_label(&catalog, "anthropic/claude-opus-5-5"),
            "Opus 5.5"
        );
        assert_eq!(super::session_model_label(&catalog, "gpt-6.1-sol"), "GPT-6.1 Sol");
        assert_eq!(super::session_model_label(&[], "claude-opus-5-5"), "Opus 5.5");
    }
}
