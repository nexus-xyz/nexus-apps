//! Overview (`ui/src/app/app/[org]/overview/page.tsx`): the organization's gateway today. Four figures (the budget,
//! today's calls against yesterday's, the error rate and the agents working now), the live routes from callers through
//! Gateway to the models, the Finish setup checklist, the newest agent sessions and the products strip. It reads again
//! every 30 seconds, as the web does.
//!
//! The routes are drawn as three columns (callers, Gateway, models) instead of the web's curves. The products open in
//! the browser, since their pages are not drawn here. Dismissing Finish setup lasts until the app closes.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

use super::usage::{
    Me, Report, Spend, amount, budget_text, card_head, cents, encode, hue, iso_day, kpi, load_of, meter, month_name,
    next_month, percent, query, share, short_date, short_model, today,
};
use super::{Ctx, Load, PageId, palette, ui};
use crate::library::chrono_lite;
use crate::palette::Palette;

/// Whether the app draws this page (true) or opens it in the browser.
pub const NATIVE: bool = true;

/// How often the figures are read again.
const LIVE: Duration = Duration::from_secs(30);

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let _ = window;
    let view = cx.new(|cx| OverviewPage::new(ctx, cx));
    Some(view.into())
}

/* ------------------------------------------------------------------ what it reads */

/// One call of `GET /v1/spend/calls`, as much as the routes need.
#[derive(Debug, Clone, PartialEq)]
struct RouteCall {
    at: i64,
    status: String,
    product: String,
    agent_session_id: Option<String>,
    user_id: Option<String>,
    model: Option<String>,
    cost: f64,
    overspend_level: Option<String>,
}

fn text_of(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(String::from)
}

fn calls_of(value: &Value) -> Option<Vec<RouteCall>> {
    Some(
        value
            .get("calls")?
            .as_array()?
            .iter()
            .filter_map(|call| {
                Some(RouteCall {
                    at: chrono_lite::parse(&text_of(call, "at")?)?,
                    status: text_of(call, "status").unwrap_or_else(|| "settled".into()),
                    product: text_of(call, "product").unwrap_or_default(),
                    agent_session_id: text_of(call, "agentSessionId"),
                    user_id: text_of(call, "userId"),
                    model: text_of(call, "model"),
                    cost: text_of(call, "costUsd").and_then(|c| c.parse().ok()).unwrap_or(0.),
                    overspend_level: text_of(call, "overspendLevel"),
                })
            })
            .collect(),
    )
}

/// One agent session (`GET /v1/agents/sessions`).
#[derive(Debug, Clone, PartialEq)]
struct Session {
    id: String,
    agent: String,
    model: String,
    via: String,
    access: Option<String>,
    person: String,
    status: String,
    started_at: String,
    duration_ms: Option<u64>,
    estimated: Option<f64>,
}

impl Session {
    /// A coding agent by its entry's name; Nexus's own agent, and an id no agent has, as "Agent".
    fn agent_name(&self) -> &'static str {
        nexus_client::agent_entries::Agent::parse(&self.agent).map_or("Agent", |agent| agent.entry().identity.name)
    }

    /// Who paid, in a word: "Their plan", "Org key", "Nexus credit", "Gateway".
    fn access_word(&self) -> &'static str {
        if self.via == "subscription" {
            return "Their plan";
        }
        match (self.access.as_deref(), self.via.as_str()) {
            (Some("byok"), _) => "Org key",
            (Some("nexus"), _) => "Nexus credit",
            (_, "nexus") => "Gateway",
            _ => "–",
        }
    }

    fn running(&self) -> bool {
        self.status == "running"
    }
}

fn sessions_of(value: &Value) -> Option<Vec<Session>> {
    Some(
        value
            .get("sessions")?
            .as_array()?
            .iter()
            .filter_map(|s| {
                Some(Session {
                    id: text_of(s, "id")?,
                    agent: text_of(s, "agent").unwrap_or_default(),
                    model: text_of(s, "model").unwrap_or_default(),
                    via: text_of(s, "via").unwrap_or_default(),
                    access: text_of(s, "access"),
                    person: text_of(s, "person").unwrap_or_else(|| "Someone".into()),
                    status: text_of(s, "status").unwrap_or_else(|| "stopped".into()),
                    started_at: text_of(s, "startedAt").unwrap_or_default(),
                    duration_ms: s.get("durationMs").and_then(Value::as_f64).map(|d| d.max(0.) as u64),
                    estimated: s.get("usage").and_then(|u| amount(u, "estimatedCostUsd")),
                })
            })
            .collect(),
    )
}

/// A run's length: "45 s", "18 min", "1 h 05 min".
fn run_length(ms: u64) -> String {
    let seconds = (ms as f64 / 1000.).round() as u64;
    if seconds < 60 {
        return format!("{seconds} s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes} min");
    }
    format!("{} h {:02} min", minutes / 60, minutes % 60)
}

/// The organization's products (`GET /v1/modules`): (slug, name, first description), and the modules not answering.
#[derive(Debug, Clone, PartialEq)]
struct Products {
    products: Vec<(String, String, String)>,
    down: usize,
}

fn products_of(value: &Value) -> Option<Products> {
    let mut products: Vec<(String, String, String)> = Vec::new();
    for module in value.get("modules")?.as_array()? {
        let Some(product) = module.get("product") else { continue };
        let Some(slug) = text_of(product, "slug") else { continue };
        if products.iter().any(|(s, ..)| *s == slug) {
            continue;
        }
        let name = text_of(product, "name").unwrap_or_else(|| slug.clone());
        let description = module
            .get("capability")
            .and_then(|c| text_of(c, "description"))
            .unwrap_or_default();
        products.push((slug, name, description));
    }
    let down = value
        .get("unavailable")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Some(Products { products, down })
}

/// The products' short name and one line, as Nexus UI's `PRODUCT_COPY` says them.
fn product_copy(slug: &str, name: &str, description: &str) -> (String, String) {
    match slug {
        "core" => (
            "Gateway".into(),
            "The control plane of your factory: any model through one gateway, on your own keys, under one set of controls.".into(),
        ),
        "robustness" => ("Robustness".into(), "Every pull request reviewed against your own rules, on your own models.".into()),
        _ => (name.trim_start_matches("Nexus ").to_string(), description.to_string()),
    }
}

