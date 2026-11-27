//! Privacy and trust for a native shell: the agent API's pause, what Nexus can see per account, and
//! Revoke all access, read strictly as the kit's `agent-privacy.ts` reads them, with the words of Nexus UI's
//! `lib/agent-privacy.ts`. Nexus GPUI draws it (`desktop-gpui/src/pages/assistant_views/privacy.rs`).
//!
//! - `GET` and `PUT /agent/pause` with `{"paused"}`: whether the person paused the Assistant.
//! - `GET /agent/privacy`: the pause, the person's live agent links, and per connected account what Nexus reads, what
//!   it writes after the person approves, and what it never does there.
//! - `POST /agent/revoke` with `{"confirm":true,"accounts":true}`: ends every agent link and disconnects the person's
//!   own accounts. The organization's accounts stay.
//!
//! A shell builds each call as a [`Request`], whose constructors check the body, so a call the agent API could not
//! have asked for is never made. An older agent API answers 404 ([`privacy_off`]).

use nexus_client::Method;
use serde_json::{Value, json};

use crate::agent::Problem;
use crate::agent_cards::connector_name;

pub const PAUSE_PATH: &str = "/agent/pause";
pub const PRIVACY_PATH: &str = "/agent/privacy";
pub const REVOKE_PATH: &str = "/agent/revoke";

/// The one sentence about training, the same everywhere.
pub const NEVER_TRAINS: &str = "Nexus never trains models on your data.";
/// The Pause dialog's words.
pub const PAUSE_TITLE: &str = "Pause the Assistant?";
pub const PAUSE_BODY: &str =
    "Every errand, watch, brief and message stops at once. Nothing is deleted. Writes already made stay made.";
/// The Revoke all access dialog's title; its body is [`revoke_text`].
pub const REVOKE_TITLE: &str = "Revoke all access?";
/// What the view says once the pause changed.
pub const PAUSED_STATUS: &str = "Nexus is paused. Nothing runs until you resume.";
pub const RESUMED_STATUS: &str = "Nexus runs again.";

/// The longest text the readers keep, in UTF-16 units as the kit counts them.
const MAX_TEXT: usize = 400;
/// The longest list the readers keep.
const MAX_ITEMS: usize = 200;

/// `GET` and `PUT /agent/pause`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PauseView {
    pub paused: bool,
    /// When the person paused it (ISO 8601).
    pub paused_at: Option<String>,
}

/// One live agent link of the person. Never its token.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub id: String,
    /// What the agent is, as a slug (`slack`, `nexus-assistant`).
    pub agent: String,
    pub label: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// Whose an account is: the person's own, which Revoke all access disconnects, or the organization's, which it never
/// touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whose {
    Yours,
    Organization,
}

/// One connected account, and what Nexus does with it, by the connector catalogue's own summaries.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub connector: String,
    /// Who the vendor says the holder is, else the account's name.
    pub account: String,
    pub whose: Whose,
    pub reads: Vec<String>,
    /// The writes the organization allows, each only after the person approves it.
    pub writes: Vec<String>,
    /// The writes Nexus never makes here.
    pub never: Vec<String>,
    /// Why the operations could not be read; the lists are then empty.
    pub error: Option<String>,
}

/// `GET /agent/privacy`.
#[derive(Debug, Clone, PartialEq)]
pub struct PrivacyView {
    pub pause: PauseView,
    pub links: Vec<Link>,
    pub links_error: Option<String>,
    pub accounts: Vec<Account>,
    pub accounts_error: Option<String>,
}

/// One part that did not end, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub id: String,
    pub error: String,
}

/// `POST /agent/revoke`: what ended, and what did not.
#[derive(Debug, Clone, PartialEq)]
pub struct RevokeResult {
    pub ended: Vec<Link>,
    pub links_failed: Vec<Failure>,
    /// Each disconnected account: its connector and its name.
    pub disconnected: Vec<(String, String)>,
    pub accounts_failed: Vec<Failure>,
}

