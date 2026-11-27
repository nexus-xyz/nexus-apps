//! The Inbox's items and how they read, ported from Nexus UI (`ui/src/components/nexus-ui/inbox.ts`): Gateway's
//! notifications (`GET /v1/notifications`), the page each opens, which need the person, and their order. The view is
//! `workspace/inbox_view.rs`.

use serde_json::Value;

use crate::model::text;

/// Gateway's notification groups, as the Inbox names them (`GROUP_LABELS`).
pub fn group_label(group: &str) -> &'static str {
    match group {
        "my_sessions" => "My sessions",
        "usage_and_billing" => "Usage and billing",
        "requests_and_approvals" => "Requests and approvals",
        "automations_and_integrations" => "Automations and integrations",
        "account_and_security" => "Account and security",
        _ => "Notifications",
    }
}

/// One notification, as the Inbox reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub group: String,
    pub subject_text: String,
    /// `organization`, `team`, `user` or `artifact`, and its id.
    pub subject_type: String,
    pub subject_id: String,
    pub sources: Vec<Source>,
    pub count: u64,
    pub read: bool,
    pub updated_at: String,
}

/// What caused a notification (its `sources`, a union on `type`); only the fields the Inbox reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Source {
    pub kind: String,
    pub artifact_id: Option<String>,
    pub outcome: Option<String>,
    pub action: Option<String>,
    pub promotion_id: Option<String>,
    /// A Policy approval record (`apr_…`) that waits on the person.
    pub approval_id: Option<String>,
}

/// One pagination read. Its cached and fresh replies share this identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    pub id: u64,
    pub cursor: Option<String>,
    pub unread: bool,
}

/// A page of notifications, newest first, and the cursor of the next.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    pub items: Vec<Item>,
    pub next_cursor: Option<String>,
}

/// What to do to a notification: read it, or put it away (Done).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Read,
    Done,
}

impl Mark {
    pub fn path(self) -> &'static str {
        match self {
            Mark::Read => "read",
            Mark::Done => "done",
        }
    }
}

impl Item {
    pub fn from_json(item: &Value) -> Option<Item> {
        let sources = item
            .get("sources")
            .and_then(Value::as_array)
            .map(|sources| {
                sources
                    .iter()
                    .map(|source| Source {
                        kind: text(source, "type").unwrap_or_default(),
                        artifact_id: text(source, "artifactId"),
                        outcome: text(source, "outcome"),
                        action: text(source, "action"),
                        promotion_id: text(source, "promotionId"),
                        approval_id: text(source, "approvalId"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let subject = item.get("subject");
        Some(Item {
            id: text(item, "id")?,
            kind: text(item, "kind").unwrap_or_default(),
            group: text(item, "group").unwrap_or_default(),
            subject_text: text(item, "subjectText").unwrap_or_default(),
            subject_type: subject.and_then(|subject| text(subject, "type")).unwrap_or_default(),
            subject_id: subject.and_then(|subject| text(subject, "id")).unwrap_or_default(),
            sources,
            count: item.get("count").and_then(Value::as_u64).unwrap_or(1),
            read: item.get("readAt").is_some_and(|read| !read.is_null()),
            updated_at: text(item, "updatedAt")
                .or_else(|| text(item, "createdAt"))
                .unwrap_or_default(),
        })
    }
}

impl Page {
    pub fn from_json(page: &Value) -> Page {
        Page {
            items: page
                .get("items")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Item::from_json).collect())
                .unwrap_or_default(),
            next_cursor: text(page, "nextCursor").filter(|cursor| !cursor.is_empty()),
        }
    }
}

/// Where an item leads (`itemPath`).
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// An artifact of this organization: it opens in the Workbench's Artifacts tab.
    Artifact(String),
    /// A Code session of this computer.
    Code(String),
    /// Another organization's artifact, through the viewer (`/a/<id>`), in the Workbench's Browser tab.
    Viewer(String),
    /// A page of the organization in Nexus UI (`/app/<org><path>`), in the Workbench's Browser tab: the admin pages
    /// this app does not draw yet.
    Org(String),
    /// Nexus UI's approval page (`/app/approvals/<id>`), in the Workbench's Browser tab: the person confirms or denies
    /// their agent's write there, signed in as themselves.
    Approval(String),
}

impl Target {
    /// The Nexus UI address a target shows in the Workbench's Browser tab, on `site` for organization `org`. `None`
    /// for a target the app draws itself: an artifact or a Code session.
    pub fn web_address(&self, site: &str, org: Option<&str>) -> Option<String> {
        let site = site.trim_end_matches('/');
        match self {
            Target::Artifact(_) | Target::Code(_) => None,
            Target::Viewer(id) => Some(format!("{site}/a/{id}")),
            Target::Org(path) => Some(match org {
                Some(org) => crate::pages::org_url(site, org, path),
                None => format!("{site}/app"),
            }),
            Target::Approval(id) => Some(format!("{site}/app/approvals/{id}")),
        }
    }
}

/// How an item looks in the list (`InboxSymbol`'s moments).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    Approval,
    Error,
    Request,
    Billing,
    Security,
    Integration,
    Activity,
    Finished,
}

/// Which items the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Needs,
    Unread,
}

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::All, Filter::Needs, Filter::Unread];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Needs => "Needs you",
            Filter::Unread => "Unread",
        }
    }
}

