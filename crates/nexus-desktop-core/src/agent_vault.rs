//! Passwords and payments for a native shell: the agent API's `/agent/vault` routes, read
//! strictly as the kit's `agent-vault.ts` reads them, and the words each row shows. Nexus GPUI draws it
//! (`desktop-gpui/src/pages/assistant_views/vault.rs`).
//!
//! The rule: Nexus never sees, stores or repeats a password or a card number. The person's password manager fills a
//! login in, and the card's own page takes a payment. The agent API sends names, sites, masked accounts and grants only.
//! Each reader here refuses a body that carries a secret's shape ([`holds_secret`]), so a bug upstream shows nothing
//! rather than a secret.
//!
//! - `GET /agent/vault`: the password manager, the logins and their grants, the card and its limits, and the asks.
//! - `PUT /agent/vault/logins/{id}` with `{ grant }`: each use, until a date, or `none` (Revoke).
//! - `PUT /agent/vault/card` with `{ frozen }`: Freeze, or unfreeze.
//! - `POST /agent/vault/asks/{id}/approve` with `{ for }`, and `…/decline`: the grant sheet.
//! - `GET /agent/vault/activity`: every use and every payment.
//!
//! The API answers 503 `vault_off` while it has no vault source, and an older API answers 404: both read as off.

use chrono::{DateTime, Datelike as _, TimeZone, Timelike as _, Utc};
use nexus_client::Method;
use serde_json::{Map, Value, json};

pub const VAULT_PATH: &str = "/agent/vault";
pub const VAULT_ACTIVITY_PATH: &str = "/agent/vault/activity";
pub const VAULT_CARD_PATH: &str = "/agent/vault/card";
pub const VAULT_LOGIN_PREFIX: &str = "/agent/vault/logins/";
pub const VAULT_INDEX_PATH: &str = "/agent/vault/index";
pub const VAULT_ASK_PREFIX: &str = "/agent/vault/asks/";

/// The file Export saves the activity log as.
pub const ACTIVITY_FILE: &str = "nexus-activity.csv";

/// A login's, a merchant's or an ask's id: 1 to 64 letters, digits, `_` or `-`.
pub fn vault_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// The password managers Nexus can ask. `Nexus` is the Nexus vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerKind {
    OnePassword,
    Bitwarden,
    Nexus,
}

impl ManagerKind {
    fn of(value: Option<&Value>) -> Option<ManagerKind> {
        Some(match value?.as_str()? {
            "1password" => ManagerKind::OnePassword,
            "bitwarden" => ManagerKind::Bitwarden,
            "nexus" => ManagerKind::Nexus,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            ManagerKind::OnePassword => "1Password",
            ManagerKind::Bitwarden => "Bitwarden",
            ManagerKind::Nexus => "Nexus vault",
        }
    }

    /// Who fills a login in, as a sentence starts: "1Password", "The Nexus vault".
    pub fn filler(self) -> &'static str {
        match self {
            ManagerKind::Nexus => "The Nexus vault",
            other => other.name(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Manager {
    pub kind: ManagerKind,
    /// The vault's name, such as "Maya’s Personal vault".
    pub vault: String,
    /// How many logins it holds, when it says (the Nexus vault does).
    pub total: Option<u64>,
}

/// What one login lets Nexus do: ask at each use, or use it until a date. Each use takes Face ID.
#[derive(Debug, Clone, PartialEq)]
pub enum Grant {
    EachUse,
    Until(String),
}

/// A weak or reused password, noted quietly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Weak,
    Reused,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    pub id: String,
    pub name: String,
    pub site: String,
    /// None: Nexus may not use it (the Nexus vault lists those too).
    pub grant: Option<Grant>,
    /// ISO 8601, or None when never used.
    pub last_used: Option<String>,
    pub flag: Option<Flag>,
    /// Masked, such as "m•••": the source gives none for some logins.
    pub account: Option<String>,
    /// Where the login lives: the Nexus vault on the person's Mac, or their own 1Password.
    pub source: Option<ManagerKind>,
}

/// The virtual card. Amounts are decimal strings in US dollars. Its period is a month.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub last4: String,
    pub frozen: bool,
    pub spent: String,
    pub limit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    AsksFirst,
    FaceId,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Per {
    Order,
    Ride,
    Booking,
}

/// One merchant's limit: an amount per order, ride or booking, or none; and how a payment there is approved.
#[derive(Debug, Clone, PartialEq)]
pub struct Merchant {
    pub id: String,
    pub merchant: String,
    pub amount: Option<String>,
    pub per: Per,
    pub approval: Approval,
}

/// A grant that waits for the person: one login, one site, one task.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub id: String,
    pub login_id: String,
    pub login_name: String,
    /// Masked, such as "MileagePlus ••• 772": four digits at most.
    pub account: String,
    pub manager: ManagerKind,
    pub site: String,
    pub task: String,
    /// The "Until" choice the sheet offers; None offers "This once" alone.
    pub until: Option<String>,
    pub expires_at: String,
}

/// `GET /agent/vault`'s answer. No manager: no password manager is linked. No card: no card.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VaultView {
    pub manager: Option<Manager>,
    pub logins: Vec<Login>,
    pub card: Option<Card>,
    pub merchants: Vec<Merchant>,
    pub apple_pay: bool,
    pub asks: Vec<Ask>,
}

/// What an ask came to. `ConfirmOnDevice`: the approval waits for Face ID on the person's phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decided {
    Approved,
    Declined,
    ConfirmOnDevice,
}