/// A string with some text and at most [`MAX_TEXT`] units, trimmed.
fn text(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?;
    let trimmed = value.trim();
    (!trimmed.is_empty() && value.encode_utf16().count() <= MAX_TEXT).then(|| trimmed.to_string())
}

/// An ISO 8601 time, as the agent API writes one.
fn time(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(value)
        .is_ok()
        .then(|| value.to_string())
}

/// Every item of a list, or None when it is not one, is too long, or holds one that does not read.
fn list_of<T>(value: Option<&Value>, read: impl Fn(&Value) -> Option<T>) -> Option<Vec<T>> {
    let items = value?.as_array()?;
    if items.len() > MAX_ITEMS {
        return None;
    }
    items.iter().map(read).collect()
}

fn texts(value: Option<&Value>) -> Option<Vec<String>> {
    list_of(value, |item| text(Some(item)))
}

/// `GET` or `PUT /agent/pause`'s body, or None when it is not one.
pub fn pause_view_of(body: &Value) -> Option<PauseView> {
    let paused = body.as_object()?.get("paused")?.as_bool()?;
    Some(PauseView {
        paused,
        paused_at: if paused { time(body.get("pausedAt")) } else { None },
    })
}

fn link_of(value: &Value) -> Option<Link> {
    value.as_object()?;
    Some(Link {
        id: text(value.get("id"))?,
        agent: text(value.get("agent"))?,
        label: text(value.get("label"))?,
        created_at: time(value.get("createdAt"))?,
        last_used_at: time(value.get("lastUsedAt")),
    })
}

fn account_of(value: &Value) -> Option<Account> {
    value.as_object()?;
    let whose = match value.get("scope")?.as_str()? {
        "user" => Whose::Yours,
        "org" => Whose::Organization,
        _ => return None,
    };
    Some(Account {
        connector: text(value.get("connector"))?,
        account: text(value.get("account"))?,
        whose,
        reads: texts(value.get("reads"))?,
        writes: texts(value.get("writes"))?,
        never: texts(value.get("never"))?,
        error: text(value.get("error")),
    })
}

/// `GET /agent/privacy`'s body, or None when any part of it is not what the agent API sends.
pub fn privacy_view_of(body: &Value) -> Option<PrivacyView> {
    body.as_object()?;
    Some(PrivacyView {
        pause: pause_view_of(body.get("pause")?)?,
        links: list_of(body.get("links"), link_of)?,
        links_error: text(body.get("linksError")),
        accounts: list_of(body.get("accounts"), account_of)?,
        accounts_error: text(body.get("accountsError")),
    })
}

fn failure_of(value: &Value) -> Option<Failure> {
    Some(Failure {
        id: text(value.get("id"))?,
        error: text(value.get("error"))?,
    })
}

/// `POST /agent/revoke`'s body, or None when it is not one.
pub fn revoke_result_of(body: &Value) -> Option<RevokeResult> {
    let links = body.get("links")?;
    let accounts = body.get("accounts")?;
    Some(RevokeResult {
        ended: list_of(links.get("ended"), link_of)?,
        links_failed: list_of(links.get("failed"), failure_of)?,
        disconnected: list_of(accounts.get("disconnected"), |item| {
            Some((text(item.get("connector"))?, text(item.get("account"))?))
        })?,
        accounts_failed: list_of(accounts.get("failed"), failure_of)?,
    })
}

/// The paused bar: `Paused at 14:02 · nothing runs until you resume`. `clock` is the time it paused, as the
/// shell writes it in the person's zone.
pub fn paused_line(clock: Option<&str>) -> String {
    match clock {
        Some(clock) => format!("Paused at {clock} · nothing runs until you resume"),
        None => "Paused · nothing runs until you resume".to_string(),
    }
}

