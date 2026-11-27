//! The Assistant's rules, for every desktop shell: the person's own agent, the one
//! `@nexus` is in Slack, through the agent API. Nexus
//! GPUI draws it natively (`desktop-gpui/src/agent.rs`).
//!
//! The agent API is its own service, not Gateway. Its base URL is `NEXUS_AGENTS_API_URL`, else the `agentsApiUrl`
//! setting. Without one, the shells hide the Assistant. With `NEXUS_AGENT_VIA_GATEWAY=on` the calls go through
//! Gateway's `/v1/agent/*` instead ([`target`]). This module holds what needs no toolkit: the URL rule, the
//! paths a shell may call (an ask, a conversation and the list of them, the memory, the approval settings, the
//! errands and the first open) and their ids, queries and bodies, the failures a
//! shell answers itself, the wire types and server-sent events (as `kit/src/agent-chat.ts`), the
//! words for each failure, and the confirm and notification texts.

use serde::Deserialize;
use serde_json::{Value, json};

pub const AGENTS_URL_VAR: &str = "NEXUS_AGENTS_API_URL";
pub const ASK_PATH: &str = "/agent/ask";
pub const CONVERSATIONS_PATH: &str = "/agent/conversations/";
/// `GET /agent/conversations?limit=&before=` lists the person's conversations from every surface (shared
/// conversations). The agent API answers 404 while that is off.
pub const CONVERSATIONS_LIST_PATH: &str = "/agent/conversations";
/// A page of the list holds at most this many (Nexus UI's `CONVERSATIONS_MAX_PAGE`).
pub const CONVERSATIONS_MAX_PAGE: u32 = 50;
/// What the agent keeps about the person: `GET` reads it, `DELETE` forgets every note.
pub const MEMORY_PATH: &str = "/agent/memory";
/// `DELETE /agent/memory/notes/{id}` forgets one note, and `PATCH` with `{"text"}` edits it.
pub const MEMORY_NOTES_PATH: &str = "/agent/memory/notes/";
/// `POST` with `{"text"}` adds a note. The agent API answers it only with its memory controls on.
pub const MEMORY_ADD_PATH: &str = "/agent/memory/notes";
/// `PUT` with `{"paused"}` pauses or resumes learning, with the memory controls on.
pub const MEMORY_LEARNING_PATH: &str = "/agent/memory/learning";
/// The agent API's longest note, in UTF-16 units as it counts them (`NOTE_CHARS` in the agent core's `recall.ts`).
pub const NOTE_CHARS: usize = 600;
/// The person's spend line and rules: `GET` reads them, `PUT` sets the line.
pub const APPROVALS_SETTINGS_PATH: &str = "/agent/approvals/settings";
/// `DELETE` revokes every "Always allow" rule.
pub const APPROVALS_RULES_PATH: &str = "/agent/approvals/rules";
/// `DELETE /agent/approvals/rules/{id}` revokes one rule.
pub const APPROVALS_RULE_PREFIX: &str = "/agent/approvals/rules/";
/// Your agent's first open: `GET` reads it, once per person. The agent API answers 404 while it is off.
pub const INTRO_PATH: &str = "/agent/intro";
/// `POST` keeps that the person saw the first open, on every app.
pub const INTRO_SEEN_PATH: &str = "/agent/intro/seen";
/// Today's morning brief: `GET` reads it, at the top of today's thread. 503 `brief_off` while it is off.
pub const BRIEF_PATH: &str = "/agent/brief/today";
/// The person's errands: `GET` lists them, and `POST` with `{"goal"}` starts one. The agent API answers
/// 503 `errands_off` while they are off.
pub const ERRANDS_PATH: &str = "/agent/errands";
/// The agent API's longest goal for a new errand, in characters (`ERRAND_GOAL_CHARS` in its `errands.ts`).
pub const ERRAND_GOAL_CHARS: usize = 500;
/// `GET /agent/errands/{id}` reads one errand; `POST` with an action confirms, closes or cancels it.
pub const ERRAND_PREFIX: &str = "/agent/errands/";
/// What `POST /agent/errands/{id}` may ask (`ERRAND_ACTIONS` in the agent API's `errands.ts`).
pub const ERRAND_ACTIONS: [&str; 3] = ["confirm", "done", "cancel"];
/// The longest action body a shell sends: `{"action":"confirm"}` and a little room.
const ERRAND_BODY_MAX: usize = 64;
/// `POST /agent/errands/{id}/messages` with `{"message"}` replies about one errand, in its own thread.
pub const ERRAND_MESSAGES_SUFFIX: &str = "/messages";
/// `GET /agent/errands/{id}/handoff` reads the errand as a session's prompt; `POST` with `{}` records the step
/// "Handed off to a session".
pub const ERRAND_HANDOFF_SUFFIX: &str = "/handoff";
/// The longest name an errand takes, in UTF-16 units as the agent core counts them (`ERRAND_TITLE_LIMIT` in its
/// `intents.ts`).
pub const ERRAND_TITLE_MAX: usize = 120;
/// The agent API's limit on a question (`MAX_MESSAGE_CHARS` in its `server.ts`).
pub const MAX_CHARS: usize = 8_000;
/// An answer that takes this long is a long one: when it ends and no app window has the focus, a notification says so.
pub const LONG_ANSWER_MS: u128 = 10_000;

/// The failure a shell answers when it has no agent API to ask (a machine-readable 503).
pub const UNCONFIGURED: (u16, &str, &str) = (
    503,
    "agents_api_unconfigured",
    "This app is not connected to the agent service.",
);
/// The failure a shell answers when the agent API does not answer.
pub const UNREACHABLE: (u16, &str, &str) = (
    502,
    "agents_api_unreachable",
    "The agent service did not answer. It may be off.",
);
/// The failure a shell answers when nobody is signed in.
pub const SIGNED_OUT: (u16, &str, &str) = (401, "unauthenticated", "Sign in first.");

/// The agent API's base URL, without a trailing slash: `env` first, then the setting. HTTPS, or HTTP to this computer
/// only, since the URL receives the person's token. None when neither is set or valid.
pub fn agents_url(env: Option<&str>, setting: Option<&str>) -> Option<String> {
    let value = [env, setting]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())?;
    let url = url::Url::parse(value).ok()?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    let allowed = url.scheme() == "https" || (url.scheme() == "http" && local);
    if !allowed || !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
        return None;
    }
    Some(value.trim_end_matches('/').to_string())
}

/// [`agents_url`] with this process's `NEXUS_AGENTS_API_URL`.
pub fn base_url(setting: Option<&str>) -> Option<String> {
    agents_url(std::env::var(AGENTS_URL_VAR).ok().as_deref(), setting)
}

/// Whether this app has an agent API to ask (through Gateway, or `env` or the setting, valid), read without a network
/// call. The sidebar shows the Assistant and the page opens only then, since the agent is not released yet.
pub fn configured(setting: Option<&str>) -> bool {
    via_gateway() || base_url(setting).is_some()
}

/// `on` sends every agent call through Gateway's `/v1/agent/*`, with the token this app already sends to
/// Gateway. Unset or anything else: straight to the agent API, as before. Gateway's proxy is off in production
/// (`NEXUS_AGENT_PROXY`), so this stays off there too.
pub const VIA_GATEWAY_VAR: &str = "NEXUS_AGENT_VIA_GATEWAY";

/// Whether `value` (of [`VIA_GATEWAY_VAR`]) turns the Gateway path on: `on` only.
pub fn via_gateway_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case("on"))
}

/// [`via_gateway_value`] with this process's `NEXUS_AGENT_VIA_GATEWAY`.
pub fn via_gateway() -> bool {
    via_gateway_value(std::env::var(VIA_GATEWAY_VAR).ok().as_deref())
}

/// Where one call goes, as `(base, path)`: through Gateway, Gateway's base and `/v1` before the agent API's path;
/// else the agent API's base and the path as it is. None: no agent API to ask.
pub fn target(via_gateway: bool, agents: Option<&str>, gateway: &str, path: &str) -> Option<(String, String)> {
    if via_gateway {
        return Some((gateway.trim_end_matches('/').to_string(), format!("/v1{path}")));
    }
    agents.map(|base| (base.to_string(), path.to_string()))
}