/// A write's answer: the vault as it is now, the errands a revoke stopped, and what an ask came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Changed {
    pub vault: VaultView,
    pub stopped: Option<u64>,
    pub decided: Option<Decided>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Approved,
    Refused,
}

/// One line of the activity log. `with` and `approved` are None for "—".
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub at: String,
    pub what: String,
    pub with: Option<String>,
    pub approved: Option<String>,
    pub result: String,
    pub outcome: Outcome,
}

/// How a read ended. `Off`: the agent API has no vault here.
#[derive(Debug, Clone, PartialEq)]
pub enum Read {
    View(Box<VaultView>),
    Off,
    Failed(u16),
}

/// A key named like a secret (the kit's `SECRET_KEYS`).
fn secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "password"
            | "passwd"
            | "passcode"
            | "passphrase"
            | "secret"
            | "pin"
            | "otp"
            | "totp"
            | "cvc"
            | "cvv"
            | "pan"
            | "card_number"
            | "cardnumber"
            | "number"
    )
}

/// A run of twelve or more digits, as a card number is, with at most one space or dash between two digits.
fn long_digit_run(text: &str) -> bool {
    let mut run = 0;
    let mut gap = false;
    for char in text.chars() {
        if char.is_ascii_digit() {
            run += 1;
            gap = false;
            if run >= 12 {
                return true;
            }
        } else if matches!(char, ' ' | '-') && run > 0 && !gap {
            gap = true;
        } else {
            run = 0;
            gap = false;
        }
    }
    false
}

/// Whether a body holds a secret's shape: a key named like one (`password`, `cvc`, `cardNumber`), or a run of twelve or
/// more digits, as a card number is, even with spaces or dashes in it. Masked values ("••4021") have four at most.
pub fn holds_secret(value: &Value) -> bool {
    fn holds(value: &Value, depth: usize) -> bool {
        if depth > 8 {
            return true;
        }
        match value {
            Value::String(text) => long_digit_run(text),
            Value::Array(items) => items.iter().any(|item| holds(item, depth + 1)),
            Value::Object(fields) => fields
                .iter()
                .any(|(key, item)| secret_key(key) || holds(item, depth + 1)),
            _ => false,
        }
    }
    holds(value, 0)
}

/// A string with some text and at most `max` UTF-16 units, kept as sent.
pub(crate) fn text(value: Option<&Value>, max: usize) -> Option<String> {
    let value = value?.as_str()?;
    (!value.trim().is_empty() && value.encode_utf16().count() <= max).then(|| value.to_string())
}

pub(crate) fn id(value: Option<&Value>) -> Option<String> {
    value?.as_str().filter(|id| vault_id(id)).map(String::from)
}

pub(crate) fn time(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?;
    (value.len() <= 40 && DateTime::parse_from_rfc3339(value).is_ok()).then(|| value.to_string())
}