/// A list in words: `a`, `a and b`, `a, b and c`.
fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The Revoke all access dialog's body: exactly what ends, by name, and what stays. Only the person's own accounts are
/// disconnected; the organization's stay.
pub fn revoke_text(view: &PrivacyView) -> String {
    let links: Vec<String> = view.links.iter().map(|link| link.label.clone()).collect();
    let own: Vec<String> = view
        .accounts
        .iter()
        .filter(|account| account.whose == Whose::Yours)
        .map(|account| format!("{} ({})", connector_name(&account.connector), account.account))
        .collect();
    let shared = view
        .accounts
        .iter()
        .filter(|account| account.whose == Whose::Organization)
        .count();
    let ends = match links.len() {
        0 => "You have no agent links to end.".to_string(),
        1 => format!("Ends your agent link: {}.", listed(&links)),
        count => format!("Ends your {count} agent links: {}.", listed(&links)),
    };
    let disconnects = match own.len() {
        0 => "You have no accounts of your own to disconnect.".to_string(),
        1 => format!("Disconnects your account: {}.", listed(&own)),
        count => format!("Disconnects your {count} accounts: {}.", listed(&own)),
    };
    let stays = match shared {
        0 => None,
        1 => Some("Your organization's account stays connected.".to_string()),
        count => Some(format!("Your organization's {count} accounts stay connected.")),
    };
    [
        Some(ends),
        Some(disconnects),
        stays,
        Some("You can reconnect one at a time.".into()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
}

/// What Revoke all access did, in one line. A part that failed says so, so the person can try again.
pub fn revoked_text(result: &RevokeResult) -> String {
    let ended = result.ended.len();
    let gone = result.disconnected.len();
    let failed = result.links_failed.len() + result.accounts_failed.len();
    let plural = |count: usize, one: &str, many: &str| if count == 1 { one } else { many }.to_string();
    let done = format!(
        "Ended {ended} agent {} and disconnected {gone} {}.",
        plural(ended, "link", "links"),
        plural(gone, "account", "accounts")
    );
    if failed > 0 {
        format!("{done} {failed} did not end. Try again.")
    } else {
        done
    }
}

/// An account's row head: `Gmail · ann@gmail.test`, and whose it is.
pub fn account_head(account: &Account) -> (String, &'static str) {
    let name = format!("{} · {}", connector_name(&account.connector), account.account);
    let whose = match account.whose {
        Whose::Yours => "Yours",
        Whose::Organization => "Your organization's",
    };
    (name, whose)
}

/// At most `max` items in one line, then how many more.
pub fn short_list(items: &[String], max: usize) -> String {
    if items.len() <= max {
        return items.join("; ");
    }
    format!("{}; {} more", items[..max].join("; "), items.len() - max)
}

/// The account's Reads column: what it reads, else Nothing.
pub fn reads_line(account: &Account) -> String {
    if account.reads.is_empty() {
        "Nothing".to_string()
    } else {
        short_list(&account.reads, 3)
    }
}

/// The line under Reads while the organization allows writes: each one only after the person approves.
pub fn writes_line(account: &Account) -> Option<String> {
    (!account.writes.is_empty()).then(|| format!("Writes after you approve: {}", short_list(&account.writes, 3)))
}

/// The account's Never column: the writes the organization does not allow, else that every write asks first.
pub fn never_line(account: &Account) -> String {
    if !account.never.is_empty() {
        return short_list(&account.never, 3);
    }
    if account.writes.is_empty() {
        "Any write".to_string()
    } else {
        "Any write without your approval".to_string()
    }
}

/// Whether the agent API has no pause, so the view says Privacy is not here yet: an older agent API's 404.
pub fn privacy_off(status: u16) -> bool {
    status == 404
}

/// What the view says when a read or a change fails (Nexus UI's `privacyProblem`).
pub fn privacy_problem(status: u16, body: Option<&Value>) -> Problem {
    crate::agent_approvals::view_problem(status, body, |detail| {
        let said = |title: &str, fallback: &str| Problem {
            title: title.into(),
            detail: if detail.is_empty() {
                fallback.into()
            } else {
                detail.clone()
            },
        };
        match status {
            400 if !detail.is_empty() => said("That did not run", ""),
            404 => Problem {
                title: "Privacy is not here yet".into(),
                detail: "This agent service has no Pause or Revoke all access yet.".into(),
            },
            502 => said(
                "Nexus Gateway did not answer",
                "Nothing changed. Try again in a moment.",
            ),
            503 => said("Your privacy settings are not available now", "Try again in a moment."),
            _ => said(
                "Your privacy settings did not open",
                "Nexus could not reach the Assistant. Try again in a moment.",
            ),
        }
    })
}

/// `PUT /agent/pause`'s body.
pub fn pause_body(paused: bool) -> String {
    json!({ "paused": paused }).to_string()
}

/// Whether `body` is exactly a pause's body: an object whose only field is `paused`, `true` or `false`.
pub fn is_pause_body(body: &str) -> bool {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    fields.len() == 1 && matches!(fields.get("paused"), Some(Value::Bool(_)))
}

/// `POST /agent/revoke`'s body: the confirm, and whether the person's own accounts go too.
pub fn revoke_body(accounts: bool) -> String {
    json!({ "confirm": true, "accounts": accounts }).to_string()
}

/// Whether `body` is exactly a revoke's body: `confirm` true, and `accounts` true or false when it is there.
pub fn is_revoke_body(body: &str) -> bool {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let accounts = match fields.get("accounts") {
        None => true,
        Some(value) => value.is_boolean(),
    };
    fields.get("confirm") == Some(&Value::Bool(true))
        && accounts
        && fields.keys().all(|key| key == "confirm" || key == "accounts")
}

/// One call of the Privacy view, its body checked when it was made.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// `GET /agent/pause`, for the sidebar row.
    Pause,
    /// `GET /agent/privacy`.
    Read,
    /// `PUT /agent/pause`.
    SetPause { body: String },
    /// `POST /agent/revoke`.
    Revoke { body: String },
}

impl Request {
    pub fn set_pause(paused: bool) -> Option<Request> {
        let body = pause_body(paused);
        is_pause_body(&body).then_some(Request::SetPause { body })
    }

    /// End every agent link, and with `accounts` disconnect the person's own accounts too.
    pub fn revoke(accounts: bool) -> Option<Request> {
        let body = revoke_body(accounts);
        is_revoke_body(&body).then_some(Request::Revoke { body })
    }

    /// Its method, path and body.
    pub fn request(&self) -> (Method, &'static str, Option<String>) {
        match self {
            Request::Pause => (Method::GET, PAUSE_PATH, None),
            Request::Read => (Method::GET, PRIVACY_PATH, None),
            Request::SetPause { body } => (Method::PUT, PAUSE_PATH, Some(body.clone())),
            Request::Revoke { body } => (Method::POST, REVOKE_PATH, Some(body.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Value {
        json!({
            "pause": { "paused": true, "pausedAt": "2026-11-17T14:02:00Z" },
            "links": [
                { "id": "l1", "agent": "slack", "label": "Slack", "createdAt": "2026-11-01T09:00:00Z" },
                { "id": "l2", "agent": "nexus-assistant", "label": "Nexus Desktop", "createdAt": "2026-11-02T09:00:00Z", "lastUsedAt": "2026-11-17T09:00:00Z" }
            ],
            "accounts": [
                { "connector": "gmail", "account": "ann@gmail.test", "scope": "user", "reads": ["Read mail", "Search mail"], "writes": ["Send mail"], "never": [] },
                { "connector": "linear", "account": "Acme", "scope": "org", "reads": ["Read issues"], "writes": [], "never": ["Delete issues"] }
            ]
        })
    }

    #[test]
    fn reads_the_view_strictly() {
        let read = privacy_view_of(&view()).expect("reads");
        assert!(read.pause.paused);
        assert_eq!(read.pause.paused_at.as_deref(), Some("2026-11-17T14:02:00Z"));
        assert_eq!(read.links.len(), 2);
        assert_eq!(read.links[1].last_used_at.as_deref(), Some("2026-11-17T09:00:00Z"));
        assert_eq!(read.accounts[0].whose, Whose::Yours);
        for broken in [
            json!({}),
            json!({ "pause": { "paused": "yes" }, "links": [], "accounts": [] }),
            {
                let mut body = view();
                body["links"][0]["createdAt"] = json!("yesterday");
                body
            },
            {
                let mut body = view();
                body["accounts"][0]["scope"] = json!("team");
                body
            },
            {
                let mut body = view();
                body["accounts"][1]["reads"] = json!(["ok", 4]);
                body
            },
        ] {
            assert_eq!(privacy_view_of(&broken), None, "{broken}");
        }
        assert_eq!(
            pause_view_of(&json!({ "paused": false, "pausedAt": "2026-11-17T14:02:00Z" })),
            Some(PauseView::default()),
            "a time without the pause is left out"
        );
    }

    #[test]
    fn the_dialog_names_what_ends_and_what_stays() {
        let read = privacy_view_of(&view()).expect("reads");
        assert_eq!(
            revoke_text(&read),
            "Ends your 2 agent links: Slack and Nexus Desktop. Disconnects your account: Gmail (ann@gmail.test). \
             Your organization's account stays connected. You can reconnect one at a time."
        );
        let empty = PrivacyView {
            pause: PauseView::default(),
            links: vec![],
            links_error: None,
            accounts: vec![],
            accounts_error: None,
        };
        assert_eq!(
            revoke_text(&empty),
            "You have no agent links to end. You have no accounts of your own to disconnect. You can reconnect one at a time."
        );
    }

    #[test]
    fn says_what_revoke_did_and_what_failed() {
        let result = revoke_result_of(&json!({
            "links": { "ended": [{ "id": "l1", "agent": "slack", "label": "Slack", "createdAt": "2026-11-01T09:00:00Z" }], "failed": [] },
            "accounts": { "disconnected": [], "failed": [{ "id": "c1", "error": "Gateway answered 500" }] }
        }))
        .expect("reads");
        assert_eq!(
            revoked_text(&result),
            "Ended 1 agent link and disconnected 0 accounts. 1 did not end. Try again."
        );
        assert_eq!(revoke_result_of(&json!({ "links": {}, "accounts": {} })), None);
    }

    #[test]
    fn the_rows_say_reads_writes_and_never() {
        let read = privacy_view_of(&view()).expect("reads");
        let gmail = &read.accounts[0];
        assert_eq!(account_head(gmail), ("Gmail · ann@gmail.test".to_string(), "Yours"));
        assert_eq!(reads_line(gmail), "Read mail; Search mail");
        assert_eq!(
            writes_line(gmail).as_deref(),
            Some("Writes after you approve: Send mail")
        );
        assert_eq!(never_line(gmail), "Any write without your approval");
        assert_eq!(never_line(&read.accounts[1]), "Delete issues");
        let many: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        assert_eq!(short_list(&many, 3), "a; b; c; 2 more");
        assert_eq!(
            paused_line(Some("14:02")),
            "Paused at 14:02 · nothing runs until you resume"
        );
    }

    #[test]
    fn builds_only_the_bodies_the_agent_api_takes() {
        assert_eq!(
            Request::set_pause(true),
            Some(Request::SetPause {
                body: r#"{"paused":true}"#.into()
            })
        );
        let Some(Request::Revoke { body }) = Request::revoke(true) else {
            panic!("a revoke")
        };
        assert!(is_revoke_body(&body));
        assert!(body.contains(r#""confirm":true"#) && body.contains(r#""accounts":true"#));
        assert!(is_revoke_body(r#"{"confirm":true}"#));
        for bad in [
            r#"{"confirm":false}"#,
            r#"{"confirm":true,"accounts":"all"}"#,
            r#"{"confirm":true,"x":1}"#,
            "[]",
        ] {
            assert!(!is_revoke_body(bad), "{bad}");
        }
        assert!(!is_pause_body(r#"{"paused":1}"#) && !is_pause_body(r#"{"paused":true,"x":1}"#));
        assert_eq!(Request::Read.request().1, PRIVACY_PATH);
        assert_eq!(privacy_problem(404, None).title, "Privacy is not here yet");
        assert_eq!(privacy_problem(401, None).title, "Sign in again");
    }
}
