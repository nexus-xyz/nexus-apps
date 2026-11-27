//! The Library's rows, as the Nexus app's (`ui/src/components/nexus-ui/library/rows.ts`): Artifacts and Pages in one
//! list shape. A row is a shared chat, a page shared with you, or one of your pages. Plain data, no UI, so the tests
//! below check it directly.

use serde_json::Value;

use crate::model::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    All,
    With,
    By,
    Pages,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::All, Tab::Pages, Tab::With, Tab::By];

    pub fn label(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::With => "Shared with me",
            Tab::By => "Shared by me",
            Tab::Pages => "My drafts",
        }
    }
}

/// The Library's kind filter, shared by rows and tab counts (`LIBRARY_KINDS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    All,
    Site,
    Doc,
    Sheet,
    Board,
    Chat,
}

impl Kind {
    pub const ALL: [Kind; 6] = [Self::All, Self::Site, Self::Doc, Self::Sheet, Self::Board, Self::Chat];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Site => "Sites",
            Self::Doc => "Docs",
            Self::Sheet => "Sheets",
            Self::Board => "Designs",
            Self::Chat => "Chats",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Site => "Site",
            Self::Doc => "Doc",
            Self::Sheet => "Sheet",
            Self::Board => "Design",
            Self::Chat => "Chat",
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            Self::All => "",
            Self::Site => "Ask for a site or a dashboard, or publish one from the CLI.",
            Self::Doc => "Ask for a doc, a plan or a write-up.",
            Self::Sheet => "Ask for a table, or import a CSV.",
            Self::Board => "Ask for a design or a whiteboard.",
            Self::Chat => "A chat shows here when someone shares one with you, or you share one.",
        }
    }

    pub fn of_format(format: Option<&str>) -> Self {
        match format {
            Some("md") => Self::Doc,
            Some("sheet") => Self::Sheet,
            Some("board") => Self::Board,
            _ => Self::Site,
        }
    }
}

/// Where a row goes when opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Open {
    /// A shared page: its frame (`GET /v1/artifacts/{id}/frame`).
    SharedPage(String),
    /// One of your pages: its latest version's frame (`GET /v1/pages/{id}/versions/latest/frame`).
    Page(String),
    /// A shared chat: its Markdown (`GET /v1/artifacts/{id}.md`).
    Chat(String),
    /// Another organization's share: its viewer, in the browser.
    Browser(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Unique in a tab: the artifact's id, or `page:<id>` for one of your pages.
    pub key: String,
    pub page: bool,
    /// Server-provided share ownership, never inferred from the display name.
    pub mine: bool,
    /// A source page confirmed in the caller's own page list.
    pub owned_page: Option<String>,
    pub source_page: Option<String>,
    /// Gateway's reader URL. Public shares keep their token here.
    pub link: Option<String>,
    pub title: String,
    /// "v3 · claude-sonnet-5.5", "Doc", "Site · React", "Sheet" or "Design".
    pub sub: String,
    pub shared_by: String,
    pub author_email: String,
    /// Every share of this page, newest first, when several rows form one item.
    pub shares: Vec<Row>,
    pub audience: String,
    /// "Chat", "Page": the Shared by me tab's Type column.
    pub kind: &'static str,
    /// "v3"; a page shared at its latest version says "latest".
    pub version: String,
    /// A page's own format from the list; none for a chat. The kind comes from this, never the label.
    pub format: Option<String>,
    pub updated_at: String,
    pub outside: bool,
    pub revoked: bool,
    /// Nexus UI's path for it, relative to the organization (`/artifacts/<id>`), for Open in browser.
    pub path: String,
    pub open: Open,
}

/// Actions are shared by the overflow button and the context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Browser,
    /// Reference it in a session: the agent reads it (`artifact_refs.rs`).
    Reference,
    Copy,
    Share,
    Versions,
    Export,
    Access,
    Revoke,
    Delete,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Browser => "Open in browser",
            Self::Reference => "Add to a session…",
            Self::Copy => "Copy link",
            Self::Share => "Share…",
            Self::Versions => "Version history…",
            Self::Export => "Download Markdown…",
            Self::Access => "Manage access in browser…",
            Self::Revoke => "Stop sharing…",
            Self::Delete => "Delete Artifact…",
        }
    }
}

