//! Settings › Audit log (as the web app's audit page): who changed what in the organization, through which
//! credential, from where, with the record before and after. Owners and admins see everything, a team lead their teams'
//! changes, everyone else their own (`GET /v1/audit`). When is a segmented control; What and Team are menus (Team
//! a searchable tree, as an organization can have a hundred teams); Who filters by email as you type. A row opens in place, with what changed, the request id and the metadata. Show older reads the next page.
//! Download CSV saves Gateway's file (`GET /v1/audit/export.csv`) for the same filters, every matching event, where the
//! person picks (`download.rs`).

use crate::workspace::MenuTrigger;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Map, Value};

use super::{Ctx, Load, Verb, fetch, palette, row, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| AuditPage::new(ctx, window, cx));
    Some(view.into())
}

/// Time ranges Gateway understands (`since=7d`); empty means every event.
const RANGES: [(&str, &str); 5] = [
    ("24h", "24h"),
    ("7d", "7d"),
    ("30d", "30d"),
    ("90d", "90d"),
    ("", "All"),
];

/// Areas of the audit log, as action prefixes (Gateway's `AUDIT_ACTIONS`).
const AREAS: [(&str, &str); 7] = [
    ("", "Every change"),
    ("team.*", "Teams"),
    ("member.*,invitation.*", "Members and invitations"),
    ("key.*,auth.*,provider_key.*,agent.*", "Keys and sign-ins"),
    ("org.*", "Organization and budget"),
    ("policy.*", "Model policy"),
    ("github.*", "Repositories"),
];

const PAGE: usize = 50;
/// Who filters once typing pauses, so each key is not a request.
const WHO_PAUSE: Duration = Duration::from_millis(300);

// ------------------------------------------------------------------ What Gateway answers

#[derive(Debug, Clone, PartialEq)]
struct Actor {
    user_id: String,
    email: String,
    key_id: String,
    kind: String,
}