/// A row of the Inbox: a Gateway notification, or one of this computer's Code sessions that needs the person.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub moment: Moment,
    pub title: String,
    pub context: String,
    /// ISO time for a notification; empty for a local session, which has `at_ms`.
    pub at: String,
    pub at_ms: Option<i64>,
    pub target: Option<Target>,
    pub action: &'static str,
    pub needs_you: bool,
    pub read: bool,
    pub count: u64,
    /// Whether it is Gateway's, so Mark read and Done apply.
    pub remote: bool,
}

impl Entry {
    /// Which hover actions the row offers: (Mark read, Done). A Code session that still needs the person has no Done.
    /// They open left of the row's time on hover.
    pub fn hover_actions(&self) -> (bool, bool) {
        let read = !self.read;
        let done = self.remote || !self.needs_you || self.moment == Moment::Error;
        (read, done)
    }
}

/// The kinds that need the person, across the groups (`needsAttention`).
pub fn needs_attention(item: &Item) -> bool {
    matches!(
        item.kind.as_str(),
        "usage.hard_stop_reached"
            | "billing.payment_failed"
            | "integrations.provider_key_rejected"
            | "requests.artifact_access_requested"
            | "requests.artifact_reported"
            | "requests.skill_promotion_requested"
            | "requests.approval_waiting"
    )
}

/// The page an item opens (`itemPath`).
pub fn target(item: &Item) -> Option<Target> {
    let source = item.sources.first();
    let source_kind = source.map_or("", |source| source.kind.as_str());
    let kind = item.kind.as_str();
    if source_kind == "artifact_access_answer" || kind == "requests.artifact_access_answered" {
        return source
            .filter(|source| source.outcome.as_deref() == Some("granted"))
            .and_then(|source| source.artifact_id.clone())
            .map(Target::Viewer);
    }
    if source_kind == "artifact_report" || kind == "requests.artifact_reported" {
        return Some(Target::Org("/artifacts/external".into()));
    }
    if source_kind == "artifact_access"
        && let Some(id) = source.and_then(|source| source.artifact_id.clone())
    {
        return Some(Target::Artifact(id));
    }
    if kind == "requests.skill_promotion_requested" || kind == "requests.skill_promotion_decided" {
        return Some(Target::Org(promotion_path(item)));
    }
    if item.subject_type == "artifact" && !item.subject_id.is_empty() {
        return Some(Target::Artifact(item.subject_id.clone()));
    }
    if source_kind == "spend" || kind == "usage.hard_stop_reached" {
        return Some(Target::Org("/spend".into()));
    }
    // A money notice and a member's question about billing open Billing too.
    let billing_sources = ["billing_change", "billing_period", "billing_state", "owner_ask"];
    if billing_sources.contains(&source_kind) || kind == "billing.payment_failed" {
        return Some(Target::Org("/billing".into()));
    }
    if source_kind == "member" || kind == "requests.member_joined" {
        return Some(Target::Org("/members".into()));
    }
    if source_kind == "security_change" {
        let action = source.and_then(|source| source.action.as_deref()).unwrap_or("");
        return Some(Target::Org(security_path(action).into()));
    }
    if source_kind == "provider_key" || kind == "integrations.provider_key_rejected" {
        return Some(Target::Org("/settings/providers".into()));
    }
    if source_kind == "github_installation" || kind == "integrations.connector_disconnected" {
        return Some(Target::Org("/settings/repositories".into()));
    }
    if source_kind == "referral" {
        return Some(Target::Org("/referrals".into()));
    }
    if source_kind == "approval"
        && let Some(id) = source.and_then(|source| source.approval_id.clone())
    {
        return Some(Target::Approval(id));
    }
    None
}

