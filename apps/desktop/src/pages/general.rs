//! Settings › General (`ui/.../settings/org/general.tsx` and `review-events-card.tsx`): the organization's card (logo,
//! name, address), sharing outside it, chats and agents (retention, web search and the model that searches, agents on
//! personal plans), and code review. Gateway keeps the settings (`GET` and `PUT /v1/orgs/settings`, `org.settings`) and
//! the review level (`/v1/policies/review-events`, `policy.manage`). The logo and the name live with the sign-in
//! provider and change only through Nexus UI's own server, so they open in the browser. Everyone sees the page; only
//! owners and admins change it.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Map, Value, json};

use super::{Ctx, Load, Verb, fetch, palette, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

#[cfg(test)]
#[path = "general_data_tests.rs"]
mod data_tests;

pub(super) fn build_locked(window: &mut Window, cx: &mut App) -> AnyView {
    let retention = cx.new(|cx| InputState::new(window, cx));
    super::ui::locked_page(cx, move |palette| {
        v_flex()
            .child(ui::section_head("Chats and agents", palette))
            .child(retention_setting(
                &retention,
                true,
                "Older messages are deleted for everyone.".into(),
                palette,
            ))
            .into_any_element()
    })
}

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| GeneralPage::new(ctx, window, cx));
    Some(view.into())
}

/// Policy uses the same Gateway controls and persistence as General.
pub(super) fn build_policy(ctx: Ctx, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let mut page = GeneralPage::new(ctx, window, cx);
        page.policy_only = true;
        page
    })
    .into()
}

const READ_ONLY: &str = "Only organization admins can change this.";
const HELD_BACK: &str =
    "No search runs now: zero data retention or a domain list is set, and Gateway cannot apply them yet.";

/// The sharing modes, as the web page offers them.
const MODES: [(&str, &str, &str); 3] = [
    (
        "public",
        "Anyone with the link",
        "Public links, and named people by email. Links can be made to expire.",
    ),
    ("named", "Named people", "Only the emails you add. Domain rules apply."),
    ("off", "No one outside", "Chats and artifacts stay in the organization."),
];

/// The review event levels (`review-events.ts`).
const REVIEW_LEVELS: [(&str, &str, &str); 3] = [
    (
        "full",
        "Approve and request changes",
        "Reviewers may approve a pull request they read in full, and request changes on a failing check.",
    ),
    (
        "request_changes",
        "Request changes only",
        "Reviewers may request changes on a failing check. They never approve.",
    ),
    (
        "comment",
        "Comment only",
        "Reviewers post comments only. They never approve or block a merge.",
    ),
];

// ------------------------------------------------------------------ What Gateway answers

/// The organization, from `GET /v1/tenants/me`.
#[derive(Debug, Clone, PartialEq)]
struct Org {
    name: String,
    /// Its segment in Nexus UI's URLs: the slug, else the id.
    segment: String,
    personal: bool,
    permissions: Vec<String>,
}

impl Org {
    fn can(&self, permission: &str) -> bool {
        self.permissions.iter().any(|held| held == permission)
    }
}

fn parse_org(answer: &Value) -> Option<Org> {
    let tenant = answer.get("tenant")?;
    let id = ui::text(tenant, "id");
    let slug = ui::text(tenant, "slug");
    Some(Org {
        name: ui::text(tenant, "name").to_string(),
        segment: if slug.is_empty() { id } else { slug }.to_string(),
        personal: ui::text(tenant, "kind") == "personal",
        permissions: strings(answer.get("permissions")),
    })
}

/// Sharing outside the organization (`ArtifactSharingSettings`), with kit's defaults for what Gateway leaves out.
#[derive(Debug, Clone, PartialEq)]
struct Sharing {
    external: String,
    who: String,
    code: bool,
    allow: Vec<String>,
    deny: Vec<String>,
    max_days: Option<u64>,
    required: bool,
    carry_on: bool,
    pages: bool,
    pages_public: bool,
    pages_queries: bool,
}

impl Sharing {
    fn read(answer: &Value) -> Sharing {
        let flag = |key: &str, default: bool| answer.get(key).and_then(Value::as_bool).unwrap_or(default);
        let word = |key: &str, default: &str| answer.get(key).and_then(Value::as_str).unwrap_or(default).to_string();
        let domains = answer.get("artifacts.external.domains");
        let expiry = answer.get("artifacts.external.expiry");
        Sharing {
            external: word("artifacts.external", "public"),
            who: word("artifacts.external.who", "everyone"),
            code: flag("artifacts.external.code", false),
            allow: strings(domains.and_then(|domains| domains.get("allow"))),
            deny: strings(domains.and_then(|domains| domains.get("deny"))),
            max_days: expiry.and_then(|expiry| expiry.get("maxDays")).and_then(Value::as_u64),
            required: expiry
                .and_then(|expiry| expiry.get("required"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            carry_on: flag("artifacts.external.continue", true),
            pages: flag("artifacts.pages", true),
            pages_public: flag("artifacts.pages.public", false),
            pages_queries: flag("artifacts.pages.queries", false),
        }
    }

    /// Each setting as `PUT /v1/orgs/settings` takes it.
    fn entries(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("artifacts.external", json!(self.external)),
            ("artifacts.external.who", json!(self.who)),
            ("artifacts.external.code", json!(self.code)),
            (
                "artifacts.external.domains",
                json!({ "allow": self.allow, "deny": self.deny }),
            ),
            (
                "artifacts.external.expiry",
                json!({ "maxDays": self.max_days, "required": self.required }),
            ),
            ("artifacts.external.continue", json!(self.carry_on)),
            ("artifacts.pages", json!(self.pages)),
            ("artifacts.pages.public", json!(self.pages_public)),
            ("artifacts.pages.queries", json!(self.pages_queries)),
        ]
    }
}

/// Only the settings that differ from `current`: what Save sends (`changedSettings`).
fn changed(current: &Sharing, next: &Sharing, pages_supported: bool) -> Map<String, Value> {
    current
        .entries()
        .into_iter()
        .zip(next.entries())
        .filter(|((key, _), _)| pages_supported || !key.starts_with("artifacts.pages"))
        .filter(|((_, before), (_, after))| before != after)
        .map(|(_, (key, after))| (key.to_string(), after))
        .collect()
}

/// What Save says (`savedText`).
fn saved_text(changes: &Map<String, Value>) -> &'static str {
    if changes.get("artifacts.pages") == Some(&Value::Bool(false)) {
        "Saved. Pages no longer open, shared or not."
    } else if changes.get("artifacts.pages.public") == Some(&Value::Bool(false)) {
        "Saved. Public page links no longer open."
    } else {
        "Saved. Links already shared keep working until revoked."
    }
}