/// A refusal in Gateway's own shape (`{"error": {code, message}}`: 403 `not_a_person`, 502 `agents_api_unreachable`,
/// 504 `upstream_timeout`, …) in the agent API's (`{error, detail}`), so every reader of a failure reads both alike.
/// None for any other body, the agent API's own included: it passes through Gateway unchanged.
pub fn gateway_failure(body: &[u8]) -> Option<String> {
    let body: Value = serde_json::from_slice(body).ok()?;
    let error = body.get("error")?.as_object()?;
    let code = error.get("code").and_then(Value::as_str)?;
    let message = error.get("message").and_then(Value::as_str).unwrap_or_default();
    Some(failure_body(code, message))
}

/// A conversation id as the agent API writes it.
pub fn conversation_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// A note id as the agent API writes it (`NOTE_ID` in its `memory.ts`): 1 to 128 letters, digits, `.`, `_`, `:` or
/// `-`. Never `.` or `..`, which a URL reads as a step up the path.
pub fn note_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && !matches!(id, "." | "..")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

/// The body of an added or edited note as the pane writes it: a JSON object with only `text`, a string with some text
/// and at most [`NOTE_CHARS`] units. Anything else never reaches the agent API.
pub fn note_body(body: Option<&str>) -> bool {
    let Some(Value::Object(fields)) = body.and_then(|body| serde_json::from_str::<Value>(body).ok()) else {
        return false;
    };
    match (fields.len(), fields.get("text")) {
        (1, Some(Value::String(text))) => !text.trim().is_empty() && text.encode_utf16().count() <= NOTE_CHARS,
        _ => false,
    }
}

/// The body of the learning switch as the pane writes it: a JSON object with only `paused`, `true` or `false`.
pub fn learning_body(body: Option<&str>) -> bool {
    let Some(Value::Object(fields)) = body.and_then(|body| serde_json::from_str::<Value>(body).ok()) else {
        return false;
    };
    fields.len() == 1 && matches!(fields.get("paused"), Some(Value::Bool(_)))
}