/// A decimal string of dollars: 1 to 9 digits, then up to 2 decimals.
fn decimal(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?;
    let (whole, cents) = value.split_once('.').unwrap_or((value, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let shaped = (1..=9).contains(&whole.len())
        && digits(whole)
        && (!value.contains('.') || ((1..=2).contains(&cents.len()) && digits(cents)));
    shaped.then(|| value.to_string())
}

/// A field that must be there and is `null` or reads: `Some(None)` for `null`, None when it does not read.
fn nullable<T>(value: Option<&Value>, read: impl FnOnce(Option<&Value>) -> Option<T>) -> Option<Option<T>> {
    match value? {
        Value::Null => Some(None),
        other => read(Some(other)).map(Some),
    }
}

fn fields(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn manager_of(value: Option<&Value>) -> Option<Manager> {
    let value = value?;
    fields(value)?;
    let total = match value.get("total") {
        None => None,
        Some(total) => Some(total.as_u64()?),
    };
    Some(Manager {
        kind: ManagerKind::of(value.get("kind"))?,
        vault: text(value.get("vault"), 120)?,
        total,
    })
}

fn grant_of(value: Option<&Value>) -> Option<Grant> {
    let value = value?;
    match value.get("kind")?.as_str()? {
        "each_use" => Some(Grant::EachUse),
        "until" => Some(Grant::Until(time(value.get("until"))?)),
        _ => None,
    }
}

fn login_of(value: &Value) -> Option<Login> {
    fields(value)?;
    let flag = match value.get("flag") {
        None => None,
        Some(flag) => Some(match flag.as_str()? {
            "weak" => Flag::Weak,
            "reused" => Flag::Reused,
            _ => return None,
        }),
    };
    Some(Login {
        id: id(value.get("id"))?,
        name: text(value.get("name"), 120)?,
        site: text(value.get("site"), 253)?,
        grant: nullable(value.get("grant"), grant_of)?,
        last_used: nullable(value.get("lastUsed"), time)?,
        flag,
        account: match value.get("account") {
            None => None,
            Some(account) => Some(text(Some(account), 80)?),
        },
        source: match value.get("source") {
            None => None,
            Some(source) => Some(ManagerKind::of(Some(source))?),
        },
    })
}

fn card_of(value: Option<&Value>) -> Option<Card> {
    let value = value?;
    let last4 = value.get("last4")?.as_str()?;
    if last4.len() != 4 || !last4.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let frozen = match value.get("state")?.as_str()? {
        "active" => false,
        "frozen" => true,
        _ => return None,
    };
    if value.get("period")?.as_str()? != "month" {
        return None;
    }
    Some(Card {
        last4: last4.to_string(),
        frozen,
        spent: decimal(value.get("spent"))?,
        limit: decimal(value.get("limit"))?,
    })
}

fn merchant_of(value: &Value) -> Option<Merchant> {
    fields(value)?;
    let per = match value.get("per")?.as_str()? {
        "order" => Per::Order,
        "ride" => Per::Ride,
        "booking" => Per::Booking,
        _ => return None,
    };
    let approval = match value.get("approval")?.as_str()? {
        "asks_first" => Approval::AsksFirst,
        "face_id" => Approval::FaceId,
        "off" => Approval::Off,
        _ => return None,
    };
    Some(Merchant {
        id: id(value.get("id"))?,
        merchant: text(value.get("merchant"), 120)?,
        amount: nullable(value.get("amount"), decimal)?,
        per,
        approval,
    })
}

fn ask_of(value: &Value) -> Option<Ask> {
    fields(value)?;
    let login = value.get("login")?;
    fields(login)?;
    let account = text(login.get("account"), 80)?;
    if account.chars().filter(char::is_ascii_digit).count() > 4 {
        return None;
    }
    Some(Ask {
        id: id(value.get("id"))?,
        login_id: id(login.get("id"))?,
        login_name: text(login.get("name"), 120)?,
        account,
        manager: ManagerKind::of(value.get("manager"))?,
        site: text(value.get("site"), 253)?,
        task: text(value.get("task"), 400)?,
        until: nullable(value.get("until"), time)?,
        expires_at: time(value.get("expiresAt"))?,
    })
}

fn list_of<T>(value: Option<&Value>, read: impl Fn(&Value) -> Option<T>, max: usize) -> Option<Vec<T>> {
    let items = value?.as_array()?;
    if items.len() > max {
        return None;
    }
    items.iter().map(read).collect()
}

/// `GET /agent/vault`'s body, read strictly; None when it is not one, or when it holds a secret's shape.
pub fn vault_view_of(body: &Value) -> Option<VaultView> {
    if fields(body).is_none() || holds_secret(body) {
        return None;
    }
    Some(VaultView {
        manager: nullable(body.get("manager"), manager_of)?,
        logins: list_of(body.get("logins"), login_of, 500)?,
        card: nullable(body.get("card"), card_of)?,
        merchants: list_of(body.get("merchants"), merchant_of, 50)?,
        apple_pay: body.get("applePay")?.as_bool()?,
        asks: list_of(body.get("asks"), ask_of, 20)?,
    })
}

/// A write's answer, read strictly.
pub fn vault_changed_of(body: &Value) -> Option<Changed> {
    fields(body)?;
    let stopped = match body.get("stopped") {
        None => None,
        Some(stopped) => Some(stopped.as_u64()?),
    };
    let decided = match body.get("decided") {
        None => None,
        Some(decided) => Some(match decided.as_str()? {
            "approved" => Decided::Approved,
            "declined" => Decided::Declined,
            "confirm_on_device" => Decided::ConfirmOnDevice,
            _ => return None,
        }),
    };
    Some(Changed {
        vault: vault_view_of(body.get("vault")?)?,
        stopped,
        decided,
    })
}

fn entry_of(value: &Value) -> Option<Entry> {
    fields(value)?;
    let outcome = match value.get("outcome")?.as_str()? {
        "done" => Outcome::Done,
        "approved" => Outcome::Approved,
        "refused" => Outcome::Refused,
        _ => return None,
    };
    Some(Entry {
        at: time(value.get("at"))?,
        what: text(value.get("what"), 200)?,
        with: nullable(value.get("with"), |value| text(value, 120))?,
        approved: nullable(value.get("approved"), |value| text(value, 120))?,
        result: text(value.get("result"), 120)?,
        outcome,
    })
}

/// `GET /agent/vault/activity`'s body, `{ entries }`, read strictly.
pub fn vault_activity_of(body: &Value) -> Option<Vec<Entry>> {
    if fields(body).is_none() || holds_secret(body) {
        return None;
    }
    list_of(body.get("entries"), entry_of, 1000)
}

/// Whether a status and body say the vault is off here: a 404, or a 503 `vault_off`.
pub fn vault_off(status: u16, body: Option<&Value>) -> bool {
    let error = body.and_then(|body| body.get("error")).and_then(Value::as_str);
    status == 404 || (status == 503 && error == Some("vault_off"))
}

/// A read from its status and body.
pub fn vault_read(status: u16, body: Option<&Value>) -> Read {
    if vault_off(status, body) {
        return Read::Off;
    }
    if !(200..300).contains(&status) {
        return Read::Failed(status);
    }
    match body.and_then(vault_view_of) {
        Some(view) => Read::View(Box::new(view)),
        None => Read::Failed(502),
    }
}

/// How a read of the activity log ended. `Off`: the agent API has no vault here, shown as nothing yet.
#[derive(Debug, Clone, PartialEq)]
pub enum Log {
    Entries(Vec<Entry>),
    Off,
    Failed,
}

/// The activity log from its status and body.
pub fn activity_read(status: u16, body: Option<&Value>) -> Log {
    if vault_off(status, body) {
        return Log::Off;
    }
    match body.and_then(vault_activity_of) {
        Some(entries) if (200..300).contains(&status) => Log::Entries(entries),
        _ => Log::Failed,
    }
}

/// What a write's failure says (Nexus UI's `vaultChangeOf`).
pub fn change_failed(status: u16, body: Option<&Value>) -> &'static str {
    if vault_off(status, body) {
        "No password manager or card is linked to Nexus here yet."
    } else if status == 410 {
        "That request is no longer waiting. Ask Nexus again."
    } else {
        "That did not save. Try again in a moment."
    }
}

/// A login change: each use, until a date, or none (Revoke).
#[derive(Debug, Clone, PartialEq)]
pub enum LoginChange {
    EachUse,
    Revoke,
    Until(String),
}

/// `PUT /agent/vault/logins/{id}`'s body.
pub fn login_body(change: &LoginChange) -> String {
    match change {
        LoginChange::EachUse => json!({ "grant": "each_use" }),
        LoginChange::Revoke => json!({ "grant": "none" }),
        LoginChange::Until(until) => json!({ "grant": { "until": until } }),
    }
    .to_string()
}

/// Whether `body` is exactly a login change (the kit's `vaultLoginChangeOf`).
pub fn is_login_body(body: &str) -> bool {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    if fields.len() != 1 {
        return false;
    }
    match fields.get("grant") {
        Some(Value::String(grant)) => grant == "each_use" || grant == "none",
        Some(Value::Object(grant)) => grant.len() == 1 && time(grant.get("until")).is_some(),
        _ => false,
    }
}

/// `PUT /agent/vault/card`'s body.
pub fn card_body(frozen: bool) -> String {
    json!({ "frozen": frozen }).to_string()
}

pub fn is_card_body(body: &str) -> bool {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    fields.len() == 1 && matches!(fields.get("frozen"), Some(Value::Bool(_)))
}

/// The grant sheet's choice: this once, or until the date the ask offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum For {
    Once,
    Until,
}

/// `POST /agent/vault/asks/{id}/approve`'s body.
pub fn approval_body(choice: For) -> String {
    let chosen = match choice {
        For::Once => "once",
        For::Until => "until",
    };
    json!({ "for": chosen }).to_string()
}

pub fn is_approval_body(body: &str) -> bool {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    fields.len() == 1 && matches!(fields.get("for").and_then(Value::as_str), Some("once" | "until"))
}

/// One call of the Passwords and payments view, its path and body checked when it was made.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// `GET /agent/vault`.
    Read,
    /// `GET /agent/vault/activity`.
    Activity,
    /// `PUT /agent/vault/logins/{id}`.
    Login { path: String, body: String },
    /// `PUT /agent/vault/card`.
    Card { body: String },
    /// `POST /agent/vault/asks/{id}/approve` or `…/decline`.
    Decide { path: String, body: String },
    /// `PUT /agent/vault/index`: this device's index. Its body was refused locally if it held a secret's
    /// shape ([`crate::vault_local::check_index_body`]), so one that is here is safe to send.
    Index { body: String },
}