impl Row {
    pub fn artifact_kind(&self) -> Kind {
        if self.page {
            Kind::of_format(self.format.as_deref())
        } else {
            Kind::Chat
        }
    }

    pub fn artifact_id(&self) -> Option<&str> {
        (!matches!(self.open, Open::Page(_))).then_some(self.key.as_str())
    }

    /// Whether someone else can open it: it, or one of its shares, is a share that is not revoked. A page of yours with
    /// no share is private; its share rows come from the artifact lists (`group_by_page`).
    pub fn shared(&self) -> bool {
        std::iter::once(self)
            .chain(&self.shares)
            .any(|row| !row.revoked && row.artifact_id().is_some())
    }

    pub fn actions(&self, admin: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        if !self.revoked {
            actions.extend([Action::Open, Action::Browser]);
            // Another organization's share opens only in the browser: no agent here can read it.
            if !matches!(self.open, Open::Browser(_)) {
                actions.push(Action::Reference);
            }
            if self.link.is_some() {
                actions.push(Action::Copy);
            }
        }
        if self.owned_page.is_some() {
            actions.extend([Action::Share, Action::Versions]);
        }
        if !self.revoked && !self.page {
            actions.push(Action::Export);
        }
        // Outside recipients do not gain authority from their role in a different organization.
        let manages = self.mine || (admin && !self.outside);
        if manages && !self.revoked && self.artifact_id().is_some() {
            if self.outside {
                actions.push(Action::Access);
            }
            actions.push(Action::Revoke);
        }
        if matches!(self.open, Open::Page(_)) {
            actions.push(Action::Delete);
        }
        actions
    }
}

/// Resolve only reader URLs against the configured site, never the desktop app's local origin.
pub fn reader_url(site: &str, value: &str) -> Option<String> {
    let base = url::Url::parse(site).ok()?;
    let resolved = base.join(value).ok()?;
    if !matches!(resolved.scheme(), "http" | "https")
        || !resolved.username().is_empty()
        || resolved.password().is_some()
    {
        return None;
    }
    Some(resolved.into())
}

/// What the Library read: the three lists, each with whether it has more to read.
#[derive(Debug, Clone, Default)]
pub struct Lists {
    /// Everything on the other tabs once: shared with you, by you and your pages, newest first.
    pub all: Vec<Row>,
    pub with: Vec<Row>,
    pub by: Vec<Row>,
    pub pages: Vec<Row>,
    pub more: [bool; 3],
}

impl Lists {
    pub fn rows(&self, tab: Tab) -> &[Row] {
        match tab {
            Tab::All => &self.all,
            Tab::With => &self.with,
            Tab::By => &self.by,
            Tab::Pages => &self.pages,
        }
    }

    /// The visible rows: ownership and page grouping first, then kind and search together.
    pub fn visible(&self, tab: Tab, kind: Kind, query: &str) -> Vec<&Row> {
        filter(self.rows(tab), query)
            .into_iter()
            .filter(|row| kind == Kind::All || row.artifact_kind() == kind)
            .collect()
    }

    /// A tab's count: "14", or "50+" when the list has more to read.
    pub fn count(&self, tab: Tab, kind: Kind) -> String {
        let more = match tab {
            Tab::All => self.more.iter().any(|more| *more),
            Tab::With => self.more[0],
            Tab::By => self.more[1],
            Tab::Pages => self.more[2],
        };
        format!(
            "{}{}",
            filter_kind(self.rows(tab), kind).len(),
            if more { "+" } else { "" }
        )
    }

    /// Find a current item across ownership tabs. A share hidden from All still has actions in Shared by me.
    pub fn find(&self, key: &str) -> Option<&Row> {
        self.all
            .iter()
            .chain(&self.with)
            .chain(&self.by)
            .chain(&self.pages)
            .flat_map(|row| std::iter::once(row).chain(row.shares.iter()))
            .find(|row| row.key == key)
    }