/// A rule id as the agent API writes it (`RULE_ID` in the agent core's `tiers.ts`): `allow_` and 24 lowercase
/// hexadecimal digits.
pub fn rule_id(id: &str) -> bool {
    id.strip_prefix("allow_").is_some_and(|hex| {
        hex.len() == 24
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// An errand id as the agent API writes it (`ERRAND_ID` in its `errands.ts`): `i` and 1 to 6 digits.
pub fn errand_id(id: &str) -> bool {
    id.strip_prefix('i')
        .is_some_and(|digits| (1..=6).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Whether `path` is one errand's: the prefix and an id the agent API could have written, with nothing after it.
pub fn errand_path(path: &str) -> bool {
    path.strip_prefix(ERRAND_PREFIX).is_some_and(errand_id)
}

/// Whether `body` is exactly one errand action: a short JSON object whose only field is `action`, one of
/// [`ERRAND_ACTIONS`]. Anything else, a missing body included, is not.
pub fn errand_action(body: Option<&str>) -> bool {
    let Some(body) = body.filter(|body| body.len() <= ERRAND_BODY_MAX) else {
        return false;
    };
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    fields.len() == 1
        && fields
            .get("action")
            .and_then(Value::as_str)
            .is_some_and(|action| ERRAND_ACTIONS.contains(&action))
}

/// `…/memory/notes/{id}`, for a note id the agent API could have written.
pub fn note_path(id: &str) -> Option<String> {
    note_id(id).then(|| format!("{MEMORY_NOTES_PATH}{id}"))
}

/// `…/approvals/rules/{id}`, for a rule id the agent API could have written.
pub fn rule_path(id: &str) -> Option<String> {
    rule_id(id).then(|| format!("{APPROVALS_RULE_PREFIX}{id}"))
}

/// `…/errands/{id}`, for an errand id the agent API could have written.
pub fn errand_one_path(id: &str) -> Option<String> {
    errand_id(id).then(|| format!("{ERRAND_PREFIX}{id}"))
}

/// The longest spend-line body a shell sends: `{"spendLimitUsd":"9999999.99"}` and a little room.
const LINE_BODY_MAX: usize = 48;

/// Whether `body` is `PUT /agent/approvals/settings`'s body as a native view writes it: an object whose only field is
/// `spendLimitUsd`, either `null` (the default) or 1 to 7 digits with up to 2 decimals. Anything else is not.
pub fn line_body(body: Option<&str>) -> bool {
    let Some(body) = body.filter(|body| body.len() <= LINE_BODY_MAX) else {
        return false;
    };
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let amount = |text: &str| {
        let (whole, cents) = text.split_once('.').unwrap_or((text, "0"));
        (1..=7).contains(&whole.len())
            && (1..=2).contains(&cents.len())
            && whole.bytes().chain(cents.bytes()).all(|byte| byte.is_ascii_digit())
    };
    fields.len() == 1
        && match fields.get("spendLimitUsd") {
            Some(Value::Null) => true,
            Some(Value::String(text)) => amount(text),
            _ => false,
        }
}

/// The longest project name an errand is filed under, in UTF-16 units (`ERRAND_PROJECT_LIMIT` in the agent core's
/// `intents.ts`).
pub const ERRAND_PROJECT_MAX: usize = 60;

/// A project's name as the agent API reads it (`projectOf` in its `errands.ts`): its spaces collapsed, some text, and
/// at most [`ERRAND_PROJECT_MAX`] units. None for anything else.
pub fn project_name(value: &str) -> Option<String> {
    let named = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!named.is_empty() && named.encode_utf16().count() <= ERRAND_PROJECT_MAX).then_some(named)
}

/// A Gateway project's id as the agent API keeps one (`PROJECT_ID` in its `projects.ts`): 1 to 64 letters,
/// digits, `_` or `-`.
pub fn project_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// `GET /agent/projects`: the person's projects.
pub const PROJECTS_PATH: &str = "/agent/projects";
/// `GET /agent/projects/view?id=` or `?name=`: one project's page.
pub const PROJECT_VIEW_PATH: &str = "/agent/projects/view";
/// `POST /agent/projects/link` with `{ name, projectId }`: a name-only project binds to its Gateway project.
pub const PROJECT_LINK_PATH: &str = "/agent/projects/link";

/// One project's page path: by its Gateway id when it has one, else by its name, each as the agent API reads it.
pub fn project_view_path(id: Option<&str>, name: &str) -> Option<String> {
    if let Some(id) = id {
        return project_id(id).then(|| format!("{PROJECT_VIEW_PATH}?id={id}"));
    }
    let name = project_name(name)?;
    let query: String = url::form_urlencoded::byte_serialize(name.as_bytes()).collect();
    Some(format!("{PROJECT_VIEW_PATH}?name={query}"))
}

/// Whether `body` is a link's body as the pane writes it: exactly `name`, a project's name, and `projectId`.
pub fn project_link_body(body: Option<&str>) -> bool {
    let Some(Value::Object(fields)) = body.and_then(|body| serde_json::from_str::<Value>(body).ok()) else {
        return false;
    };
    fields.len() == 2
        && matches!(fields.get("name"), Some(Value::String(name)) if project_name(name).is_some())
        && matches!(fields.get("projectId"), Some(Value::String(id)) if project_id(id))
}

/// Whether `body` is a project note's body as the pane writes it: `text` as [`note_body`] takes it, and
/// `project`, an object with a project's `name` and, for a Gateway project, its `id`.
pub fn project_note_body(body: Option<&str>) -> bool {
    let Some(Value::Object(mut fields)) = body.and_then(|body| serde_json::from_str::<Value>(body).ok()) else {
        return false;
    };
    let Some(Value::Object(project)) = fields.remove("project") else {
        return false;
    };
    let named = matches!(project.get("name"), Some(Value::String(name)) if project_name(name).is_some());
    let id = match project.get("id") {
        None => true,
        Some(Value::String(id)) => project_id(id),
        Some(_) => false,
    };
    named
        && id
        && project.keys().all(|key| key == "name" || key == "id")
        && note_body(Some(&Value::Object(fields).to_string()))
}

/// Whether `body` is exactly a new errand's goal, as the pane writes it: a JSON object with `goal`, a string with some
/// text and at most [`ERRAND_GOAL_CHARS`] characters, and optionally `project`, the project it starts in, or
/// `projectId`, a Gateway project's id, never both. Anything else never reaches the agent API.
pub fn errand_goal(body: Option<&str>) -> bool {
    let Some(Value::Object(fields)) = body.and_then(|body| serde_json::from_str::<Value>(body).ok()) else {
        return false;
    };
    let goal = match fields.get("goal") {
        Some(Value::String(goal)) => !goal.trim().is_empty() && goal.chars().count() <= ERRAND_GOAL_CHARS,
        _ => false,
    };
    let project = match (fields.get("project"), fields.get("projectId")) {
        (None, None) => true,
        (Some(Value::String(project)), None) => project_name(project).is_some(),
        (None, Some(Value::String(id))) => project_id(id),
        _ => false,
    };
    goal && project
        && fields
            .keys()
            .all(|key| key == "goal" || key == "project" || key == "projectId")
}

/// `…/errands/{id}/messages`, for an errand id the agent API could have written.
pub fn errand_messages_path(id: &str) -> Option<String> {
    errand_id(id).then(|| format!("{ERRAND_PREFIX}{id}{ERRAND_MESSAGES_SUFFIX}"))
}

/// `…/errands/{id}/handoff`, for an errand id the agent API could have written.
pub fn errand_handoff_path(id: &str) -> Option<String> {
    errand_id(id).then(|| format!("{ERRAND_PREFIX}{id}{ERRAND_HANDOFF_SUFFIX}"))
}

/// Whether `path` is one errand's thread: `…/errands/{id}/messages`, for an id the agent API could have written.
pub fn errand_messages(path: &str) -> bool {
    path.strip_prefix(ERRAND_PREFIX)
        .and_then(|rest| rest.strip_suffix(ERRAND_MESSAGES_SUFFIX))
        .is_some_and(errand_id)
}

/// Whether `body` is a `PATCH /agent/errands/{id}` body as the agent API reads it (`parseErrandEdit`): an object with
/// one or more of `title` (1 to [`ERRAND_TITLE_MAX`] units once trimmed, or `null` to drop the name), `pinned` and
/// `archived` (true or false), `done` (true) and `project` (a name, or `null` to make it loose). Any other field,
/// or none, is not.
pub fn errand_edit(body: Option<&str>) -> bool {
    let Some(Value::Object(fields)) = body.and_then(|body| serde_json::from_str::<Value>(body).ok()) else {
        return false;
    };
    !fields.is_empty()
        && fields.iter().all(|(key, value)| match (key.as_str(), value) {
            ("title", Value::Null) => true,
            ("title", Value::String(title)) => {
                let title = title.trim();
                !title.is_empty() && title.encode_utf16().count() <= ERRAND_TITLE_MAX
            }
            ("pinned" | "archived", Value::Bool(_)) | ("done", Value::Bool(true)) | ("project", Value::Null) => true,
            ("project", Value::String(project)) => project_name(project).is_some(),
            _ => false,
        })
}

/// Whether `body` is a reply about an errand (`parseErrandMessage`): an object whose only field is `message`, with
/// some text and at most [`MAX_CHARS`] UTF-16 units.
pub fn errand_message(body: Option<&str>) -> bool {
    let Some(Value::Object(fields)) = body.and_then(|body| serde_json::from_str::<Value>(body).ok()) else {
        return false;
    };
    match (fields.len(), fields.get("message")) {
        (1, Some(Value::String(message))) => !message.trim().is_empty() && message.encode_utf16().count() <= MAX_CHARS,
        _ => false,
    }
}

/// `POST /agent/writes/{card}/approve` or `…/decline`: the person decides a write of theirs that waits (the
/// agent API's `writes.ts`).
pub const WRITES_PREFIX: &str = "/agent/writes/";
/// The longest decision body a shell sends: `{"always":true}` and a little room.
const WRITE_BODY_MAX: usize = 32;

/// A card id as the agent core writes it (`CARD_ID` in its `cards.ts`): `card_` and 32 lowercase hexadecimal digits.
pub fn card_id(id: &str) -> bool {
    id.strip_prefix("card_").is_some_and(|hex| {
        hex.len() == 32
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Whether `path` is one decision on one card: the prefix, a card id, then `/approve` or `/decline`, and nothing more.
pub fn write_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(WRITES_PREFIX) else {
        return false;
    };
    let Some((card, decision)) = rest.split_once('/') else {
        return false;
    };
    card_id(card) && matches!(decision, "approve" | "decline")
}

/// Whether `body` is a decision's body: `{}`, or `{"always":true}` on an approve. Anything else, a missing body
/// included, is not.
pub fn write_body(path: &str, body: Option<&str>) -> bool {
    let Some(body) = body.filter(|body| body.len() <= WRITE_BODY_MAX) else {
        return false;
    };
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    match fields.len() {
        0 => true,
        1 => path.ends_with("/approve") && fields.get("always") == Some(&Value::Bool(true)),
        _ => false,
    }
}

/// A cursor of the list as the apps admit it (Nexus UI's `CURSOR`): opaque, 1 to 256 letters, digits, `.`, `_`, `~`,
/// `:`, `=`, `+`, `/` or `-`.
pub fn cursor(value: &str) -> bool {
    (1..=256).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~:=+/-".contains(&byte))
}

/// A page size of the list: one or two digits, 1 to 50 (Nexus UI's `limitOf`).
fn page_size(value: &str) -> bool {
    (1..=2).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value
            .parse()
            .is_ok_and(|limit: u32| (1..=CONVERSATIONS_MAX_PAGE).contains(&limit))
}

/// Whether `path` is the list with a query a shell may send: nothing, or `limit` and `before`, each once and valid.
/// The query is form-encoded as the pane's `URLSearchParams` writes it, so its raw bytes are letters, digits and
/// `-._~*%=&+` only, with no empty pair. A `#`, a second `?` or a space never passes.
pub fn conversations_list(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(CONVERSATIONS_LIST_PATH) else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let Some(query) = rest.strip_prefix('?').filter(|query| !query.is_empty()) else {
        return false;
    };
    let raw = query
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"-._~*%=&+".contains(&byte));
    if !raw || query.split('&').any(str::is_empty) {
        return false;
    }
    let (mut limit, mut before) = (false, false);
    url::form_urlencoded::parse(query.as_bytes()).all(|(key, value)| match key.as_ref() {
        "limit" if !limit => {
            limit = true;
            page_size(&value)
        }
        "before" if !before => {
            before = true;
            cursor(&value)
        }
        _ => false,
    })
}

/// `GET /agent/conversations/{id}`'s path, for an id the agent API could have written.
pub fn conversation_path(id: &str) -> Option<String> {
    conversation_id(id).then(|| format!("{CONVERSATIONS_PATH}{id}"))
}

/// The body of an ask: streamed, in the conversation when there is one.
pub fn ask_body(message: &str, conversation: Option<&str>) -> String {
    ask_body_on(message, conversation, None)
}

/// [`ask_body`] from a named surface (`desktop` for the Mac chat bar), as the web asks from `web`. An agent API without
/// shared conversations ignores it.
pub fn ask_body_on(message: &str, conversation: Option<&str>, surface: Option<&str>) -> String {
    let mut body = json!({ "message": message, "stream": true });
    if let Some(surface) = surface.filter(|surface| !surface.is_empty()) {
        body["surface"] = json!(surface);
    }
    if let Some(conversation) = conversation.filter(|id| !id.is_empty()) {
        body["conversation"] = json!(conversation);
    }
    body.to_string()
}

/// A failure in the agent API's own shape (`{ error, detail }`).
pub fn failure_body(error: &str, detail: &str) -> String {
    json!({ "error": error, "detail": detail }).to_string()
}

/// A write the agent asks for, as the pane shows it in the confirm dialog.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedWrite {
    pub summary: String,
    pub connector: String,
    pub operation: String,
    pub effect: String,
    pub approve_url: String,
}

/// The confirm dialog's title.
pub const CONFIRM_TITLE: &str = "Nexus asks to make a change";
/// The confirm dialog's buttons: the first opens the approval page in the browser.
pub const CONFIRM_REVIEW: &str = "Review in Nexus";
pub const CONFIRM_LATER: &str = "Not now";

/// The dialog's words. The summary is the agent's; the rest says what happens next.
pub fn confirm_text(write: &ProposedWrite) -> String {
    let undo = if write.effect == "irreversible" {
        "It cannot be undone."
    } else {
        "It can be undone."
    };
    format!(
        "{}\n\n{}.{}. {undo}\n\nNothing changes until you approve it on its page in Nexus.",
        write.summary.trim(),
        write.connector,
        write.operation
    )
}

/// What a notification says: that something waits, never what. `approval` or `answer`.
pub fn notice(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "answer" => Some(("Nexus answered", "Open Nexus to read it.")),
        "approval" => Some(("Nexus needs your approval", "Open Nexus to review it.")),
        _ => None,
    }
}

/// [`notice`] with the Assistant's name in its title. The body names the app, so it stays "Open Nexus".
pub fn named_notice(kind: &str, assistant: &str) -> Option<(String, &'static str)> {
    match kind {
        "answer" => Some((format!("{assistant} answered"), "Open Nexus to read it.")),
        "approval" => Some((format!("{assistant} needs your approval"), "Open Nexus to review it.")),
        _ => None,
    }
}

/// The pane's page, to open on a click: `/app/<org>/agent`, with the conversation when there is one.
pub fn pane_path(org: &str, conversation: Option<&str>) -> String {
    let enc = nexus_client::url::enc;
    match conversation.filter(|id| conversation_id(id)) {
        Some(id) => format!("/app/{}/agent?c={}", enc(org), enc(id)),
        None => format!("/app/{}/agent", enc(org)),
    }
}

/// A link a reply may open: http(s) only, so a `javascript:` URL from a reply never becomes a link.
pub fn safe_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.to_string())
}

/* ------------------------------------------------------------------ the wire */

/// A connector write the agent asked for. The API never runs it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Write {
    pub connector: String,
    pub operation: String,
    pub summary: String,
    pub effect: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Choice {
    pub keyword: String,
    #[serde(default)]
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCost {
    pub answers: u32,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Note {
    pub text: String,
}

/// One message of a reply (`kit`'s `AgentMessage`). A kind this build does not know is `Unknown`, and draws nothing.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    #[serde(rename_all = "camelCase")]
    Answer {
        /// Standard Markdown. Untrusted model output.
        text: String,
        #[serde(default)]
        model: String,
        #[serde(default)]
        cost_usd: Option<f64>,
        #[serde(default)]
        choice: Option<Choice>,
        #[serde(default)]
        notes_used: Option<u32>,
        #[serde(default)]
        conversation: Option<ConversationCost>,
        #[serde(default)]
        ai_notice: bool,
        #[serde(default)]
        sources: Vec<Source>,
        /// The connectors the answer needed and the person has not connected, as the API sent them. Read them
        /// with `agent_cards::answer_connects`, which drops an entry that is not one; a bad entry never drops the answer.
        #[serde(default)]
        connect: Value,
        /// The person's coding sessions the answer read or set up, as the API sent them. Read them with
        /// `agent_thread::answer_sessions`.
        #[serde(default)]
        sessions: Value,
        /// Up to three quick replies, as the API sent them. Read them with `agent_thread::answer_suggestions`.
        #[serde(default)]
        suggestions: Value,
    },
    #[serde(rename_all = "camelCase")]
    Refused {
        #[serde(default)]
        detail: String,
        #[serde(default)]
        billing_url: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    PendingAction {
        write: Write,
        #[serde(default)]
        approve_url: String,
        #[serde(default)]
        detail: String,
        /// The card's id, which a decision in the chat names.
        #[serde(default)]
        card: String,
        /// The write's card (`asks`), as the API sent it. Read it with `agent_cards::write_ask`.
        #[serde(default)]
        asks: Option<Value>,
        /// When the write stops waiting (ISO 8601).
        #[serde(default)]
        expires_at: Option<String>,
        /// What the person may decide in the chat: `approve-or-decline` or `decline-only`. None: only on its page.
        #[serde(default)]
        in_chat: Option<String>,
        /// The fields the person may change. Read them with `agent_cards::write_edit_fields`.
        #[serde(default)]
        edit: Option<Value>,
        /// The person edited the write it replaces.
        #[serde(default)]
        edited: bool,
    },
    /// A notice. `topic` and `state` say what it is about: the "I opened an errand" notice (`errands`, `opened`)
    /// carries its `errand`, which `agent_thread::notice_errand` reads.
    Notice {
        #[serde(default)]
        text: String,
        #[serde(default)]
        topic: String,
        #[serde(default)]
        state: String,
        #[serde(default)]
        errand: Value,
        /// The proposal of a watch notice (`watches`, `proposed`), which `agent_watches::notice_proposal` reads.
        #[serde(default)]
        watch: Value,
    },
    Memories {
        #[serde(default)]
        enabled: bool,
        #[serde(default)]
        notes: Vec<Note>,
    },
    Forgotten {
        #[serde(default)]
        count: u32,
    },
    Insight {
        #[serde(default)]
        text: String,
        #[serde(default)]
        url: String,
    },
    #[serde(rename_all = "camelCase")]
    Error {
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        cut: bool,
    },
    #[serde(other)]
    Unknown,
}

/// `POST /agent/ask`'s answer, and the `done` event's data.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reply {
    pub status: u16,
    pub conversation: String,
    pub messages: Vec<Message>,
}

impl Reply {
    fn from_json(body: &Value) -> Reply {
        Reply {
            status: body
                .get("status")
                .and_then(Value::as_u64)
                .and_then(|status| u16::try_from(status).ok())
                .unwrap_or(200),
            conversation: body
                .get("conversation")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            messages: body
                .get("messages")
                .and_then(Value::as_array)
                .map(|messages| {
                    messages
                        .iter()
                        .filter_map(|message| serde_json::from_value(message.clone()).ok())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// One turn of `GET /agent/conversations/{id}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub role: String,
    pub text: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// When it was said, as the API writes it (RFC 3339). The desktop thread's time lines read it.
    #[serde(default)]
    pub at: Option<String>,
}

impl Turn {
    /// A turn read back, as an answer the pane draws. A turn keeps no sources.
    pub fn answer(&self) -> Message {
        Message::Answer {
            text: self.text.clone(),
            model: self.model.clone().unwrap_or_default(),
            cost_usd: self.cost_usd,
            choice: None,
            notes_used: None,
            conversation: None,
            ai_notice: false,
            sources: Vec::new(),
            connect: Value::Null,
            sessions: Value::Null,
            suggestions: Value::Null,
        }
    }
}

/// The turns of `GET /agent/conversations/{id}`'s body.
pub fn turns(body: &Value) -> Vec<Turn> {
    body.get("turns")
        .and_then(Value::as_array)
        .map(|turns| {
            turns
                .iter()
                .filter_map(|turn| serde_json::from_value(turn.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The agent API's events: `started`, `model`, `text` (each new piece) and `step` (the tool that runs now, as one line
/// such as "Searching Linear"), then `done` or `error`.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Started { conversation: String },
    Model { model: String },
    Text { delta: String },
    Step { line: String },
    Done { reply: Reply },
    Error { status: u16, error: String, detail: String },
}

/// Reads server-sent events as they arrive (`kit`'s `AgentEventReader`). `push` takes the next bytes and returns the
/// events they completed; the rest waits. Comment lines (`: keepalive`) and unknown events are skipped.
#[derive(Debug, Default)]
pub struct EventReader {
    /// Bytes not yet text: an incomplete UTF-8 character, or a `\r` that may start a `\r\n`.
    bytes: Vec<u8>,
    /// Text not yet a whole block.
    text: String,
}

impl EventReader {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.bytes.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&self.bytes) {
            Ok(_) => self.bytes.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => self.bytes.len(),
        };
        let rest = self.bytes.split_off(valid);
        let mut chunk = String::from_utf8_lossy(&std::mem::replace(&mut self.bytes, rest)).into_owned();
        if chunk.ends_with('\r') {
            chunk.pop();
            self.bytes.insert(0, b'\r');
        }
        self.text.push_str(&chunk.replace("\r\n", "\n").replace('\r', "\n"));
        let mut events = Vec::new();
        while let Some(end) = self.text.find("\n\n") {
            let block: String = self.text.drain(..end + 2).collect();
            if let Some(event) = parse_block(&block[..end]) {
                events.push(event);
            }
        }
        events
    }

    /// The stream ended: a last block without its blank line still counts, as `readAgentStream` reads it.
    pub fn finish(&mut self) -> Vec<Event> {
        self.push(b"\n\n")
    }
}

fn parse_block(block: &str) -> Option<Event> {
    let mut name = "message";
    let mut data: Vec<&str> = Vec::new();
    for line in block.split('\n') {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => name = value,
            "data" => data.push(value),
            _ => {}
        }
    }
    if data.is_empty() {
        return None;
    }
    let body: Value = serde_json::from_str(&data.join("\n")).ok()?;
    if !body.is_object() {
        return None;
    }
    let text = |key: &str| body.get(key).and_then(Value::as_str).map(String::from);
    match name {
        "started" => Some(Event::Started {
            conversation: text("conversation")?,
        }),
        "model" => Some(Event::Model { model: text("model")? }),
        "text" => Some(Event::Text { delta: text("delta")? }),
        "step" => Some(Event::Step {
            line: step_line(body.get("line"))?,
        }),
        "done" => Some(Event::Done {
            reply: Reply::from_json(&body),
        }),
        "error" => Some(Event::Error {
            status: body
                .get("status")
                .and_then(Value::as_u64)
                .and_then(|status| u16::try_from(status).ok())
                .unwrap_or(502),
            error: text("error").unwrap_or_else(|| "error".into()),
            detail: text("detail").unwrap_or_default(),
        }),
        _ => None,
    }
}

/// The longest step line an app shows (the kit's `AGENT_STEP_CHARS`).
pub const STEP_CHARS: usize = 80;

/// A step line, read strictly as the kit's `stepLineOf`: plain text on one line, at most [`STEP_CHARS`] once trimmed.
/// None for anything else, so a bad line never reaches the thread.
pub fn step_line(value: Option<&Value>) -> Option<String> {
    let line = value?.as_str()?;
    if line.chars().any(control) {
        return None;
    }
    let line = line.trim();
    (!line.is_empty() && js_len(line) <= STEP_CHARS).then(|| line.to_string())
}

/// A control character, as the kit's `/[\u0000-\u001f\u007f]/` finds one.
pub(crate) fn control(c: char) -> bool {
    c <= '\u{1f}' || c == '\u{7f}'
}

/// A string's length as JavaScript counts it (UTF-16 units), so a limit here is the kit's limit.
pub(crate) fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/* ------------------------------------------------------------------ what the pane says */

/// What the pane says when an ask fails.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub title: String,
    pub detail: String,
}

/// The pane's words for a failed ask: the shell's own two failures first, then the
/// agent API's, worded as `kit`'s `agentProblem`.
pub fn problem(status: u16, error: Option<&str>, detail: Option<&str>) -> Problem {
    let code = error.unwrap_or("");
    let detail = detail.unwrap_or("");
    let said = |title: &str, words: &str| Problem {
        title: title.into(),
        detail: words.into(),
    };
    let or = |fallback: &str| {
        if detail.is_empty() {
            fallback.to_string()
        } else {
            detail.to_string()
        }
    };
    match (status, code) {
        (503, "agents_api_unconfigured") => said(
            "Nexus is not available here",
            "This app is not connected to the agent service. Use @nexus in Slack for now.",
        ),
        (502, "agents_api_unreachable") => said(
            "Nexus is off",
            "The agent service did not answer. Try again later, or use @nexus in Slack.",
        ),
        (401, _) => said(
            "Sign in again",
            "Your sign-in ended. Sign in to the app again, then ask again.",
        ),
        (403, _) => said(
            "Nexus cannot answer with this sign-in",
            &or("Nexus refused this sign-in."),
        ),
        (429, _) => said("Too many questions at once", "Wait a moment, then ask again."),
        (504, _) => said(
            "The answer took too long",
            "It may still be billed. Ask again to get a new answer.",
        ),
        (503, _) => said("Nexus is busy", &or("Ask again in a moment.")),
        (409, _) => said("This question was already asked", "Ask it again to get a new answer."),
        (400 | 413, _) => said(
            "The question was not sent",
            &or("Check the question and send it again."),
        ),
        _ => said(
            "Nexus did not answer",
            &or("The app could not reach Nexus. Try again in a moment."),
        ),
    }
}

/// [`problem`] from a failure's JSON body (`{ error, detail }`), when there is one.
pub fn problem_of(status: u16, body: Option<&Value>) -> Problem {
    let text = |key: &str| body.and_then(|body| body.get(key)).and_then(Value::as_str);
    problem(status, text("error"), text("detail"))
}

/// A stream that ended before its `done` or `error` event.
pub fn cut_problem() -> Problem {
    Problem {
        title: "The answer stopped before it finished".into(),
        detail: "Ask again to get a new answer.".into(),
    }
}

/// A write the agent asks for, for the native confirm; None when its approval link is not a web link.
pub fn proposed_write(message: &Message) -> Option<ProposedWrite> {
    let Message::PendingAction { write, approve_url, .. } = message else {
        return None;
    };
    Some(ProposedWrite {
        summary: write.summary.clone(),
        connector: write.connector.clone(),
        operation: write.operation.clone(),
        effect: write.effect.clone(),
        approve_url: safe_url(approve_url)?,
    })
}

/// What to announce when a reply ends: an approval always, an answer only after a long wait. The shell shows it only
/// when no app window has the focus, and never with the reply's words.
pub fn notice_for(reply: &Reply, elapsed_ms: u128) -> Option<&'static str> {
    let pending = reply
        .messages
        .iter()
        .any(|message| matches!(message, Message::PendingAction { .. }));
    if pending {
        return Some("approval");
    }
    let answered = reply
        .messages
        .iter()
        .any(|message| matches!(message, Message::Answer { .. }));
    (elapsed_ms >= LONG_ANSWER_MS && answered).then_some("answer")
}

/// A model's short name, as Nexus UI's `modelName`: what follows the last `/`.
pub fn model_name(model: &str) -> &str {
    match model.rfind('/') {
        Some(at) if at + 1 < model.len() => &model[at + 1..],
        _ => model,
    }
}

/// Dollars as Nexus UI's `usd`: cents when whole, else four places, six under a cent.
pub fn usd(value: f64) -> String {
    let abs = value.abs();
    if abs == 0. {
        return "$0.00".into();
    }
    if abs >= 100. || (value * 100. - (value * 100.).round()).abs() < 1e-9 {
        let cents = format!("{abs:.2}");
        let (whole, fraction) = cents.split_once('.').unwrap_or((&cents, "00"));
        let mut grouped = String::new();
        for (ix, digit) in whole.chars().enumerate() {
            if ix > 0 && (whole.len() - ix).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(digit);
        }
        let sign = if value < 0. { "-" } else { "" };
        return format!("{sign}${grouped}.{fraction}");
    }
    if abs >= 0.01 {
        format!("${value:.4}")
    } else {
        format!("${value:.6}")
    }
}

/// The line under an answer (Nexus UI's `answerFooter`): the model, why it ran, the cost, the notes it used and the
/// conversation's total.
pub fn answer_footer(message: &Message) -> Vec<String> {
    let Message::Answer {
        model,
        cost_usd,
        choice,
        notes_used,
        conversation,
        ..
    } = message
    else {
        return Vec::new();
    };
    let why = choice.as_ref().and_then(|choice| {
        if choice.default {
            Some("your default")
        } else {
            match choice.keyword.as_str() {
                "fast" => Some("you asked for fast"),
                "best" => Some("you asked for best"),
                "model" => Some("you picked it"),
                _ => None,
            }
        }
    });
    let notes = notes_used
        .filter(|count| *count > 0)
        .map(|count| format!("{count} {} about you", if count == 1 { "note" } else { "notes" }));
    let total = conversation
        .as_ref()
        .filter(|conversation| conversation.answers > 1)
        .map(|conversation| format!("{} in this conversation", usd(conversation.cost_usd)));
    [
        (!model.is_empty()).then(|| model_name(model).to_string()),
        why.map(String::from),
        cost_usd.filter(|cost| cost.is_finite()).map(usd),
        notes,
        total,
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_is_https_or_this_computer() {
        assert_eq!(
            agents_url(Some("https://agents.nexus.test/"), None).as_deref(),
            Some("https://agents.nexus.test")
        );
        assert_eq!(
            agents_url(None, Some("http://localhost:8081")).as_deref(),
            Some("http://localhost:8081")
        );
        for refused in [
            "http://agents.nexus.test",
            "https://u:p@a.test",
            "https://a.test/?k=1",
            "nope",
        ] {
            assert_eq!(agents_url(Some(refused), None), None, "{refused}");
        }
    }

    /// `NEXUS_AGENT_VIA_GATEWAY` is off unless it says `on`: off, a call goes to the agent API as before.
    #[test]
    fn the_gateway_path_is_off_by_default() {
        for off in [None, Some(""), Some("off"), Some("1"), Some("true")] {
            assert!(!via_gateway_value(off), "{off:?}");
        }
        assert!(via_gateway_value(Some(" On ")));
        assert_eq!(
            target(false, Some("https://agents.test"), "https://api.test", ASK_PATH),
            Some(("https://agents.test".into(), "/agent/ask".into()))
        );
        assert_eq!(target(false, None, "https://api.test", ASK_PATH), None);
    }

    /// On, every call goes to Gateway's `/v1/agent/*`, whatever the agent API's URL.
    #[test]
    fn on_a_call_goes_through_gateway() {
        for agents in [None, Some("https://agents.test")] {
            assert_eq!(
                target(true, agents, "https://api.test/", "/agent/errands/i1?x=1"),
                Some(("https://api.test".into(), "/v1/agent/errands/i1?x=1".into()))
            );
        }
    }

    /// Gateway's own refusals read as the agent API's: 502 `agents_api_unreachable` is the shell's own "Nexus is off".
    #[test]
    fn a_gateway_refusal_reads_as_the_agent_apis() {
        let unreachable = br#"{"error":{"message":"the agent service did not answer","type":"api_error","code":"agents_api_unreachable"}}"#;
        let body: Value = serde_json::from_str(&gateway_failure(unreachable).expect("a refusal")).expect("json");
        assert_eq!(
            body,
            json!({ "error": "agents_api_unreachable", "detail": "the agent service did not answer" })
        );
        assert_eq!(problem_of(502, Some(&body)).title, "Nexus is off");
        for (status, code) in [(504, "upstream_timeout"), (403, "not_a_person")] {
            let gateway = json!({ "error": { "message": "no", "code": code } }).to_string();
            let body: Value =
                serde_json::from_str(&gateway_failure(gateway.as_bytes()).expect("a refusal")).expect("json");
            assert_eq!(body["error"], code);
            assert_ne!(problem_of(status, Some(&body)).title, "Nexus did not answer");
        }
        // The agent API's own shape, and anything else, stays as it is.
        assert_eq!(gateway_failure(br#"{"error":"errands_off","detail":"off"}"#), None);
        assert_eq!(gateway_failure(b"not json"), None);
    }

    #[test]
    fn a_conversation_path_takes_only_an_api_id() {
        assert_eq!(
            conversation_path("c-1.a_b").as_deref(),
            Some("/agent/conversations/c-1.a_b")
        );
        assert_eq!(conversation_path("../v1/keys"), None);
        assert_eq!(conversation_path(""), None);
    }

    #[test]
    fn the_list_takes_only_a_page_size_and_a_cursor() {
        for admitted in [
            "/agent/conversations",
            "/agent/conversations?limit=20",
            "/agent/conversations?limit=50&before=cur_2",
            "/agent/conversations?before=2026-11-09T10%3A00%3A00.000Z&limit=1",
            "/agent/conversations?limit=20&before=YWJj%2BZA%3D%3D",
            "/agent/conversations?before=a%2Fb~c",
        ] {
            assert!(conversations_list(admitted), "{admitted}");
        }
        for refused in [
            "/agent/conversations?",
            "/agent/conversations/",
            "/agent/conversationsx",
            "/agent/conversations?limit=0",
            "/agent/conversations?limit=51",
            "/agent/conversations?limit=020",
            "/agent/conversations?limit=2.5",
            "/agent/conversations?limit=",
            "/agent/conversations?limit=20&limit=30",
            "/agent/conversations?before=",
            "/agent/conversations?before=a+b",
            "/agent/conversations?before=a%20b",
            "/agent/conversations?before=a%26x%3D1",
            "/agent/conversations?limit=20&org=acme",
            "/agent/conversations?limit=20#x",
            "/agent/conversations?limit=20?x",
            "/agent/conversations?limit=20&before=../v1/keys",
            "/agent/conversations?limit=20&",
            "/agent/conversations?&limit=20",
            "/agent/conversations?before=2026-11-09T10%3A00%3A00Z%7Cc_1",
            "/agent/conversations?limit=20 ",
        ] {
            assert!(!conversations_list(refused), "{refused}");
        }
        let long = format!("/agent/conversations?before={}", "c".repeat(257));
        assert!(!conversations_list(&long));
        assert!(conversations_list(&long.replace(&"c".repeat(257), &"c".repeat(256))));
    }

    #[test]
    fn a_write_decision_is_one_card_one_decision_and_its_own_body() {
        let card = format!("card_{}", "a".repeat(32));
        for decision in ["approve", "decline"] {
            let path = format!("/agent/writes/{card}/{decision}");
            assert!(write_path(&path), "{path}");
            assert!(write_body(&path, Some("{}")), "{path}");
        }
        let approve = format!("/agent/writes/{card}/approve");
        let decline = format!("/agent/writes/{card}/decline");
        assert!(write_body(&approve, Some(r#"{"always":true}"#)));
        assert!(!write_body(&decline, Some(r#"{"always":true}"#)));
        for body in [
            None,
            Some(""),
            Some("null"),
            Some(r#"{"always":false}"#),
            Some(r#"{"user":"usr_bob"}"#),
            Some(r#"{"always":true,"x":1}"#),
        ] {
            assert!(!write_body(&approve, body), "{body:?}");
        }
        for refused in [
            format!("/agent/writes/{card}/confirm"),
            format!("/agent/writes/{card}/approve/x"),
            format!("/agent/writes/{card}"),
            format!("/agent/writes/{}/approve", "card_".to_string() + &"A".repeat(32)),
            "/agent/writes/card_1/approve".to_string(),
            format!("/agent/writes/../memory/{card}/approve"),
        ] {
            assert!(!write_path(&refused), "{refused}");
        }
    }

    /// The native views' paths (writes, errands, memory): an id the agent API could not have written never
    /// becomes a path.
    #[test]
    fn a_native_view_builds_only_paths_the_shell_allows() {
        assert_eq!(note_path("n_1:a").as_deref(), Some("/agent/memory/notes/n_1:a"));
        assert_eq!(note_path(".."), None);
        assert_eq!(note_path("a/b"), None);
        let rule = "allow_0123456789abcdef01234567";
        assert_eq!(rule_path(rule), Some(format!("/agent/approvals/rules/{rule}")));
        assert_eq!(rule_path("allow_../x"), None);
        assert_eq!(errand_one_path("i3").as_deref(), Some("/agent/errands/i3"));
        assert!(errand_one_path("i3").is_some_and(|path| errand_path(&path)));
        assert_eq!(errand_one_path("i3/../memory"), None);
    }

    /// The spend line's `PUT` carries only `spendLimitUsd`: an amount in dollars, or null for the default.
    #[test]
    fn the_spend_line_body_is_an_amount_or_null_and_nothing_else() {
        for body in [
            r#"{"spendLimitUsd":"50"}"#,
            r#"{"spendLimitUsd":"49.99"}"#,
            r#"{"spendLimitUsd":"9999999.9"}"#,
            r#"{"spendLimitUsd":null}"#,
        ] {
            assert!(line_body(Some(body)), "{body}");
        }
        for body in [
            None,
            Some(""),
            Some("{}"),
            Some(r#"{"spendLimitUsd":50}"#),
            Some(r#"{"spendLimitUsd":"-5"}"#),
            Some(r#"{"spendLimitUsd":"1.234"}"#),
            Some(r#"{"spendLimitUsd":"12345678"}"#),
            Some(r#"{"spendLimitUsd":"50","rules":[]}"#),
            Some(r#"["spendLimitUsd"]"#),
        ] {
            assert!(!line_body(body), "{body:?}");
        }
    }

    #[test]
    fn an_errand_is_only_one_the_api_writes_and_its_action_one_it_knows() {
        for id in ["i1", "i42", "i999999"] {
            assert!(errand_id(id), "{id}");
            assert!(errand_path(&format!("/agent/errands/{id}")), "{id}");
        }
        for refused in [
            "",
            "i",
            "1",
            "I3",
            "i1234567",
            "i3/",
            "i3?x=1",
            "i-1",
            "i3 ",
            "..",
            "i3/../memory",
        ] {
            assert!(!errand_id(refused), "{refused}");
            assert!(!errand_path(&format!("/agent/errands/{refused}")), "{refused}");
        }
        assert!(!errand_path("/agent/errands"));
        assert!(!errand_path("/agent/errandsi3"));
        for body in [
            r#"{"action":"confirm"}"#,
            r#"{"action":"done"}"#,
            r#" { "action" : "cancel" } "#,
        ] {
            assert!(errand_action(Some(body)), "{body}");
        }
        for refused in [
            r#"{"action":"archive"}"#,
            r#"{"action":"done","note":"x"}"#,
            r#"{"action":["done"]}"#,
            r#"["done"]"#,
            r#""done""#,
            "done",
            "",
        ] {
            assert!(!errand_action(Some(refused)), "{refused}");
        }
        assert!(!errand_action(None));
        let padded = format!(r#"{{"action":"done"{}}}"#, " ".repeat(64));
        assert!(!errand_action(Some(&padded)));
    }

    /// An errand's edit and its reply are only what the agent API reads.
    #[test]
    fn an_errand_edit_and_reply_are_only_what_the_api_reads() {
        assert_eq!(
            errand_messages_path("i3").as_deref(),
            Some("/agent/errands/i3/messages")
        );
        assert_eq!(errand_messages_path("i3/x"), None);
        assert_eq!(errand_handoff_path("i3").as_deref(), Some("/agent/errands/i3/handoff"));
        assert_eq!(errand_handoff_path("../i3"), None);
        assert!(errand_messages("/agent/errands/i3/messages"));
        assert!(!errand_messages("/agent/errands/i3"));
        assert!(!errand_messages("/agent/errands/x/messages"));
        let longest = format!(r#"{{"title":"{}"}}"#, "é".repeat(ERRAND_TITLE_MAX));
        for body in [
            r#"{"title":"Dinner"}"#,
            r#"{"title":null}"#,
            r#"{"pinned":false}"#,
            r#"{"archived":true,"pinned":true}"#,
            r#"{"done":true}"#,
            &longest,
        ] {
            assert!(errand_edit(Some(body)), "{body}");
        }
        let too_long = format!(r#"{{"title":"{}"}}"#, "t".repeat(ERRAND_TITLE_MAX + 1));
        for refused in [
            "{}",
            r#"{"title":"  "}"#,
            r#"{"title":3}"#,
            r#"{"done":false}"#,
            r#"{"pinned":"yes"}"#,
            r#"{"pinned":true,"goal":"x"}"#,
            "[]",
            &too_long,
        ] {
            assert!(!errand_edit(Some(refused)), "{refused}");
        }
        assert!(!errand_edit(None));
        assert!(errand_message(Some(r#"{"message":"book it"}"#)));
        let too_long = format!(r#"{{"message":"{}"}}"#, "m".repeat(MAX_CHARS + 1));
        for refused in [
            r#"{"message":" "}"#,
            r#"{"message":"a","x":1}"#,
            r#"{"text":"a"}"#,
            &too_long,
        ] {
            assert!(!errand_message(Some(refused)), "{refused}");
        }
    }

    #[test]
    fn a_new_errand_is_its_goal_alone_with_some_text() {
        let longest = format!(r#"{{"goal":"{}"}}"#, "é".repeat(ERRAND_GOAL_CHARS));
        for body in [
            r#"{"goal":"Dinner Friday"}"#,
            r#" { "goal" : "Gift for Ana" } "#,
            &longest,
        ] {
            assert!(errand_goal(Some(body)), "{body}");
        }
        let too_long = format!(r#"{{"goal":"{}"}}"#, "g".repeat(ERRAND_GOAL_CHARS + 1));
        for refused in [
            r#"{"goal":""}"#,
            r#"{"goal":"   "}"#,
            r#"{"goal":1}"#,
            r#"{"goal":"a","to":"bob"}"#,
            r#"{"action":"done"}"#,
            r#"["goal"]"#,
            "goal",
            "",
            &too_long,
        ] {
            assert!(!errand_goal(Some(refused)), "{refused}");
        }
        assert!(!errand_goal(None));
    }

    /// An errand starts in a project, and a `PATCH` files it in one or makes it loose. The name is
    /// what the agent API reads: its spaces collapsed, 1 to 60 units.
    #[test]
    fn an_errand_takes_a_project_as_the_api_reads_it() {
        assert_eq!(project_name("  Acme   Bio ").as_deref(), Some("Acme Bio"));
        assert_eq!(project_name("   "), None);
        assert_eq!(
            project_name(&"p".repeat(ERRAND_PROJECT_MAX)).map(|name| name.len()),
            Some(60)
        );
        assert_eq!(project_name(&"p".repeat(ERRAND_PROJECT_MAX + 1)), None);
        for body in [
            r#"{"goal":"Book the venue","project":"Fundraise"}"#,
            r#"{"project":"Fundraise","goal":"Book the venue"}"#,
        ] {
            assert!(errand_goal(Some(body)), "{body}");
        }
        let too_long = format!(r#"{{"goal":"g","project":"{}"}}"#, "p".repeat(ERRAND_PROJECT_MAX + 1));
        // A Gateway project by its id, alone.
        assert!(errand_goal(Some(r#"{"goal":"Book the venue","projectId":"prj_fund"}"#)));
        for refused in [
            r#"{"goal":"g","projectId":"prj_fund","project":"Fundraise"}"#,
            r#"{"goal":"g","projectId":"bad id"}"#,
            r#"{"goal":"g","project":""}"#,
            r#"{"goal":"g","project":null}"#,
            r#"{"goal":"g","project":7}"#,
            r#"{"project":"Fundraise"}"#,
            &too_long,
        ] {
            assert!(!errand_goal(Some(refused)), "{refused}");
        }
        for body in [
            r#"{"project":"Fundraise"}"#,
            r#"{"project":null}"#,
            r#"{"project":"F","pinned":true}"#,
        ] {
            assert!(errand_edit(Some(body)), "{body}");
        }
        for refused in [r#"{"project":""}"#, r#"{"project":["F"]}"#, r#"{"project":false}"#] {
            assert!(!errand_edit(Some(refused)), "{refused}");
        }
    }

    #[test]
    fn a_note_or_rule_id_is_only_one_the_api_writes() {
        assert!(note_id("dm:U1:1730000000.123-a_b"));
        assert!(note_id(&"n".repeat(128)));
        for refused in ["", ".", "..", "a/b", "a%2Fb", "a b", "a?b", &"n".repeat(129)] {
            assert!(!note_id(refused), "{refused}");
        }
        assert!(rule_id("allow_0123456789abcdef01234567"));
        for refused in [
            "allow_0123456789ABCDEF01234567",
            "allow_0123456789abcdef0123456",
            "allow_0123456789abcdef012345678",
            "deny_0123456789abcdef01234567",
            "allow_../0123456789abcdef0123",
        ] {
            assert!(!rule_id(refused), "{refused}");
        }
    }

    #[test]
    fn a_memory_change_takes_only_the_body_the_pane_writes() {
        assert!(note_body(Some(r#"{"text":"Lily is in 1st grade."}"#)));
        assert!(note_body(Some(&json!({ "text": "é".repeat(NOTE_CHARS) }).to_string())));
        for refused in [
            None,
            Some(""),
            Some("text=hi"),
            Some(r#"["text"]"#),
            Some(r#"{"text":"   "}"#),
            Some(r#"{"text":7}"#),
            Some(r#"{"text":"hi","audience":"channel"}"#),
            Some(r#"{"note":"hi"}"#),
        ] {
            assert!(!note_body(refused), "{refused:?}");
        }
        // An emoji is two UTF-16 units, as the agent API counts them.
        assert!(!note_body(Some(
            &json!({ "text": "😀".repeat(NOTE_CHARS / 2 + 1) }).to_string()
        )));
        assert!(!note_body(Some(
            &json!({ "text": "x".repeat(NOTE_CHARS + 1) }).to_string()
        )));

        assert!(learning_body(Some(r#"{"paused":true}"#)));
        assert!(learning_body(Some(r#"{"paused":false}"#)));
        for refused in [
            None,
            Some(r#"{"paused":"yes"}"#),
            Some(r#"{"paused":true,"all":true}"#),
            Some("true"),
            Some("{}"),
        ] {
            assert!(!learning_body(refused), "{refused:?}");
        }
    }

    #[test]
    fn an_ask_streams_in_its_conversation() {
        assert_eq!(
            ask_body("hi", Some("c1")),
            r#"{"message":"hi","stream":true,"conversation":"c1"}"#
        );
        assert_eq!(ask_body("hi", None), r#"{"message":"hi","stream":true}"#);
    }

    const RECORDED: &str = concat!(
        ": keepalive\n\n",
        "event: started\ndata: {\"conversation\":\"c1\"}\n\n",
        "event: model\ndata: {\"model\":\"anthropic/claude-sonnet\"}\n\n",
        "event: text\ndata: {\"delta\":\"Do\"}\n\n",
        "event: text\r\ndata: {\"delta\":\"ne. é\"}\r\n\r\n",
        "event: done\ndata: {\"status\":200,\"conversation\":\"c1\",\"messages\":[",
        "{\"kind\":\"answer\",\"text\":\"Done.\",\"model\":\"nexus/auto\",\"costUsd\":0.01,",
        "\"sources\":[{\"title\":\"Runbook\",\"url\":\"https://wiki.test/runbook\"}]},",
        "{\"kind\":\"later_kind\",\"x\":1}]}\n\n",
    );

    #[test]
    fn the_reader_reads_a_recorded_stream_in_any_pieces() {
        for size in [1, 3, 7, RECORDED.len()] {
            let mut reader = EventReader::default();
            let mut events = Vec::new();
            for piece in RECORDED.as_bytes().chunks(size) {
                events.extend(reader.push(piece));
            }
            events.extend(reader.finish());
            assert_eq!(events.len(), 5, "pieces of {size}");
            assert_eq!(
                events[0],
                Event::Started {
                    conversation: "c1".into()
                }
            );
            assert_eq!(events[3], Event::Text { delta: "ne. é".into() });
            let Event::Done { reply } = &events[4] else {
                panic!("done last")
            };
            assert_eq!(reply.messages.len(), 2);
            assert!(matches!(&reply.messages[0], Message::Answer { sources, .. } if sources[0].title == "Runbook"));
            assert_eq!(reply.messages[1], Message::Unknown);
        }
    }

    #[test]
    fn an_error_event_and_a_last_block_without_its_blank_line() {
        let mut reader = EventReader::default();
        let events = reader.push(b"event: error\ndata: {\"status\":429,\"error\":\"rate\"}");
        assert!(events.is_empty());
        assert_eq!(
            reader.finish(),
            vec![Event::Error {
                status: 429,
                error: "rate".into(),
                detail: String::new()
            }]
        );
    }

    #[test]
    fn the_pane_names_the_shells_failures_and_the_apis() {
        let (status, error, detail) = UNCONFIGURED;
        assert_eq!(
            problem(status, Some(error), Some(detail)).title,
            "Nexus is not available here"
        );
        let (status, error, detail) = UNREACHABLE;
        assert_eq!(problem(status, Some(error), Some(detail)).title, "Nexus is off");
        assert_eq!(
            problem(401, None, None).detail,
            "Your sign-in ended. Sign in to the app again, then ask again."
        );
        assert_eq!(problem(429, None, None).title, "Too many questions at once");
        assert_eq!(problem(403, None, Some("Use a key.")).detail, "Use a key.");
        assert_eq!(problem(500, None, None).title, "Nexus did not answer");
    }

    fn pending(url: &str) -> Message {
        serde_json::from_value(json!({
            "kind": "pending_action", "card": "c1", "approveUrl": url, "detail": "Approve it in Nexus.",
            "write": { "connector": "slack", "operation": "post_message", "summary": "Post in #general",
                       "effect": "irreversible", "arguments": {} }
        }))
        .expect("a pending action")
    }

    #[test]
    fn only_a_write_with_a_web_link_is_confirmed() {
        assert_eq!(
            proposed_write(&pending("https://nexus.test/app/approvals/a1")).map(|write| write.approve_url),
            Some("https://nexus.test/app/approvals/a1".into())
        );
        assert_eq!(proposed_write(&pending("javascript:alert(1)")), None);
    }

    #[test]
    fn an_approval_is_announced_always_and_an_answer_after_a_long_wait() {
        let answer = Turn {
            role: "assistant".into(),
            text: "Done.".into(),
            model: None,
            cost_usd: None,
            at: None,
        }
        .answer();
        let reply = |messages| Reply {
            status: 200,
            conversation: "c1".into(),
            messages,
        };
        assert_eq!(
            notice_for(&reply(vec![pending("https://n.test/a")]), 0),
            Some("approval")
        );
        assert_eq!(notice_for(&reply(vec![answer.clone()]), LONG_ANSWER_MS - 1), None);
        assert_eq!(notice_for(&reply(vec![answer]), LONG_ANSWER_MS), Some("answer"));
    }

    #[test]
    fn the_footer_reads_as_nexus_uis() {
        let answer: Message = serde_json::from_value(json!({
            "kind": "answer", "text": "x", "model": "anthropic/claude-sonnet", "costUsd": 0.0123,
            "choice": { "keyword": "fast" }, "notesUsed": 1, "conversation": { "answers": 2, "costUsd": 1234.5 }
        }))
        .expect("an answer");
        assert_eq!(
            answer_footer(&answer),
            [
                "claude-sonnet",
                "you asked for fast",
                "$0.0123",
                "1 note about you",
                "$1,234.50 in this conversation"
            ]
        );
        assert_eq!(usd(0.5), "$0.50");
        assert_eq!(usd(0.000_12), "$0.000120");
    }

    /// A notification names the Assistant as the person named it, and the app as Nexus.
    #[test]
    fn a_notice_names_the_assistant_and_the_app() {
        assert_eq!(
            named_notice("answer", "Ada"),
            Some(("Ada answered".to_string(), "Open Nexus to read it."))
        );
        assert_eq!(
            named_notice("approval", "Nexus").map(|(title, _)| title).as_deref(),
            notice("approval").map(|(title, _)| title)
        );
        assert_eq!(named_notice("the answer text", "Ada"), None);
    }

    /// A `step` event is read strictly, as the kit's `stepLineOf`. A bad line is no event at all.
    #[test]
    fn a_step_event_is_one_plain_line_or_nothing() {
        let step = |data: &str| {
            let mut reader = EventReader::default();
            reader.push(format!("event: step\ndata: {data}\n\n").as_bytes())
        };
        assert_eq!(
            step(r#"{"line":"  Searching Linear "}"#),
            vec![Event::Step {
                line: "Searching Linear".into()
            }]
        );
        let long = format!(r#"{{"line":"{}"}}"#, "a".repeat(STEP_CHARS));
        assert_eq!(step(&long).len(), 1, "80 characters is the limit");
        let too_long = format!(r#"{{"line":"{}"}}"#, "a".repeat(STEP_CHARS + 1));
        for refused in [
            r#"{"line":""}"#,
            r#"{"line":"   "}"#,
            r#"{"line":"two\nlines"}"#,
            r#"{"line":"tab\there"}"#,
            r#"{"line":"del\u007f"}"#,
            r#"{"line":42}"#,
            r#"{"line":["Searching"]}"#,
            r#"{}"#,
            too_long.as_str(),
        ] {
            assert!(step(refused).is_empty(), "{refused}");
        }
        // UTF-16 units, as the kit counts them: 40 emoji are 80 units.
        assert_eq!(step(&format!(r#"{{"line":"{}"}}"#, "🙂".repeat(40))).len(), 1);
        assert!(step(&format!(r#"{{"line":"{}"}}"#, "🙂".repeat(41))).is_empty());
    }
}