impl Request {
    pub fn login(id: &str, change: &LoginChange) -> Option<Request> {
        let body = login_body(change);
        (vault_id(id) && is_login_body(&body)).then(|| Request::Login {
            path: format!("{VAULT_LOGIN_PREFIX}{id}"),
            body,
        })
    }

    pub fn card(frozen: bool) -> Option<Request> {
        let body = card_body(frozen);
        is_card_body(&body).then_some(Request::Card { body })
    }

    /// Approve the ask for `choice`, or decline it with None.
    pub fn decide(ask: &str, choice: Option<For>) -> Option<Request> {
        if !vault_id(ask) {
            return None;
        }
        let (decision, body) = match choice {
            Some(choice) => ("approve", approval_body(choice)),
            None => ("decline", "{}".to_string()),
        };
        (choice.is_none() || is_approval_body(&body)).then(|| Request::Decide {
            path: format!("{VAULT_ASK_PREFIX}{ask}/{decision}"),
            body,
        })
    }

    /// The device's index, or None when it is not an index, or holds a secret's shape (refused here, never sent).
    pub fn index(body: String) -> Option<Request> {
        crate::vault_local::check_index_body(&body)
            .is_ok()
            .then_some(Request::Index { body })
    }

    /// Its method, path and body.
    pub fn request(&self) -> (Method, String, Option<String>) {
        match self {
            Request::Index { body } => (Method::PUT, VAULT_INDEX_PATH.into(), Some(body.clone())),
            Request::Read => (Method::GET, VAULT_PATH.into(), None),
            Request::Activity => (Method::GET, VAULT_ACTIVITY_PATH.into(), None),
            Request::Login { path, body } => (Method::PUT, path.clone(), Some(body.clone())),
            Request::Card { body } => (Method::PUT, VAULT_CARD_PATH.into(), Some(body.clone())),
            Request::Decide { path, body } => (Method::POST, path.clone(), Some(body.clone())),
        }
    }
}