/// The organization's settings, from `GET /v1/orgs/settings`.
#[derive(Debug, Clone, PartialEq)]
struct Settings {
    retention_days: Option<u64>,
    default_retention: Option<u64>,
    agent_subscriptions: bool,
    /// `None` on a Gateway older than 0.4.0, which has no sharing settings.
    sharing: Option<Sharing>,
    pages_supported: bool,
    /// `off` or `borrowed`; `None` on a Gateway without web search.
    search_engine: Option<String>,
    /// Whether Gateway answers `webSearch.model`.
    search_model_supported: bool,
    /// `None` for the default (each chat's own model), else `cheapest` or a model's id.
    search_model: Option<String>,
    search_held_back: bool,
}

fn parse_settings(answer: &Value) -> Option<Settings> {
    let object = answer.as_object()?;
    let domains = answer.get("webSearch.domains");
    let listed = |key: &str| {
        domains
            .and_then(|domains| domains.get(key))
            .and_then(Value::as_array)
            .is_some_and(|list| !list.is_empty())
    };
    Some(Settings {
        retention_days: answer.get("chatRetentionDays").and_then(Value::as_u64),
        default_retention: answer
            .get("defaults")
            .and_then(|defaults| defaults.get("chatRetentionDays"))
            .and_then(Value::as_u64),
        agent_subscriptions: answer
            .get("agentSubscriptions")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        sharing: object.contains_key("artifacts.external").then(|| Sharing::read(answer)),
        pages_supported: object.contains_key("artifacts.pages"),
        search_engine: answer.get("webSearch.engine").and_then(Value::as_str).map(String::from),
        search_model_supported: object.contains_key("webSearch.model"),
        search_model: answer.get("webSearch.model").and_then(Value::as_str).map(String::from),
        search_held_back: answer
            .get("webSearch.requireZdr")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || listed("allow")
            || listed("deny"),
    })
}

/// The review event level, from `GET /v1/policies/review-events`.
#[derive(Debug, Clone, PartialEq)]
struct Review {
    level: String,
    default: bool,
}

