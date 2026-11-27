//! The Assistant's cards and first open for a native shell, read strictly from the agent API's answers, with their
//! words. Nexus GPUI draws them (`desktop-gpui/src/pages/agent.rs`). Each part names the kit
//! module it follows.
//!
//! - The Connect card (`kit/src/agent-chat.ts`'s `answerConnects`): a connector the answer needed.
//! - The approval card (the agent API's `PendingAsk`): what a waiting write shows.
//! - The first open (`kit/src/agent-intro.ts`): `GET /agent/intro`'s body.
//! - The Assistant's name (Nexus UI's `lib/assistant-name.ts`): the person's choice, Nexus by default.
//! - The exact text in its parts and the two voices (the kit's `agent-writes.ts`).

use serde_json::Value;

use crate::agent::{Message, safe_url};

/* ------------------------------------------------------------ the Assistant's name */

/// The Assistant's name when the person chose none, or the Gateway keeps no profiles.
pub const DEFAULT_ASSISTANT_NAME: &str = "Nexus";

/// The person's own Assistant's name, from `GET /v1/agents/profiles`'s body, as Nexus UI's `assistantNameOf`: the
/// `name` of the profile whose owner is the person, trimmed; Nexus without one. "Nexus" stays where it names the
/// product, not the agent: "Sent using Nexus", "Review in Nexus".
pub fn assistant_name_of(body: Option<&Value>) -> String {
    body.and_then(|body| body.get("data"))
        .and_then(Value::as_array)
        .and_then(|profiles| {
            profiles
                .iter()
                .find(|profile| profile.get("owner").and_then(Value::as_str) == Some("person"))
        })
        .and_then(|profile| profile.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(DEFAULT_ASSISTANT_NAME)
        .to_string()
}

/* ------------------------------------------------------------ the Connect card */

/// A connector the answer needed and the person has not connected.
#[derive(Debug, Clone, PartialEq)]
pub struct Connect {
    /// Its id in Gateway's catalogue, such as `linear`.
    pub connector: String,
    /// Its name as a person reads it, such as `Linear`.
    pub label: String,
    /// Where the person connects it: its card in Nexus UI. http(s) only.
    pub url: String,
    /// What Nexus tried to do with it.
    pub summary: Option<String>,
}

/// The longest summary a Connect card shows.
const CONNECT_SUMMARY_CHARS: usize = 200;
/// The most Connect cards one answer shows.
const MAX_CONNECTS: usize = 4;

/// A connector id as Gateway names one: a letter or digit, then up to 63 letters, digits, `-` and `_`.
fn connector_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_alphanumeric())
        && id.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Runs of whitespace as one space, trimmed.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An answer's Connect cards, as the kit's `answerConnects`: a known connector id, an http(s) link and plain text. An
/// entry that is not one is left out, a missing label is the connector's name, and each connector shows once.
pub fn answer_connects(message: &Message) -> Vec<Connect> {
    let Message::Answer { connect, .. } = message else {
        return Vec::new();
    };
    let mut found: Vec<Connect> = Vec::new();
    for entry in connect.as_array().into_iter().flatten() {
        let Some(connector) = entry
            .get("connector")
            .and_then(Value::as_str)
            .filter(|id| connector_id(id))
        else {
            continue;
        };
        let Some(url) = entry.get("url").and_then(Value::as_str).and_then(safe_url) else {
            continue;
        };
        if found.iter().any(|seen| seen.connector == connector) {
            continue;
        }
        let label = match entry.get("label").and_then(Value::as_str).map(str::trim) {
            Some(label) if !label.is_empty() => label.chars().take(64).collect(),
            _ => connector_name(connector),
        };
        let summary = entry
            .get("summary")
            .and_then(Value::as_str)
            .map(|summary| {
                one_line(summary)
                    .chars()
                    .take(CONNECT_SUMMARY_CHARS)
                    .collect::<String>()
            })
            .filter(|summary| !summary.is_empty());
        found.push(Connect {
            connector: connector.to_string(),
            label,
            url,
            summary,
        });
        if found.len() == MAX_CONNECTS {
            break;
        }
    }
    found
}

/// The Connect card's one line, as the kit's `connectLine`: what the Assistant can do with the account, in
/// plain words. The agent sends it as `summary`, from the kit's map; a hint without one gets the mirrored map's line.
/// `assistant`: the Assistant's name.
pub fn connect_line(connect: &Connect, assistant: &str) -> String {
    match connect
        .summary
        .as_deref()
        .map(|summary| summary.trim_end_matches(|c: char| c == '.' || c.is_whitespace()))
        .filter(|summary| !summary.is_empty())
    {
        Some(summary) => format!("{summary}."),
        None => connector_purpose(&connect.connector, assistant),
    }
}

/// What the Assistant can do with a connector, in plain words: the kit's `CONNECTOR_PURPOSES` (`connector-names.ts`),
/// whose test keeps this list alike. An id the list lacks gets Integrations' own line.
pub fn connector_purpose(id: &str, assistant: &str) -> String {
    let known = match id {
        "asana" => "It reads your tasks and projects; each change asks first.",
        "bitbucket" => "It reads your repositories and pull requests; each change asks first.",
        "cloudflare" => "It reads your zones and settings; each change asks first.",
        "confluence" => "It reads your pages and spaces; each edit asks first.",
        "datadog" => "It reads your monitors, logs and metrics; each change asks first.",
        "dropbox" => "It finds and reads your files; each change asks first.",
        "figma" => "It reads your files and comments; each change asks first.",
        "github" => "It reads your repositories, issues and pull requests; each change asks first.",
        "gitlab" => "It reads your projects, issues and merge requests; each change asks first.",
        "gmail" => "It reads your mail; sending asks first.",
        "google-calendar" => "It reads your calendar and finds time; booking asks first.",
        "google-drive" => "It finds and reads your files; each change asks first.",
        "hubspot" => "It reads your contacts and deals; each change asks first.",
        "intercom" => "It reads your conversations; each reply asks first.",
        "jira" => "It reads your issues and projects; each change asks first.",
        "linear" => "It reads your issues and projects; each change asks first.",
        "microsoft-todo" => "It reads your tasks; each change asks first.",
        "notion" => "It reads your pages and databases; each edit asks first.",
        "okta" => "It reads your users and groups; each change asks first.",
        "pagerduty" => "It reads your incidents and who is on call; each change asks first.",
        "postgres" => "It reads your database; each change asks first.",
        "quickbooks" => "It reads your invoices and accounts; each change asks first.",
        "salesforce" => "It reads your accounts and opportunities; each change asks first.",
        "sentry" => "It reads your issues and errors; each change asks first.",
        "shopify" => "It reads your orders and products; each change asks first.",
        "slack" => "It reads your messages; each reply asks first.",
        "snowflake" => "It reads your data; each change asks first.",
        "stripe" => "It reads your payments and customers; each change asks first.",
        "teams" => "It reads your messages; each reply asks first.",
        "x" => "It reads your mentions; each post asks first.",
        "zendesk" => "It reads your tickets; each reply asks first.",
        "zoom" => "It reads your meetings; each booking asks first.",
        _ => return format!("{assistant} can read it, and each write asks you first."),
    };
    known.to_string()
}