// The words each row shows. Times read in the person's own zone, English only for now.

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const DAY_MS: i64 = 86_400_000;

fn local<Tz: TimeZone>(iso: &str, zone: &Tz) -> Option<DateTime<Tz>> {
    DateTime::parse_from_rfc3339(iso).ok().map(|at| at.with_timezone(zone))
}

fn weekday<Tz: TimeZone>(at: &DateTime<Tz>) -> String {
    at.weekday().to_string()
}

fn day_month<Tz: TimeZone>(at: &DateTime<Tz>) -> String {
    format!("{} {}", at.day(), MONTHS[at.month0() as usize])
}

fn clock<Tz: TimeZone>(at: &DateTime<Tz>) -> String {
    format!("{:02}:{:02}", at.hour(), at.minute())
}

/// "Sun 8 Nov".
pub fn vault_day<Tz: TimeZone>(iso: &str, zone: &Tz) -> String {
    match local(iso, zone) {
        Some(at) => format!("{} {}", weekday(&at), day_month(&at)),
        None => String::new(),
    }
}

/// A grant as the table says it: "Each use · Face ID", "Until Sun 8 Nov", or "Off".
pub fn grant_label<Tz: TimeZone>(grant: Option<&Grant>, zone: &Tz) -> String {
    match grant {
        None => "Off".into(),
        Some(Grant::EachUse) => "Each use · Face ID".into(),
        Some(Grant::Until(until)) => format!("Until {}", vault_day(until, zone)),
    }
}

/// When `at` was, against `now` in `zone`: today, within the week, or before it.
enum Ago {
    Today,
    Week,
    Before,
}

fn ago<Tz: TimeZone>(at: &DateTime<Tz>, now: DateTime<Utc>, zone: &Tz) -> Ago {
    let today = now.with_timezone(zone);
    if at.date_naive() == today.date_naive() {
        return Ago::Today;
    }
    let age = now.timestamp_millis() - at.timestamp_millis();
    if (0..6 * DAY_MS).contains(&age) {
        Ago::Week
    } else {
        Ago::Before
    }
}

/// "Today 07:41", "Tue" within the week, "12 Oct" before it, "never".
pub fn vault_when<Tz: TimeZone>(iso: Option<&str>, now: DateTime<Utc>, zone: &Tz) -> String {
    let Some(at) = iso.and_then(|iso| local(iso, zone)) else {
        return "never".into();
    };
    match ago(&at, now, zone) {
        Ago::Today => format!("Today {}", clock(&at)),
        Ago::Week => weekday(&at),
        Ago::Before => day_month(&at),
    }
}