/// What Finish setup checks: each read's answer, or none while it runs. A failed read counts as not done.
#[derive(Debug, Clone, Default, PartialEq)]
struct Setup {
    keys: Option<bool>,
    cli: Option<bool>,
    github: Option<bool>,
    team: Option<bool>,
}

impl Setup {
    fn known(&self) -> bool {
        self.keys.is_some() && self.cli.is_some() && self.github.is_some() && self.team.is_some()
    }
}

fn setup_keys(value: &Value) -> bool {
    value
        .get("providers")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .any(|p| p.get("unused").and_then(Value::as_bool) != Some(true))
        })
        .unwrap_or(false)
}

fn setup_cli(value: &Value) -> bool {
    value
        .get("tokens")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .any(|t| t.get("kind").and_then(Value::as_str) == Some("cli"))
        })
        .unwrap_or(false)
}

fn listed(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_array).map(Vec::len).unwrap_or(0)
}

/* ------------------------------------------------------------------ routes */

#[derive(Debug, Clone, PartialEq)]
struct Caller {
    key: String,
    label: String,
    calls: u64,
    people: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct RouteModel {
    key: String,
    label: String,
    calls: u64,
    cost: f64,
    /// Route hue 1 to 3; none is the grey Other line.
    route: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
struct Blocked {
    label: String,
    calls: u64,
    model: Option<String>,
    /// The row of Usage › Limits the label opens: the one limit that stopped the calls, else the Limits tab.
    row: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
struct Routes {
    callers: Vec<Caller>,
    models: Vec<RouteModel>,
    blocked: Option<Blocked>,
    calls: u64,
}

/// Model rows drawn, Other included.
const MODEL_ROWS: usize = 5;
/// Usage's Limits tab (Settings › Budgets' target), for blocked calls with no one limit.
const LIMITS_TAB: &str = "usage-limits";

/// Who made a call, in a word: the agent behind its session, else the product.
fn caller_of(call: &RouteCall, agents: &HashMap<String, String>) -> (String, String) {
    let pair = |key: &str, label: &str| (key.to_string(), label.to_string());
    match call.product.as_str() {
        "agents" => match call
            .agent_session_id
            .as_ref()
            .and_then(|s| agents.get(s))
            .and_then(|id| nexus_client::agent_entries::Agent::parse(id))
        {
            // A coding agent here has a route through Gateway (Claude Code, Codex). Nexus's own agent is no coding agent,
            // so its calls read as "Coding agents".
            Some(agent) => pair(agent.as_str(), agent.entry().identity.name),
            None => pair("agents", "Coding agents"),
        },
        "chat" => pair("chat", "Chat"),
        "core" => pair("api", "API"),
        other => {
            let mut chars = other.chars();
            let label = chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default();
            (other.to_string(), label)
        }
    }
}

/// The busiest models with their hue, the rest folded into Other (one leftover keeps its own name).
fn rank_models(mut models: Vec<RouteModel>) -> Vec<RouteModel> {
    models.sort_by(|a, b| b.calls.cmp(&a.calls).then(b.cost.total_cmp(&a.cost)));
    let rest = models.split_off(models.len().min(MODEL_ROWS - 1));
    let mut out: Vec<RouteModel> = models
        .into_iter()
        .enumerate()
        .map(|(n, model)| RouteModel {
            route: (n < 3).then_some(n + 1),
            ..model
        })
        .collect();
    match rest.len() {
        0 => {}
        1 => out.extend(rest.into_iter().map(|model| RouteModel { route: None, ..model })),
        n => out.push(RouteModel {
            key: "__other".into(),
            label: format!("Other ({n})"),
            calls: rest.iter().map(|m| m.calls).sum(),
            cost: rest.iter().map(|m| m.cost).sum(),
            route: None,
        }),
    }
    out
}

/// The routes of the calls made at or after `since` (seconds). Refused calls are the blocked line; released ones are
/// left out.
fn routes_from_calls(calls: &[RouteCall], agents: &HashMap<String, String>, since: i64) -> Routes {
    let mut callers: Vec<(Caller, HashSet<String>)> = Vec::new();
    let mut models: Vec<RouteModel> = Vec::new();
    let mut blocked: Vec<(String, u64, &'static str)> = Vec::new();
    let mut refused_models: HashSet<String> = HashSet::new();
    let mut total = 0;
    for call in calls {
        if call.at < since || call.status == "released" {
            continue;
        }
        total += 1;
        let (key, label) = caller_of(call, agents);
        let at = match callers.iter().position(|(c, _)| c.key == key) {
            Some(at) => at,
            None => {
                callers.push((
                    Caller {
                        key,
                        label,
                        calls: 0,
                        people: 0,
                    },
                    HashSet::new(),
                ));
                callers.len() - 1
            }
        };
        callers[at].0.calls += 1;
        if let Some(user) = &call.user_id {
            callers[at].1.insert(user.clone());
        }
        if call.status == "refused" {
            let label = match call.overspend_level.as_deref() {
                Some("person") => "Person limit",
                Some("team") => "Team budget",
                Some("organization") => "Organization budget",
                _ => "Refused",
            };
            let row = call
                .overspend_level
                .as_deref()
                .and_then(crate::limit_stop::Level::wire)
                .map_or(LIMITS_TAB, crate::limit_stop::Level::row);
            match blocked.iter_mut().find(|(l, _, _)| l == label) {
                Some(entry) => entry.1 += 1,
                None => blocked.push((label.to_string(), 1, row)),
            }
            refused_models.insert(call.model.clone().unwrap_or_default());
            continue;
        }
        let Some(model) = &call.model else { continue };
        match models.iter_mut().find(|m| &m.key == model) {
            Some(entry) => {
                entry.calls += 1;
                entry.cost += call.cost;
            }
            None => models.push(RouteModel {
                key: model.clone(),
                label: short_model(model).to_string(),
                calls: 1,
                cost: call.cost,
                route: None,
            }),
        }
    }
    let mut callers: Vec<Caller> = callers
        .into_iter()
        .map(|(caller, who)| Caller {
            people: who.len(),
            ..caller
        })
        .collect();
    callers.sort_by(|a, b| b.calls.cmp(&a.calls));
    blocked.sort_by(|a, b| b.1.cmp(&a.1));
    let blocked = (!blocked.is_empty()).then(|| Blocked {
        label: if blocked.len() == 1 {
            blocked[0].0.clone()
        } else {
            "Limits".into()
        },
        calls: blocked.iter().map(|(_, n, _)| n).sum(),
        row: if blocked.len() == 1 { blocked[0].2 } else { LIMITS_TAB },
        model: (refused_models.len() == 1)
            .then(|| refused_models.iter().next().cloned())
            .flatten()
            .filter(|m| !m.is_empty())
            .map(|m| short_model(&m).to_string()),
    });
    Routes {
        callers,
        models: rank_models(models),
        blocked,
        calls: total,
    }
}

/// The routes from today's usage by product and by model: what a person who can't read the organization's calls sees.
fn routes_from_usage(by_product: &Report, by_model: &Report) -> Routes {
    let caller_label = |key: &str, label: &str| -> String {
        let base = key.split('/').next().unwrap_or(key);
        match base {
            "agents" => "Coding agents".into(),
            "chat" => "Chat".into(),
            "core" => "API".into(),
            _ => label.to_string(),
        }
    };
    let mut callers: Vec<Caller> = by_product
        .groups
        .iter()
        .filter(|g| g.figures.calls > 0)
        .map(|g| Caller {
            key: g.key.clone(),
            label: caller_label(&g.key, &g.label),
            calls: g.figures.calls,
            people: 0,
        })
        .collect();
    callers.sort_by(|a, b| b.calls.cmp(&a.calls));
    let models = by_model
        .groups
        .iter()
        .filter(|g| g.figures.calls > 0)
        .map(|g| RouteModel {
            key: g.key.clone(),
            label: short_model(&g.label).to_string(),
            calls: g.figures.calls,
            cost: g.figures.spent,
            route: None,
        })
        .collect();
    Routes {
        callers,
        models: rank_models(models),
        blocked: None,
        calls: by_model.totals.calls,
    }
}

/// "+18% vs. yesterday", or plain words when yesterday had none.
fn versus(today: u64, yesterday: u64) -> String {
    if yesterday == 0 {
        return if today == 0 {
            "None yesterday either".into()
        } else {
            "None yesterday".into()
        };
    }
    let change = ((today as f64 - yesterday as f64) / yesterday as f64 * 100.).round() as i64;
    match change {
        0 => "Same as yesterday".into(),
        c if c > 0 => format!("+{c}% vs. yesterday"),
        c => format!("−{}% vs. yesterday", c.abs()),
    }
}

/// An error rate as the hero shows it: one decimal under 10%.
fn rate_text(rate: f64) -> String {
    let value = rate * 100.;
    if rate < 0.1 {
        format!("{value:.1}%")
    } else {
        format!("{}%", value.round())
    }
}

fn calls_word(n: u64) -> String {
    format!("{} {}", ui::count(n), if n == 1 { "call" } else { "calls" })
}

/* ------------------------------------------------------------------ the page */

/// Replace a figure with a new answer, but keep the last good one when a refresh fails.
fn settle<T>(slot: &mut Load<T>, next: Load<T>) {
    if matches!((&*slot, &next), (Load::Ready(_), Load::Failed(_))) {
        return;
    }
    *slot = next;
}

struct OverviewPage {
    ctx: Ctx,
    me: Load<Me>,
    spend: Load<Spend>,
    /// Usage by product over yesterday and today: the calls figure.
    two_days: Load<Report>,
    /// Usage by model today: the error rate, and the models for a member's routes.
    today_models: Load<Report>,
    /// Usage by product today: the callers for a member's routes.
    today_products: Load<Report>,
    /// The organization's latest calls, for owners and admins; none for anyone else.
    calls: Option<Load<Vec<RouteCall>>>,
    /// When the calls came back (seconds), so "the last hour" counts from there.
    calls_at: i64,
    sessions: Load<Vec<Session>>,
    products: Load<Products>,
    setup: Setup,
    dismissed: bool,
    _poll: Task<()>,
}

impl OverviewPage {
    fn new(ctx: Ctx, cx: &mut Context<Self>) -> OverviewPage {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(crate::wakeups::pace(LIVE, cx)).await;
                if this.update(cx, |page, cx| page.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        let mut page = OverviewPage {
            ctx,
            me: Load::Loading,
            spend: Load::Loading,
            two_days: Load::Loading,
            today_models: Load::Loading,
            today_products: Load::Loading,
            calls: None,
            calls_at: 0,
            sessions: Load::Loading,
            products: Load::Loading,
            setup: Setup::default(),
            dismissed: false,
            _poll: poll,
        };
        page.load_once(cx);
        page.refresh(cx);
        page
    }

    fn get(
        &self,
        path: String,
        cx: &mut Context<Self>,
        mut apply: impl FnMut(&mut Self, Result<Value, String>) + 'static,
    ) {
        super::read(cx, self.ctx.gateway.query(path), move |this, answer, _| {
            apply(this, answer)
        });
    }

    /// What does not change from one refresh to the next: who you are, the products and the setup checks.
    fn load_once(&mut self, cx: &mut Context<Self>) {
        self.get("/v1/tenants/me".into(), cx, |this, a| {
            this.me = load_of(a, Me::parse);
        });
        self.get("/v1/modules".into(), cx, |this, a| {
            this.products = load_of(a, products_of)
        });
        self.get("/v1/keys/providers".into(), cx, |this, a| {
            this.setup.keys = Some(a.map(|v| setup_keys(&v)).unwrap_or(false))
        });
        self.get("/v1/auth/tokens".into(), cx, |this, a| {
            this.setup.cli = Some(a.map(|v| setup_cli(&v)).unwrap_or(false))
        });
        self.get("/v1/github/repositories".into(), cx, |this, a| {
            this.setup.github = Some(a.map(|v| listed(&v, "repositories") > 0).unwrap_or(false))
        });
        self.get("/v1/orgs/members".into(), cx, |this, a| {
            this.setup.team = Some(a.map(|v| listed(&v, "members") > 1).unwrap_or(false))
        });
    }

    /// The live figures, read again every 30 seconds.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let today_iso = iso_day(today());
        let yesterday_iso = iso_day(today() - 1);
        let usage = |by: &str, from: &str| {
            format!(
                "/v1/analytics/usage{}",
                query(&[("groupBy", by), ("from", from), ("to", &today_iso)])
            )
        };
        self.get("/v1/spend".into(), cx, |this, a| {
            settle(&mut this.spend, load_of(a, Spend::parse))
        });
        self.get(usage("product", &yesterday_iso), cx, |this, a| {
            settle(&mut this.two_days, load_of(a, Report::parse))
        });
        self.get(usage("model", &today_iso), cx, |this, a| {
            settle(&mut this.today_models, load_of(a, Report::parse))
        });
        self.get(usage("product", &today_iso), cx, |this, a| {
            settle(&mut this.today_products, load_of(a, Report::parse))
        });
        self.get("/v1/agents/sessions".into(), cx, |this, a| {
            settle(&mut this.sessions, load_of(a, sessions_of))
        });
        self.refresh_calls(cx);
    }

    /// The organization's calls, only for who may read them (`spend.read_all`).
    fn refresh_calls(&mut self, cx: &mut Context<Self>) {
        match &self.me {
            Load::Ready(me) if me.can("spend.read_all") => {}
            Load::Loading => {
                // Ask again once Gateway says who this is.
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    this.update(cx, |page, cx| {
                        if !matches!(page.me, Load::Loading) {
                            page.refresh_calls(cx)
                        }
                    })
                    .ok();
                })
                .detach();
                return;
            }
            _ => return,
        }
        if self.calls.is_none() {
            self.calls = Some(Load::Loading);
        }
        self.get(
            format!("/v1/spend/calls{}", query(&[("limit", "50")])),
            cx,
            |this, a| {
                this.calls_at = chrono_lite::now();
                let next = load_of(a, calls_of);
                match &mut this.calls {
                    Some(slot) => settle(slot, next),
                    None => this.calls = Some(next),
                }
            },
        );
    }

    fn can(&self, permission: &str) -> bool {
        matches!(&self.me, Load::Ready(me) if me.can(permission))
    }

    fn agents(&self) -> HashMap<String, String> {
        match &self.sessions {
            Load::Ready(list) => list.iter().map(|s| (s.id.clone(), s.agent.clone())).collect(),
            _ => HashMap::new(),
        }
    }

    /// The routes to draw and their window: the last hour of calls, else the latest ones, else today's usage.
    fn routes(&self) -> Option<(Routes, &'static str)> {
        if let Some(Load::Ready(calls)) = &self.calls {
            let agents = self.agents();
            let hour = routes_from_calls(calls, &agents, self.calls_at - 3600);
            if hour.calls > 0 {
                return Some((hour, "Last hour"));
            }
            let latest = routes_from_calls(calls, &agents, 0);
            let span = if latest.calls > 0 { "Latest calls" } else { "Last hour" };
            return Some((latest, span));
        }
        match (&self.today_products, &self.today_models) {
            (Load::Ready(by_product), Load::Ready(by_model)) => {
                Some((routes_from_usage(by_product, by_model), "Today"))
            }
            _ => None,
        }
    }

    fn open(&self, page: PageId, window: &mut Window, cx: &mut App) {
        (self.ctx.navigate)(page, window, cx);
    }
}

/// What a member's Ask sends for a setup step they can't do.
fn ask_step(title: &str, org: &str) -> String {
    let org = match org.trim() {
        "" => "our organization",
        org => org,
    };
    format!("Could you do a Nexus setup step for {org}? It is \"{title}\".")
}

impl Render for OverviewPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let org = if self.ctx.org_name.is_empty() {
            "your organization".to_string()
        } else {
            self.ctx.org_name.clone()
        };
        let (outer, column) = ui::place(
            "overview-page",
            "Overview",
            format!("Every model call in {org}, through one gateway, on your own keys."),
            palette,
        );
        let routes = self.routes();
        let hues: HashMap<String, Option<usize>> = routes
            .as_ref()
            .map(|(r, _)| r.models.iter().map(|m| (m.key.clone(), m.route)).collect())
            .unwrap_or_default();
        let mut middle = h_flex()
            .items_start()
            .gap(px(16.))
            .child(self.render_routes(routes, palette, cx));
        if let Some(setup) = self.render_setup(palette, cx) {
            middle = middle.child(setup);
        }
        let body = v_flex()
            .gap(px(16.))
            .child(self.render_code_lead(palette, cx))
            .child(self.render_hero(palette))
            .child(middle)
            .child(self.render_sessions(&hues, palette, cx))
            .child(self.render_products(palette, cx));
        outer.child(column.child(body))
    }
}