/// A connector's name as a person reads it, as the kit's `connectorName`: `github` is GitHub, and an id the list lacks
/// is capitalised by word.
pub fn connector_name(id: &str) -> String {
    let known = match id {
        "asana" => "Asana",
        "bitbucket" => "Bitbucket",
        "cloudflare" => "Cloudflare",
        "confluence" => "Confluence",
        "datadog" => "Datadog",
        "dropbox" => "Dropbox",
        "figma" => "Figma",
        "github" => "GitHub",
        "gitlab" => "GitLab",
        "gmail" => "Gmail",
        "google-calendar" => "Google Calendar",
        "google-drive" => "Google Drive",
        "hubspot" => "HubSpot",
        "intercom" => "Intercom",
        "jira" => "Jira",
        "linear" => "Linear",
        "microsoft-todo" => "Microsoft To Do",
        "notion" => "Notion",
        "okta" => "Okta",
        "pagerduty" => "PagerDuty",
        "postgres" => "Postgres",
        "quickbooks" => "QuickBooks",
        "salesforce" => "Salesforce",
        "sentry" => "Sentry",
        "shopify" => "Shopify",
        "slack" => "Slack",
        "snowflake" => "Snowflake",
        "stripe" => "Stripe",
        "teams" => "Microsoft Teams",
        "zendesk" => "Zendesk",
        "zoom" => "Zoom",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    id.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/* ------------------------------------------------------------ the approval card */

/// The approval card a waiting write carries (the kit's `AgentWriteAsk`).
#[derive(Debug, Clone, PartialEq)]
pub struct WriteAsk {
    /// `always-asks` for a write that is never learned away; else asks first.
    pub always_asks: bool,
    /// Who or where it goes.
    pub to: Vec<String>,
    /// The exact text that goes.
    pub text: Option<String>,
    /// The amount in dollars, a decimal string.
    pub amount_usd: Option<String>,
    /// The person's own words, and when they said them (ISO 8601).
    pub why_said: Option<String>,
    pub why_at: Option<String>,
    /// The entries of `to` its errand has not written to yet: the To line says "new to this errand".
    pub new_to: Vec<String>,
}

/// A waiting write's approval card, read strictly; None for another message or a write without one.
pub fn write_ask(message: &Message) -> Option<WriteAsk> {
    let Message::PendingAction { asks: Some(asks), .. } = message else {
        return None;
    };
    let text = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(String::from)
    };
    let why = asks.get("why");
    Some(WriteAsk {
        always_asks: asks.get("tier").and_then(Value::as_str) == Some("always-asks"),
        to: asks
            .get("to")
            .and_then(Value::as_array)
            .map(|to| to.iter().filter_map(|item| text(Some(item))).collect())
            .unwrap_or_default(),
        text: text(asks.get("text")),
        amount_usd: text(asks.get("amountUsd")),
        why_said: text(why.and_then(|why| why.get("said"))),
        why_at: text(why.and_then(|why| why.get("at"))),
        new_to: asks
            .get("newTo")
            .and_then(Value::as_array)
            .map(|fresh| fresh.iter().filter_map(|item| text(Some(item))).collect())
            .unwrap_or_default(),
    })
}

/// The card's tier, as the kit's `writeTierLabel`.
pub fn write_tier_label(ask: Option<&WriteAsk>) -> &'static str {
    if ask.is_some_and(|ask| ask.always_asks) {
        "Always asks"
    } else {
        "Asks first"
    }
}

/// "Expires in 30 min", or "Expired" once it passed, from the minutes left (rounded up), as `writeExpiryText`.
pub fn write_expiry_text(minutes_left: i64) -> String {
    if minutes_left <= 0 {
        "Expired".into()
    } else {
        format!("Expires in {minutes_left} min")
    }
}

/// The Why line, as `writeWhyText`: "You said “tell them” at 08:24.", without the time when there is none.
pub fn write_why_text(said: &str, at: Option<&str>) -> String {
    match at {
        Some(at) => format!("You said “{said}” at {at}."),
        None => format!("You said “{said}”."),
    }
}

/// An amount in dollars as the card shows it, as `writeAmountText`: `31.4` is "$31.40". None for anything else.
pub fn write_amount_text(amount_usd: &str) -> Option<String> {
    let (whole, cents) = amount_usd.split_once('.').unwrap_or((amount_usd, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    (!whole.is_empty() && digits(whole) && cents.len() <= 2 && digits(cents) && !amount_usd.ends_with('.'))
        .then(|| format!("${whole}.{cents:0<2}"))
}

/// A decision on a waiting write, in the chat (the agent API's `writes.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Decline,
}

impl Decision {
    fn word(self) -> &'static str {
        match self {
            Decision::Approve => "approve",
            Decision::Decline => "decline",
        }
    }
}

/// What the card offers in the chat, as the kit's `writeChoices`: Approve and Decline for a write that asks first,
/// Decline alone for one that always asks, and "Always allow this kind" only beside Approve, where the card offers it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Choices {
    pub approve: bool,
    pub decline: bool,
    pub always: bool,
}