/// The activity log's When: "07:41" today, "Tue 19:02" within the week, "12 Oct 08:30" before it.
pub fn log_when<Tz: TimeZone>(iso: &str, now: DateTime<Utc>, zone: &Tz) -> String {
    let Some(at) = local(iso, zone) else {
        return String::new();
    };
    match ago(&at, now, zone) {
        Ago::Today => clock(&at),
        Ago::Week => format!("{} {}", weekday(&at), clock(&at)),
        Ago::Before => format!("{} {}", day_month(&at), clock(&at)),
    }
}

/// "$142", "$38.60", "$1,250".
pub fn money(amount: &str) -> String {
    let (whole, cents) = amount.split_once('.').unwrap_or((amount, ""));
    let whole = if whole.is_empty() { "0" } else { whole };
    let mut grouped = String::new();
    for (at, digit) in whole.chars().enumerate() {
        if at > 0 && (whole.len() - at) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if cents.bytes().any(|byte| byte != b'0') {
        format!("${grouped}.{cents:0<2}")
    } else {
        format!("${grouped}")
    }
}

/// "$142 of $500 this month".
pub fn spent_line(card: &Card) -> String {
    format!("{} of {} this month", money(&card.spent), money(&card.limit))
}

/// How far the month's spend is, 0 to 1.
pub fn spent_share(card: &Card) -> f32 {
    let limit = card.limit.parse::<f64>().unwrap_or(0.);
    let spent = card.spent.parse::<f64>().unwrap_or(0.);
    if limit > 0. {
        (spent / limit).clamp(0., 1.) as f32
    } else {
        0.
    }
}

/// A merchant's Limit: "$60 an order", "Each booking", or "Off".
pub fn limit_label(merchant: &Merchant) -> String {
    let (per, each) = match merchant.per {
        Per::Order => ("an order", "order"),
        Per::Ride => ("a ride", "ride"),
        Per::Booking => ("a booking", "booking"),
    };
    match (merchant.approval, &merchant.amount) {
        (Approval::Off, _) => "Off".into(),
        (_, Some(amount)) => format!("{} {per}", money(amount)),
        (_, None) => format!("Each {each}"),
    }
}

/// A merchant's Approval: "Asks first", "Face ID", or "Can’t pay".
pub fn approval_label(approval: Approval) -> &'static str {
    match approval {
        Approval::AsksFirst => "Asks first",
        Approval::FaceId => "Face ID",
        Approval::Off => "Can’t pay",
    }
}

/// The manager card's line: how many logins Nexus can ask to use, and that it never reads them.
pub fn manager_line(view: &VaultView) -> String {
    let usable = view.logins.iter().filter(|login| login.grant.is_some()).count();
    let logins = if usable == 1 {
        "1 login".to_string()
    } else {
        format!("{usable} logins")
    };
    format!("Nexus can ask to use {logins}. It never reads, stores or repeats them.")
}

/// What a revoke says: who, and what it stopped.
pub fn revoked_text(name: &str, stopped: u64) -> String {
    match stopped {
        0 => format!("{name} revoked. Nothing running was using it."),
        1 => format!("{name} revoked. It stopped one errand that used it."),
        count => format!("{name} revoked. It stopped {count} errands that used it."),
    }
}

/// What the grant sheet says once the ask is decided.
pub fn decided_text(ask: &Ask, decided: Option<Decided>) -> String {
    match decided {
        Some(Decided::ConfirmOnDevice) => "Confirm with Face ID on your phone to finish.".into(),
        Some(Decided::Declined) => "Declined. Nexus won’t use it.".into(),
        _ => format!("Approved. {} fills it in on {}.", ask.manager.filler(), ask.site),
    }
}

/// What Freeze or unfreeze says once it saved.
pub fn frozen_text(frozen: bool) -> &'static str {
    if frozen {
        "The card is frozen. Nothing can be paid with it."
    } else {
        "The card works again."
    }
}

/// The log as a CSV file's text, for Export. Each cell quoted.
pub fn activity_csv(entries: &[Entry]) -> String {
    let cell = |value: Option<&str>| format!("\"{}\"", value.unwrap_or("—").replace('"', "\"\""));
    let mut lines = vec!["When,What,With,Approved,Result".to_string()];
    lines.extend(entries.iter().map(|entry| {
        [
            Some(entry.at.as_str()),
            Some(entry.what.as_str()),
            entry.with.as_deref(),
            entry.approved.as_deref(),
            Some(entry.result.as_str()),
        ]
        .map(cell)
        .join(",")
    }));
    lines.join("\n")
}