impl OverviewPage {
    fn render_hero(&self, palette: Palette) -> Div {
        let wait = "…";
        let budget = match &self.spend {
            Load::Ready(spend) => match spend.budget {
                // A member's answer has no budget: what is left for them.
                None => kpi(
                    "Left for you",
                    cents(spend.remaining.max(0.)),
                    spend
                        .resets_at
                        .as_deref()
                        .map(|r| format!("Resets {}", short_date(r)))
                        .unwrap_or_else(|| "No period on your limit".into()),
                    palette,
                ),
                Some(budget) => {
                    let resets = spend.resets_at.clone().unwrap_or_else(next_month);
                    // The budget's month is the one before it resets.
                    let month = chrono_lite::parse(&resets)
                        .map(|s| iso_day(s.div_euclid(86_400) - 1))
                        .map(|d| month_name(&d))
                        .unwrap_or("This month");
                    let used = share(spend.spent, budget);
                    let card = kpi(
                        format!("Org budget · {month}"),
                        if budget > 0. {
                            format!("{} left of {}", cents(spend.remaining), budget_text(budget))
                        } else {
                            "No budget set".into()
                        },
                        if budget > 0. {
                            format!(
                                "{} used · {} · resets {}",
                                cents(spend.spent),
                                percent(used),
                                short_date(&resets)
                            )
                        } else {
                            format!("{} spent", cents(spend.spent))
                        },
                        palette,
                    );
                    if budget > 0. {
                        card.child(meter(
                            160.,
                            used,
                            0.,
                            if used >= 1. {
                                palette.destructive
                            } else {
                                palette.strong
                            },
                            palette,
                        ))
                    } else {
                        card
                    }
                }
            },
            Load::Failed(error) => kpi("Budget", "–", error.clone(), palette),
            Load::Loading => kpi("Budget", wait, wait, palette),
        };
        let calls = match &self.two_days {
            Load::Ready(report) => {
                let day = |d: i64| {
                    let iso = iso_day(d);
                    report
                        .series
                        .iter()
                        .find(|s| s.day == iso)
                        .map(|s| s.figures.calls)
                        .unwrap_or(0)
                };
                let (now, before) = (day(today()), day(today() - 1));
                kpi("Calls today", ui::count(now), versus(now, before), palette)
            }
            Load::Failed(error) => kpi("Calls today", "–", error.clone(), palette),
            Load::Loading => kpi("Calls today", wait, wait, palette),
        };
        let errors = match &self.today_models {
            Load::Ready(report) => kpi(
                "Error rate",
                report.totals.error_rate.map(rate_text).unwrap_or_else(|| "None".into()),
                format!(
                    "{} failed · {} refused",
                    ui::count(report.totals.failed),
                    ui::count(report.totals.refused)
                ),
                palette,
            ),
            Load::Failed(error) => kpi("Error rate", "–", error.clone(), palette),
            Load::Loading => kpi("Error rate", wait, wait, palette),
        };
        let agents = match &self.sessions {
            Load::Ready(list) => {
                let working: Vec<&Session> = list.iter().filter(|s| s.running()).collect();
                let by: Vec<String> = [("claude", "Claude Code"), ("codex", "Codex")]
                    .into_iter()
                    .filter_map(|(agent, name)| {
                        let n = working.iter().filter(|s| s.agent == agent).count();
                        (n > 0).then(|| format!("{n} {name}"))
                    })
                    .collect();
                kpi(
                    "Active agents",
                    ui::count(working.len() as u64),
                    if by.is_empty() {
                        "None working now".into()
                    } else {
                        by.join(" · ")
                    },
                    palette,
                )
            }
            Load::Failed(error) => kpi("Active agents", "–", error.clone(), palette),
            Load::Loading => kpi("Active agents", wait, wait, palette),
        };
        h_flex()
            .flex_wrap()
            .gap(px(10.))
            .child(budget)
            .child(calls)
            .child(errors)
            .child(agents)
    }