    /// The item an open page belongs to: the Library's row for it, with its other shares. `key` is the row the
    /// person opened, when they opened it from the Library; otherwise the page's id finds it.
    pub fn item_for(&self, key: Option<&str>, page: Option<&str>) -> Option<&Row> {
        let items = || self.all.iter().chain(&self.pages).chain(&self.with).chain(&self.by);
        key.and_then(|key| items().find(|row| row.key == key))
            .or_else(|| page.and_then(|page| items().find(|row| row.source_page.as_deref() == Some(page))))
    }

    /// Whether someone else can open `row`'s page or chat. All drops your own share of a page it lists (`from_responses`),
    /// so a page's shares are looked for on every list.
    pub fn shared(&self, row: &Row) -> bool {
        row.shared()
            || row.source_page.as_deref().is_some_and(|page| {
                self.all
                    .iter()
                    .chain(&self.with)
                    .chain(&self.by)
                    .flat_map(|row| std::iter::once(row).chain(&row.shares))
                    .any(|share| share.source_page.as_deref() == Some(page) && share.shared())
            })
    }

    /// Gateway's artifact scopes and owned pages. My drafts contains only owned pages; every tab groups pages once.
    pub fn from_responses(with: &Value, by: &Value, pages: Option<&Value>, org: Option<&str>) -> Lists {
        let artifacts = |list: &Value| -> Vec<Value> {
            list.get("artifacts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let with_items = artifacts(with);
        let mut page_rows: Vec<Row> = pages
            .and_then(|pages| pages.get("pages").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .filter_map(page_row)
            .collect();
        let owned: Vec<String> = page_rows.iter().filter_map(|row| row.owned_page.clone()).collect();
        page_rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let next = |list: Option<&Value>| list.and_then(|list| text(list, "nextCursor")).is_some();
        let mut with_rows: Vec<Row> = with_items
            .iter()
            .filter_map(|artifact| artifact_row(artifact, org))
            .collect();
        let mut by_rows: Vec<Row> = artifacts(by)
            .iter()
            .filter_map(|artifact| artifact_row(artifact, org))
            .collect();
        for row in page_rows.iter_mut().chain(&mut with_rows).chain(&mut by_rows) {
            if let Some(id) = &row.source_page
                && owned.contains(id)
            {
                row.owned_page = Some(id.clone());
            }
        }
        for page in &mut page_rows {
            page.audience = shares_audience(page.source_page.as_deref(), with_rows.iter().chain(&by_rows));
        }
        // One row per artifact: something you shared to yourself is on both lists.
        let mut all: Vec<Row> = Vec::new();
        for row in page_rows.iter().chain(&with_rows).chain(&by_rows) {
            // The owned page list may be unavailable or paginated. Keep its share until the page is loaded.
            if row.page && row.mine && row.owned_page.is_some() {
                continue;
            }
            if !all.iter().any(|seen| seen.key == row.key) {
                all.push(row.clone());
            }
        }
        all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Lists {
            all: group_by_page(all),
            with: group_by_page(with_rows),
            by: group_by_page(by_rows),
            pages: group_by_page(page_rows),
            more: [next(Some(with)), next(Some(by)), next(pages)],
        }
    }
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A page's format as the list says it.
pub fn format_label(format: Option<&str>) -> &'static str {
    match format {
        Some("md") => "Markdown",
        Some("jsx") => "React",
        Some("sheet") => "Sheet",
        Some("board") => "Design",
        _ => "HTML",
    }
}

/// A page's second line, as `pageSub`: the kind, and the format only for Sites.
fn page_sub(format: Option<&str>) -> String {
    let kind = Kind::of_format(format);
    if kind == Kind::Site {
        format!("Site · {}", format_label(format))
    } else {
        kind.name().into()
    }
}

/// One item per source page, in its first position. Prefer the owned page, then the newest active share.
fn group_by_page(rows: Vec<Row>) -> Vec<Row> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in &rows {
        let Some(id) = &row.source_page else {
            result.push(row.clone());
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        let mut shares: Vec<Row> = rows
            .iter()
            .filter(|one| one.source_page.as_ref() == Some(id))
            .cloned()
            .collect();
        if shares.len() == 1 {
            result.push(row.clone());
            continue;
        }
        shares.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let mut selected = shares
            .iter()
            .find(|one| matches!(one.open, Open::Page(_)))
            .or_else(|| shares.iter().find(|one| !one.revoked))
            .unwrap_or(&shares[0])
            .clone();
        selected.shares = shares;
        result.push(selected);
    }
    result
}

/// The chosen kind applies before search, and to each tab's count.
pub fn filter_kind(rows: &[Row], kind: Kind) -> Vec<&Row> {
    rows.iter()
        .filter(|row| kind == Kind::All || row.artifact_kind() == kind)
        .collect()
}

/// Who can read it: "Anyone in Acme", "web, platform" (teams), "People outside" (`libraryAudience`).
fn audience(audience: Option<&Value>, org: Option<&str>) -> String {
    let Some(audience) = audience else {
        return "Only you".into();
    };
    let list = |key: &str| -> Vec<String> {
        audience
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    match text(audience, "kind").as_deref() {
        Some("org") => org.map_or("Everyone in the organization".into(), |org| format!("Anyone in {org}")),
        Some("external") => "People outside".into(),
        Some("public") => "Anyone with the link".into(),
        Some("teams") => {
            let teams = list("teams");
            if teams.is_empty() {
                "No team".into()
            } else {
                teams.join(", ")
            }
        }
        _ => {
            let people = list("people");
            match people.len() {
                0 => "Nobody else".into(),
                1 | 2 => people.join(", "),
                n => format!("{n} people"),
            }
        }
    }
}

/// Who can read a page through its active shares, newest first, each once. "–" when no share is active.
fn shares_audience<'a>(page: Option<&str>, shares: impl Iterator<Item = &'a Row>) -> String {
    let mut active: Vec<&Row> = shares
        .filter(|share| page.is_some() && share.source_page.as_deref() == page && !share.revoked)
        .collect();
    active.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let mut audiences: Vec<&str> = Vec::new();
    for share in active {
        if !audiences.contains(&share.audience.as_str()) {
            audiences.push(&share.audience);
        }
    }
    if audiences.is_empty() {
        "–".into()
    } else {
        audiences.join(", ")
    }
}

/// An artifact as a row (`artifactRow`).
fn artifact_row(artifact: &Value, org: Option<&str>) -> Option<Row> {
    let id = text(artifact, "id")?;
    let page = text(artifact, "kind").as_deref() == Some("page");
    let version = artifact.get("version").and_then(Value::as_u64).unwrap_or(1);
    let model = artifact
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.first())
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| text(artifact, "model").filter(|model| !model.is_empty()));
    let sub = if page {
        page_sub(text(artifact, "pageFormat").as_deref())
    } else {
        let short = model.map(|model| model.rsplit('/').next().unwrap_or(&model).to_string());
        std::iter::once(format!("v{version}"))
            .chain(short)
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let mine = flag(artifact, "mine");
    let external = flag(artifact, "external");
    let foreign = external && !mine;
    let author = artifact.get("author");
    let name = author
        .and_then(|author| text(author, "name").or_else(|| text(author, "email")))
        .unwrap_or_else(|| "Someone".into());
    let shared_by = if mine {
        "You".into()
    } else {
        match text(artifact, "organization").filter(|_| foreign) {
            Some(org) => format!("{name} · {org}"),
            None => name,
        }
    };
    let page_version = artifact.get("pageVersion").and_then(Value::as_u64);
    let open = if foreign {
        Open::Browser(text(artifact, "viewerUrl").unwrap_or_else(|| format!("/a/{id}")))
    } else if page {
        Open::SharedPage(id.clone())
    } else {
        Open::Chat(id.clone())
    };
    Some(Row {
        key: id.clone(),
        page,
        mine,
        owned_page: None,
        source_page: page.then(|| text(artifact, "pageId")).flatten(),
        link: text(artifact, "publicUrl")
            .or_else(|| text(artifact, "viewerUrl"))
            .or_else(|| text(artifact, "url")),
        title: text(artifact, "title")
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "Untitled".into()),
        sub,
        shared_by,
        author_email: author.and_then(|author| text(author, "email")).unwrap_or_default(),
        shares: Vec::new(),
        audience: audience(artifact.get("audience"), org),
        kind: if page { "Page" } else { "Chat" },
        format: page.then(|| text(artifact, "pageFormat").unwrap_or_else(|| "html".into())),
        version: if page {
            page_version.map_or("latest".into(), |v| format!("v{v}"))
        } else {
            format!("v{version}")
        },
        updated_at: text(artifact, "revokedAt")
            .or_else(|| text(artifact, "createdAt"))
            .unwrap_or_default(),
        outside: external,
        revoked: text(artifact, "status").as_deref() == Some("revoked"),
        path: format!("/artifacts/{id}"),
        open,
    })
}

/// One of your pages as a row (`pageRow`).
fn page_row(page: &Value) -> Option<Row> {
    let id = text(page, "id")?;
    Some(Row {
        key: format!("page:{id}"),
        page: true,
        mine: false,
        owned_page: Some(id.clone()),
        source_page: Some(id.clone()),
        link: None,
        title: text(page, "title")
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "Untitled".into()),
        sub: page_sub(text(page, "format").as_deref()),
        shared_by: "You".into(),
        author_email: String::new(),
        shares: Vec::new(),
        audience: "–".into(),
        kind: "Page",
        format: Some(text(page, "format").unwrap_or_else(|| "html".into())),
        version: format!("v{}", page.get("latestVersion").and_then(Value::as_u64).unwrap_or(1)),
        updated_at: text(page, "updatedAt").unwrap_or_default(),
        outside: false,
        revoked: false,
        path: format!("/pages/{id}"),
        open: Open::Page(id),
    })
}