/// The choices of a waiting write; none for another message, or an agent API without the decision routes.
pub fn write_choices(message: &Message) -> Choices {
    let Message::PendingAction { in_chat, asks, .. } = message else {
        return Choices::default();
    };
    let approve = in_chat.as_deref() == Some("approve-or-decline");
    let decline = approve || in_chat.as_deref() == Some("decline-only");
    let offered = asks.as_ref().and_then(|asks| asks.get("always")) == Some(&Value::Bool(true));
    Choices {
        approve,
        decline,
        always: approve && offered,
    }
}

/// The decision's path, for a card id of the agent API's own shape (`agent::card_id`); None for any other.
pub fn write_decision_path(card: &str, decision: Decision) -> Option<String> {
    crate::agent::card_id(card).then(|| format!("{}{card}/{}", crate::agent::WRITES_PREFIX, decision.word()))
}

/// The decision's body: `{}`, or `{"always":true}` on an approve.
pub fn write_decision_body(decision: Decision, always: bool) -> String {
    if decision == Decision::Approve && always {
        r#"{"always":true}"#.into()
    } else {
        "{}".into()
    }
}

/// How a decided write ended (the kit's `AgentWriteDecided`).
#[derive(Debug, Clone, PartialEq)]
pub struct WriteDecided {
    /// `written`, `failed`, `declined` or `running`.
    pub outcome: String,
    /// The agent's words for it, such as "Done: Comment on an issue."
    pub text: String,
    /// What it wrote, http(s) only.
    pub url: Option<String>,
    /// The title of what a written result made, such as an event's. Only for `written`.
    pub title: Option<String>,
    /// The person allowed this kind from now on.
    pub always: bool,
}

/// The agent API's answer to a decision, read strictly, as `writeDecidedOf`; None when it is not one.
pub fn write_decided_of(body: &Value) -> Option<WriteDecided> {
    body.get("card")?.as_str()?;
    let text = body.get("text")?.as_str()?.to_string();
    let decision = body.get("decision")?.as_str()?;
    let outcome = body.get("outcome")?.as_str()?;
    if !matches!(decision, "approve" | "decline") || !matches!(outcome, "written" | "failed" | "declined" | "running") {
        return None;
    }
    Some(WriteDecided {
        outcome: outcome.to_string(),
        text,
        url: body
            .get("result")
            .and_then(|result| result.get("url"))
            .and_then(Value::as_str)
            .and_then(safe_url),
        title: body
            .get("result")
            .and_then(|result| result.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| outcome == "written" && !title.is_empty())
            .map(String::from),
        always: body.get("always") == Some(&Value::Bool(true)),
    })
}

/* ------------------------------------------------------------ the exact text and the two voices */

/// The ends each connector puts on a message the agent sends as the person (the kit's `MESSAGE_FOOTERS`).
const MESSAGE_FOOTERS: [&str; 3] = [
    "_Sent using <https://nexus.inc|Nexus>_",
    "Sent using Nexus (https://nexus.inc)",
    "— Sent using Nexus",
];

/// The exact text in its three parts, as the kit's `writeTextParts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextParts {
    /// An email's subject, from the write's `subject` argument.
    pub subject: Option<String>,
    pub body: String,
    /// The line that ends it: the agent's signature ("Sent by Nexus, Maya's assistant"), or "Sent using Nexus".
    pub end: Option<String>,
}

/// The exact text as the card draws it: an email's subject first, the body, and the line that ends it, quieter.
/// A footer reads as the recipient sees it, so Slack's link markup is "Sent using Nexus". A text with no end, or a
/// lone line, stays whole.
pub fn write_text_parts(text: &str, arguments: &Value) -> TextParts {
    let subject = arguments
        .get("subject")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|subject| !subject.is_empty())
        .map(String::from);
    let trimmed = text.trim_end();
    if let Some(footer) = MESSAGE_FOOTERS
        .iter()
        .find(|footer| trimmed.len() > footer.len() && trimmed.ends_with(*footer))
    {
        return TextParts {
            subject,
            body: trimmed[..trimmed.len() - footer.len()].trim_end().to_string(),
            end: Some("Sent using Nexus".into()),
        };
    }
    // The agents core's signature: one line that starts "Sent by", after a blank line.
    if let Some(cut) = trimmed.rfind("\n\n") {
        let last = trimmed[cut + 2..].trim();
        if last.len() > "Sent by ".len() && last.starts_with("Sent by ") && !last.contains('\n') {
            return TextParts {
                subject,
                body: trimmed[..cut].trim_end().to_string(),
                end: Some(last.to_string()),
            };
        }
    }
    TextParts {
        subject,
        body: text.to_string(),
        end: None,
    }
}

/// One of the two voices a write goes in, as the kit's `AgentWriteVoice`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    pub title: &'static str,
    pub line: &'static str,
    /// How the message ends, or whose it reads as.
    pub sample: String,
}

/// The two voices, as Approvals explains them (the kit's `writeVoices`): as the person's assistant, signed, or
/// as the person, with only words they approved. `first` and `full`: the person's names, for the samples.
pub fn write_voices(first: Option<&str>, full: Option<&str>) -> [Voice; 2] {
    [
        Voice {
            title: "As your assistant",
            line: "Logistics, scheduling, follow-ups, first contact. It writes as itself and says so.",
            sample: match first {
                Some(first) => format!("Sent by Nexus, {first}'s assistant"),
                None => "Sent by Nexus, your assistant".into(),
            },
        },
        Voice {
            title: "As you",
            line: "Only words you have read and approved, exactly. It reads as you, with no signature of its own.",
            sample: full
                .map(str::trim)
                .filter(|full| !full.is_empty())
                .unwrap_or("Your name")
                .to_string(),
        },
    ]
}

/// What a refused decision tells the person, by the agent API's code, as `writeDecisionProblem`.
pub fn write_decision_problem(status: u16, body: Option<&Value>) -> String {
    fn text(value: Option<&Value>) -> &str {
        value.and_then(Value::as_str).unwrap_or_default()
    }
    let code = text(body.and_then(|body| body.get("error")));
    if status == 410 || code == "expired" {
        return "This approval expired. Nothing was written. Ask again.".into();
    }
    if code == "already_decided" {
        return "This write was already approved or declined.".into();
    }
    if status == 404 {
        return "This write no longer waits. Nothing more happens.".into();
    }
    if status == 403 && code == "person_credential_required" {
        return "Only you can approve this, with your own sign-in.".into();
    }
    match text(body.and_then(|body| body.get("detail"))) {
        "" => "Nexus could not take your answer. Try again in a moment.".into(),
        detail => detail.to_string(),
    }
}