fn parse_review(answer: &Value) -> Option<Review> {
    Some(Review {
        level: answer.get("level")?.as_str()?.to_string(),
        default: answer.get("default").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// A model the organization may pin to run every chat's search: allowed, and able to search.
#[derive(Debug, Clone, PartialEq)]
struct Model {
    id: String,
    name: String,
    allowed: bool,
    searches: bool,
}

fn parse_models(answer: &Value) -> Vec<Model> {
    answer
        .get("data")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    let id = model.get("id")?.as_str()?.to_string();
                    let name = model.get("name").and_then(Value::as_str).unwrap_or(&id).to_string();
                    Some(Model {
                        name,
                        allowed: model.get("allowed").and_then(Value::as_bool).unwrap_or(false),
                        searches: model.get("web_search_usd").is_some_and(|price| !price.is_null()),
                        id,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The model menu's options (`searchModelOptions`): the default, the cheapest, each allowed model that can search, and
/// the pinned one when it is none of those.
fn search_options(models: Option<&[Model]>, current: &str) -> Vec<(String, String)> {
    let mut options = vec![
        (
            "default".to_string(),
            "The chat's model (the cheapest one on auto)".to_string(),
        ),
        ("cheapest".to_string(), "The cheapest model that can search".to_string()),
    ];
    for model in models.unwrap_or_default() {
        if model.allowed && model.searches {
            options.push((model.id.clone(), model.name.clone()));
        }
    }
    if !options.iter().any(|(value, _)| value == current) {
        let listed = models.and_then(|models| models.iter().find(|model| model.id == current));
        let label = match (models, listed) {
            (None, _) => current.to_string(),
            (Some(_), Some(model)) => format!("{} (your policy denies it)", model.name),
            (Some(_), None) => format!("{current} (not available)"),
        };
        options.push((current.to_string(), label));
    }
    options
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default()
}

/// Domains as typed: split on commas, spaces and semicolons, lowercased, without a leading `@`.
fn domain_list(text: &str) -> Vec<String> {
    text.split([',', ';', ' ', '\n', '\t'])
        .map(|part| part.trim().trim_start_matches('@').to_lowercase())
        .filter(|part| !part.is_empty())
        .collect()
}

/// A whole number of days from 1 to 3650, as typed.
fn days(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() || !text.chars().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|days| (1..=3650).contains(days))
}

/// Gateway's refusal said plainly: a 403 names the role it wants ("only the org's owner or admins can …").
fn plain(error: &str) -> String {
    if error.starts_with("only the org's") || error.contains("role_required") {
        READ_ONLY.to_string()
    } else {
        error.to_string()
    }
}

/// A route this Gateway does not have answers 404: the part is left out.
fn missing(error: &str) -> bool {
    error.contains("no such endpoint") || error.contains("HTTP 404") || error.to_lowercase().contains("not found")
}

// ------------------------------------------------------------------ The page

/// A line under a section after a change: what happened, or why not.
#[derive(Debug, Clone)]
struct Outcome {
    text: String,
    error: bool,
}

impl Outcome {
    fn done(text: impl Into<String>) -> Option<Outcome> {
        Some(Outcome {
            text: text.into(),
            error: false,
        })
    }

    fn failed(error: &str) -> Option<Outcome> {
        Some(Outcome {
            text: plain(error),
            error: true,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Part {
    Sharing,
    Chats,
    Review,
}

fn settings_form_snapshot(
    draft: Option<Sharing>,
    mut fields: [String; 4],
    filled: bool,
    settings: Option<&Settings>,
) -> (Option<Sharing>, [String; 4]) {
    if !filled && let Some(settings) = settings {
        if let Some(sharing) = &settings.sharing {
            fields[0] = sharing.allow.join(", ");
            fields[1] = sharing.deny.join(", ");
            fields[2] = sharing.max_days.map(|days| days.to_string()).unwrap_or_default();
        }
        fields[3] = settings.retention_days.map(|days| days.to_string()).unwrap_or_default();
    }
    (draft, fields)
}

struct GeneralPage {
    policy_only: bool,
    ctx: Ctx,
    marker: Option<String>,
    load_epoch: u64,
    settings_epoch: u64,
    org: Load<Org>,
    settings: Load<Settings>,
    /// `None` when this Gateway has no review events.
    review: Load<Option<Review>>,
    models: Option<Vec<Model>>,
    /// Sharing as changed and not saved yet: the mode, who, and the switches. The text fields hold the rest.
    draft: Option<Sharing>,
    allow: Entity<InputState>,
    deny: Entity<InputState>,
    max_days: Entity<InputState>,
    retention: Entity<InputState>,
    /// The text fields hold the settings read last; false until they are filled again.
    filled: bool,
    busy: Option<Part>,
    sharing_note: Option<Outcome>,
    chats_note: Option<Outcome>,
    review_note: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl GeneralPage {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> GeneralPage {
        let allow = cx.new(|cx| InputState::new(window, cx).placeholder("Any domain"));
        let deny = cx.new(|cx| InputState::new(window, cx).placeholder("None"));
        let max_days = cx.new(|cx| InputState::new(window, cx).placeholder("Never"));
        let retention = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&retention, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.save_retention(cx);
                }
            }),
        ];
        let marker = ctx.gateway.marker();
        let mut page = GeneralPage {
            policy_only: false,
            ctx,
            marker,
            load_epoch: 0,
            settings_epoch: 0,
            org: Load::Loading,
            settings: Load::Loading,
            review: Load::Loading,
            models: None,
            draft: None,
            allow,
            deny,
            max_days,
            retention,
            filled: false,
            busy: None,
            sharing_note: None,
            chats_note: None,
            review_note: None,
            _subscriptions: subscriptions,
        };
        page.load(cx);
        page
    }

    fn current(&self) -> bool {
        self.marker.is_some() && self.marker == self.ctx.gateway.marker()
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if !self.current() || self.busy.is_some() {
            return;
        }
        self.load_epoch += 1;
        let epoch = self.load_epoch;
        self.org = Load::Loading;
        self.settings = Load::Loading;
        self.review = Load::Loading;
        let gateway = self.ctx.gateway.clone();
        super::read(cx, gateway.query("/v1/tenants/me".into()), move |this, answer, _| {
            if !this.current() || this.load_epoch != epoch {
                return;
            }
            this.org = match answer {
                Ok(answer) => parse_org(&answer)
                    .map(Load::Ready)
                    .unwrap_or_else(|| Load::Failed("Gateway sent no organization.".into())),
                Err(error) => Load::Failed(error.into()),
            };
        });
        self.read_settings(cx);
        super::read(
            cx,
            gateway.query("/v1/policies/review-events".into()),
            move |this, answer, _| {
                if !this.current() || this.load_epoch != epoch {
                    return;
                }
                this.review = match answer {
                    Ok(answer) => Load::Ready(parse_review(&answer)),
                    Err(error) if missing(&error) => Load::Ready(None),
                    Err(error) => Load::Failed(error.into()),
                };
            },
        );
        super::read(
            cx,
            gateway.query("/v1/models?status=all".into()),
            move |this, answer, _| {
                if !this.current() || this.load_epoch != epoch {
                    return;
                }
                // Without the list, a pinned model shows by its id.
                this.models = answer.ok().map(|answer| parse_models(&answer));
            },
        );
    }

    fn read_settings(&mut self, cx: &mut Context<Self>) {
        if !self.current() {
            return;
        }
        self.settings_epoch += 1;
        let epoch = self.settings_epoch;
        let mut draft = self.form_snapshot(cx);
        super::read(
            cx,
            self.ctx.gateway.query("/v1/orgs/settings".into()),
            move |this, answer, cx| {
                if !this.current() || this.settings_epoch != epoch {
                    return;
                }
                if this.form_snapshot(cx) == draft {
                    this.take_settings(answer);
                    draft = this.form_snapshot(cx);
                }
            },
        );
    }

    fn form_snapshot(&self, cx: &App) -> (Option<Sharing>, [String; 4]) {
        let fields = [
            self.allow.read(cx).value().to_string(),
            self.deny.read(cx).value().to_string(),
            self.max_days.read(cx).value().to_string(),
            self.retention.read(cx).value().to_string(),
        ];
        settings_form_snapshot(
            self.draft.clone(),
            fields,
            self.filled,
            match &self.settings {
                Load::Ready(settings) => Some(settings),
                _ => None,
            },
        )
    }

    fn take_settings(&mut self, answer: Result<Value, String>) {
        self.settings = match answer {
            Ok(answer) => parse_settings(&answer)
                .map(Load::Ready)
                .unwrap_or_else(|| Load::Failed("Gateway sent settings this app cannot read.".into())),
            Err(error) => Load::Failed(error.into()),
        };
        self.draft = None;
        self.filled = false;
    }

    /// Fill the text fields from the settings read last. Done while drawing, where the window is at hand.
    fn fill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(settings) = &self.settings else {
            return;
        };
        let sharing = settings.sharing.clone();
        let retention = settings.retention_days.map(|days| days.to_string()).unwrap_or_default();
        if let Some(sharing) = sharing {
            let allow = sharing.allow.join(", ");
            let deny = sharing.deny.join(", ");
            let max = sharing.max_days.map(|days| days.to_string()).unwrap_or_default();
            self.allow.update(cx, |state, cx| state.set_value(allow, window, cx));
            self.deny.update(cx, |state, cx| state.set_value(deny, window, cx));
            self.max_days.update(cx, |state, cx| state.set_value(max, window, cx));
        }
        self.retention
            .update(cx, |state, cx| state.set_value(retention, window, cx));
        self.filled = true;
    }

    /// The sharing settings as they stand on screen: the draft, then what the text fields hold. `None` while the
    /// expiry field holds something that is not a number of days.
    fn sharing_next(&self, current: &Sharing, cx: &App) -> Option<Sharing> {
        let mut next = self.draft.clone().unwrap_or_else(|| current.clone());
        next.allow = domain_list(&self.allow.read(cx).value());
        next.deny = domain_list(&self.deny.read(cx).value());
        let max = self.max_days.read(cx).value();
        next.max_days = if max.trim().is_empty() { None } else { Some(days(&max)?) };
        Some(next)
    }

    fn edit_sharing(&mut self, change: impl FnOnce(&mut Sharing), cx: &mut Context<Self>) {
        if !self.can_manage() || self.busy.is_some() {
            return;
        }
        let Load::Ready(Settings {
            sharing: Some(current), ..
        }) = &self.settings
        else {
            return;
        };
        let mut draft = self.draft.clone().unwrap_or_else(|| current.clone());
        change(&mut draft);
        self.draft = Some(draft);
        self.sharing_note = None;
        cx.notify();
    }

    fn discard_sharing(&mut self, cx: &mut Context<Self>) {
        if !self.current() || self.busy.is_some() {
            return;
        }
        self.draft = None;
        self.filled = false;
        self.sharing_note = None;
        cx.notify();
    }

    fn save_sharing(&mut self, cx: &mut Context<Self>) {
        if !self.can_manage() || self.busy.is_some() {
            return;
        }
        let Load::Ready(settings) = &self.settings else {
            return;
        };
        let Some(current) = &settings.sharing else {
            return;
        };
        let Some(next) = self.sharing_next(current, cx) else {
            return;
        };
        let changes = changed(current, &next, settings.pages_supported);
        if changes.is_empty() || self.busy.is_some() {
            return;
        }
        let said = saved_text(&changes);
        self.put_settings(Part::Sharing, Value::Object(changes), said.to_string(), cx);
    }

    fn save_retention(&mut self, cx: &mut Context<Self>) {
        let Load::Ready(settings) = &self.settings else {
            return;
        };
        if self.busy.is_some() || !self.can_manage() {
            return;
        }
        let Some(value) = days(&self.retention.read(cx).value()) else {
            self.chats_note = Some(Outcome {
                text: "Keep chats for 1 to 3650 days.".into(),
                error: true,
            });
            cx.notify();
            return;
        };
        if Some(value) == settings.retention_days {
            return;
        }
        self.put_settings(
            Part::Chats,
            json!({ "chatRetentionDays": value }),
            "Saved. Messages older than this are deleted.".into(),
            cx,
        );
    }

    fn put_settings(&mut self, part: Part, body: Value, said: String, cx: &mut Context<Self>) {
        if !self.can_manage() || self.busy.is_some() {
            return;
        }
        self.settings_epoch += 1;
        let epoch = self.load_epoch;
        self.busy = Some(part);
        match part {
            Part::Sharing => self.sharing_note = None,
            _ => self.chats_note = None,
        }
        cx.notify();
        let plans_on = body.get("agentSubscriptions") == Some(&Value::Bool(true));
        fetch(
            cx,
            self.ctx
                .gateway
                .request(Verb::Put, "/v1/orgs/settings".into(), Some(body)),
            move |this, answer, cx| {
                if !this.current() || this.load_epoch != epoch {
                    return;
                }
                this.busy = None;
                if plans_on && answer.is_ok() {
                    plans_turned_on(cx);
                }
                let note = match &answer {
                    Ok(_) => Outcome::done(said),
                    Err(error) => Outcome::failed(error),
                };
                match part {
                    Part::Sharing => this.sharing_note = note,
                    _ => this.chats_note = note,
                }
                match answer {
                    Ok(answer) if parse_settings(&answer).is_some_and(|read| read.retention_days.is_some()) => {
                        this.take_settings(Ok(answer))
                    }
                    Ok(_) => this.read_settings(cx),
                    // Refused: the fields go back to what is saved.
                    Err(_) => {
                        if part == Part::Chats {
                            this.filled = false;
                        }
                    }
                }
            },
        );
    }

    fn save_review(&mut self, level: &'static str, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Part::Review);
        self.review_note = None;
        cx.notify();
        fetch(
            cx,
            self.ctx.gateway.request(
                Verb::Put,
                "/v1/policies/review-events".into(),
                Some(json!({ "level": level })),
            ),
            move |this, answer, _| {
                this.busy = None;
                match answer {
                    Ok(answer) => {
                        this.review = Load::Ready(Some(parse_review(&answer).unwrap_or(Review {
                            level: level.to_string(),
                            default: false,
                        })));
                        this.review_note = Outcome::done(match level {
                            "full" => "Saved. Reviewers may approve and request changes.",
                            "request_changes" => "Saved. Reviewers may request changes, and never approve.",
                            _ => "Saved. Reviewers post comments only.",
                        });
                    }
                    Err(error) => this.review_note = Outcome::failed(&error),
                }
            },
        );
    }

    fn can_manage(&self) -> bool {
        self.current() && matches!(&self.org, Load::Ready(org) if org.can("org.settings"))
    }

    // -------------------------------------------------------------- Drawing

    fn render_org(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let head = ui::section_head("Organization", palette);
        let org = match &self.org {
            Load::Loading => return v_flex().child(head).child(ui::skeleton(3, palette)).into_any_element(),
            Load::Failed(error) => {
                return v_flex()
                    .child(head)
                    .child(ui::error_line(
                        format!("The organization didn't load: {error}"),
                        palette,
                    ))
                    .into_any_element();
            }
            Load::Ready(org) => org,
        };
        let editable = org.can("org.settings") && !org.personal;
        let initial = org
            .name
            .trim()
            .chars()
            .next()
            .map(|first| first.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into());
        let logo = h_flex()
            .gap(px(10.))
            .child(
                div()
                    .size(px(32.))
                    .rounded(px(crate::tokens::radius::CONTROL))
                    .bg(palette.hover)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child(initial),
            )
            .when(editable, |row| {
                row.child(
                    ui::web_link("general-logo", "Change in the browser", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.ctx.open_web("settings/general", cx))),
                )
            });
        let logo_detail = if org.personal {
            "A personal organization shows its initial. A company organization can have its own logo."
        } else {
            "Shows in the org switcher and on shared pages."
        };
        let name_detail = if org.personal {
            "A personal organization keeps the name Personal. A company organization can have its own name."
        } else if editable {
            "The name lives with your sign-in provider, so it changes in Nexus on the web."
        } else {
            "Only owners and admins can rename it."
        };
        let name = h_flex()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child(org.name.clone()),
            )
            .when(editable, |row| {
                row.child(
                    ui::web_link("general-rename", "Rename in the browser", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.ctx.open_web("settings/general", cx))),
                )
            });
        let host = self.ctx.gateway.site_url();
        let host = host.trim_start_matches("https://").trim_start_matches("http://");
        let address = h_flex()
            .text_size(px(13.))
            .child(div().text_color(palette.faint).child(format!("{host}/app/")))
            .child(
                div()
                    .font_family(ui::MONO)
                    .text_color(palette.foreground)
                    .child(org.segment.clone()),
            );
        v_flex()
            .child(head)
            .child(self.ctx.settings_fields.field(
                "org-logo",
                ui::setting("Logo", logo_detail, logo, palette),
                None,
                cx,
            ))
            .child(self.ctx.settings_fields.field(
                "org-name",
                ui::setting("Name", name_detail, name, palette),
                None,
                cx,
            ))
            .child(self.ctx.settings_fields.field(
                "org-address",
                ui::setting(
                    "Address",
                    "Where your organization's pages live. Changing it is not available yet.",
                    address,
                    palette,
                ),
                None,
                cx,
            ))
            // Design systems stays under General and opens in the app.
            .child(ui::setting(
                "Branding",
                "Colors and type for the pages people make.",
                ui::button("general-branding", "Design systems", palette).on_click(
                    cx.listener(|this, _, window, cx| (this.ctx.navigate)(super::PageId::DesignSystems, window, cx)),
                ),
                palette,
            ))
            .into_any_element()
    }

    fn render_sharing(&self, settings: &Settings, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let org_name = match &self.org {
            Load::Ready(org) => org.name.clone(),
            _ => "your organization".into(),
        };
        let head = ui::section_head(format!("Sharing outside {org_name}"), palette);
        let Some(current) = &settings.sharing else {
            return v_flex()
                .child(head)
                .child(ui::setting(
                    "Not available yet",
                    "Needs Nexus Gateway 0.4.0 or later.",
                    div(),
                    palette,
                ))
                .into_any_element();
        };
        let manage = self.can_manage();
        let locked = !manage || self.busy.is_some();
        let value = self.draft.clone().unwrap_or_else(|| current.clone());
        let next = self.sharing_next(current, cx);
        let changes = next
            .as_ref()
            .map(|next| changed(current, next, settings.pages_supported))
            .unwrap_or_default();
        let dirty = !changes.is_empty() || next.is_none() || self.draft.as_ref().is_some_and(|draft| draft != current);
        let mode = value.external.clone();

        let modes = h_flex()
            .gap(px(8.))
            .children(MODES.into_iter().map(|(id, title, body)| {
                let picked = mode == id;
                v_flex()
                    .id(SharedString::from(format!("general-mode-{id}")))
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(4.))
                    .p(px(12.))
                    .rounded(px(10.))
                    .bg(if picked { ui::thumb(palette) } else { palette.raised })
                    .when(!locked, |card| {
                        card.cursor_pointer()
                            .hover(|style| style.bg(palette.hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit_sharing(|draft| draft.external = id.to_string(), cx)
                            }))
                    })
                    .child(div().text_size(px(13.)).text_color(palette.strong).child(title))
                    .child(div().text_size(px(12.)).text_color(palette.faint).child(body))
            }));

        let mut column = v_flex().child(head).child(
            v_flex()
                .gap(px(8.))
                .py(px(12.))
                .child(ui::setting_lines(
                    "Who can open a shared chat or artifact",
                    format!("Inside {org_name}, sharing always works."),
                    palette,
                ))
                .child(modes),
        );

        if mode != "off" {
            let who = h_flex()
                .gap(px(6.))
                .children(
                    [("everyone", "Everyone"), ("admins", "Owners and admins")].map(|(id, label)| {
                        ui::chip(
                            SharedString::from(format!("general-who-{id}")),
                            label,
                            value.who == id,
                            palette,
                        )
                        .when(!locked, |chip| {
                            chip.on_click(cx.listener(move |this, _, _, cx| {
                                this.edit_sharing(|draft| draft.who = id.to_string(), cx)
                            }))
                        })
                    }),
                );
            column = column
                .child(ui::setting("Who may share outside", "", who, palette))
                .child(
                    v_flex()
                        .gap(px(8.))
                        .py(px(12.))
                        .child(ui::setting_lines(
                            "Email domains",
                            "Comma-separated; a domain includes its subdomains. Empty: any domain.",
                            palette,
                        ))
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap(px(4.))
                                        .child(ui::label("Only these", palette))
                                        .child(field(&self.allow, None, locked, palette)),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap(px(4.))
                                        .child(ui::label("Never these", palette))
                                        .child(field(&self.deny, None, locked, palette)),
                                ),
                        ),
                );
            if mode == "public" {
                let max_text = self.max_days.read(cx).value();
                let max_valid = max_text.trim().is_empty() || days(&max_text).is_some();
                column = column
                    .child(ui::setting(
                        "Public links expire",
                        if max_valid {
                            "Empty: links can live until revoked."
                        } else {
                            "1 to 3650 days, or empty for none."
                        },
                        h_flex()
                            .gap(px(6.))
                            .text_size(px(13.))
                            .text_color(palette.faint)
                            .child("within")
                            .child(field(&self.max_days, Some(72.), locked, palette))
                            .child("days"),
                        palette,
                    ))
                    .child(ui::setting(
                        "Every public link must have an expiry",
                        "",
                        self.toggle("general-required", value.required, locked, palette, cx, |draft| {
                            draft.required = !draft.required
                        }),
                        palette,
                    ));
            }
            column = column
                .child(ui::setting(
                    "Code sources go with a shared chat",
                    "File paths, code excerpts and repository names.",
                    self.toggle("general-code", value.code, locked, palette, cx, |draft| {
                        draft.code = !draft.code
                    }),
                    palette,
                ))
                .child(ui::setting(
                    "Readers outside may continue a chat",
                    "In their own organization, at its expense, if the sharer allows.",
                    self.toggle("general-continue", value.carry_on, locked, palette, cx, |draft| {
                        draft.carry_on = !draft.carry_on
                    }),
                    palette,
                ));
        }
        if settings.pages_supported {
            column = column.child(ui::setting(
                "Pages",
                "People may make and share pages. Off: existing pages stop opening.",
                self.toggle("general-pages", value.pages, locked, palette, cx, |draft| {
                    draft.pages = !draft.pages
                }),
                palette,
            ));
            if value.pages {
                let public = ui::setting(
                    "Public links to pages",
                    "Off blocks existing public page links too.",
                    self.toggle(
                        "general-pages-public",
                        value.pages_public,
                        locked,
                        palette,
                        cx,
                        |draft| draft.pages_public = !draft.pages_public,
                    ),
                    palette,
                );
                // A page's Share sheet turns public links on here (`share_ui::blocked`).
                column = column.child(self.ctx.settings_fields.field("pages-public-links", public, None, cx));
            }
        }
        if settings.pages_supported && value.pages {
            column = column.child(ui::setting(
                "Live data on pages",
                "A page's owner may approve read-only queries on a connection they use. Readers see the results, never the query or the connection.",
                self.toggle("general-pages-queries", value.pages_queries, locked, palette, cx,
                    |draft| draft.pages_queries = !draft.pages_queries),
                palette,
            ));
        }
        let saving = self.busy == Some(Part::Sharing);
        column
            .when(dirty && manage, |column| {
                column.child(
                    h_flex()
                        .pt(px(12.))
                        .gap(px(8.))
                        .justify_end()
                        .child(
                            ui::button("general-discard", "Discard", palette)
                                .on_click(cx.listener(|this, _, _, cx| this.discard_sharing(cx))),
                        )
                        .child(if next.is_none() || saving {
                            ui::disabled_button("general-save", if saving { "Saving…" } else { "Save" }, palette)
                        } else {
                            ui::primary_button("general-save", "Save", palette)
                                .on_click(cx.listener(|this, _, _, cx| this.save_sharing(cx)))
                        }),
                )
            })
            .children(outcome(&self.sharing_note, palette))
            .when(!manage, |column| column.child(ui::note(READ_ONLY, palette).pt(px(8.))))
            .into_any_element()
    }

    /// A switch for a sharing setting in the draft.
    fn toggle(
        &self,
        id: &'static str,
        on: bool,
        locked: bool,
        palette: Palette,
        cx: &mut Context<Self>,
        change: fn(&mut Sharing),
    ) -> Stateful<Div> {
        ui::switch(id, on, locked, palette).when(!locked, |switch| {
            switch.on_click(cx.listener(move |this, _, _, cx| this.edit_sharing(change, cx)))
        })
    }

    fn render_chats(&self, settings: &Settings, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let manage = self.can_manage();
        let locked = !manage || self.busy.is_some();
        let retention_detail = format!(
            "Older messages are deleted for everyone. 1 to 3650 days{}. Saves on Enter.",
            settings
                .default_retention
                .map(|days| format!("; {days} by default"))
                .unwrap_or_default()
        );
        let mut column =
            v_flex()
                .child(ui::section_head("Chats and agents", palette))
                .child(self.ctx.settings_fields.field(
                    "retention-days",
                    retention_setting(&self.retention, locked, retention_detail, palette),
                    Some(self.retention.read(cx).focus_handle(cx)),
                    cx,
                ));
        if let Some(engine) = &settings.search_engine {
            let on = engine != "off";
            let mut detail = WEB_SEARCH_DETAIL.to_owned();
            if on && settings.search_held_back {
                detail.push(' ');
                detail.push_str(HELD_BACK);
            }
            let held_back = settings.search_held_back;
            column = column.child(self.ctx.settings_fields.field(
                "web-search",
                web_search_setting(
                    detail,
                    ui::switch("general-web-search", on, locked, palette).when(!locked, |switch| {
                        switch.on_click(cx.listener(move |this, _, _, cx| {
                            let said = if on {
                                "Saved. Web search is off.".to_string()
                            } else if held_back {
                                format!("Saved. {HELD_BACK}")
                            } else {
                                "Saved. People may turn on web search in a chat.".to_string()
                            };
                            this.put_settings(
                                Part::Chats,
                                json!({ "webSearch.engine": if on { "off" } else { "borrowed" } }),
                                said,
                                cx,
                            );
                        }))
                    }),
                    palette,
                ),
                None,
                cx,
            ));
            if on && settings.search_model_supported {
                let current = settings.search_model.clone().unwrap_or_else(|| "default".into());
                let options = search_options(self.models.as_deref(), &current);
                let chips = h_flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(options.into_iter().enumerate().map(|(ix, (value, label))| {
                        let picked = value == current;
                        ui::chip(
                            SharedString::from(format!("general-search-model-{ix}")),
                            label,
                            picked,
                            palette,
                        )
                        .when(!locked && !picked, |chip| {
                            chip.on_click(cx.listener(move |this, _, _, cx| {
                                let said = match value.as_str() {
                                    "default" => "Saved. Each chat's own model runs its search.",
                                    "cheapest" => "Saved. The cheapest model that can search runs every chat's search.",
                                    _ => "Saved. This model runs every chat's search.",
                                };
                                let model = if value == "default" { Value::Null } else { json!(value) };
                                this.put_settings(Part::Chats, json!({ "webSearch.model": model }), said.into(), cx);
                            }))
                        })
                    }));
                column = column.child(
                    v_flex()
                        .gap(px(8.))
                        .py(px(12.))
                        .child(ui::setting_lines(
                            "Model that searches",
                            "By default, each chat's own model runs its search. A model picked here runs every chat's search; a person whose policy denies it gets none.",
                            palette,
                        ))
                        .child(self.ctx.settings_fields.field("web-search-model-row", chips, None, cx)),
                );
            }
        }
        if settings.search_engine.is_none() {
            column = column.child(self.ctx.settings_fields.field(
                "web-search",
                ui::note("Web search controls need a newer Nexus Gateway.", palette),
                None,
                cx,
            ));
        }
        if settings.search_engine.as_deref() == Some("off")
            || !settings.search_model_supported
            || settings.search_engine.is_none()
        {
            column = column.child(self.ctx.settings_fields.field(
                "web-search-model-row",
                ui::note(
                    "The search model follows Gateway support and requires web search to be on.",
                    palette,
                ),
                None,
                cx,
            ));
        }
        let agents_on = settings.agent_subscriptions;
        column
            .child(self.ctx.settings_fields.field(
                "personal-agent-plans",
                personal_plans_setting(
                    ui::switch("general-agent-plans", agents_on, locked, palette).when(!locked, |switch| {
                        switch.on_click(cx.listener(move |this, _, _, cx| {
                            let said = if agents_on {
                                "Saved. Agents on personal plans are refused."
                            } else {
                                "Saved. People may run agents on their own plans."
                            };
                            this.put_settings(
                                Part::Chats,
                                json!({ "agentSubscriptions": !agents_on }),
                                said.into(),
                                cx,
                            );
                        }))
                    }),
                    palette,
                ),
                None,
                cx,
            ))
            .children(outcome(&self.chats_note, palette))
            .when(!manage, |column| column.child(ui::note(READ_ONLY, palette).pt(px(8.))))
            .into_any_element()
    }

    fn render_review(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let head = || {
            v_flex().child(ui::section_head("Code review", palette)).child(ui::note(
                "What automated reviewers, such as Nexus Review, may post on your pull requests.",
                palette,
            ))
        };
        let review = match &self.review {
            Load::Loading | Load::Ready(None) => return None,
            Load::Failed(error) => {
                return Some(
                    head()
                        .child(ui::error_line(
                            format!("Code review settings didn't load: {error}"),
                            palette,
                        ))
                        .into_any_element(),
                );
            }
            Load::Ready(Some(review)) => review,
        };
        let editable = matches!(&self.org, Load::Ready(org) if org.can("policy.manage"));
        let locked = !editable || self.busy.is_some();
        let hint = REVIEW_LEVELS
            .iter()
            .find(|(id, ..)| *id == review.level)
            .unwrap_or(&REVIEW_LEVELS[0])
            .2;
        let detail = format!(
            "{hint}{} A reviewer's own check and posting mode still apply.",
            if review.default { " This is the default." } else { "" }
        );
        let chips = h_flex()
            .flex_wrap()
            .gap(px(6.))
            .children(REVIEW_LEVELS.into_iter().map(|(id, label, _)| {
                let picked = review.level == id;
                ui::chip(
                    SharedString::from(format!("general-review-{id}")),
                    label,
                    picked,
                    palette,
                )
                .when(!locked && !picked, |chip| {
                    chip.on_click(cx.listener(move |this, _, _, cx| this.save_review(id, cx)))
                })
            }));
        Some(
            head()
                .child(
                    v_flex()
                        .gap(px(8.))
                        .py(px(12.))
                        .child(ui::setting_lines("Approvals and requests for changes", detail, palette))
                        .child(chips),
                )
                .children(outcome(&self.review_note, palette))
                .when(!editable, |column| {
                    column.child(ui::note("Only owners and admins can change this.", palette).pt(px(8.)))
                })
                .into_any_element(),
        )
    }
}