    fn render_routes(&self, routes: Option<(Routes, &'static str)>, palette: Palette, cx: &mut Context<Self>) -> Div {
        let span = routes.as_ref().map(|(_, s)| *s).unwrap_or("Last hour");
        let mut card = ui::card(palette).flex_1().min_w(px(0.)).child(
            card_head("Live routes", span, palette).child(
                ui::web_link("overview-open-usage", "Open in Usage", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.open(PageId::Usage, window, cx))),
            ),
        );
        let Some((routes, _)) = routes else {
            if let Some(Load::Failed(error)) = &self.calls {
                return card.child(ui::failed(error.clone(), palette));
            }
            if let Load::Failed(error) = &self.today_models {
                return card.child(ui::failed(error.clone(), palette));
            }
            return card.child(ui::skeleton(4, palette));
        };
        if routes.calls == 0 {
            return card.child(ui::note(
                "No model calls in this window. Calls from Claude Code, Codex and Chat show here as they happen.",
                palette,
            ));
        }
        let max = routes
            .callers
            .iter()
            .map(|c| c.calls)
            .chain(routes.models.iter().map(|m| m.calls))
            .chain(routes.blocked.iter().map(|b| b.calls))
            .max()
            .unwrap_or(1)
            .max(1);
        // A line's thickness by its calls: 1.25 to 4 px, so a quiet route still shows.
        let weight = |calls: u64| 1.25 + calls as f32 / max as f32 * 2.75;
        let line = |calls: u64, color: Hsla| div().w(px(36.)).h(px(weight(calls))).rounded(px(2.)).bg(color);
        let callers = v_flex()
            .flex_1()
            .min_w(px(0.))
            .gap(px(10.))
            .children(routes.callers.iter().map(|caller| {
                let sub = if caller.people > 1 {
                    format!("{} people", caller.people)
                } else {
                    calls_word(caller.calls)
                };
                h_flex()
                    .gap(px(8.))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .items_end()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(palette.strong)
                                    .truncate()
                                    .child(caller.label.clone()),
                            )
                            .child(div().text_size(px(11.)).text_color(palette.faint).child(sub)),
                    )
                    .child(line(caller.calls, palette.muted))
            }));
        let hub = v_flex()
            .w(px(96.))
            .flex_none()
            .items_center()
            .justify_center()
            .self_stretch()
            .child(
                div()
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(palette.raised)
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child("Gateway"),
            )
            .child(
                div()
                    .mt(px(6.))
                    .text_size(px(11.))
                    .text_color(palette.faint)
                    .child(calls_word(routes.calls)),
            );
        let mut models = v_flex()
            .flex_1()
            .min_w(px(0.))
            .gap(px(10.))
            .children(routes.models.iter().map(|model| {
                let color = hue(model.route.unwrap_or(0), palette);
                h_flex().gap(px(8.)).child(line(model.calls, color)).child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(palette.strong)
                                .truncate()
                                .child(model.label.clone()),
                        )
                        .child(div().text_size(px(11.)).text_color(palette.faint).child(format!(
                            "{} · {}",
                            calls_word(model.calls),
                            cents(model.cost)
                        ))),
                )
            }));
        if let Some(blocked) = &routes.blocked {
            let sub = match &blocked.model {
                Some(model) => format!("{} · asked for {model}", calls_word(blocked.calls)),
                None => calls_word(blocked.calls),
            };
            // A limit is amber, never red. The label opens the limit's row in Usage › Limits (`limit_stop.rs`).
            let fixer = self.ctx.fixer.clone();
            let row = blocked.row;
            models = models.child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        // Dashed, as the web draws a refused line.
                        h_flex().w(px(36.)).gap(px(3.)).children((0..4).map(|_| {
                            div()
                                .w(px(6.))
                                .h(px(weight(blocked.calls)))
                                .bg(palette.warning.opacity(0.7))
                        })),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                h_flex()
                                    .id("overview-blocked-limit")
                                    .gap(px(6.))
                                    .text_size(px(13.))
                                    .text_color(palette.strong)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(palette.foreground))
                                    .child(div().size(px(7.)).flex_none().rounded_full().bg(palette.warning))
                                    .child(format!("{} ›", blocked.label))
                                    .on_click(move |_, window, cx| {
                                        let fix = crate::setup::Fix::open_at(crate::pages::PageId::Usage, row);
                                        fixer.run("overview:blocked".into(), fix, window, cx);
                                    }),
                            )
                            .child(div().text_size(px(11.)).text_color(palette.faint).child(sub)),
                    ),
            );
        }
        card = card.child(
            h_flex()
                .items_center()
                .gap(px(8.))
                .py(px(8.))
                .child(callers)
                .child(hub)
                .child(models),
        );
        card
    }

    fn render_code_lead(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        ui::card(palette)
            .child(card_head("Start coding here", String::new(), palette))
            .child(
                h_flex().gap(px(16.)).items_center()
                    .child(v_flex().flex_1().min_w(px(0.)).gap(px(4.))
                        .child(div().text_size(px(14.)).text_color(palette.foreground).child("Run Claude Code in Nexus"))
                        .child(div().text_size(px(13.)).text_color(palette.muted)
                            .child("Open a repository. Approve what the agent runs and see each session's cost. You are already signed in.")))
                    .child(ui::button("overview-open-code", "Open Code", palette)
                        .on_click(cx.listener(|this, _, window, cx| (this.ctx.new_code_session)(window, cx)))),
            )
    }

    fn render_setup(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        if self.dismissed || !self.setup.known() {
            return None;
        }
        let personal = matches!(&self.me, Load::Ready(me) if me.personal);
        let s = &self.setup;
        // (key, title, done, command, (page, action, permission)).
        type Link = (PageId, &'static str, &'static str);
        let all: [(&str, &str, bool, Option<&str>, Option<Link>); 5] = [
            ("install", "Install the nexus CLI", s.cli == Some(true), None, None),
            (
                "cli",
                "Sign in with nexus auth login",
                s.cli == Some(true),
                Some("nexus auth login"),
                None,
            ),
            (
                "keys",
                "Add a provider key",
                s.keys == Some(true),
                None,
                Some((PageId::Providers, "Providers", "provider_keys.manage")),
            ),
            (
                "team",
                "Invite your team",
                s.team == Some(true),
                None,
                Some((PageId::Members, "Members", "members.invite")),
            ),
            (
                "github",
                "Link a repository",
                s.github == Some(true),
                None,
                Some((PageId::Repositories, "GitHub App", "repositories.manage")),
            ),
        ];
        let steps: Vec<_> = all
            .into_iter()
            .filter(|(key, ..)| !(personal && *key == "team"))
            .collect();
        let done = steps.iter().filter(|step| step.2).count();
        if done == steps.len() {
            return None;
        }
        let next = steps.iter().find(|step| !step.2).map(|step| step.0);
        let mut card = ui::card(palette).w(px(320.)).flex_none().child(
            card_head("Finish setup", format!("{done}/{}", steps.len()), palette).child(
                div()
                    .id("overview-setup-dismiss")
                    .cursor_pointer()
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Dismiss").build(window, cx))
                    .child(ui::icon(gpui_kit::assets::IconName::Close, palette.icon))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dismissed = true;
                        cx.notify();
                    })),
            ),
        );
        card = card.child(
            meter(
                288.,
                share(done as f64, steps.len() as f64),
                0.,
                palette.strong,
                palette,
            )
            .mb(px(8.)),
        );
        for (ix, (key, title, is_done, command, link)) in steps.into_iter().enumerate() {
            let mark = if is_done {
                ui::icon(gpui_kit::assets::IconName::CircleCheck, palette.strong)
            } else {
                ui::icon(gpui_kit::assets::IconName::Circle, palette.faint)
            };
            let action: AnyElement = match (is_done, command, link) {
                (true, ..) => div().into_any_element(),
                (false, Some(command), _) => div()
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(palette.hover)
                    .font_family("Geist Mono")
                    .text_size(px(11.))
                    .text_color(palette.foreground)
                    .child(command)
                    .into_any_element(),
                (false, None, Some((page, label, permission))) if self.can(permission) => h_flex()
                    .id(("overview-setup-go", ix))
                    .gap(px(4.))
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(palette.strong))
                    .child(label)
                    .child(ui::icon(gpui_kit::assets::IconName::ArrowRight, palette.icon))
                    .on_click(cx.listener(move |this, _, window, cx| this.open(page, window, cx)))
                    .into_any_element(),
                // Who can't do the step asks the owners and admins by name (`setup::access`).
                (false, None, Some((page, label, permission))) => {
                    crate::setup::Locked::new(format!("overview:{key}"), "")
                        .fix(label, crate::setup::Fix::open(page).needs(permission))
                        .ask(ask_step(title, &self.ctx.org_name))
                        .render(&self.ctx.fixer, palette, cx)
                        .into_any_element()
                }
                (false, None, None) => div().into_any_element(),
            };
            card = card.child(
                h_flex()
                    .gap(px(8.))
                    .py(px(6.))
                    .child(mark)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .text_color(if is_done {
                                palette.faint
                            } else if Some(key) == next {
                                palette.strong
                            } else {
                                palette.foreground
                            })
                            .when(is_done, |t| t.line_through())
                            .child(title),
                    )
                    .child(action),
            );
        }
        Some(card)
    }

    fn render_sessions(&self, hues: &HashMap<String, Option<usize>>, palette: Palette, cx: &mut Context<Self>) -> Div {
        let running = match &self.sessions {
            Load::Ready(list) => list.iter().filter(|s| s.running()).count(),
            _ => 0,
        };
        let mut card = ui::card(palette).child(
            card_head(
                "Agent sessions",
                if running > 0 {
                    format!("{running} working now")
                } else {
                    String::new()
                },
                palette,
            )
            .child(
                div()
                    .id("overview-sessions-usage")
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(palette.strong))
                    .child("All in Usage →")
                    .on_click(cx.listener(|this, _, window, cx| this.open(PageId::Usage, window, cx))),
            ),
        );
        let list = match &self.sessions {
            Load::Loading => return card.child(ui::skeleton(3, palette)),
            Load::Failed(error) => return card.child(ui::failed(error.clone(), palette)),
            Load::Ready(list) => list,
        };
        if list.is_empty() {
            return card.child(ui::empty(
                "No agent sessions yet",
                "Run nexus claude or nexus codex, and the session shows here while it works.",
                palette,
            ));
        }
        let mut shown: Vec<&Session> = list.iter().collect();
        shown.sort_by(|a, b| b.running().cmp(&a.running()).then(b.started_at.cmp(&a.started_at)));
        shown.truncate(5);
        // What each Gateway session's calls cost, from the page of calls read for the routes.
        let mut spent: HashMap<&str, f64> = HashMap::new();
        if let Some(Load::Ready(calls)) = &self.calls {
            for call in calls {
                if let Some(session) = &call.agent_session_id {
                    *spent.entry(session.as_str()).or_insert(0.) += call.cost;
                }
            }
        }
        let now = chrono_lite::now();
        let head = |w: f32, text: &'static str| div().w(px(w)).flex_none().child(text);
        card = card.child(
            ui::head_row(palette)
                .child(div().flex_1().child("Person"))
                .child(head(100., "Agent"))
                .child(head(160., "Model"))
                .child(head(100., "Access"))
                .child(head(90., "Status"))
                .child(head(90., "Duration").text_right())
                .child(head(80., "Cost").text_right()),
        );
        for session in shown {
            let took = match (session.duration_ms, session.running()) {
                (Some(ms), _) => run_length(ms),
                (None, true) => chrono_lite::parse(&session.started_at)
                    .map(|start| run_length(((now - start).max(0) * 1000) as u64))
                    .unwrap_or_else(|| "–".into()),
                _ => "–".into(),
            };
            let cost = if session.via == "subscription" {
                session
                    .estimated
                    .map(|e| format!("~{}", cents(e)))
                    .unwrap_or_else(|| "–".into())
            } else {
                spent
                    .get(session.id.as_str())
                    .map(|c| cents(*c))
                    .unwrap_or_else(|| "–".into())
            };
            let dot = match hues.get(&session.model) {
                Some(slot) => hue(slot.unwrap_or(0), palette),
                None => palette.faint,
            };
            let status = {
                let mut chars = session.status.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            };
            card = card.child(
                ui::row(palette)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(13.))
                            .text_color(palette.strong)
                            .child(session.person.clone()),
                    )
                    .child(
                        div()
                            .w(px(100.))
                            .flex_none()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .child(session.agent_name()),
                    )
                    .child(
                        div()
                            .w(px(160.))
                            .flex_none()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(palette.foreground)
                            .child(short_model(&session.model).to_string()),
                    )
                    .child(
                        div()
                            .w(px(100.))
                            .flex_none()
                            .child(ui::badge(session.access_word(), palette)),
                    )
                    .child(
                        h_flex()
                            .w(px(90.))
                            .flex_none()
                            .gap(px(6.))
                            .child(div().size(px(7.)).rounded_full().bg(if session.running() {
                                dot
                            } else {
                                palette.faint
                            }))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(if session.running() {
                                        palette.strong
                                    } else {
                                        palette.muted
                                    })
                                    .child(status),
                            ),
                    )
                    .child(
                        div()
                            .w(px(90.))
                            .flex_none()
                            .text_right()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .child(took),
                    )
                    .child(
                        div()
                            .w(px(80.))
                            .flex_none()
                            .text_right()
                            .text_size(px(13.))
                            .text_color(palette.strong)
                            .child(cost),
                    ),
            );
        }
        card
    }

    fn render_products(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let strip = h_flex().flex_wrap().gap(px(10.)).child(ui::label("Products", palette));
        match &self.products {
            Load::Loading => strip,
            Load::Failed(error) => strip.child(
                div()
                    .id("overview-products-failed")
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .tooltip({
                        let error = error.clone();
                        move |window, cx| gpui_kit::component::tooltip::Tooltip::new(error.clone()).build(window, cx)
                    })
                    .child("Could not load your products. Nothing you did was lost."),
            ),
            Load::Ready(products) if products.products.is_empty() && products.down == 0 => div(),
            Load::Ready(products) => {
                let mut strip = strip;
                for (ix, (slug, name, description)) in products.products.iter().enumerate() {
                    let (short, line) = product_copy(slug, name, description);
                    let path = format!("products/{}", encode(slug));
                    strip = strip.child(
                        h_flex()
                            .id(("overview-product", ix))
                            .flex_1()
                            .min_w(px(240.))
                            .gap(px(8.))
                            .px(px(12.))
                            .py(px(10.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .hover(|s| s.bg(palette.hover))
                            .child(ui::two_lines(short, line, palette).flex_1())
                            .child(ui::icon(gpui_kit::assets::IconName::ExternalLink, palette.icon))
                            .on_click(cx.listener(move |this, _, _, cx| this.ctx.open_web(&path, cx))),
                    );
                }
                for _ in 0..products.down {
                    strip = strip.child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.destructive)
                            .child("A module is not answering"),
                    );
                }
                strip
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The gpui_kit glob brings its own `test` macro; this is the standard one.
    use core::prelude::v1::test;
    use serde_json::json;

    fn call(at: &str, status: &str, product: &str, session: Option<&str>, model: Option<&str>, cost: &str) -> Value {
        json!({ "id": "c", "at": at, "status": status, "product": product, "capability": null, "userId": "u1",
                "agentSessionId": session, "model": model, "costUsd": cost, "overspendLevel": if status == "refused" { json!("person") } else { json!(null) } })
    }

    #[test]
    fn draws_routes_from_calls() {
        let page = json!({ "calls": [
            call("2026-11-05T10:00:00Z", "settled", "agents", Some("s1"), Some("anthropic/claude-opus-5.5"), "0.200000"),
            call("2026-11-05T10:01:00Z", "settled", "chat", None, Some("openai/gpt-6"), "0.100000"),
            call("2026-11-05T10:02:00Z", "refused", "agents", Some("s1"), Some("anthropic/claude-opus-5.5"), "0"),
            call("2026-11-05T10:03:00Z", "released", "core", None, None, "0"),
            call("2026-11-05T08:00:00Z", "settled", "core", None, Some("google/gemini-3"), "0.050000"),
        ], "nextCursor": null });
        let calls = calls_of(&page).unwrap();
        let agents = HashMap::from([("s1".to_string(), "claude".to_string())]);
        let since = chrono_lite::parse("2026-11-05T09:30:00Z").unwrap();
        let routes = routes_from_calls(&calls, &agents, since);
        assert_eq!(routes.calls, 3);
        assert_eq!(routes.callers[0].label, "Claude Code");
        assert_eq!(routes.callers[0].calls, 2);
        assert_eq!(routes.models.len(), 2);
        assert_eq!(routes.models[0].route, Some(1));
        let blocked = routes.blocked.unwrap();
        assert_eq!((blocked.label.as_str(), blocked.calls), ("Person limit", 1));
        assert_eq!(blocked.model.as_deref(), Some("claude-opus-5.5"));
        // The label opens the person's limit in Usage › Limits.
        assert_eq!(blocked.row, crate::limit_stop::PERSON_ROW);
    }

    #[test]
    fn folds_quiet_models_into_other() {
        let models = (0..7)
            .map(|n| RouteModel {
                key: format!("m{n}"),
                label: format!("m{n}"),
                calls: 10 - n,
                cost: 0.,
                route: None,
            })
            .collect();
        let ranked = rank_models(models);
        assert_eq!(ranked.len(), 5);
        assert_eq!(ranked[2].route, Some(3));
        assert_eq!(ranked[3].route, None);
        assert_eq!(ranked[4].label, "Other (3)");
    }

    #[test]
    fn reads_sessions_products_and_setup() {
        let sessions = sessions_of(
            &serde_json::from_str::<Value>(
                r#"{ "sessions": [
            { "id": "s1", "agent": "codex", "model": "openai/gpt-6", "via": "subscription", "person": "ana@x.io",
              "status": "running", "startedAt": "2026-11-05T10:00:00Z", "usage": { "estimatedCostUsd": 1.234 } },
            { "id": "s2", "agent": "claude", "model": "anthropic/claude-opus-5.5", "via": "nexus", "access": "byok",
              "person": "Bo", "status": "done", "startedAt": "2026-11-05T09:00:00Z", "durationMs": 3900000 }
        ] }"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sessions[0].access_word(), "Their plan");
        assert_eq!(sessions[0].estimated, Some(1.234));
        assert_eq!(sessions[1].access_word(), "Org key");
        assert_eq!(run_length(sessions[1].duration_ms.unwrap()), "1 h 05 min");
        assert_eq!(run_length(45_000), "45 s");

        let products = products_of(&serde_json::from_str::<Value>(r#"{
            "modules": [
                { "product": { "slug": "core", "name": "Nexus Gateway" }, "capability": { "slug": "models", "name": "Models", "description": "x" } },
                { "product": { "slug": "core", "name": "Nexus Gateway" }, "capability": { "slug": "keys", "name": "Keys", "description": "y" } },
                { "product": { "slug": "robustness", "name": "Nexus Review" }, "capability": { "slug": "review", "name": "Review", "description": "z" } }
            ],
            "unavailable": [{ "url": "http://r", "error": "timeout" }]
        }"#).unwrap())
        .unwrap();
        assert_eq!(products.products.len(), 2);
        assert_eq!(products.down, 1);
        assert_eq!(product_copy("core", "Nexus Gateway", "x").0, "Gateway");

        assert!(setup_keys(
            &json!({ "providers": [{ "unused": true }, { "unused": false }] })
        ));
        assert!(!setup_keys(&json!({ "providers": [{ "unused": true }] })));
        assert!(setup_cli(&json!({ "tokens": [{ "kind": "api" }, { "kind": "cli" }] })));
        assert_eq!(versus(12, 10), "+20% vs. yesterday");
        assert_eq!(versus(5, 0), "None yesterday");
        assert_eq!(rate_text(0.012), "1.2%");
    }

    #[test]
    fn member_routes_from_usage() {
        let figures = |calls: u64, spent: f64| {
            json!({ "spentUsd": spent, "calls": calls, "inputTokens": 0, "outputTokens": 0,
            "callsByStatus": { "settled": calls, "failed": 0, "refused": 0, "released": 0 }, "errorRate": null })
        };
        let mut by_product = json!({ "scope": "member", "totals": figures(3, 0.3), "groups": [], "series": [] });
        let mut group = figures(3, 0.3);
        group["key"] = json!("chat/stream");
        group["label"] = json!("Chat");
        by_product["groups"] = json!([group]);
        let mut by_model = json!({ "scope": "member", "totals": figures(3, 0.3), "groups": [], "series": [] });
        let mut model = figures(3, 0.3);
        model["key"] = json!("openai/gpt-6");
        model["label"] = json!("openai/gpt-6");
        by_model["groups"] = json!([model]);
        let routes = routes_from_usage(&Report::parse(&by_product).unwrap(), &Report::parse(&by_model).unwrap());
        assert_eq!(routes.callers[0].label, "Chat");
        assert_eq!(routes.models[0].label, "gpt-6");
        assert_eq!(routes.calls, 3);
    }
}