/// What the card says once allowed for good, beside how it ended.
pub const WRITE_ALWAYS_NOTE: &str = "I won’t ask for this kind again. Change it under Approvals.";

/* ------------------------------------------------------------ edit a waiting write */

/// What an edited card says (the kit's `WRITE_EDITED_LABEL`).
pub const WRITE_EDITED_LABEL: &str = "Edited by you";
/// What the To line says after a recipient its errand has not written to yet (the kit's `WRITE_NEW_TO_LABEL`).
pub const WRITE_NEW_TO_LABEL: &str = "new to this errand";

/// How a field is edited: one line, several lines, a list of addresses, or a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Line,
    Lines,
    Addresses,
    Time,
}

/// One field the person may change on a waiting write (the kit's `AgentWriteEditField`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditField {
    pub name: String,
    pub label: String,
    pub kind: EditKind,
    pub required: bool,
    /// It holds the card's exact text with its signature, which the person edits without.
    pub signed: bool,
}

/// The fields the card's Edit opens, read strictly as the kit's `writeEditFields`; none when it offers no Edit. Only a
/// card the person decides in the chat can be edited there.
pub fn write_edit_fields(message: &Message) -> Vec<EditField> {
    let Message::PendingAction {
        in_chat: Some(_),
        edit: Some(Value::Array(items)),
        ..
    } = message
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?;
            let label = item.get("label")?.as_str()?;
            let valid_name = !name.is_empty()
                && name.len() <= 64
                && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid_name || label.trim().is_empty() || label.chars().count() > 40 {
                return None;
            }
            let kind = match item.get("kind")?.as_str()? {
                "line" => EditKind::Line,
                "lines" => EditKind::Lines,
                "addresses" => EditKind::Addresses,
                "time" => EditKind::Time,
                _ => return None,
            };
            let flag = |key: &str| item.get(key) == Some(&Value::Bool(true));
            Some(EditField {
                name: name.to_string(),
                label: label.to_string(),
                kind,
                required: flag("required"),
                signed: flag("signed"),
            })
        })
        .collect()
}

/// The text each field's input starts with, as the kit's `writeEditValues`: addresses as one line split by commas, and a
/// signed text without its signature, which the agent adds again. In the fields' order.
pub fn write_edit_values(message: &Message) -> Vec<(String, String)> {
    let Message::PendingAction { write, .. } = message else {
        return Vec::new();
    };
    write_edit_fields(message)
        .into_iter()
        .map(|field| {
            let value = write.arguments.get(&field.name);
            let text = match field.kind {
                EditKind::Addresses => match value {
                    Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
                    Some(Value::String(one)) => one.clone(),
                    _ => String::new(),
                },
                _ => {
                    let text = value.and_then(Value::as_str).unwrap_or_default();
                    if field.signed {
                        write_text_parts(text, &Value::Null).body
                    } else {
                        text.to_string()
                    }
                }
            };
            (field.name, text)
        })
        .collect()
}