impl Render for GeneralPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.filled && matches!(self.settings, Load::Ready(_)) {
            self.fill(window, cx);
        }
        let palette = palette(cx);
        if !self.current() {
            return ui::empty(
                "Sign-in changed",
                "Open General again for your current account.",
                palette,
            )
            .into_any_element();
        }
        if self.policy_only {
            return match &self.settings {
                Load::Ready(settings) => div().w_full().child(self.render_chats(settings, palette, cx)),
                Load::Loading => div().w_full().child(ui::skeleton(3, palette)),
                Load::Failed(error) => div().w_full().child(ui::failed(error.clone(), palette)).child(
                    ui::button("policy-settings-retry", "Retry", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                ),
            }
            .into_any_element();
        }
        let mut page = v_flex()
            .w_full()
            .child(ui::lead(
                "Your organization's name, who can open what you share, and how long chats are kept.",
                palette,
            ))
            .child(ui::settings_notice("Identity and branding", "Manage your organization’s name, logo and address in Nexus UI. Other supported settings stay editable here.", palette).mb(px(12.)))
            .child(self.render_org(palette, cx))
            .child(self.ctx.settings_fields.field("synced-sessions", ui::settings_notice("Synced coding sessions", "Manage content sync, encryption and recovery keys in your browser.", palette)
                .child(ui::web_link("general-sync-browser", "Manage session sync", palette).on_click({
                    let ctx = self.ctx.clone(); move |_, _, cx| ctx.open_web("settings/general#synced-sessions", cx)
                })), None, cx));
        match &self.settings {
            Load::Loading => {
                page = page
                    .child(ui::section_head("Sharing outside", palette))
                    .child(ui::skeleton(4, palette));
            }
            Load::Failed(error) => {
                page = page.child(
                    v_flex().items_center().child(ui::failed(error.clone(), palette)).child(
                        ui::button("general-retry", "Try again", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                    ),
                );
            }
            Load::Ready(settings) => {
                let settings = settings.clone();
                page = page
                    .child(self.ctx.settings_fields.field(
                        "sharing-settings",
                        self.render_sharing(&settings, palette, cx),
                        None,
                        cx,
                    ))
                    .child(self.render_chats(&settings, palette, cx));
            }
        }
        page.children(
            self.render_review(palette, cx)
                .map(|body| self.ctx.settings_fields.field("review-events", body, None, cx)),
        )
        .into_any_element()
    }
}