/// The rows whose title, second line, sharer or audience has every word of `query`, any case (`filterRows`).
pub fn filter<'a>(rows: &'a [Row], query: &str) -> Vec<&'a Row> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    rows.iter()
        .filter(|row| {
            let audiences = if row.shares.is_empty() {
                row.audience.clone()
            } else {
                row.shares
                    .iter()
                    .map(|share| share.audience.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let haystack = format!(
                "{} {} {} {} {}",
                row.title, row.sub, row.shared_by, audiences, row.author_email
            )
            .to_lowercase();
            words.iter().all(|word| haystack.contains(word))
        })
        .collect()
}

/// A short age for the Updated column: "now", "5m", "2h", "3d", then the day ("Oct 18") (`shortAgo`).
pub fn short_ago(iso: &str, now: chrono_lite::Seconds) -> String {
    let Some(then) = chrono_lite::parse(iso) else {
        return String::new();
    };
    let seconds = (now - then).max(0);
    match seconds {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s if s < 30 * 86_400 => format!("{}d", s / 86_400),
        _ => chrono_lite::month_day(iso),
    }
}

/// The few date sums the list needs, on RFC 3339 times, without a date crate.
pub mod chrono_lite {
    pub type Seconds = i64;

    pub fn now() -> Seconds {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Seconds since 1970 of `2026-10-18T09:30:00Z` (or with `.123` or an offset); none when it is not one.
    pub fn parse(iso: &str) -> Option<Seconds> {
        let b = iso.as_bytes();
        if b.len() < 19 {
            return None;
        }
        let n = |from: usize, len: usize| iso.get(from..from + len)?.parse::<i64>().ok();
        let (y, mo, d, h, mi, s) = (n(0, 4)?, n(5, 2)?, n(8, 2)?, n(11, 2)?, n(14, 2)?, n(17, 2)?);
        // Days from the civil date (Howard Hinnant's algorithm).
        let y = if mo <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        let mut offset = 0;
        let rest = &iso[19..];
        let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
        if let Some(sign @ ('+' | '-')) = rest.chars().next() {
            let parts: Vec<i64> = rest[1..].split(':').filter_map(|p| p.parse().ok()).collect();
            let minutes = parts.first().copied().unwrap_or(0) * 60 + parts.get(1).copied().unwrap_or(0);
            offset = if sign == '+' { minutes * 60 } else { -minutes * 60 };
        }
        Some(days * 86_400 + h * 3600 + mi * 60 + s - offset)
    }

    /// "Oct 18".
    pub fn month_day(iso: &str) -> String {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let month = iso
            .get(5..7)
            .and_then(|m| m.parse::<usize>().ok())
            .filter(|m| (1..=12).contains(m));
        let day = iso.get(8..10).and_then(|d| d.parse::<u32>().ok());
        match (month, day) {
            (Some(m), Some(d)) => format!("{} {d}", MONTHS[m - 1]),
            _ => String::new(),
        }
    }
}

/// A chat's Markdown without the metadata block (`---` … `---`) the Gateway puts before it.
pub fn without_front_matter(markdown: &str) -> &str {
    markdown
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(markdown, |(_, body)| body.trim_start())
}

/// The page a link opens in Nexus UI's page viewer (`<ui>/app/<org>/pages/<id>`, or `/app?next=/pages/<id>`), as the
/// Nexus app's `pageLink` (`nexus_desktop_core::page_links`); none for any other link.
pub fn page_link(href: &str) -> Option<String> {
    nexus_desktop_core::page_links::page_link(href)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat() -> Value {
        json!({ "id": "art_1", "version": 3, "title": "Q3 plan", "mode": "chat", "model": "anthropic/claude-sonnet-5.5",
            "models": [], "author": { "name": "Ada" }, "audience": { "kind": "org" }, "status": "active", "mine": false,
            "createdAt": "2026-11-01T10:00:00Z" })
    }

    #[test]
    fn actions_preserve_ownership_and_revoke_only_shared_links() {
        let mut share = chat();
        share["url"] = json!("/app/acme/artifacts/art_1");
        let recipient = artifact_row(&share, None).unwrap();
        assert_eq!(
            recipient.actions(false),
            [
                Action::Open,
                Action::Browser,
                Action::Reference,
                Action::Copy,
                Action::Export
            ]
        );
        assert!(recipient.actions(true).contains(&Action::Revoke));
        share["external"] = json!(true);
        let outside = artifact_row(&share, None).unwrap();
        assert!(!outside.actions(true).contains(&Action::Revoke));
        share["mine"] = json!(true);
        let author = artifact_row(&share, None).unwrap();
        assert!(author.actions(false).contains(&Action::Access));
        assert!(author.actions(false).contains(&Action::Revoke));
        assert!(!author.actions(false).contains(&Action::Delete));
        share["status"] = json!("revoked");
        assert!(artifact_row(&share, None).unwrap().actions(true).is_empty());
        let page = page_row(&json!({"id":"pag_1"})).unwrap();
        assert_eq!(
            page.actions(false),
            [
                Action::Open,
                Action::Browser,
                Action::Reference,
                Action::Share,
                Action::Versions,
                Action::Delete
            ]
        );
        // Another organization's share opens only in the browser, so no session here can reference it.
        let mut elsewhere = chat();
        elsewhere["external"] = json!(true);
        elsewhere["url"] = json!("https://other.example/artifacts/art_1");
        let elsewhere = artifact_row(&elsewhere, None).unwrap();
        if matches!(elsewhere.open, Open::Browser(_)) {
            assert!(!elsewhere.actions(false).contains(&Action::Reference));
        }
    }

    #[test]
    fn share_author_is_not_used_as_page_ownership() {
        let share = json!({"id":"art_1", "kind":"page", "pageId":"pag_1", "mine":true});
        let empty = json!({});
        let by = json!({"artifacts":[share]});
        let without_page = Lists::from_responses(&empty, &by, None, None);
        assert!(!without_page.by[0].actions(false).contains(&Action::Share));
        let own_pages = json!({"pages":[{"id":"pag_1"}]});
        let with_page = Lists::from_responses(&empty, &by, Some(&own_pages), None);
        assert!(with_page.by[0].actions(false).contains(&Action::Share));
        assert!(with_page.by[0].actions(false).contains(&Action::Revoke));
        assert!(!with_page.by[0].actions(false).contains(&Action::Delete));
    }

    #[test]
    fn an_open_page_finds_its_item_by_the_opened_row_or_its_page() {
        let shared = json!({"artifacts":[chat()]});
        let by = json!({"artifacts":[{"id":"art_2", "kind":"page", "pageId":"pag_1", "mine":true}]});
        let pages = json!({"pages":[{"id":"pag_1"}]});
        let lists = Lists::from_responses(&shared, &by, Some(&pages), None);
        // A page opened from a chat: its own page, which can be shared and deleted, not only its share.
        let page = lists.item_for(None, Some("pag_1")).unwrap();
        assert_eq!(page.key, "page:pag_1");
        assert!(page.actions(false).contains(&Action::Delete));
        // The row the person opened wins over the page's own.
        assert_eq!(lists.item_for(Some("art_2"), Some("pag_1")).unwrap().key, "art_2");
        // A shared chat has no page: only the opened row names it.
        assert_eq!(lists.item_for(Some("art_1"), None).unwrap().key, "art_1");
        assert!(lists.item_for(None, Some("pag_unknown")).is_none());
        assert!(lists.item_for(None, None).is_none());
    }

    #[test]
    fn links_keep_public_tokens_and_use_the_configured_site() {
        let mut share = chat();
        share["publicUrl"] = json!("/a/secret-token");
        share["url"] = json!("/a/art_1");
        assert_eq!(
            artifact_row(&share, None).unwrap().link.as_deref(),
            Some("/a/secret-token")
        );
        assert_eq!(
            reader_url("https://nexus.inc", "/a/secret-token").as_deref(),
            Some("https://nexus.inc/a/secret-token")
        );
        assert_eq!(
            reader_url("http://localhost:3000", "/a/art_1").as_deref(),
            Some("http://localhost:3000/a/art_1")
        );
        assert!(reader_url("https://nexus.inc", "javascript:alert(1)").is_none());
        assert!(reader_url("https://nexus.inc", "https://u:p@example.com/a/art_1").is_none());
    }

    #[test]
    fn a_chat_reads_as_the_nexus_app_lists_it() {
        let lists = Lists::from_responses(
            &json!({ "artifacts": [chat()], "nextCursor": "c" }),
            &json!({}),
            None,
            Some("Acme"),
        );
        let row = &lists.with[0];
        assert_eq!(row.sub, "v3 · claude-sonnet-5.5");
        assert_eq!(row.shared_by, "Ada");
        assert_eq!(row.audience, "Anyone in Acme");
        assert_eq!(row.open, Open::Chat("art_1".into()));
        assert_eq!(lists.count(Tab::With, Kind::All), "1+");
        assert_eq!(lists.count(Tab::By, Kind::All), "0");
    }

    #[test]
    fn my_drafts_excludes_pages_shared_with_me() {
        let shared = json!({ "id": "art_2", "kind": "page", "pageFormat": "md", "title": "Spec", "author": { "email": "b@x.y" },
            "audience": { "kind": "teams", "teams": ["web"] }, "mine": false, "createdAt": "2026-11-03T00:00:00Z" });
        let mine = json!({ "id": "art_3", "kind": "page", "pageId": "pag_a", "title": "Mine", "mine": true, "createdAt": "2026-11-04T00:00:00Z" });
        let pages = json!({ "pages": [{ "id": "pag_a", "title": "Deck", "format": "html", "latestVersion": 2, "updatedAt": "2026-11-02T00:00:00Z" }] });
        let lists = Lists::from_responses(&json!({ "artifacts": [shared, mine] }), &json!({}), Some(&pages), None);
        let keys: Vec<&str> = lists.pages.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(keys, ["page:pag_a"]);
        assert_eq!(lists.pages[0].open, Open::Page("pag_a".into()));
        assert_eq!(lists.with[0].key, "art_2");
        assert_eq!(lists.with[0].version, "latest");
        assert_eq!(lists.with[0].audience, "web");
        assert_eq!(
            lists.all.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
            ["art_2", "page:pag_a"]
        );
    }

    #[test]
    fn an_owned_page_shows_the_audience_of_its_active_shares() {
        let share = |id: &str, page: &str, audience: Value, status: &str, at: &str| {
            json!({ "id": id, "kind": "page", "pageId": page, "mine": true, "audience": audience,
                "status": status, "createdAt": at })
        };
        let by = json!({ "artifacts": [
            share("art_1", "pag_a", json!({ "kind": "org" }), "active", "2026-11-05T00:00:00Z"),
            share("art_2", "pag_a", json!({ "kind": "teams", "teams": ["web"] }), "active", "2026-11-04T00:00:00Z"),
            share("art_3", "pag_a", json!({ "kind": "org" }), "active", "2026-11-03T00:00:00Z"),
            share("art_4", "pag_b", json!({ "kind": "org" }), "revoked", "2026-11-03T00:00:00Z"),
        ] });
        let pages = json!({ "pages": [
            { "id": "pag_a", "updatedAt": "2026-11-02T00:00:00Z" },
            { "id": "pag_b", "updatedAt": "2026-11-02T00:00:00Z" },
            { "id": "pag_c", "updatedAt": "2026-11-01T00:00:00Z" },
        ] });
        let lists = Lists::from_responses(&json!({}), &by, Some(&pages), Some("Acme"));
        let audience = |rows: &[Row], key: &str| rows.iter().find(|row| row.key == key).unwrap().audience.clone();
        for rows in [&lists.pages, &lists.all] {
            assert_eq!(audience(rows, "page:pag_a"), "Anyone in Acme, web");
            assert_eq!(audience(rows, "page:pag_b"), "–");
            assert_eq!(audience(rows, "page:pag_c"), "–");
        }
    }

    #[test]
    fn another_organizations_chat_opens_in_the_browser() {
        let mut foreign = chat();
        foreign["external"] = json!(true);
        foreign["organization"] = json!("Globex");
        foreign["viewerUrl"] = json!("https://nexus.inc/a/art_1");
        let row = artifact_row(&foreign, None).unwrap();
        assert_eq!(row.shared_by, "Ada · Globex");
        assert_eq!(row.open, Open::Browser("https://nexus.inc/a/art_1".into()));
    }

    #[test]
    fn search_needs_every_word() {
        let lists = Lists::from_responses(&json!({ "artifacts": [chat()] }), &json!({}), None, Some("Acme"));
        assert_eq!(filter(&lists.with, "q3 ada").len(), 1);
        assert_eq!(filter(&lists.with, "q3 bob").len(), 0);
    }

    #[test]
    fn ages_read_short() {
        let now = chrono_lite::parse("2026-11-05T12:00:00Z").unwrap();
        assert_eq!(short_ago("2026-11-05T11:59:30Z", now), "now");
        assert_eq!(short_ago("2026-11-05T11:00:00.123+00:00", now), "1h");
        assert_eq!(short_ago("2026-11-02T12:00:00Z", now), "3d");
        assert_eq!(short_ago("2026-08-18T12:00:00Z", now), "Aug 18");
        assert_eq!(chrono_lite::parse("1970-01-02T00:00:00Z"), Some(86_400));
    }

    #[test]
    fn front_matter_is_dropped() {
        assert_eq!(without_front_matter("---\ntitle: x\n---\n\n# Hi\n"), "# Hi\n");
        assert_eq!(without_front_matter("# Hi\n---\n"), "# Hi\n---\n");
    }

    #[test]
    fn page_links_are_nexus_page_viewers_only() {
        assert_eq!(
            page_link("https://nexus.inc/app/acme/pages/pag_x1-y"),
            Some("pag_x1-y".into())
        );
        assert_eq!(
            page_link("http://localhost:3000/app?next=%2Fpages%2Fpag_q"),
            Some("pag_q".into())
        );
        assert_eq!(page_link("https://nexus.inc/app/acme/pages/art_1"), None);
        assert_eq!(page_link("https://u:p@example.com/app/acme/pages/pag_x"), None);
        assert_eq!(page_link("https://example.com/docs"), None);
    }
}

#[path = "library_layout.rs"]
pub mod layout;

#[cfg(test)]
#[path = "library_parity_tests.rs"]
mod parity_tests;