/// The addresses in one input line: split by commas, semicolons or new lines, each trimmed.
pub fn write_edit_addresses(text: &str) -> Vec<String> {
    text.split([',', ';', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(String::from)
        .collect()
}

/// The fields the person changed, as `PATCH /agent/writes/{card}` takes them (the kit's `writeEditChanges`): only those
/// whose input differs from what it started with, each of its own type. Empty when nothing changed.
pub fn write_edit_changes(
    fields: &[EditField],
    start: &[(String, String)],
    values: &[(String, String)],
) -> serde_json::Map<String, Value> {
    let find =
        |list: &[(String, String)], name: &str| list.iter().find(|(key, _)| key == name).map(|(_, text)| text.clone());
    let mut changes = serde_json::Map::new();
    for field in fields {
        let Some(value) = find(values, &field.name) else {
            continue;
        };
        let began = find(start, &field.name).unwrap_or_default();
        if field.kind == EditKind::Addresses {
            let next = write_edit_addresses(&value);
            if next != write_edit_addresses(&began) {
                changes.insert(field.name.clone(), Value::from(next));
            }
            continue;
        }
        let (next, was) = if field.kind == EditKind::Lines {
            (value.clone(), began.clone())
        } else {
            (value.trim().to_string(), began.trim().to_string())
        };
        if next != was {
            changes.insert(field.name.clone(), Value::String(next));
        }
    }
    changes
}

/// The edit's path, for a card id of the agent API's own shape; None for any other.
pub fn write_edit_path(card: &str) -> Option<String> {
    crate::agent::card_id(card).then(|| format!("{}{card}", crate::agent::WRITES_PREFIX))
}

/// The edit's body: `{"arguments":{…}}`.
pub fn write_edit_body(changes: &serde_json::Map<String, Value>) -> String {
    serde_json::json!({ "arguments": changes }).to_string()
}

/// The edit's answer, read strictly as the kit's `writeEditedOf`: the card it replaces and the edited `pending_action`;
/// None when it is not one.
pub fn write_edited_of(body: &Value) -> Option<(String, Message)> {
    let replaces = body.get("replaces")?.as_str()?;
    let action = body.get("action")?;
    let card = action.get("card")?.as_str()?;
    if !crate::agent::card_id(replaces) || !crate::agent::card_id(card) || card == replaces {
        return None;
    }
    let message: Message = serde_json::from_value(action.clone()).ok()?;
    matches!(message, Message::PendingAction { .. }).then(|| (replaces.to_string(), message))
}

/// What a failed edit tells the person, by the agent API's code, as the kit's `writeEditProblem`.
pub fn write_edit_problem(status: u16, body: Option<&Value>) -> String {
    fn text(value: Option<&Value>) -> &str {
        value.and_then(Value::as_str).unwrap_or_default()
    }
    let code = text(body.and_then(|body| body.get("error")));
    if status == 410 || code == "expired" {
        return "This approval expired. Nothing was written. Ask again.".into();
    }
    if code == "already_decided" || status == 404 {
        return "This write no longer waits, so it cannot change.".into();
    }
    if code == "not_editable" {
        return "This write cannot be edited. Decline it and ask again.".into();
    }
    if status == 403 && code == "person_credential_required" {
        return "Only you can edit this, with your own sign-in.".into();
    }
    match text(body.and_then(|body| body.get("detail"))) {
        "" => "Nexus could not save your edit. Try again in a moment.".into(),
        detail => detail.to_string(),
    }
}

/// The card says it was edited (the person edited the write it replaces).
pub fn write_edited(message: &Message) -> bool {
    matches!(message, Message::PendingAction { edited: true, .. })
}

/// The To line's entries, each with whether its errand has not written to it yet (the kit's `writeToEntries`).
pub fn write_to_entries(ask: &WriteAsk) -> Vec<(String, bool)> {
    ask.to
        .iter()
        .map(|to| (to.clone(), ask.new_to.iter().any(|fresh| fresh == to)))
        .collect()
}

/// The To line as text (the kit's `writeToText`): "legal@example.com · new to this errand" for one new recipient. With
/// several, each new one reads "(new to this errand)" after it. Empty without a recipient.
pub fn write_to_text(ask: &WriteAsk) -> String {
    let entries = write_to_entries(ask);
    match entries.as_slice() {
        [] => String::new(),
        [(to, true)] => format!("{to} · {WRITE_NEW_TO_LABEL}"),
        [(to, false)] => to.clone(),
        many => many
            .iter()
            .map(|(to, fresh)| {
                if *fresh {
                    format!("{to} ({WRITE_NEW_TO_LABEL})")
                } else {
                    to.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" · "),
    }
}

/* ------------------------------------------------------------ the first open */

/// The most starters a shell shows.
pub const INTRO_STARTERS: usize = 4;
/// What "See what I can do" sends: `discover` lists example asks, with no model call.
pub const DISCOVER_LINE: &str = "discover";
const MAX_LINE: usize = 300;
const MAX_STARTER: usize = 300;
const MAX_READS: usize = 12;
const MAX_NAME: usize = 60;

/// The first open (the kit's `AgentIntro`): one line, up to four starters, what the agent can read, and whether each
/// quiet link applies.
#[derive(Debug, Clone, PartialEq)]
pub struct Intro {
    pub line: String,
    pub starters: Vec<String>,
    pub reads: Vec<String>,
    /// "Connect your accounts" applies.
    pub accounts: bool,
    /// "See what I can do" applies.
    pub discover: bool,
}

fn bounded(value: &Value, max: usize) -> Option<String> {
    let text = value.as_str()?;
    (!text.trim().is_empty() && text.chars().count() <= max).then(|| text.trim().to_string())
}

fn unique(values: Option<&Vec<Value>>, max: usize, most: usize) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for text in values.into_iter().flatten().filter_map(|value| bounded(value, max)) {
        if !found.contains(&text) {
            found.push(text);
        }
    }
    found.truncate(most);
    found
}

/// The first open to show, from `GET /agent/intro`'s status and body, as the kit's `introOf` and Nexus UI's
/// `introToShow`: None when the call failed, the person saw it already, it is off (404), or it has no starter.
pub fn intro_to_show(status: u16, body: Option<&Value>) -> Option<Intro> {
    if !(200..300).contains(&status) {
        return None;
    }
    let body = body?;
    if body.get("firstOpen") != Some(&Value::Bool(true)) {
        return None;
    }
    let line = bounded(body.get("line")?, MAX_LINE)?;
    let starters = unique(Some(body.get("starters")?.as_array()?), MAX_STARTER, INTRO_STARTERS);
    if starters.is_empty() {
        return None;
    }
    Some(Intro {
        line,
        starters,
        reads: unique(body.get("reads").and_then(Value::as_array), MAX_NAME, MAX_READS),
        accounts: body.get("accounts") == Some(&Value::Bool(true)),
        discover: body.get("discover") == Some(&Value::Bool(true)),
    })
}

/// The first word of a person's name, for the greeting, as `firstNameOf`.
pub fn first_name_of(name: &str) -> Option<String> {
    name.split_whitespace()
        .next()
        .filter(|first| first.chars().count() <= MAX_NAME)
        .map(String::from)
}

/// The first open's first line, as `introHeadline`: `Hi Maya. Hand me anything: …`, or `Hi, I'm Nexus. …` with the
/// Assistant's name.
pub fn intro_headline(intro: &Intro, first_name: Option<&str>, assistant: &str) -> String {
    match first_name {
        Some(name) => format!("Hi {name}. {}", intro.line),
        None => format!("Hi, I'm {assistant}. {}", intro.line),
    }
}

/// What the agent can read, in one line, as `introReads`: `Acme's Nexus can read GitHub and Linear.` at work, or
/// `Nexus can read …` in a personal account, with the Assistant's name. None when it reads nothing.
pub fn intro_reads(intro: &Intro, org_name: Option<&str>, personal: bool, assistant: &str) -> Option<String> {
    let (last, rest) = intro.reads.split_last()?;
    let listed = if rest.is_empty() {
        last.clone()
    } else {
        format!("{} and {last}", rest.join(", "))
    };
    let owner = match org_name.map(str::trim).filter(|name| !name.is_empty() && !personal) {
        Some(name) => format!("{name}'s {assistant}"),
        None => assistant.into(),
    };
    Some(format!("{owner} can read {listed}."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(value: Value) -> Message {
        serde_json::from_value(value).expect("a message")
    }

    /// The Connect card, as the kit's `answerConnects` tests: strict entries, a label from the id, once each,
    /// at most four.
    #[test]
    fn an_answer_reads_its_connect_cards_strictly() {
        let answer = message(json!({
            "kind": "answer", "text": "I need Linear.",
            "connect": [
                { "connector": "linear", "label": "Linear", "url": "https://nexus.test/app/acme/settings/connectors#linear",
                  "summary": "  List   your issues.  " },
                { "connector": "linear", "label": "Again", "url": "https://nexus.test/x" },
                { "connector": "github", "url": "https://nexus.test/app/acme/settings/connectors#github" },
                { "connector": "evil", "url": "javascript:alert(1)" },
                { "connector": "../keys", "url": "https://nexus.test/y" },
                "not an entry",
                { "connector": "google-drive", "label": " ", "url": "https://nexus.test/z" },
                { "connector": "my_tool", "url": "https://nexus.test/w" },
                { "connector": "zoom", "url": "https://nexus.test/v" }
            ]
        }));
        let found = answer_connects(&answer);
        let labels: Vec<&str> = found.iter().map(|connect| connect.label.as_str()).collect();
        assert_eq!(labels, ["Linear", "GitHub", "Google Drive", "My Tool"]);
        assert_eq!(found[0].summary.as_deref(), Some("List your issues."));
        // The agent's plain words, else the kit's line for the connector.
        assert_eq!(connect_line(&found[0], "Nexus"), "List your issues.");
        assert_eq!(
            connect_line(&found[1], "Nexus"),
            "It reads your repositories, issues and pull requests; each change asks first."
        );
        assert_eq!(
            connect_line(&found[3], "Ada"),
            "Ada can read it, and each write asks you first."
        );
        // An answer with no `connect`, and one whose `connect` is not a list, still read; neither has a card.
        assert!(answer_connects(&message(json!({ "kind": "answer", "text": "x" }))).is_empty());
        let odd = message(json!({ "kind": "answer", "text": "x", "connect": "linear" }));
        assert!(matches!(&odd, Message::Answer { text, .. } if text == "x"));
        assert!(answer_connects(&odd).is_empty());
    }

    /// The approval card's words, as the kit's `agent-writes.ts` tests.
    #[test]
    fn a_waiting_write_reads_its_approval_card() {
        let pending = message(json!({
            "kind": "pending_action", "card": "card_1", "approveUrl": "https://nexus.test/app/approvals/a1",
            "detail": "Approve it in Nexus.", "expiresAt": "2026-11-09T12:30:00.000Z", "inChat": "approve-or-decline",
            "write": { "connector": "slack", "operation": "post_message", "summary": "Post in #general", "effect": "irreversible" },
            "asks": { "tier": "asks-first", "kind": "message", "to": ["#general", " "], "text": "Ship it.",
                      "amountUsd": "31.4", "why": { "said": "tell them", "at": "2026-11-09T08:24:00.000Z" },
                      "voice": "assistant", "always": true }
        }));
        let ask = write_ask(&pending).expect("an approval card");
        assert_eq!(ask.to, ["#general"]);
        assert_eq!(ask.text.as_deref(), Some("Ship it."));
        assert_eq!(write_tier_label(Some(&ask)), "Asks first");
        assert_eq!(
            write_amount_text(ask.amount_usd.as_deref().unwrap_or_default()).as_deref(),
            Some("$31.40")
        );
        let Message::PendingAction {
            expires_at,
            in_chat,
            card,
            ..
        } = &pending
        else {
            panic!("pending")
        };
        assert_eq!(expires_at.as_deref(), Some("2026-11-09T12:30:00.000Z"));
        assert_eq!(
            (in_chat.as_deref(), card.as_str()),
            (Some("approve-or-decline"), "card_1")
        );
        assert_eq!(write_tier_label(None), "Asks first");
        let face = message(json!({
            "kind": "pending_action", "write": { "connector": "x", "operation": "y", "summary": "z", "effect": "read" },
            "asks": { "tier": "always-asks", "to": [], "why": {} }
        }));
        assert_eq!(write_tier_label(write_ask(&face).as_ref()), "Always asks");
        assert_eq!(write_expiry_text(30), "Expires in 30 min");
        assert_eq!(write_expiry_text(0), "Expired");
        assert_eq!(
            write_why_text("tell them", Some("08:24")),
            "You said “tell them” at 08:24."
        );
        assert_eq!(write_why_text("tell them", None), "You said “tell them”.");
        assert_eq!(write_amount_text("50").as_deref(), Some("$50.00"));
        for bad in ["nope", "1.234", "", ".5", "5.", "-1"] {
            assert_eq!(write_amount_text(bad), None, "{bad}");
        }
        // A write from before the approval card has none.
        let old = message(json!({
            "kind": "pending_action", "write": { "connector": "x", "operation": "y", "summary": "z", "effect": "read" }
        }));
        assert_eq!(write_ask(&old), None);
    }

    /// Approvals in the chat, as the kit's `agent-writes.ts` tests: the choices, the path and body, the answer
    /// and refusals.
    #[test]
    fn a_decision_names_one_card_and_reads_its_answer_strictly() {
        let card = format!("card_{}", "a".repeat(32));
        let pending = |in_chat: Value, always: bool| {
            message(json!({
                "kind": "pending_action", "card": card, "inChat": in_chat,
                "write": { "connector": "x", "operation": "y", "summary": "z", "effect": "read" },
                "asks": { "tier": "asks-first", "to": [], "why": {}, "always": always }
            }))
        };
        assert_eq!(
            write_choices(&pending(json!("approve-or-decline"), true)),
            Choices {
                approve: true,
                decline: true,
                always: true
            }
        );
        assert_eq!(
            write_choices(&pending(json!("decline-only"), true)),
            Choices {
                approve: false,
                decline: true,
                always: false
            }
        );
        assert_eq!(write_choices(&pending(Value::Null, true)), Choices::default());
        assert_eq!(
            write_decision_path(&card, Decision::Approve),
            Some(format!("/agent/writes/{card}/approve"))
        );
        let decline = write_decision_path(&card, Decision::Decline).expect("a path");
        assert!(crate::agent::write_path(&decline));
        assert_eq!(
            write_decision_path("card_x/../../agent/memory", Decision::Approve),
            None
        );
        assert_eq!(write_decision_body(Decision::Approve, true), r#"{"always":true}"#);
        assert_eq!(write_decision_body(Decision::Approve, false), "{}");
        assert_eq!(write_decision_body(Decision::Decline, true), "{}");
        let approve = write_decision_path(&card, Decision::Approve).expect("a path");
        assert!(crate::agent::write_body(
            &approve,
            Some(&write_decision_body(Decision::Approve, true))
        ));

        let decided = write_decided_of(&json!({
            "card": card, "decision": "approve", "outcome": "written", "text": "Done.",
            "result": { "url": "https://x.test/1" }, "always": true
        }))
        .expect("decided");
        assert_eq!(
            decided,
            WriteDecided {
                outcome: "written".into(),
                text: "Done.".into(),
                url: Some("https://x.test/1".into()),
                title: None,
                always: true
            }
        );
        // A written result with a title shows as its own row; a title on another outcome does not.
        let made = |outcome: &str| {
            write_decided_of(&json!({
                "card": card, "decision": "approve", "outcome": outcome, "text": "Sent. It's on both calendars.",
                "result": { "title": "  SOW review with Priya N. ", "url": "https://calendar.test/e/1" }
            }))
            .expect("decided")
        };
        assert_eq!(made("written").title.as_deref(), Some("SOW review with Priya N."));
        assert_eq!(made("running").title, None);
        assert_eq!(
            write_decided_of(&json!({ "card": card, "decision": "approve", "outcome": "maybe", "text": "?" })),
            None
        );
        assert_eq!(write_decided_of(&Value::Null), None);
        assert!(write_decision_problem(410, Some(&json!({ "error": "expired" }))).contains("expired"));
        assert!(write_decision_problem(409, Some(&json!({ "error": "already_decided" }))).contains("already approved"));
        assert!(write_decision_problem(404, Some(&json!({ "error": "not_found" }))).contains("no longer waits"));
        assert_eq!(
            write_decision_problem(500, Some(&json!({ "detail": "Busy." }))),
            "Busy."
        );
        assert!(write_decision_problem(502, None).starts_with("Nexus could not take your answer"));
    }

    /// The first open, as the kit's `agent-intro.ts` and Nexus UI's `introToShow`.
    #[test]
    fn the_first_open_reads_strictly_and_shows_only_with_a_starter() {
        let body = json!({
            "firstOpen": true, "line": "Hand me anything: errands, reminders, research.",
            "starters": ["Remind me tomorrow at 9", "Remind me tomorrow at 9", "Find a dinner spot", "Summarise my inbox",
                         "Track my packages", "One more"],
            "reads": ["Gmail", "GitHub", "Linear"], "accounts": true, "discover": true
        });
        let intro = intro_to_show(200, Some(&body)).expect("a first open");
        assert_eq!(intro.starters.len(), INTRO_STARTERS);
        assert_eq!(intro.starters[1], "Find a dinner spot");
        assert!(intro.accounts && intro.discover);
        assert_eq!(
            intro_headline(&intro, first_name_of("Maya Chen").as_deref(), "Nexus"),
            "Hi Maya. Hand me anything: errands, reminders, research."
        );
        assert_eq!(
            intro_headline(&intro, None, "Nexus"),
            "Hi, I'm Nexus. Hand me anything: errands, reminders, research."
        );
        assert_eq!(
            intro_headline(&intro, None, "Ada"),
            "Hi, I'm Ada. Hand me anything: errands, reminders, research."
        );
        assert_eq!(
            intro_reads(&intro, Some("Acme"), false, "Nexus").as_deref(),
            Some("Acme's Nexus can read Gmail, GitHub and Linear.")
        );
        assert_eq!(
            intro_reads(&intro, Some("Acme"), false, "Ada").as_deref(),
            Some("Acme's Ada can read Gmail, GitHub and Linear.")
        );
        assert_eq!(
            intro_reads(&intro, Some("Personal"), true, "Nexus").as_deref(),
            Some("Nexus can read Gmail, GitHub and Linear.")
        );
        assert_eq!(first_name_of("  "), None);
        // Seen, off, failed, or with no starter: the empty chat as before.
        assert_eq!(intro_to_show(200, Some(&json!({ "firstOpen": false }))), None);
        assert_eq!(intro_to_show(404, Some(&body)), None);
        assert_eq!(intro_to_show(200, None), None);
        let mut none = body.clone();
        none["starters"] = json!([]);
        assert_eq!(intro_to_show(200, Some(&none)), None);
        let mut bare = body;
        bare["reads"] = json!("nope");
        bare["accounts"] = json!("yes");
        let bare = intro_to_show(200, Some(&bare)).expect("a first open");
        assert!(bare.reads.is_empty() && !bare.accounts);
        assert_eq!(intro_reads(&bare, Some("Acme"), false, "Nexus"), None);
    }

    /// As Nexus UI's `assistant-name.test.ts`: the person's own profile's name, trimmed; Nexus without one.
    #[test]
    fn the_assistants_name_is_the_persons_own_profiles_and_nexus_without_one() {
        let profiles = |list: Value| json!({ "data": list });
        let profile = |owner: &str, name: &str| json!({ "id": format!("agp_{owner}"), "owner": owner, "name": name });
        assert_eq!(
            assistant_name_of(Some(&profiles(json!([
                profile("team", "Support Bot"),
                profile("person", "  Ada ")
            ])))),
            "Ada"
        );
        assert_eq!(
            assistant_name_of(Some(&profiles(json!([profile("team", "Support Bot")])))),
            "Nexus"
        );
        assert_eq!(
            assistant_name_of(Some(&profiles(json!([profile("person", "   ")])))),
            "Nexus"
        );
        assert_eq!(assistant_name_of(Some(&json!({ "error": "not_found" }))), "Nexus");
        assert_eq!(assistant_name_of(None), DEFAULT_ASSISTANT_NAME);
    }

    /// The exact text, as the kit's `writeTextParts` tests: the subject first, the body, and the end apart.
    #[test]
    fn the_exact_text_reads_as_its_subject_body_and_end() {
        let parts = |subject: Option<&str>, body: &str, end: Option<&str>| TextParts {
            subject: subject.map(String::from),
            body: body.into(),
            end: end.map(String::from),
        };
        let text = "Hi Priya, Maya asked me to follow up on the SOW.\n\nSent by Nexus, Maya's assistant";
        assert_eq!(
            write_text_parts(
                text,
                &json!({ "subject": " SOW v3, two questions ", "to": "priya@example.com" })
            ),
            parts(
                Some("SOW v3, two questions"),
                "Hi Priya, Maya asked me to follow up on the SOW.",
                Some("Sent by Nexus, Maya's assistant")
            )
        );
        // Each connector's Sent using Nexus, as the recipient sees it.
        for (text, body) in [
            (
                "Running late.\n\n_Sent using <https://nexus.inc|Nexus>_",
                "Running late.",
            ),
            ("Shipped it — Sent using Nexus", "Shipped it"),
            (
                "Notes attached.\n\nSent using Nexus (https://nexus.inc)\n",
                "Notes attached.",
            ),
        ] {
            assert_eq!(
                write_text_parts(text, &Value::Null),
                parts(None, body, Some("Sent using Nexus")),
                "{text}"
            );
        }
        // No end, a lone line, a "Sent by" that is not the last line alone, and a subject that is not text.
        for text in [
            "Dinner Friday 7:30 at Nopa.",
            "Sent by Nexus, Maya's assistant",
            "Hi.\n\nSent by courier\nTomorrow",
        ] {
            assert_eq!(write_text_parts(text, &Value::Null), parts(None, text, None), "{text}");
        }
        assert_eq!(
            write_text_parts("Hi.", &json!({ "subject": 3 })),
            parts(None, "Hi.", None)
        );
    }

    /// The two voices, as the kit's `writeVoices` tests: the person's names in the samples, plain words without them.
    #[test]
    fn the_two_voices_name_the_person() {
        let named = write_voices(Some("Maya"), Some("Maya Chen"));
        assert_eq!(
            named.each_ref().map(|voice| (voice.title, voice.sample.as_str())),
            [
                ("As your assistant", "Sent by Nexus, Maya's assistant"),
                ("As you", "Maya Chen")
            ]
        );
        assert_eq!(
            write_voices(None, None).each_ref().map(|voice| voice.sample.as_str()),
            ["Sent by Nexus, your assistant", "Your name"]
        );
    }

    fn email(extra: Value) -> Message {
        let mut body = json!({
            "kind": "pending_action", "card": format!("card_{}", "a".repeat(32)), "inChat": "approve-or-decline",
            "approveUrl": "https://nexus.test/a", "detail": "d",
            "write": { "connector": "outlook", "operation": "send", "summary": "Send the redline", "effect": "mutate",
                "arguments": { "to": ["legal@example.com"], "subject": "Redline", "body": "Term at 12 months.\n\nSent by Nexus, Maya's assistant" } },
            "asks": { "tier": "asks-first", "to": ["legal@example.com"], "newTo": ["legal@example.com"], "why": { "said": "x", "at": "2026-11-09T08:24:00Z" }, "always": true },
            "edit": [
                { "name": "to", "label": "To", "kind": "addresses", "required": true },
                { "name": "subject", "label": "Subject", "kind": "line", "required": true },
                { "name": "body", "label": "Message", "kind": "lines", "signed": true },
                { "name": "x y", "label": "Bad", "kind": "line" },
                { "name": "t", "label": "T", "kind": "html" }
            ]
        });
        body.as_object_mut()
            .expect("an object")
            .extend(extra.as_object().cloned().unwrap_or_default());
        serde_json::from_value(body).expect("a pending action")
    }

    /// Editing a waiting write, as the kit's `agent-writes.test.ts`: the fields, their starting text, the
    /// changes, the path and the answer.
    #[test]
    fn a_waiting_write_is_edited_by_its_changed_fields() {
        let message = email(json!({}));
        let fields = write_edit_fields(&message);
        assert_eq!(
            fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["to", "subject", "body"]
        );
        assert!(fields[2].signed && fields[0].required);
        assert_eq!(write_edit_fields(&email(json!({ "inChat": "decline-only" }))).len(), 3);
        assert!(write_edit_fields(&email(json!({ "inChat": null }))).is_empty());
        let start = write_edit_values(&message);
        assert_eq!(start[0].1, "legal@example.com");
        assert_eq!(start[2].1, "Term at 12 months.", "the signature is not edited");
        assert!(write_edit_changes(&fields, &start, &start).is_empty());
        let values = vec![
            ("to".to_string(), "a@x.io; b@x.io".to_string()),
            ("subject".to_string(), "  Redline  ".to_string()),
            ("body".to_string(), "Twelve months.".to_string()),
        ];
        let changes = write_edit_changes(&fields, &start, &values);
        assert_eq!(
            Value::Object(changes.clone()),
            json!({ "to": ["a@x.io", "b@x.io"], "body": "Twelve months." })
        );
        let body: Value = serde_json::from_str(&write_edit_body(&changes)).expect("json");
        assert_eq!(
            body,
            json!({ "arguments": { "to": ["a@x.io", "b@x.io"], "body": "Twelve months." } })
        );
        let card = format!("card_{}", "a".repeat(32));
        assert_eq!(write_edit_path(&card), Some(format!("/agent/writes/{card}")));
        assert_eq!(write_edit_path("card_x/../../agent/memory"), None);
        let new = format!("card_{}", "c".repeat(32));
        let answer = json!({ "replaces": card, "action": { "kind": "pending_action", "card": new, "edited": true,
            "write": { "connector": "outlook", "operation": "send", "summary": "s", "effect": "mutate" } } });
        let (replaces, edited) = write_edited_of(&answer).expect("an edited card");
        assert_eq!(replaces, card);
        assert!(write_edited(&edited));
        assert_eq!(
            write_edited_of(&json!({ "replaces": card, "action": { "kind": "pending_action", "card": card } })),
            None
        );
        assert_eq!(
            write_edited_of(&json!({ "replaces": "x", "action": answer["action"] })),
            None
        );
        assert!(write_edit_problem(409, Some(&json!({ "error": "not_editable" }))).contains("cannot be edited"));
        assert!(write_edit_problem(404, None).contains("no longer waits"));
        assert!(write_edit_problem(502, None).starts_with("Nexus could not save"));
    }

    #[test]
    fn the_to_line_says_which_recipient_is_new_to_the_errand() {
        let ask = |to: &[&str], new: &[&str]| WriteAsk {
            always_asks: false,
            to: to.iter().map(|s| s.to_string()).collect(),
            text: None,
            amount_usd: None,
            why_said: None,
            why_at: None,
            new_to: new.iter().map(|s| s.to_string()).collect(),
        };
        let message = email(json!({}));
        assert_eq!(write_ask(&message).expect("an ask").new_to, ["legal@example.com"]);
        assert_eq!(
            write_to_text(&ask(&["legal@example.com"], &["legal@example.com"])),
            "legal@example.com · new to this errand"
        );
        assert_eq!(write_to_text(&ask(&["a@x.io"], &[])), "a@x.io");
        assert_eq!(
            write_to_text(&ask(&["p@acme.com", "legal@example.com"], &["legal@example.com"])),
            "p@acme.com · legal@example.com (new to this errand)"
        );
        assert_eq!(write_to_text(&ask(&[], &[])), "");
    }
}