const WEB_SEARCH_DETAIL: &str =
    "Lets people turn on web search in a chat. A search sends the query to the provider of the model that runs it.";

fn web_search_setting(detail: impl Into<SharedString>, control: impl IntoElement, palette: Palette) -> Div {
    ui::setting("Web search", detail, control, palette)
}

fn personal_plans_setting(control: impl IntoElement, palette: Palette) -> Div {
    ui::setting(
        "Agents on personal plans",
        "Lets people run nexus claude and nexus codex on their own Claude or ChatGPT plan. Gateway reports their usage but does not check models or budgets.",
        control,
        palette,
    )
}

pub(super) fn locked_chat_controls(palette: Palette) -> Div {
    v_flex()
        .child(web_search_setting(
            WEB_SEARCH_DETAIL,
            ui::switch("general-web-search", false, true, palette),
            palette,
        ))
        .child(personal_plans_setting(
            ui::switch("general-agent-plans", false, true, palette),
            palette,
        ))
}

fn retention_setting(input: &Entity<InputState>, locked: bool, detail: String, palette: Palette) -> Div {
    ui::setting(
        "Chat retention",
        detail,
        h_flex()
            .gap(px(6.))
            .text_size(px(13.))
            .text_color(palette.faint)
            .child("Keep for")
            .child(field(input, Some(72.), locked, palette))
            .child("days"),
        palette,
    )
}