/// One promotion request opens that request; several (or none) open the list (`promotionItemPath`).
fn promotion_path(item: &Item) -> String {
    let requested = item.kind == "requests.skill_promotion_requested";
    let mut ids: Vec<&str> = item
        .sources
        .iter()
        .filter(|source| source.kind == "skill_promotion")
        .filter_map(|source| source.promotion_id.as_deref())
        .collect();
    ids.dedup();
    match ids.as_slice() {
        [only] if item.count <= 1 => format!("/skills/promotions/{}", nexus_client::url::enc(only)),
        _ if requested => "/skills?view=requests&of=reach&state=held".into(),
        _ => "/skills?view=requests&of=mine".into(),
    }
}

/// The settings a security change touched; sign-ins and the rest open the audit log (`securityPath`).
fn security_path(action: &str) -> &'static str {
    if action.starts_with("provider_key.") {
        "/settings/providers"
    } else if action.starts_with("key.") {
        "/keys"
    } else if action.starts_with("member.") || action.starts_with("org.") {
        "/members"
    } else {
        "/settings/audit"
    }
}

/// Fixed wording for routine security changes (`SECURITY_TEXT`).
fn security_text(action: &str) -> Option<&'static str> {
    Some(match action {
        "key.created" => "An API key was created",
        "key.revoked" => "An API key was revoked",
        "provider_key.added" => "A provider key was added",
        "provider_key.removed" => "A provider key was removed",
        "provider_key.disabled" => "A provider key was disabled",
        "provider_key.enabled" => "A provider key was enabled",
        "member.role_changed" => "A member's role changed",
        "member.removed" => "A member was removed",
        "org.owner_transferred" => "Organization ownership changed",
        "auth.device_approved" => "A new sign-in was approved",
        "cloud_history.break_glass_read" => "Nexus read a cloud session's history",
        _ => return None,
    })
}

/// A join is done when the item is made: nobody has a request to answer, so it is activity, not a request (`isNotice`).
fn is_notice(item: &Item) -> bool {
    item.kind == "requests.member_joined"
}

/// A notification as a row (`gatewayEntry`).
pub fn entry(item: &Item) -> Entry {
    let source = item.sources.first();
    let security = source.is_some_and(|source| source.kind == "security_change");
    let moment = if security {
        Moment::Security
    } else {
        match item.group.as_str() {
            "usage_and_billing" => Moment::Billing,
            "requests_and_approvals" if !is_notice(item) => Moment::Request,
            "automations_and_integrations" => Moment::Integration,
            _ => Moment::Activity,
        }
    };
    let title = source
        .filter(|_| security)
        .and_then(|source| source.action.as_deref())
        .and_then(security_text)
        .map(String::from)
        .unwrap_or_else(|| item.subject_text.clone());
    Entry {
        id: item.id.clone(),
        moment,
        title,
        context: group_label(&item.group).into(),
        at: item.updated_at.clone(),
        at_ms: None,
        target: target(item),
        action: match moment {
            Moment::Security => "Review change",
            Moment::Request => "Open request",
            Moment::Billing => "Review details",
            Moment::Integration => "Check connection",
            _ if is_notice(item) => "View members",
            _ => "Open details",
        },
        needs_you: needs_attention(item),
        read: item.read,
        count: item.count,
        remote: true,
    }
}

/// A Code session of this computer that waits for the person or failed.
pub fn code_entry(id: &str, title: &str, failed: bool, updated_at_ms: i64) -> Entry {
    Entry {
        id: format!("code:{id}"),
        moment: if failed { Moment::Error } else { Moment::Approval },
        title: if title.is_empty() {
            "Code session".into()
        } else {
            title.into()
        },
        context: if failed {
            "My sessions · stopped with an error".into()
        } else {
            "My sessions · waiting for you".into()
        },
        at: String::new(),
        at_ms: Some(updated_at_ms),
        target: Some(Target::Code(id.into())),
        action: "Open session",
        needs_you: !failed,
        read: false,
        count: 1,
        remote: false,
    }
}

/// A completion's identity includes its host and timestamp, so later turns create new notices.
pub fn completion_entry(id: &str, title: &str, machine: Option<&str>, context: String, at: i64) -> Entry {
    let mut entry = code_entry(id, title, false, at);
    entry.id = format!("code:{}:{id}:finished:{at}", machine.unwrap_or("local"));
    entry.moment = Moment::Finished;
    entry.title = format!(
        "{} is ready to review",
        if title.is_empty() { "Your session" } else { title }
    );
    entry.context = context;
    entry.action = "Review session";
    entry.needs_you = false;
    entry
}