/// Where an import can come from: its name and what it does.
pub const IMPORT_SOURCES: [(&str, &str); 5] = [
    (
        "Chrome",
        "Import directly (your Mac asks to allow it) or choose Chrome’s CSV export",
    ),
    ("1Password", "Link it, as now, or choose an export file"),
    ("Bitwarden", "Link it, or choose an export file"),
    ("Apple Passwords", "Choose an export file"),
    ("A CSV file", "From any password manager"),
];
/// The import sheet's steps.
pub const IMPORT_STEPS: [&str; 4] = ["Source", "What’s in it", "What Nexus may use", "Done"];
/// The import sheet's and the Nexus vault's foot.
pub const VAULT_FOOT: &str =
    "Encrypted on this device before it leaves. Nexus never sees or repeats a password, and each use takes Face ID.";
/// What a source says while no importer exists here: nothing is read.
pub const IMPORT_NOT_ON: &str = "Importing needs the Nexus vault, which is not on here yet. Nothing was read.";

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Value {
        json!({
            "manager": { "kind": "1password", "vault": "Maya’s Personal vault" },
            "logins": [
                { "id": "united", "name": "United", "site": "united.com", "grant": { "kind": "each_use" }, "lastUsed": "2026-11-17T07:41:00Z" },
                { "id": "marriott", "name": "Marriott Bonvoy", "site": "marriott.com", "grant": { "kind": "until", "until": "2026-11-08T12:00:00Z" }, "lastUsed": null },
                { "id": "amazon", "name": "Amazon", "site": "amazon.com", "grant": null, "lastUsed": null, "flag": "reused" }
            ],
            "card": { "last4": "4021", "state": "active", "spent": "142", "limit": "500", "period": "month" },
            "merchants": [
                { "id": "doordash", "merchant": "DoorDash", "amount": "60", "per": "order", "approval": "asks_first" },
                { "id": "united", "merchant": "United", "amount": null, "per": "booking", "approval": "face_id" },
                { "id": "amazon", "merchant": "Amazon", "amount": null, "per": "order", "approval": "off" }
            ],
            "applePay": true,
            "asks": [{ "id": "ask_1", "login": { "id": "united", "name": "United", "account": "MileagePlus ••• 772" }, "manager": "1password",
                       "site": "united.com", "task": "Check in for UA 1532", "until": "2026-11-08T12:00:00Z", "expiresAt": "2026-11-17T08:00:00Z" }]
        })
    }

    #[test]
    fn reads_the_vault_strictly() {
        let view = vault_view_of(&body()).expect("reads");
        assert_eq!(
            view.manager.as_ref().map(|manager| manager.kind),
            Some(ManagerKind::OnePassword)
        );
        assert_eq!(view.logins.len(), 3);
        assert_eq!(view.logins[2].flag, Some(Flag::Reused));
        assert_eq!(view.card.as_ref().map(|card| card.frozen), Some(false));
        assert_eq!(view.asks[0].until.as_deref(), Some("2026-11-08T12:00:00Z"));
        let mut broken = body();
        broken["card"]["state"] = json!("lost");
        assert_eq!(vault_view_of(&broken), None);
        let mut broken = body();
        broken["logins"][0].as_object_mut().expect("a login").remove("lastUsed");
        assert_eq!(vault_view_of(&broken), None, "lastUsed is there, or null");
        let mut broken = body();
        broken["asks"][0]["login"]["account"] = json!("MileagePlus 12345");
        assert_eq!(vault_view_of(&broken), None, "a masked account has four digits at most");
        let unlinked =
            json!({ "manager": null, "logins": [], "card": null, "merchants": [], "applePay": false, "asks": [] });
        assert_eq!(vault_view_of(&unlinked), Some(VaultView::default()));
    }

    #[test]
    fn refuses_anything_shaped_like_a_secret() {
        for secret in [
            json!({ "password": "x" }),
            json!({ "deep": [{ "cardNumber": "4021" }] }),
            json!({ "note": "4111 1111 1111 1111" }),
            json!({ "note": "4111-1111-1111" }),
            json!({ "CVC": "123" }),
        ] {
            assert!(holds_secret(&secret), "{secret}");
        }
        for safe in [
            json!({ "account": "MileagePlus ••• 772" }),
            json!({ "note": "card ••4021, 2026-11-17" }),
            json!({ "note": "1 2 3 4 5 6 7 8 9 0 1" }),
        ] {
            assert!(!holds_secret(&safe), "{safe}");
        }
        let mut leaked = body();
        leaked["logins"][0]["name"] = json!("United 4111111111111111");
        assert_eq!(vault_view_of(&leaked), None);
        let mut leaked = body();
        leaked["card"]["number"] = json!("4021");
        assert_eq!(
            vault_view_of(&leaked),
            None,
            "a key named like a secret, whatever it holds"
        );
        let mut deep = json!("x");
        for _ in 0..10 {
            deep = json!([deep]);
        }
        assert!(holds_secret(&deep), "too deep reads as unsafe");
    }

    #[test]
    fn reads_off_as_nothing_linked() {
        assert_eq!(vault_read(503, Some(&json!({ "error": "vault_off" }))), Read::Off);
        assert_eq!(vault_read(404, None), Read::Off);
        assert_eq!(vault_read(503, Some(&json!({ "error": "busy" }))), Read::Failed(503));
        assert_eq!(vault_read(200, Some(&json!({}))), Read::Failed(502));
        assert_eq!(activity_read(503, Some(&json!({ "error": "vault_off" }))), Log::Off);
        assert_eq!(
            change_failed(410, None),
            "That request is no longer waiting. Ask Nexus again."
        );
    }

    #[test]
    fn says_each_row_as_a8_draws_it() {
        let view = vault_view_of(&body()).expect("reads");
        let utc = Utc;
        let now = DateTime::parse_from_rfc3339("2026-11-17T12:00:00Z")
            .expect("a time")
            .with_timezone(&Utc);
        assert_eq!(grant_label(view.logins[0].grant.as_ref(), &utc), "Each use · Face ID");
        assert_eq!(grant_label(view.logins[1].grant.as_ref(), &utc), "Until Sun 8 Nov");
        assert_eq!(grant_label(None, &utc), "Off");
        assert_eq!(
            vault_when(view.logins[0].last_used.as_deref(), now, &utc),
            "Today 07:41"
        );
        assert_eq!(vault_when(Some("2026-11-12T19:02:00Z"), now, &utc), "Thu");
        assert_eq!(vault_when(Some("2026-10-12T08:30:00Z"), now, &utc), "12 Oct");
        assert_eq!(vault_when(None, now, &utc), "never");
        assert_eq!(log_when("2026-11-17T07:41:00Z", now, &utc), "07:41");
        assert_eq!(log_when("2026-11-12T19:02:00Z", now, &utc), "Thu 19:02");
        assert_eq!(log_when("2026-10-12T08:30:00Z", now, &utc), "12 Oct 08:30");
        let card = view.card.as_ref().expect("a card");
        assert_eq!(spent_line(card), "$142 of $500 this month");
        assert!((spent_share(card) - 0.284).abs() < 0.001);
        assert_eq!(money("38.6"), "$38.60");
        assert_eq!(money("1250.00"), "$1,250");
        let limits: Vec<String> = view.merchants.iter().map(limit_label).collect();
        assert_eq!(limits, ["$60 an order", "Each booking", "Off"]);
        assert_eq!(approval_label(Approval::Off), "Can’t pay");
        assert_eq!(
            manager_line(&view),
            "Nexus can ask to use 2 logins. It never reads, stores or repeats them."
        );
        assert_eq!(
            revoked_text("United", 0),
            "United revoked. Nothing running was using it."
        );
        assert_eq!(
            revoked_text("United", 2),
            "United revoked. It stopped 2 errands that used it."
        );
        assert_eq!(
            decided_text(&view.asks[0], Some(Decided::Approved)),
            "Approved. 1Password fills it in on united.com."
        );
    }

    #[test]
    fn builds_only_the_calls_the_agent_api_takes() {
        assert_eq!(
            Request::login("united", &LoginChange::Revoke),
            Some(Request::Login {
                path: "/agent/vault/logins/united".into(),
                body: r#"{"grant":"none"}"#.into()
            })
        );
        assert_eq!(Request::login("../x", &LoginChange::EachUse), None);
        assert_eq!(Request::login("united", &LoginChange::Until("Friday".into())), None);
        assert!(Request::login("united", &LoginChange::Until("2026-11-20T00:00:00Z".into())).is_some());
        assert_eq!(
            Request::decide("ask_1", Some(For::Until)).map(|request| request.request()),
            Some((
                Method::POST,
                "/agent/vault/asks/ask_1/approve".into(),
                Some(r#"{"for":"until"}"#.into())
            ))
        );
        assert_eq!(
            Request::decide("ask_1", None).map(|request| request.request().1),
            Some("/agent/vault/asks/ask_1/decline".into())
        );
        assert_eq!(
            Request::card(true).map(|request| request.request().2),
            Some(Some(r#"{"frozen":true}"#.into()))
        );
        assert!(!is_login_body(r#"{"grant":"each_use","x":1}"#));
        assert!(!is_approval_body(r#"{"for":"always"}"#));
    }

    #[test]
    fn exports_the_log_as_csv() {
        let entries = vault_activity_of(&json!({ "entries": [
            { "at": "2026-11-17T07:41:00Z", "what": "Checked in \"UA 1532\"", "with": "United login · 1Password", "approved": null, "result": "Done", "outcome": "done" }
        ]}))
        .expect("reads");
        assert_eq!(
            activity_csv(&entries),
            "When,What,With,Approved,Result\n\"2026-11-17T07:41:00Z\",\"Checked in \"\"UA 1532\"\"\",\"United login · 1Password\",\"—\",\"Done\""
        );
    }
}