/// A text field in a soft box, as the API keys section's.
fn field(input: &Entity<InputState>, width: Option<f32>, disabled: bool, palette: Palette) -> Div {
    h_flex()
        .h(px(32.))
        .px(px(10.))
        .rounded(px(8.))
        .bg(palette.hover)
        .text_size(px(13.))
        .text_color(palette.foreground)
        .map(|field| match width {
            Some(width) => field.w(px(width)),
            None => field.flex_1().min_w(px(0.)),
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .child(Input::new(input).appearance(false).disabled(disabled)),
        )
}

fn outcome(note: &Option<Outcome>, palette: Palette) -> Option<Div> {
    note.as_ref().map(|note| {
        if note.error {
            ui::error_line(note.text.clone(), palette).pt(px(8.))
        } else {
            ui::note(note.text.clone(), palette).pt(px(8.))
        }
    })
}

/// Agents on personal plans was saved on: what waits on it resumes (`setup::plans_off`), such as a New session's
/// start on the plan. The switch was just pressed, so the active window is the one that waits.
fn plans_turned_on(cx: &mut App) {
    cx.defer(|cx| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let key = crate::setup::plans_off::key();
        if let Err(error) = window.update(cx, |_, window, cx| crate::setup::done(&key, window, cx)) {
            eprintln!("nexus-desktop-gpui: plans on: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    // The glob import of gpui_kit brings its own `test` macro: name the built-in one.
    use core::prelude::v1::test;

    #[test]
    fn a_cached_form_baseline_survives_fill_but_detects_unsaved_edits() {
        let mut settings = parse_settings(&serde_json::json!({})).expect("settings");
        let mut sharing = Sharing::read(&serde_json::json!({}));
        sharing.allow = vec!["saved.example".into()];
        sharing.max_days = Some(14);
        settings.sharing = Some(sharing);
        settings.retention_days = Some(30);
        let baseline = settings_form_snapshot(None, Default::default(), false, Some(&settings));
        assert_eq!(baseline.1, ["saved.example", "", "14", "30"].map(str::to_string));
        assert_eq!(
            settings_form_snapshot(None, baseline.1.clone(), true, Some(&settings)),
            baseline
        );
        let mut edited = baseline.1.clone();
        edited[0] = "unsaved.example".into();
        settings.retention_days = Some(7);
        let current = settings_form_snapshot(None, edited.clone(), true, Some(&settings));
        assert_ne!(current, baseline, "a changed server reply must not reset this form");
        assert_eq!(current.1, edited);
    }

    /// A sample answer, written as Gateway sends it.
    fn value(text: &str) -> Value {
        serde_json::from_str(text).expect("JSON")
    }

    #[test]
    fn reads_the_organization_and_its_settings() {
        let me = value(
            r#"{
            "tenant": { "id": "ten_1", "name": "Acme", "slug": "acme", "kind": "company", "createdAt": "2026-01-01T00:00:00Z" },
            "role": "admin",
            "permissions": ["org.read", "org.settings"]
        }"#,
        );
        let org = parse_org(&me).unwrap();
        assert_eq!(org.segment, "acme");
        assert!(org.can("org.settings") && !org.can("policy.manage") && !org.personal);

        let answer = value(
            r#"{
            "chatRetentionDays": 30,
            "agentSubscriptions": true,
            "defaults": { "chatRetentionDays": 90, "agentSubscriptions": false },
            "artifacts.external": "named",
            "artifacts.external.who": "admins",
            "artifacts.external.code": false,
            "artifacts.external.domains": { "allow": ["acme.com"], "deny": [] },
            "artifacts.external.expiry": { "maxDays": 30, "required": true },
            "artifacts.external.continue": true,
            "artifacts.pages": true,
            "artifacts.pages.public": false,
            "webSearch.engine": "borrowed",
            "webSearch.model": null,
            "webSearch.domains": { "allow": [], "deny": [] },
            "webSearch.maxPerTurn": 3,
            "webSearch.requireZdr": false
        }"#,
        );
        let settings = parse_settings(&answer).unwrap();
        assert_eq!(settings.retention_days, Some(30));
        assert_eq!(settings.default_retention, Some(90));
        assert!(settings.agent_subscriptions && settings.pages_supported && settings.search_model_supported);
        assert!(!settings.search_held_back);
        assert_eq!(settings.search_model, None);
        let sharing = settings.sharing.unwrap();
        assert_eq!(sharing.external, "named");
        assert_eq!(sharing.allow, vec!["acme.com"]);
        assert_eq!(sharing.max_days, Some(30));

        let mut next = sharing.clone();
        next.allow = domain_list("@Acme.com, example.org;");
        next.pages = false;
        let changes = changed(&sharing, &next, true);
        assert_eq!(changes.len(), 2);
        assert_eq!(
            changes["artifacts.external.domains"],
            value(r#"{ "allow": ["acme.com", "example.org"], "deny": [] }"#)
        );
        assert_eq!(saved_text(&changes), "Saved. Pages no longer open, shared or not.");
    }

    #[test]
    fn an_older_gateway_has_no_sharing() {
        let settings = parse_settings(&value(
            r#"{ "chatRetentionDays": 90, "agentSubscriptions": false, "defaults": {} }"#,
        ))
        .unwrap();
        assert!(settings.sharing.is_none() && settings.search_engine.is_none());
    }

    #[test]
    fn reads_review_events_and_search_models() {
        let review = parse_review(&value(r#"{ "level": "comment", "default": true }"#)).unwrap();
        assert_eq!(review.level, "comment");
        assert!(review.default);

        let models = parse_models(&value(
            r#"{ "data": [
            { "id": "a/cheap", "name": "Cheap", "allowed": true, "web_search_usd": 0.01 },
            { "id": "b/denied", "name": "Denied", "allowed": false, "web_search_usd": 0.02 },
            { "id": "c/nosearch", "name": "No search", "allowed": true }
        ] }"#,
        ));
        let options = search_options(Some(&models), "b/denied");
        let values: Vec<_> = options.iter().map(|(value, _)| value.as_str()).collect();
        assert_eq!(values, ["default", "cheapest", "a/cheap", "b/denied"]);
        assert_eq!(options[3].1, "Denied (your policy denies it)");
    }

    #[test]
    fn days_and_refusals() {
        assert_eq!(days("30"), Some(30));
        assert_eq!(days("0"), None);
        assert_eq!(days("3651"), None);
        assert_eq!(days("1e3"), None);
        assert_eq!(
            plain("only the org's owner or admins can change settings (org.settings)"),
            READ_ONLY
        );
        assert_eq!(
            plain("Nexus Gateway did not answer in time"),
            "Nexus Gateway did not answer in time"
        );
    }
}