/// The rows to show: one per id, filtered, what needs the person first, then newest first (`inboxEntries`).
pub fn entries(items: impl IntoIterator<Item = Entry>, filter: Filter) -> Vec<Entry> {
    let mut unique: Vec<Entry> = Vec::new();
    for item in items {
        match unique.iter_mut().find(|held| held.id == item.id) {
            Some(held) => {
                if sort_key(&item) >= sort_key(held) {
                    *held = item;
                }
            }
            None => unique.push(item),
        }
    }
    unique.retain(|item| match filter {
        Filter::All => true,
        Filter::Needs => item.needs_you,
        Filter::Unread => !item.read,
    });
    unique.sort_by(|a, b| {
        b.needs_you
            .cmp(&a.needs_you)
            .then_with(|| sort_key(b).cmp(&sort_key(a)))
            .then_with(|| a.id.cmp(&b.id))
    });
    unique
}

/// A presentation-only group. Every original event stays available with its own destination and read/Done id.
#[derive(Debug, Clone, PartialEq)]
pub struct PresentationGroup {
    pub key: String,
    pub title: String,
    pub items: Vec<Entry>,
}

/// Routine account changes on one UTC day, even with different titles or intervening activity (`presentationGroups`).
/// The caller supplies the already ordered and filtered entries. Other moments keep native consecutive folding,
/// evaluated on the original order before any intervening security events move into an earlier daily group.
pub fn presentation_groups(entries: Vec<Entry>) -> Vec<PresentationGroup> {
    let mut groups: Vec<PresentationGroup> = Vec::new();
    let mut security_days = std::collections::HashMap::<String, usize>::new();
    let mut entries = entries.into_iter().peekable();
    while let Some(entry) = entries.next() {
        let day = (entry.moment == Moment::Security && !entry.needs_you)
            .then(|| entry.at.chars().take(10).collect::<String>())
            .filter(|day| !day.is_empty());
        if let Some(index) = day.as_ref().and_then(|day| security_days.get(day)).copied() {
            let group = &mut groups[index];
            group.items.push(entry);
            let count: u64 = group.items.iter().map(|entry| entry.count).sum();
            group.title = format!("{count} account changes");
        } else {
            let key = day
                .as_ref()
                .map_or_else(|| entry.id.clone(), |day| format!("account:{day}"));
            if let Some(day) = day {
                security_days.insert(day, groups.len());
            }
            let title = entry.title.clone();
            let mut items = vec![entry];
            if items[0].moment != Moment::Security {
                while let Some(next) = entries.peek() {
                    let head = &items[0];
                    if next.needs_you == head.needs_you
                        && next.remote == head.remote
                        && next.title == head.title
                        && next.context == head.context
                        && next.moment == head.moment
                    {
                        items.push(entries.next().expect("peeked entry"));
                    } else {
                        break;
                    }
                }
            }
            groups.push(PresentationGroup { key, title, items });
        }
    }
    groups
}