#[derive(Debug, Clone, PartialEq)]
struct Event {
    id: String,
    at: String,
    actor: Actor,
    surface: String,
    action: String,
    target_type: String,
    target_id: String,
    target_slug: String,
    before: Option<Value>,
    after: Option<Value>,
    request_id: String,
    metadata: Option<Map<String, Value>>,
    /// The event as Gateway sent it, for Copy as JSON.
    raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
struct Page {
    scope: String,
    events: Vec<Event>,
    next_cursor: Option<String>,
}

fn parse_page(answer: &Value) -> Option<Page> {
    let events = answer.get("events")?.as_array()?;
    Some(Page {
        scope: ui::text(answer, "scope").to_string(),
        events: events.iter().filter_map(parse_event).collect(),
        next_cursor: answer
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(String::from),
    })
}

fn parse_event(event: &Value) -> Option<Event> {
    let actor = event.get("actor").unwrap_or(&Value::Null);
    let target = event.get("target").unwrap_or(&Value::Null);
    let present = |key: &str| event.get(key).filter(|value| !value.is_null()).cloned();
    Some(Event {
        id: event.get("id")?.as_str()?.to_string(),
        at: ui::text(event, "at").to_string(),
        actor: Actor {
            user_id: ui::text(actor, "userId").to_string(),
            email: ui::text(actor, "email").to_string(),
            key_id: ui::text(actor, "keyId").to_string(),
            kind: ui::text(actor, "kind").to_string(),
        },
        surface: ui::text(event, "surface").to_string(),
        action: ui::text(event, "action").to_string(),
        target_type: ui::text(target, "type").to_string(),
        target_id: ui::text(target, "id").to_string(),
        target_slug: ui::text(target, "slug").to_string(),
        before: present("before"),
        after: present("after"),
        request_id: ui::text(event, "requestId").to_string(),
        metadata: event
            .get("metadata")
            .and_then(Value::as_object)
            .filter(|metadata| !metadata.is_empty())
            .cloned(),
        raw: event.clone(),
    })
}

/// A team for the Team filter, from `GET /v1/teams?tree=1`, flattened in tree order.
#[derive(Debug, Clone, PartialEq)]
struct Team {
    slug: String,
    name: String,
    depth: u64,
}

fn parse_teams(answer: &Value) -> Vec<Team> {
    fn walk(nodes: &[Value], out: &mut Vec<Team>) {
        for node in nodes {
            let slug = ui::text(node, "slug");
            if !slug.is_empty() {
                out.push(Team {
                    slug: slug.to_string(),
                    name: ui::text(node, "name").to_string(),
                    depth: node.get("depth").and_then(Value::as_u64).unwrap_or(1),
                });
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                walk(children, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(tree) = answer.get("tree").and_then(Value::as_array) {
        walk(tree, &mut out);
    }
    out
}

fn parse_permissions(answer: &Value) -> Vec<String> {
    answer
        .get("permissions")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default()
}

// ------------------------------------------------------------------ How rows read

fn scope_text(scope: &str) -> &'static str {
    match scope {
        "all" => "Every change in your organization.",
        "team" => "Your changes and those to the teams you lead.",
        _ => "The changes you made.",
    }
}

fn surface_text(surface: &str) -> &str {
    match surface {
        "api" => "API",
        "cli" => "CLI",
        "mcp" => "MCP",
        "ui" => "Nexus UI",
        "chat" => "Chat",
        "webhook" => "Webhook",
        "system" => "Nexus Gateway",
        other => other,
    }
}

fn kind_text(kind: &str) -> &str {
    match kind {
        "person" => "signed in",
        "api_key" => "API key",
        "cli" => "CLI sign-in",
        "ui" => "Nexus UI session",
        "module" => "module key",
        "service" => "service account",
        "sandbox" => "sandbox session",
        "operator" => "Nexus operator",
        "agent" => "agent link",
        "platform" => "Nexus service",
        "system" => "system",
        other => other,
    }
}

fn actor_name(event: &Event) -> String {
    let actor = &event.actor;
    if !actor.email.is_empty() {
        actor.email.clone()
    } else if !actor.user_id.is_empty() {
        actor.user_id.clone()
    } else if actor.key_id == "github" {
        "GitHub".into()
    } else if actor.key_id == "clerk" {
        // The sign-in provider's webhook made the change; Gateway names its key after the provider.
        "Sign-in provider".into()
    } else {
        "Nexus Gateway".into()
    }
}

fn target_name(event: &Event) -> String {
    let field = |key: &str| {
        [&event.after, &event.before]
            .into_iter()
            .flatten()
            .find_map(|record| record.get(key).and_then(Value::as_str).map(String::from))
    };
    let slug_or_id = || {
        if event.target_slug.is_empty() {
            event.target_id.clone()
        } else {
            event.target_slug.clone()
        }
    };
    match event.target_type.as_str() {
        "member" => field("email")
            .or_else(|| {
                event
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("email"))
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .unwrap_or_else(|| event.target_id.clone()),
        "org" if event.target_slug.is_empty() => "This organization".into(),
        "team" => match field("email") {
            Some(email) => format!("{} · {email}", slug_or_id()),
            None => slug_or_id(),
        },
        _ => slug_or_id(),
    }
}

/// One changed field: its name, and its value before and after (`None`: not there).
#[derive(Debug, Clone, PartialEq)]
struct Change {
    field: String,
    before: Option<String>,
    after: Option<String>,
}

/// Field / Before / After for what changed. A record that is not an object reads as `{ value }`. With both sides
/// present, only the fields that differ.
fn changes(event: &Event) -> Vec<Change> {
    fn record(value: &Option<Value>) -> Option<Map<String, Value>> {
        match value {
            None => None,
            Some(Value::Object(map)) => Some(map.clone()),
            Some(other) => Some(Map::from_iter([("value".to_string(), other.clone())])),
        }
    }
    let before = record(&event.before);
    let after = record(&event.after);
    let mut fields: Vec<String> = Vec::new();
    for map in [&before, &after].into_iter().flatten() {
        for key in map.keys() {
            if !fields.contains(key) {
                fields.push(key.clone());
            }
        }
    }
    fields
        .into_iter()
        .filter_map(|field| {
            let from = before.as_ref().and_then(|map| map.get(&field));
            let to = after.as_ref().and_then(|map| map.get(&field));
            if before.is_some() && after.is_some() && from == to {
                return None;
            }
            Some(Change {
                field,
                before: from.map(show),
                after: to.map(show),
            })
        })
        .collect()
}

fn show(value: &Value) -> String {
    match value {
        Value::Null => "none".into(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// An ISO time as `2026-11-05 14:03`.
fn when(at: &str) -> String {
    match (at.get(..10), at.get(11..16)) {
        (Some(day), Some(time)) => format!("{day} {time}"),
        _ => at.to_string(),
    }
}

/// A query value for a URL.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'*' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Gateway's CSV of every event the filters match: no `limit`, no `cursor` (the web's `auditCsv`).
fn export_path(filters: &str) -> String {
    if filters.is_empty() {
        "/v1/audit/export.csv".into()
    } else {
        format!("/v1/audit/export.csv?{filters}")
    }
}

/// The events in Gateway's CSV: its records past the header. A quoted cell may hold a line break (`before`, `after`,
/// `metadata`), so records are counted by CRLF, which ends each one in Gateway's export, else by line.
fn csv_rows(text: &str) -> usize {
    let records = if text.contains("\r\n") {
        text.split("\r\n").filter(|record| !record.is_empty()).count()
    } else {
        text.lines().count()
    };
    records.saturating_sub(1)
}

/// "1 event", "1,204 events".
fn events_text(count: usize) -> String {
    match count {
        1 => "1 event".into(),
        count => format!("{} events", ui::count(count as u64)),
    }
}

/// The filters as a query string, without the leading `?`.
fn query(since: &str, action: &str, actor: &str, team: &str) -> String {
    [("since", since), ("action", action), ("actor", actor), ("team", team)]
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

// ------------------------------------------------------------------ The page

#[derive(Clone, Copy, PartialEq)]
enum Menu {
    What,
    Team,
}

/// The teams the Team menu lists for what was typed: the whole tree, else the names that contain it (flat).
fn matching_teams<'a>(teams: &'a [Team], search: &str) -> Vec<(&'a Team, usize)> {
    let search = search.trim().to_lowercase();
    teams
        .iter()
        .filter(|team| search.is_empty() || team.name.to_lowercase().contains(&search) || team.slug.contains(&search))
        .map(|team| {
            let depth = if search.is_empty() {
                team.depth.saturating_sub(1) as usize
            } else {
                0
            };
            (team, depth)
        })
        .collect()
}

struct AuditPage {
    ctx: Ctx,
    permissions: Vec<String>,
    since: &'static str,
    action: &'static str,
    team: String,
    /// The Who filter as applied; the field may hold newer text.
    actor: String,
    who: Entity<InputState>,
    teams: Vec<Team>,
    first: Load<Page>,
    /// The pages Show older read after the first.
    older: Vec<Event>,
    cursor: Option<String>,
    loading_more: bool,
    more_error: Option<String>,
    /// The open row's event id.
    open: Option<String>,
    metadata_open: bool,
    copied: Option<&'static str>,
    /// The open filter menu.
    menu: Option<Menu>,
    team_search: Entity<InputState>,
    /// Counts the reads, so an answer to older filters is dropped.
    seq: u64,
    /// Download CSV: while it reads, and how the last one ended.
    exporting: bool,
    exported: Option<Result<String, String>>,
    _who_pause: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl AuditPage {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> AuditPage {
        let who = cx.new(|cx| InputState::new(window, cx).placeholder("Who, by email"));
        let team_search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a team"));
        let subscriptions = vec![
            cx.subscribe(&team_search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe_in(&who, window, |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::PressEnter { .. } => this.apply_who(cx),
                InputEvent::Change => {
                    this._who_pause = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(WHO_PAUSE).await;
                        this.update(cx, |this, cx| this.apply_who(cx)).ok();
                    }));
                }
                _ => {}
            }),
        ];
        let mut page = AuditPage {
            ctx,
            permissions: Vec::new(),
            since: "7d",
            action: "",
            team: String::new(),
            actor: String::new(),
            who,
            teams: Vec::new(),
            first: Load::Loading,
            older: Vec::new(),
            cursor: None,
            loading_more: false,
            more_error: None,
            open: None,
            metadata_open: false,
            copied: None,
            menu: None,
            team_search,
            seq: 0,
            exporting: false,
            exported: None,
            _who_pause: None,
            _subscriptions: subscriptions,
        };
        let gateway = page.ctx.gateway.clone();
        super::read(cx, gateway.query("/v1/tenants/me".into()), |this, answer, _| {
            if let Ok(answer) = answer {
                this.permissions = parse_permissions(&answer);
            }
        });
        super::read(cx, gateway.query("/v1/teams?tree=1".into()), |this, answer, _| {
            // Without teams the Team filter is not shown, as on the web.
            if let Ok(answer) = answer {
                this.teams = parse_teams(&answer);
            }
        });
        page.read(cx);
        page
    }

    fn query(&self) -> String {
        query(self.since, self.action, &self.actor, &self.team)
    }

    /// Download CSV: Gateway's file for these filters with no limit (the web's `auditCsv`), saved where the person picks
    /// as `nexus-audit-<org>-<date>.csv`.
    fn download_csv(&mut self, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        self.exporting = true;
        self.exported = None;
        let request = self.ctx.gateway.text(export_path(&self.query()));
        let name = format!(
            "nexus-audit-{}-{}.csv",
            super::download::name_part(&self.ctx.org),
            super::download::today_utc()
        );
        super::download::save_text(cx, request, name, |this: &mut Self, saved, _| {
            this.exporting = false;
            this.exported = match saved {
                super::download::Saved::File { path, text } => Some(Ok(format!(
                    "Saved {}: {}.",
                    super::download::file_name(&path),
                    events_text(csv_rows(&text))
                ))),
                super::download::Saved::Cancelled => None,
                super::download::Saved::Failed(error) => Some(Err(error)),
            };
        });
        cx.notify();
    }

    /// Read the first page for the filters as they are now.
    fn read(&mut self, cx: &mut Context<Self>) {
        self.seq += 1;
        let seq = self.seq;
        self.first = Load::Loading;
        self.older.clear();
        self.cursor = None;
        self.loading_more = false;
        self.more_error = None;
        self.open = None;
        let filters = self.query();
        let path = if filters.is_empty() {
            format!("/v1/audit?limit={PAGE}")
        } else {
            format!("/v1/audit?{filters}&limit={PAGE}")
        };
        super::read(cx, self.ctx.gateway.query(path), move |this, answer, _| {
            if this.loading_more || !this.older.is_empty() {
                return;
            }
            if this.seq != seq {
                return;
            }
            this.first = match answer {
                Ok(answer) => match parse_page(&answer) {
                    Some(page) => {
                        this.cursor = page.next_cursor.clone();
                        Load::Ready(page)
                    }
                    None => Load::Failed("Gateway sent an audit log this app cannot read.".into()),
                },
                Err(error) => Load::Failed(error.into()),
            };
        });
        cx.notify();
    }

    fn show_older(&mut self, cx: &mut Context<Self>) {
        let Some(cursor) = self.cursor.clone() else {
            return;
        };
        if self.loading_more {
            return;
        }
        self.loading_more = true;
        self.more_error = None;
        let seq = self.seq;
        let filters = self.query();
        let path = format!(
            "/v1/audit?{}{}limit={PAGE}&cursor={}",
            filters,
            if filters.is_empty() { "" } else { "&" },
            encode(&cursor)
        );
        fetch(
            cx,
            self.ctx.gateway.request(Verb::Get, path, None),
            move |this, answer, _| {
                if this.seq != seq {
                    return;
                }
                this.loading_more = false;
                match answer.map(|answer| parse_page(&answer)) {
                    Ok(Some(page)) => {
                        this.older.extend(page.events);
                        this.cursor = page.next_cursor;
                    }
                    Ok(None) => this.more_error = Some("Older events didn't load.".into()),
                    Err(error) => this.more_error = Some(format!("Older events didn't load: {error}")),
                }
            },
        );
        cx.notify();
    }

    fn apply_who(&mut self, cx: &mut Context<Self>) {
        self._who_pause = None;
        let who = self.who.read(cx).value().trim().to_string();
        if who != self.actor {
            self.actor = who;
            self.read(cx);
        }
    }

    fn toggle(&mut self, id: String, cx: &mut Context<Self>) {
        self.open = if self.open.as_deref() == Some(id.as_str()) {
            None
        } else {
            Some(id)
        };
        self.metadata_open = false;
        self.copied = None;
        cx.notify();
    }

    fn copy(&mut self, text: String, what: &'static str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(what);
        cx.notify();
    }

    fn toggle_menu(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = (self.menu != Some(menu)).then_some(menu);
        if self.menu == Some(Menu::Team) {
            self.team_search.update(cx, |input, cx| input.set_value("", window, cx));
            let focus = self.team_search.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        }
        cx.notify();
    }

    fn render_filters(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let when = ui::tabs(palette).children(RANGES.into_iter().map(|(id, label)| {
            ui::tab(
                SharedString::from(format!("audit-since-{id}")),
                label,
                self.since == id,
                palette,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.since != id {
                    this.since = id;
                    this.read(cx);
                }
            }))
        }));
        let what_label = AREAS
            .iter()
            .find(|(id, _)| *id == self.action)
            .map_or("Every change", |(_, label)| label);
        let what_open = self.menu == Some(Menu::What);
        let what = div()
            .relative()
            .child(
                row::select_button("audit-what", "What", what_label, what_open, palette).menu_trigger(
                    what_open,
                    cx,
                    |this| this.menu == Some(Menu::What),
                    |this, window, cx| this.toggle_menu(Menu::What, window, cx),
                ),
            )
            .when(what_open, |anchor| {
                anchor.child(row::popover(
                    v_flex()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }))
                        .children(AREAS.into_iter().enumerate().map(|(ix, (id, label))| {
                            row::menu_item(
                                SharedString::from(format!("audit-area-{ix}")),
                                label,
                                self.action == id,
                                0,
                                palette,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                if this.action != id {
                                    this.action = id;
                                    this.read(cx);
                                }
                                cx.notify();
                            }))
                        })),
                    palette,
                ))
            });
        let team_open = self.menu == Some(Menu::Team);
        let team = (!self.teams.is_empty()).then(|| {
            let current = self
                .teams
                .iter()
                .find(|team| team.slug == self.team)
                .map_or("Any".to_string(), |team| team.name.clone());
            let search = self.team_search.read(cx).value().to_string();
            let matches = matching_teams(&self.teams, &search);
            let empty = matches.is_empty();
            div()
                .relative()
                .child(
                    row::select_button("audit-team", "Team", current, team_open, palette).menu_trigger(
                        team_open,
                        cx,
                        |this| this.menu == Some(Menu::Team),
                        |this, window, cx| this.toggle_menu(Menu::Team, window, cx),
                    ),
                )
                .when(team_open, |anchor| {
                    anchor.child(row::popover(
                        v_flex()
                            .w(px(280.))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                cx.notify();
                            }))
                            .child(
                                h_flex()
                                    .h(px(ui::CONTROL_HEIGHT))
                                    .px(px(8.))
                                    .mb(px(4.))
                                    .gap(px(6.))
                                    .rounded(px(6.))
                                    .bg(palette.hover)
                                    .child(ui::icon(Lucide::Search, palette.icon))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .child(Input::new(&self.team_search).appearance(false)),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .id("audit-team-list")
                                    .max_h(px(320.))
                                    .overflow_y_scroll()
                                    .when(search.trim().is_empty(), |list| {
                                        list.child(
                                            row::menu_item(
                                                "audit-team-any",
                                                "Any team",
                                                self.team.is_empty(),
                                                0,
                                                palette,
                                            )
                                            .on_click(cx.listener(
                                                |this, _, _, cx| {
                                                    this.menu = None;
                                                    if !this.team.is_empty() {
                                                        this.team.clear();
                                                        this.read(cx);
                                                    }
                                                    cx.notify();
                                                },
                                            )),
                                        )
                                    })
                                    .children(matches.into_iter().enumerate().map(|(ix, (team, depth))| {
                                        let slug = team.slug.clone();
                                        row::menu_item(
                                            SharedString::from(format!("audit-team-{ix}")),
                                            team.name.clone(),
                                            self.team == team.slug,
                                            depth,
                                            palette,
                                        )
                                        .on_click(cx.listener(
                                            move |this, _, _, cx| {
                                                this.menu = None;
                                                if this.team != slug {
                                                    this.team = slug.clone();
                                                    this.read(cx);
                                                }
                                                cx.notify();
                                            },
                                        ))
                                    }))
                                    .when(empty, |list| {
                                        list.child(
                                            div()
                                                .px(px(8.))
                                                .py(px(6.))
                                                .text_color(palette.faint)
                                                .child("No team by that name"),
                                        )
                                    }),
                            ),
                        palette,
                    ))
                })
        });
        let can_export = self.permissions.iter().any(|held| held == "audit.export");
        let who = h_flex()
            .flex_1()
            .min_w(px(160.))
            .h(px(ui::CONTROL_HEIGHT))
            .px(px(10.))
            .gap(px(6.))
            .rounded(px(8.))
            .bg(palette.hover)
            .text_size(px(13.))
            .child(ui::icon(Lucide::Search, palette.icon))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(Input::new(&self.who).appearance(false)),
            );
        let top = h_flex()
            .gap(px(8.))
            .child(when)
            .child(div().flex_1())
            .when(can_export, |row| {
                row.child(if self.exporting {
                    ui::disabled_button("audit-csv", "Downloading…", palette).into_any_element()
                } else {
                    download_button(palette, true)
                        .on_click(cx.listener(|this, _, _, cx| this.download_csv(cx)))
                        .into_any_element()
                })
            });
        let exported = self.exported.clone().map(|outcome| match outcome {
            Ok(saved) => ui::note(saved, palette),
            Err(error) => ui::error_line(format!("The CSV didn't download: {error}"), palette),
        });
        // Two lines: how far back, with the download at the right; then what, which team and who.
        v_flex()
            .gap(px(8.))
            .mb(px(16.))
            .child(top)
            .children(exported)
            .child(h_flex().gap(px(8.)).child(what).children(team).child(who))
    }

    fn render_row(&self, event: &Event, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let expanded = self.open.as_deref() == Some(event.id.as_str());
        let id = event.id.clone();
        let row = ui::row(palette)
            .id(SharedString::from(format!("audit-row-{}", event.id)))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(id.clone(), cx)))
            .child(
                div()
                    .w(px(124.))
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child(when(&event.at)),
            )
            .child(
                ui::two_lines(
                    actor_name(event),
                    format!("{} · {}", kind_text(&event.actor.kind), surface_text(&event.surface)),
                    palette,
                )
                .flex_1(),
            )
            .child(
                div()
                    .w(px(170.))
                    .flex_none()
                    .truncate()
                    .font_family(ui::MONO)
                    .text_size(px(12.))
                    .text_color(palette.foreground)
                    .child(event.action.clone()),
            )
            .child(ui::two_lines(target_name(event), event.target_type.replace('_', " "), palette).flex_1())
            .child(ui::icon(
                if expanded {
                    Lucide::ChevronDown
                } else {
                    Lucide::ChevronRight
                },
                palette.icon,
            ));
        if !expanded {
            return row.into_any_element();
        }
        v_flex()
            .child(row)
            .child(self.render_details(event, palette, cx))
            .into_any_element()
    }

    fn render_details(&self, event: &Event, palette: Palette, cx: &mut Context<Self>) -> Div {
        let rows = changes(event);
        let mono = |text: Option<String>, struck: bool| {
            let empty = text.is_none();
            div()
                .flex_1()
                .min_w(px(0.))
                .font_family(ui::MONO)
                .text_size(px(12.))
                .text_color(if empty { palette.faint } else { palette.foreground })
                .when(struck && !empty, |cell| cell.line_through())
                .child(text.unwrap_or_else(|| "–".into()))
        };
        let diff: AnyElement = if rows.is_empty() {
            ui::note("No field changed.", palette).into_any_element()
        } else {
            v_flex()
                .child(
                    h_flex()
                        .gap(px(12.))
                        .pb(px(4.))
                        .child(div().w(px(160.)).child(ui::label("Field", palette)))
                        .child(div().flex_1().child(ui::label("Before", palette)))
                        .child(div().flex_1().child(ui::label("After", palette))),
                )
                .children(rows.into_iter().map(|change| {
                    h_flex()
                        .gap(px(12.))
                        .py(px(3.))
                        .items_start()
                        .child(
                            div()
                                .w(px(160.))
                                .flex_none()
                                .truncate()
                                .font_family(ui::MONO)
                                .text_size(px(12.))
                                .text_color(palette.strong)
                                .child(change.field),
                        )
                        .child(mono(change.before, true))
                        .child(mono(change.after, false))
                }))
                .into_any_element()
        };
        let request_id = event.request_id.clone();
        let json = serde_json::to_string_pretty(&event.raw).unwrap_or_default();
        let metadata = event
            .metadata
            .as_ref()
            .map(|metadata| serde_json::to_string_pretty(metadata).unwrap_or_default());
        let has_metadata = metadata.is_some();
        let meta = h_flex()
            .gap(px(14.))
            .pt(px(8.))
            .text_size(px(12.))
            .text_color(palette.muted)
            .when(!request_id.is_empty(), |meta| {
                let copy = request_id.clone();
                meta.child(
                    h_flex()
                        .id("audit-request-id")
                        .gap(px(6.))
                        .cursor_pointer()
                        .hover(|style| style.text_color(palette.strong))
                        .child("Request")
                        .child(div().font_family(ui::MONO).child(request_id.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| this.copy(copy.clone(), "id", cx))),
                )
            })
            .when(has_metadata, |meta| {
                meta.child(
                    div()
                        .id("audit-metadata")
                        .cursor_pointer()
                        .hover(|style| style.text_color(palette.strong))
                        .child(if self.metadata_open {
                            "Hide metadata"
                        } else {
                            "Metadata"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.metadata_open = !this.metadata_open;
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_1())
            .when(self.copied == Some("id"), |meta| meta.child("Request id copied"))
            .child(
                div()
                    .id("audit-copy-json")
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.strong))
                    .child(if self.copied == Some("json") {
                        "Copied"
                    } else {
                        "Copy as JSON"
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.copy(json.clone(), "json", cx))),
            );
        v_flex()
            .px(px(16.))
            .py(px(12.))
            .bg(palette.raised)
            .child(diff)
            .child(meta)
            .when_some(metadata.filter(|_| self.metadata_open), |column, text| {
                column.child(
                    div()
                        .mt(px(8.))
                        .p(px(10.))
                        .rounded(px(8.))
                        .bg(palette.hover)
                        .font_family(ui::MONO)
                        .text_size(px(12.))
                        .text_color(palette.foreground)
                        .child(text),
                )
            })
    }
}

fn download_button(palette: Palette, enabled: bool) -> Stateful<Div> {
    if enabled {
        ui::ghost_button("audit-csv", "Download CSV", palette)
    } else {
        ui::disabled_button("audit-csv", "Download CSV", palette)
    }
}

fn older_button(palette: Palette, enabled: bool) -> Stateful<Div> {
    if enabled {
        ui::button("audit-older", "Show older", palette)
    } else {
        ui::disabled_button("audit-older", "Show older", palette)
    }
}

pub(super) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        h_flex()
            .gap(px(8.))
            .child(download_button(palette, false))
            .child(older_button(palette, false))
            .into_any_element()
    })
}