/// When an entry happened, in seconds, for ordering a notification next to a local session.
fn sort_key(entry: &Entry) -> i64 {
    entry
        .at_ms
        .map(|ms| ms / 1000)
        .or_else(|| crate::library::chrono_lite::parse(&entry.at))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(kind: &str, group: &str, source: Value) -> Item {
        Item::from_json(&json!({
            "id": "ntf_1", "kind": kind, "group": group, "subjectText": "Something",
            "subject": {"type": "organization", "id": "org_1"}, "sources": [source], "count": 1,
            "createdAt": "2026-11-05T10:00:00Z", "updatedAt": "2026-11-05T10:00:00Z"
        }))
        .expect("an item")
    }

    #[test]
    fn hover_actions_follow_read_state_and_whether_the_row_can_be_done() {
        let mut row = completion_entry("t", "Fix sign-in", None, "Project · This computer".into(), 2000);
        row.read = false;
        assert_eq!(row.hover_actions(), (true, true));
        row.read = true;
        assert_eq!(row.hover_actions(), (false, true));
        row.needs_you = true;
        row.moment = Moment::Approval;
        assert_eq!(
            row.hover_actions(),
            (false, false),
            "nothing covers the time, so it stays"
        );
        row.remote = true;
        assert_eq!(row.hover_actions(), (false, true));
    }

    #[test]
    fn completion_notices_keep_hosts_and_turns_separate() {
        let local = completion_entry("t", "Fix sign-in", None, "Project · This computer".into(), 2000);
        let remote = completion_entry("t", "Fix sign-in", Some("host"), "Project · Server".into(), 2000);
        let later = completion_entry("t", "Fix sign-in", None, "Project · This computer".into(), 3000);
        assert_ne!(local.id, remote.id);
        assert_ne!(local.id, later.id);
        assert_eq!(local.moment, Moment::Finished);
        assert!(!local.needs_you);
        assert_eq!(local.target, Some(Target::Code("t".into())));
        let rows = entries([local, remote, later.clone()], Filter::All);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, later.id);
        assert!(entries([later], Filter::Needs).is_empty());
    }

    #[test]
    fn an_approval_waiting_opens_its_page_and_needs_the_person() {
        let waiting = item(
            "requests.approval_waiting",
            "requests_and_approvals",
            json!({"type": "approval", "approvalId": "apr_1"}),
        );
        assert_eq!(target(&waiting), Some(Target::Approval("apr_1".into())));
        assert!(needs_attention(&waiting));
        let no_id = item(
            "requests.approval_waiting",
            "requests_and_approvals",
            json!({"type": "approval"}),
        );
        assert_eq!(target(&no_id), None);
    }

    #[test]
    fn items_lead_where_nexus_ui_sends_them() {
        let access = item(
            "requests.artifact_access_requested",
            "requests_and_approvals",
            json!({"type": "artifact_access", "artifactId": "art_1", "requestId": "r"}),
        );
        assert_eq!(target(&access), Some(Target::Artifact("art_1".into())));
        assert!(needs_attention(&access));
        let denied = item(
            "requests.artifact_access_answered",
            "requests_and_approvals",
            json!({"type": "artifact_access_answer", "artifactId": "art_2", "requestId": "r", "outcome": "denied"}),
        );
        assert_eq!(target(&denied), None);
        let key = item(
            "account.security_changed",
            "account_and_security",
            json!({"type": "security_change", "action": "key.created"}),
        );
        assert_eq!(target(&key), Some(Target::Org("/keys".into())));
        assert_eq!(entry(&key).title, "An API key was created");
        assert_eq!(entry(&key).moment, Moment::Security);
        assert_eq!(entry(&access).action, "Open request");
        let joined = item(
            "requests.member_joined",
            "requests_and_approvals",
            json!({"type": "member", "userId": "usr_2"}),
        );
        assert_eq!(target(&joined), Some(Target::Org("/members".into())));
        assert_eq!(entry(&joined).moment, Moment::Activity);
        assert_eq!(entry(&joined).action, "View members");
    }

    #[test]
    fn what_needs_you_comes_first_then_the_newest() {
        let mut old = entry(&item(
            "usage.hard_stop_reached",
            "usage_and_billing",
            json!({"type": "spend"}),
        ));
        old.id = "a".into();
        old.at = "2026-11-01T00:00:00Z".into();
        let mut new = entry(&item("x", "my_sessions", json!({"type": "member"})));
        new.id = "b".into();
        new.at = "2026-11-05T00:00:00Z".into();
        let ids: Vec<_> = entries([new.clone(), old.clone()], Filter::All)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(entries([new, old], Filter::Needs).len(), 1);
    }

    #[test]
    fn web_targets_have_a_nexus_ui_address_and_in_app_targets_none() {
        let site = "https://nexus.example/";
        assert_eq!(
            Target::Viewer("art_9".into())
                .web_address(site, Some("acme"))
                .as_deref(),
            Some("https://nexus.example/a/art_9")
        );
        assert_eq!(
            Target::Approval("apr_1".into())
                .web_address(site, Some("acme"))
                .as_deref(),
            Some("https://nexus.example/app/approvals/apr_1")
        );
        assert_eq!(
            Target::Org("/keys".into()).web_address(site, Some("acme")).as_deref(),
            Some("https://nexus.example/app/acme/keys")
        );
        assert_eq!(
            Target::Org("/keys".into()).web_address(site, None).as_deref(),
            Some("https://nexus.example/app")
        );
        assert_eq!(Target::Artifact("art_1".into()).web_address(site, Some("acme")), None);
        assert_eq!(Target::Code("t".into()).web_address(site, Some("acme")), None);
    }

    #[test]
    fn a_page_keeps_its_cursor() {
        let page = Page::from_json(&json!({"items": [{"id": "n", "readAt": null}], "nextCursor": "c"}));
        assert_eq!(page.next_cursor.as_deref(), Some("c"));
    }
}

#[cfg(test)]
#[path = "inbox_group_tests.rs"]
mod group_tests;