impl Render for AuditPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let filters = self.render_filters(palette, cx);
        let body: AnyElement = match &self.first {
            Load::Loading => ui::skeleton(6, palette).into_any_element(),
            Load::Failed(error) => v_flex()
                .items_center()
                .child(ui::failed(error.clone(), palette))
                .child(
                    ui::button("audit-retry", "Try again", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.read(cx))),
                )
                .into_any_element(),
            Load::Ready(page) if page.events.is_empty() => {
                let filtered = !self.action.is_empty() || !self.actor.is_empty() || !self.team.is_empty();
                ui::empty(
                    format!(
                        "No changes {}{}",
                        if self.since.is_empty() { "yet" } else { "in this period" },
                        if filtered { " for these filters" } else { "" }
                    ),
                    scope_text(&page.scope),
                    palette,
                )
                .into_any_element()
            }
            Load::Ready(page) => {
                let events: Vec<Event> = page.events.iter().chain(self.older.iter()).cloned().collect();
                let total = events.len();
                let scope = page.scope.clone();
                let head = ui::head_row(palette)
                    .child(div().w(px(124.)).child("Time"))
                    .child(div().flex_1().child("Who"))
                    .child(div().w(px(170.)).child("Change"))
                    .child(div().flex_1().child("Target"))
                    .child(div().w(px(14.)));
                let rows: Vec<AnyElement> = events.iter().map(|event| self.render_row(event, palette, cx)).collect();
                let foot: AnyElement = if self.cursor.is_some() {
                    if self.loading_more {
                        ui::disabled_button("audit-older", "Loading…", palette).into_any_element()
                    } else {
                        older_button(palette, true)
                            .on_click(cx.listener(|this, _, _, cx| this.show_older(cx)))
                            .into_any_element()
                    }
                } else {
                    ui::note(
                        format!(
                            "{total} change{}, the oldest included. {}",
                            if total == 1 { "" } else { "s" },
                            scope_text(&scope)
                        ),
                        palette,
                    )
                    .into_any_element()
                };
                v_flex()
                    .child(head)
                    .children(rows)
                    .child(
                        v_flex()
                            .pt(px(12.))
                            .gap(px(6.))
                            .items_center()
                            .child(foot)
                            .when_some(self.more_error.clone(), |column, error| {
                                column.child(ui::error_line(error, palette))
                            }),
                    )
                    .into_any_element()
            }
        };
        let name = self.ctx.org_name.clone();
        v_flex()
            .w_full()
            .child(ui::lead(
                format!(
                    "Every change in {}, newest first.",
                    if name.is_empty() {
                        "your organization".to_string()
                    } else {
                        name
                    }
                ),
                palette,
            ))
            .child(filters)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The glob import of gpui_kit brings its own `test` macro: name the built-in one.
    use core::prelude::v1::test;

    #[test]
    fn exports_every_event_for_the_filters() {
        assert_eq!(export_path(""), "/v1/audit/export.csv");
        let filters = query("30d", "member.role_changed", "ana@acme.test", "core");
        let path = export_path(&filters);
        assert!(path.starts_with("/v1/audit/export.csv?since=30d"), "{path}");
        assert!(!path.contains("limit") && !path.contains("cursor"), "{path}");
        assert_eq!(
            csv_rows("id,at\r\na,\"x\ny\"\r\nb,z\r\n"),
            2,
            "a line break in a cell is one event"
        );
        assert_eq!(csv_rows("id,at\n"), 0);
        assert_eq!(events_text(1), "1 event");
        assert_eq!(events_text(1204), "1,204 events");
    }

    /// A sample answer, written as Gateway sends it.
    fn value(text: &str) -> Value {
        serde_json::from_str(text).expect("JSON")
    }

    fn sample() -> Value {
        value(
            r#"{
            "scope": "all",
            "nextCursor": "c_2",
            "events": [
                {
                    "id": "aud_1",
                    "at": "2026-11-05T14:03:21.000Z",
                    "actor": { "userId": "usr_1", "email": "ana@acme.test", "keyId": "ui", "kind": "person" },
                    "surface": "ui",
                    "action": "member.role_changed",
                    "target": { "type": "member", "id": "usr_2" },
                    "before": { "role": "member", "email": "bo@acme.test" },
                    "after": { "role": "admin", "email": "bo@acme.test" },
                    "requestId": "req_9",
                    "metadata": {}
                },
                {
                    "id": "aud_2",
                    "at": "2026-11-04T09:00:00Z",
                    "actor": { "keyId": "github", "kind": "system" },
                    "surface": "webhook",
                    "action": "github.installed",
                    "target": { "type": "github_installation", "id": "inst_1", "slug": "acme" },
                    "after": true
                }
            ]
        }"#,
        )
    }

    #[test]
    fn reads_a_page_of_the_audit_log() {
        let page = parse_page(&sample()).unwrap();
        assert_eq!(page.scope, "all");
        assert_eq!(page.next_cursor.as_deref(), Some("c_2"));
        assert_eq!(page.events.len(), 2);

        let first = &page.events[0];
        assert_eq!(actor_name(first), "ana@acme.test");
        assert_eq!(target_name(first), "bo@acme.test");
        assert!(first.metadata.is_none());
        assert_eq!(
            changes(first),
            vec![Change {
                field: "role".into(),
                before: Some("member".into()),
                after: Some("admin".into())
            }]
        );
        assert_eq!(when(&first.at), "2026-11-05 14:03");

        let second = &page.events[1];
        assert_eq!(actor_name(second), "GitHub");
        let mut by_provider = second.clone();
        by_provider.actor.key_id = "clerk".into();
        assert_eq!(actor_name(&by_provider), "Sign-in provider");
        assert_eq!(target_name(second), "acme");
        assert_eq!(
            changes(second),
            vec![Change {
                field: "value".into(),
                before: None,
                after: Some("true".into())
            }]
        );
    }

    #[test]
    fn reads_teams_and_builds_the_query() {
        let teams = parse_teams(&value(
            r#"{ "default": null, "tree": [
            { "id": "t1", "slug": "eng", "name": "Engineering", "depth": 1, "children": [
                { "id": "t2", "slug": "eng-web", "name": "Web", "depth": 2, "children": [] }
            ] }
        ] }"#,
        ));
        assert_eq!(
            teams.iter().map(|team| team.slug.as_str()).collect::<Vec<_>>(),
            ["eng", "eng-web"]
        );
        assert_eq!(
            query("7d", "member.*,invitation.*", "ana@acme.test", ""),
            "since=7d&action=member.*%2Cinvitation.*&actor=ana%40acme.test"
        );
        assert_eq!(query("", "", "", ""), "");
    }
}
