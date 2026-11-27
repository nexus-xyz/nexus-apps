//! Settings › Agents and plans: one section for each agent that `nexus claude` and
//! `nexus codex` launch. A section says whether the agent is ready, who pays for it (Nexus, or the person's own Claude
//! or ChatGPT plan), what this computer has of it, the steps left to set it up, and the plan accounts on this computer.
//!
//! Who pays is the person's, kept by Gateway for every app they sign in to: `GET /v1/agents/access` reads it and
//! `PUT /v1/agents/access` saves it (the CLI's `nexus agents use`). So the choice writes no file: every app and the
//! CLI see the same choice. What is installed and signed in is read here with `nexus_client::local_agents`. The plan accounts are this computer's ([`super::subscriptions::Plans`]), drawn by [`accounts`].

mod accounts;
mod computer_setup;
mod plans_off;
mod sandbox;
mod sign_in_card;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::InputState;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_client::agent_setup::{self, SetupOptions, SetupStep};
use nexus_client::agents::Route;
use nexus_client::local_agents::{LocalAgent, Login};
use nexus_workbench::allowance;
use nexus_workbench::event::SessionAgent;
use nexus_workbench::plans::PlanUsage;
use serde_json::{Value, json};

use super::subscriptions::Plans;
use super::{Ctx, DeviceCtx, Load, Verb, fetch, palette, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

pub(crate) const CLAUDE_ROUTING: crate::settings_fields::AccountField =
    crate::settings_fields::AccountField("agent-claude");
pub(crate) const CODEX_ROUTING: crate::settings_fields::AccountField =
    crate::settings_fields::AccountField("agent-codex");

thread_local! {
    /// The agent whose selected profile signs in as this page opens next (`start_blockers.rs`).
    static CONNECT_ON_OPEN: std::cell::Cell<Option<SessionAgent>> = const { std::cell::Cell::new(None) };
}

/// Start browser sign-in for the selected profile of `agent` the next time this page opens.
pub(crate) fn connect_on_open(agent: SessionAgent) {
    CONNECT_ON_OPEN.with(|slot| slot.set(Some(agent)));
}

/// The agent [`connect_on_open`] named, once: a later open of the page starts nothing.
fn take_connect_on_open() -> Option<SessionAgent> {
    CONNECT_ON_OPEN.with(std::cell::Cell::take)
}

pub fn build(ctx: DeviceCtx, account: Option<Ctx>, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let plans = Plans::new(&ctx, cx);
    if let Some(agent) = take_connect_on_open() {
        let account = super::subscriptions::chosen(&crate::desktop_settings::load(), agent);
        plans.update(cx, |plans, cx| plans.connect(agent, account, cx));
    }
    let sandbox = cx.new(|cx| sandbox::Sandbox::new(ctx.clone(), cx));
    let view = cx.new(|cx| {
        cx.observe(&plans, |_, _, cx| cx.notify()).detach();
        // A sign-in finished in a terminal: read it when the window comes back to the front.
        cx.observe_window_activation(window, |page: &mut AgentsPage, window, cx| {
            if window.is_window_active() {
                page.plans.update(cx, |plans, cx| plans.window_active(cx));
            }
        })
        .detach();
        // The code Claude's browser may show: masked, and cleared once sent.
        let code_input = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Paste the code"));
        let mut page = AgentsPage {
            code_input,
            sandbox,
            ctx,
            access: if account.is_some() {
                Load::Loading
            } else {
                Load::Ready(local_access())
            },
            account,
            local: Load::Loading,
            detecting: false,
            saving: None,
            errors: Vec::new(),
            plans_refused: Vec::new(),
            copied: None,
            plans,
            menu: None,
            updating: None,
            update_error: None,
        };
        page.load(cx);
        page.check_computer(cx);
        page
    });
    Some(view.into())
}

/// Gateway's answer, or the offline catalog when this Gateway keeps no routes.
#[derive(Debug, Clone)]
struct Access {
    /// The organization allows sessions on a person's own plan.
    agent_subscriptions: bool,
    agents: Vec<AgentEntry>,
    /// This Gateway does not keep routes (it answered null): the route cannot be changed here.
    unsupported: bool,
}

/// One agent's entry. `raw` keeps Gateway's shape for `nexus_client::agent_setup`.
#[derive(Debug, Clone)]
struct AgentEntry {
    agent: String,
    name: String,
    route: Route,
    /// `preference`, `default` or `organization`.
    from: String,
    routes: Vec<RouteView>,
    raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
struct RouteView {
    route: Route,
    available: bool,
    /// Why it is not available, as Gateway words it.
    reason: Option<String>,
}

fn read_access(value: &Value) -> Access {
    let unsupported = value.is_null();
    let fallback;
    let value = if unsupported {
        fallback = agent_setup::offline_agent_access();
        &fallback
    } else {
        value
    };
    let agents = value
        .get("agents")
        .and_then(Value::as_array)
        .map(|agents| agents.iter().filter_map(read_entry).collect())
        .unwrap_or_default();
    Access {
        agent_subscriptions: value
            .get("agentSubscriptions")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        agents,
        unsupported,
    }
}

fn local_access() -> Access {
    let mut access = read_access(&Value::Null);
    for entry in &mut access.agents {
        entry.route = Route::Subscription;
    }
    access
}

fn read_entry(entry: &Value) -> Option<AgentEntry> {
    let agent = entry.get("agent")?.as_str()?;
    if SessionAgent::parse(agent).is_some_and(|agent| !agent.supported_on_current_platform()) {
        return None;
    }
    let routes = entry
        .get("routes")
        .and_then(Value::as_array)
        .map(|routes| {
            routes
                .iter()
                .filter_map(|view| {
                    Some(RouteView {
                        route: Route::parse(view.get("route")?.as_str()?)?,
                        available: view.get("available") != Some(&Value::Bool(false)),
                        reason: view.get("reason").and_then(Value::as_str).map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(AgentEntry {
        name: ui::text(entry, "name").to_string(),
        route: Route::parse(ui::text(entry, "route")).unwrap_or(Route::Gateway),
        from: ui::text(entry, "from").to_string(),
        agent: agent.to_string(),
        routes,
        raw: entry.clone(),
    })
}

/// Why the route is what it is (`FROM` in agent-access.tsx).
fn from_label(from: &str) -> &'static str {
    match from {
        "preference" => "Your choice",
        "organization" => "Set by your organization",
        _ => "Default",
    }
}

/// The plan's name as Gateway gives it: `Claude`, `ChatGPT`.
fn plan_of(entry: &AgentEntry) -> String {
    agent_setup::route_of(&entry.raw, Route::Subscription)
        .and_then(|view| view.get("plan"))
        .and_then(Value::as_str)
        .unwrap_or("own")
        .to_string()
}

/// A route's name on the "Pays with" control: "Nexus", "Claude plan".
fn route_button(entry: &AgentEntry, route: Route) -> String {
    match route {
        Route::Gateway => "Nexus".into(),
        Route::Subscription => format!("{} plan", plan_of(entry)),
    }
}

/// A provider as people name it.
fn provider_name(id: &str) -> String {
    match id {
        "anthropic" => "Anthropic".into(),
        "openai" => "OpenAI".into(),
        "google" => "Google".into(),
        other => other.to_string(),
    }
}

/// Who pays on the route the agent takes now, and what Nexus does with the calls.
fn pays_line(entry: &AgentEntry) -> String {
    let view = agent_setup::route_of(&entry.raw, entry.route);
    match entry.route {
        Route::Subscription => format!(
            "Your {} plan pays. Nexus records each session but does not bill it.",
            plan_of(entry)
        ),
        Route::Gateway if entry.routes.iter().any(|v| v.route == Route::Gateway && !v.available) => {
            let provider = view
                .and_then(|view| view.get("provider"))
                .and_then(Value::as_str)
                .map(provider_name)
                .unwrap_or_else(|| "provider".into());
            format!(
                "Your organization's {provider} key pays. Nobody added the key yet, so Nexus refuses the calls from {}.",
                entry.name
            )
        }
        Route::Gateway => {
            let payer = agent_setup::payer_label(view);
            let mut chars = payer.chars();
            let payer: String = match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => payer,
            };
            format!("{payer} pays. Nexus meters and checks every call.")
        }
    }
}

/// Why the person's own plan is locked, when it is.
fn plan_locked(entry: &AgentEntry, access: &Access) -> Option<String> {
    let view = entry.routes.iter().find(|view| view.route == Route::Subscription)?;
    if view.available && access.agent_subscriptions {
        return None;
    }
    Some(
        view.reason
            .clone()
            .unwrap_or_else(|| "Your organization does not allow personal plans.".into()),
    )
}

/// Why the checklist's plan sign-in can't run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignInBlock {
    /// The agent is not installed on this computer.
    Install,
    /// The organization or this Gateway does not allow personal plans.
    PlansOff,
}

fn sign_in_blocked(entry: &AgentEntry, local: Option<&LocalAgent>, access: &Access) -> Option<SignInBlock> {
    if !local.is_some_and(|local| local.installed) {
        Some(SignInBlock::Install)
    } else if plan_locked(entry, access).is_some() {
        Some(SignInBlock::PlansOff)
    } else {
        None
    }
}

/// How the computer row reads: fine, needs the person, or not there.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Health {
    Fine,
    NeedsYou,
    Missing,
}

/// What this computer has of the agent, in a sentence, and how that reads.
fn computer_line(entry: &AgentEntry, local: Option<&LocalAgent>) -> (String, Health) {
    let Some(local) = local else {
        return ("Not checked yet.".into(), Health::Missing);
    };
    if !local.installed {
        return (format!("{} is not installed.", entry.name), Health::Missing);
    }
    let installed = match &local.version {
        Some(version) => format!("Version {version} is installed"),
        None => format!("{} is installed", entry.name),
    };
    if entry.route == Route::Gateway {
        return (format!("{installed}."), Health::Fine);
    }
    let plan = plan_of(entry);
    match local.login {
        Some(Login::Plan) => {
            let name = local.plan.clone().unwrap_or_else(|| format!("your {plan} plan"));
            (format!("{installed} and signed in to {name}."), Health::Fine)
        }
        Some(Login::ApiKey) => (
            format!(
                "{} is signed in with an API key. The calls bill the API, not your {plan} plan.",
                entry.name
            ),
            Health::NeedsYou,
        ),
        Some(Login::SignedOut) => (
            format!("{installed}, but it is not signed in to your {plan} plan."),
            Health::NeedsYou,
        ),
        _ => (format!("{installed}."), Health::Fine),
    }
}

/// What a new session of the agent finds on this computer. `local` reads only the agent's own folder (`~/.claude`,
/// `~/.codex`), but a new session starts on the Active account, which has a folder of its own. So when the plan
/// accounts check found a signed-in plan for new sessions (`allowance::plan_for`), that login counts.
fn session_local(local: Option<&LocalAgent>, plans: &[PlanUsage]) -> Option<LocalAgent> {
    let mut local = local?.clone();
    let Some(agent) = plan_agent(&local.agent).filter(|_| local.installed) else {
        return Some(local);
    };
    if let Some(plan) = allowance::plan_for(plans, agent, allowance::Login::NewSession, None) {
        local.login = Some(Login::Plan);
        local.plan = Some(super::subscriptions::plan_name(plan));
    }
    Some(local)
}

/// The section heading's state: "Ready", "2 steps left", "Plans off", "Needs an admin".
fn heading_status(steps: &[SetupStep]) -> (String, bool) {
    let open: Vec<&SetupStep> = steps.iter().filter(|step| step.done != Some(true)).collect();
    match open.len() {
        0 => ("Ready".into(), true),
        1 if open[0].id == plans_off::STEP => ("Plans off".into(), false),
        _ if open.iter().all(|step| step.who == "admin") => ("Needs an admin".into(), false),
        1 => ("1 step left".into(), false),
        n => (format!("{n} steps left"), false),
    }
}

/// The agents a Gateway connect recipe exists for (`recipeFor` in the web kit's recipes): those with a route through
/// Gateway.
fn has_recipe(agent: &str) -> bool {
    nexus_client::agent_entries::Agent::parse(agent).is_some_and(|agent| agent.entry().billing.gateway.is_some())
}

/// The workbench's name for an agent Gateway lists, when Settings › Subscriptions reads its plan.
fn plan_agent(agent: &str) -> Option<SessionAgent> {
    nexus_client::agent_entries::Agent::parse(agent)
        .and_then(|agent| SessionAgent::try_from(agent).ok())
        .filter(|agent| super::subscriptions::reads_plan(*agent))
}

/// The desktop can run these plan steps using the agent's native browser sign-in.
fn checklist_provider(step: &SetupStep, agent: &str) -> Option<SessionAgent> {
    if step.done == Some(true) || !matches!(step.id, "plan_login" | "plan_relogin") {
        return None;
    }
    plan_agent(agent).filter(|agent| nexus_workbench::harness::harness(*agent).accounts().is_some())
}

struct AgentsPage {
    ctx: DeviceCtx,
    account: Option<Ctx>,
    access: Load<Access>,
    /// What this computer has of each agent.
    local: Load<Vec<LocalAgent>>,
    /// A look at this computer runs.
    detecting: bool,
    /// The agent whose route is being saved.
    saving: Option<String>,
    /// The last failed save, per agent: (agent, text).
    errors: Vec<(String, String)>,
    /// The agents whose plan the organization refused on save: their setup line shows (`plans_off`).
    plans_refused: Vec<String>,
    /// The command just copied.
    copied: Option<String>,
    /// The plan accounts on this computer.
    plans: Entity<Plans>,
    /// The account row whose "…" menu is open, by its fold key.
    menu: Option<String>,
    /// The sign-in card's private field for the code Claude's browser may show.
    code_input: Entity<InputState>,
    /// Antigravity's Sandbox switch, a row of its section.
    sandbox: Entity<sandbox::Sandbox>,
    /// The agent whose update runs. One at a time: two npm installs at once can clash.
    updating: Option<String>,
    /// The last failed update: (agent, text).
    update_error: Option<(String, String)>,
}

impl AgentsPage {
    /// Run the agent's own update command (`Install.update`), then look at this computer again for its new version.
    fn update_agent(&mut self, agent: String, cx: &mut Context<Self>) {
        let Some(client_agent) = nexus_client::agent_entries::Agent::parse(&agent) else {
            return;
        };
        if self.updating.is_some() {
            return;
        }
        self.updating = Some(agent.clone());
        self.update_error = None;
        let answer = crate::code_run::update_agent(client_agent);
        cx.spawn(async move |this, cx| {
            let result = answer
                .await
                .unwrap_or_else(|_| Err("The update stopped before it finished.".into()));
            this.update(cx, |page, cx| {
                page.updating = None;
                if let Err(error) = result {
                    page.update_error = Some((agent, error));
                }
                page.check_computer(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(account) = &self.account else {
            return;
        };
        let request = account.gateway.query("/v1/agents/access".into());
        super::read(cx, request, |this, answer, _| {
            this.access = match answer {
                Ok(value) => Load::Ready(read_access(&value)),
                // Keep what was read: a failed refresh after a save says so on the card instead.
                Err(error) => match &this.access {
                    Load::Ready(_) => return,
                    _ => Load::Failed(error.into()),
                },
            };
        });
    }

    /// Look for Claude Code and Codex on this computer, off the UI thread: each status command may take seconds.
    fn check_computer(&mut self, cx: &mut Context<Self>) {
        #[cfg(test)]
        if crate::code_run::agent_catalog_is_fixture() {
            self.local = Load::Ready(Vec::new());
            cx.notify();
            return;
        }
        if self.detecting {
            return;
        }
        self.detecting = true;
        self.local = Load::Loading;
        let (send, receive) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let env = |name: &str| std::env::var(name).ok();
            send.send(nexus_client::local_agents::detect_all(&env)).ok();
        });
        cx.spawn(async move |this, cx| {
            let found = receive.await;
            this.update(cx, |page, cx| {
                page.detecting = false;
                page.local = match found {
                    Ok(found) => Load::Ready(found),
                    Err(_) => Load::Failed("Could not check this computer.".into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Check again: this computer, the plans, and Gateway's answer.
    fn check_all(&mut self, cx: &mut Context<Self>) {
        self.check_computer(cx);
        self.plans.update(cx, |plans, cx| plans.check(cx));
        self.load(cx);
    }

    fn choose(&mut self, agent: String, route: Route, cx: &mut Context<Self>) {
        let Some(account) = &self.account else {
            return;
        };
        let Load::Ready(access) = &self.access else {
            return;
        };
        let Some(entry) = access.agents.iter().find(|entry| entry.agent == agent) else {
            return;
        };
        // The route it already takes: nothing to save.
        if entry.route == route || self.saving.is_some() || access.unsupported {
            return;
        }
        self.saving = Some(agent.clone());
        self.errors.retain(|(each, _)| *each != agent);
        let request = account.gateway.request(
            Verb::Put,
            "/v1/agents/access".into(),
            Some(json!({ agent.as_str(): route.as_str() })),
        );
        self.plans_refused.retain(|each| *each != agent);
        fetch(cx, request, move |this, answer, cx| {
            this.saving = None;
            match answer {
                Ok(_) => this.load(cx),
                // The organization refused the plan: the setup line says so, with its fix.
                Err(error) if route == Route::Subscription && plans_off::refused_by_org(&error) => {
                    this.plans_refused.push(agent);
                    this.load(cx);
                }
                Err(error) => this.errors.push((agent, refusal(&error))),
            }
        });
        cx.notify();
    }

    /// "Use this plan" on an account row: the account becomes the active one, and the agent pays with the plan.
    fn use_plan(&mut self, agent: String, account: Option<String>, cx: &mut Context<Self>) {
        if let Some(plan_agent) = plan_agent(&agent) {
            self.plans
                .update(cx, |plans, cx| plans.use_account(plan_agent, account, cx));
        }
        self.choose(agent, Route::Subscription, cx);
    }

    fn route_focus_id(agent: &str, route: Route) -> &'static str {
        match (agent, route) {
            ("codex", Route::Gateway) => "route-codex-gateway",
            ("codex", Route::Subscription) => "route-codex-subscription",
            ("antigravity", Route::Gateway) => "route-antigravity-gateway",
            ("antigravity", Route::Subscription) => "route-antigravity-subscription",
            (_, Route::Gateway) => "route-claude-gateway",
            (_, Route::Subscription) => "route-claude-subscription",
        }
    }

    /// The "Pays with" control: Nexus or the plan, in a soft track. A locked choice cannot be picked.
    fn pays_control(&self, entry: &AgentEntry, access: &Access, cx: &mut Context<Self>) -> Div {
        let palette = palette(cx);
        let saving = self.saving.is_some();
        let mut track = ui::tabs(palette).flex_none();
        for view in &entry.routes {
            let picked = entry.route == view.route;
            let locked = access.unsupported
                || saving
                || (view.route == Route::Subscription && plan_locked(entry, access).is_some());
            let id = SharedString::from(format!("agents-{}-{}", entry.agent, view.route.as_str()));
            let label = route_button(entry, view.route);
            if self.account.is_none() || (locked && !picked) {
                track = track.child(
                    h_flex()
                        .id(id)
                        .h(px(ui::CONTROL_HEIGHT - 4.))
                        .px(px(10.))
                        .gap(px(5.))
                        .text_size(px(13.))
                        .text_color(palette.faint)
                        .opacity(0.6)
                        .when(view.route == Route::Subscription, |tab| {
                            tab.child(ui::icon(Lucide::Lock, palette.faint).size(px(12.)))
                        })
                        .child(label),
                );
                continue;
            }
            let agent = entry.agent.clone();
            let route = view.route;
            let arrow_agent = agent.clone();
            let next = entry
                .routes
                .iter()
                .find(|other| other.route != route && other.available)
                .map(|other| other.route);
            track = track.child(
                ui::tab(id, label, picked, palette)
                    .track_focus(&self.ctx.settings_fields.focus(Self::route_focus_id(&agent, route), cx))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        if locked || !matches!(event.keystroke.key.as_str(), "left" | "right" | "up" | "down") {
                            return;
                        }
                        if let Some(next) = next {
                            this.ctx
                                .settings_fields
                                .focus(Self::route_focus_id(&arrow_agent, next), cx)
                                .focus(window, cx);
                            this.choose(arrow_agent.clone(), next, cx);
                        }
                        cx.stop_propagation();
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(agent.clone(), route, cx))),
            );
        }
        track
    }

    /// One agent's section: its heading and state, who pays, this computer, and the setup steps or the command.
    /// The `first` section's heading sits next to the Check again button, so it has no space above.
    fn render_section(&self, entry: &AgentEntry, access: &Access, first: bool, cx: &mut Context<Self>) -> Div {
        let palette = palette(cx);
        let found = match &self.local {
            Load::Ready(found) => found.iter().find(|local| local.agent == entry.agent),
            _ => None,
        };
        let local = match &self.plans.read(cx).plans {
            Load::Ready(plans) => session_local(found, plans),
            _ => session_local(found, &[]),
        };
        let local = local.as_ref();
        let signed_in = self.account.is_some();
        let mut steps = agent_setup::setup_steps(
            &entry.raw,
            entry.route,
            SetupOptions {
                local,
                signed_in: Some(signed_in),
                agent_subscriptions: Some(access.agent_subscriptions),
                cli_install: None,
            },
        );
        if !signed_in {
            steps.retain(|step| step.id != "nexus_login");
        }
        let checking = matches!(self.local, Load::Loading);
        // Signed out, there is no organization whose plans are off.
        let plans_line = signed_in && plans_off::shows(entry, access, self.plans_refused.contains(&entry.agent));
        let (status_text, ready) = heading_status(&steps);
        // What waits on an owner or an admin is no limit: its line below says who can, so the state stays neutral.
        let waits_on_admin = steps
            .iter()
            .filter(|step| step.done != Some(true))
            .all(|step| step.who == "admin");
        let status = if checking {
            ui::status("Checking…", palette.icon, palette)
        } else if ready {
            ui::status(status_text, Palette::success(), palette)
        } else if waits_on_admin {
            ui::status(status_text, palette.icon, palette)
        } else {
            ui::status(status_text, palette.warning, palette)
        };

        // Who pays.
        let mut pays = v_flex().gap(px(2.)).child(ui::setting_lines(
            "Pays with",
            if !signed_in {
                format!("Local sessions can use your {} plan.", plan_of(entry))
            } else if access.unsupported {
                format!(
                    "This Gateway cannot save how {} pays. Run nexus {} --subscription or nexus {} --api.",
                    entry.name, entry.agent, entry.agent
                )
            } else {
                pays_line(entry)
            },
            palette,
        ));
        // Gateway's own reason for a locked plan; plans off is the setup line's.
        if let Some(reason) = plan_locked(entry, access).filter(|_| entry.route == Route::Gateway && !plans_line) {
            pays = pays.child(
                h_flex()
                    .gap(px(5.))
                    .child(ui::icon(Lucide::Lock, palette.faint).size(px(12.)))
                    .child(ui::setting_detail(reason, palette)),
            );
        }
        if entry.from == "organization" {
            pays = pays.child(ui::setting_detail(from_label(&entry.from), palette));
        }
        let pays_row = ui::card_row()
            .when(!signed_in, |row| row.flex_col().items_start().gap(px(6.)))
            .child(pays.min_w(px(0.)).when(signed_in, |pays| pays.flex_1()))
            .when(self.ctx.local_workspace, |row| row.child(ui::requires_sign_in(palette)))
            .child(self.pays_control(entry, access, cx));

        // This computer.
        let (computer_text, health) = match &self.local {
            Load::Loading => ("Checking this computer…".to_string(), Health::Missing),
            Load::Failed(_) => (
                "Could not check this computer. Run nexus agents in a terminal to see what is installed.".to_string(),
                Health::Missing,
            ),
            Load::Ready(_) => computer_line(entry, local),
        };
        // What is missing here carries its fix on its own line: Install, or Sign in.
        let computer_setup = computer_setup::need(local, entry.route != Route::Gateway)
            .filter(|_| !checking && matches!(self.local, Load::Ready(_)))
            .and_then(|need| {
                let can_sign_in = local.is_some_and(|local| local.installed)
                    && plan_locked(entry, access).is_none()
                    && !self.plans.read(cx).busy;
                self.computer_setup(entry, need, &plan_of(entry), can_sign_in, palette, cx)
            });
        let computer_mark = match (health, checking) {
            (_, true) => None,
            (Health::Fine, _) => Some(ui::icon(Lucide::Check, Palette::success()).into_any_element()),
            (Health::NeedsYou | Health::Missing, _) => None,
        };
        let computer_detail = ui::setting_detail(computer_text, palette);
        let updating = self.updating.as_deref() == Some(entry.agent.as_str());
        let update_error = self
            .update_error
            .as_ref()
            .filter(|(agent, _)| *agent == entry.agent)
            .map(|(_, error)| ui::setting_detail(error.clone(), palette).text_color(palette.warning));
        let update = (!checking
            && local.is_some_and(|local| local.installed)
            && nexus_client::agent_entries::Agent::parse(&entry.agent).is_some())
        .then(|| {
            let id = SharedString::from(format!("agents-update-{}", entry.agent));
            let button = ui::ghost_button(id, if updating { "Updating…" } else { "Update" }, palette);
            if self.updating.is_some() {
                return button.opacity(0.5);
            }
            let agent = entry.agent.clone();
            button.on_click(cx.listener(move |this, _, _, cx| this.update_agent(agent.clone(), cx)))
        });
        let computer_row = ui::card_row()
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.strong)
                            .child("This computer"),
                    )
                    .map(|column| match computer_setup {
                        Some(line) => column.child(div().pt(px(6.)).child(line)),
                        None => column.child(computer_detail),
                    })
                    .children(update_error),
            )
            .children(update)
            .children(computer_mark);

        let start_row = (ready && (signed_in || self.ctx.local_workspace)).then(|| {
            let command = SessionAgent::parse(&entry.agent)
                .and_then(SessionAgent::coding)
                .map_or(entry.agent.as_str(), |agent| agent.command());
            ui::setting_row(
                "Start a session",
                "Run it in any terminal.",
                if signed_in {
                    self.command(&format!("nexus {command}"), cx).into_any_element()
                } else {
                    ui::requires_sign_in(palette).into_any_element()
                },
                palette,
            )
            .into_any_element()
        });
        let sandbox_row = (plan_agent(&entry.agent) == Some(SessionAgent::Antigravity))
            .then(|| self.sandbox.clone().into_any_element());
        let card = ui::group(
            [
                Some(pays_row.into_any_element()),
                Some(computer_row.into_any_element()),
                sandbox_row,
                start_row,
            ],
            palette,
        );

        let heading = h_flex()
            .mt(px(if first { 0. } else { 28. }))
            .mb(px(10.))
            .gap(px(10.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.strong)
                    .child(entry.name.clone()),
            )
            .child(status);
        let mut section = v_flex()
            .child(heading)
            .child(card)
            .when(plans_line, |section| section.children(self.plans_off_line(entry, cx)))
            .children(
                self.errors
                    .iter()
                    .filter(|(agent, _)| *agent == entry.agent)
                    .map(|(_, text)| ui::error_line(text.clone(), palette).mt(px(8.))),
            )
            .when(signed_in, |section| {
                section.child(ui::footnote(
                    "Nexus saves this choice to your account. Every Nexus app that you sign in to uses it.",
                    palette,
                ))
            });
        // The step plans off stands for is the line's, so the list skips it, and goes when nothing else is left.
        let steps = if plans_line {
            plans_off::without_line(&steps)
        } else {
            steps
        };
        let show_steps = !ready && !checking && steps.iter().any(|step| step.done != Some(true));
        let checklist_card = self
            .plans
            .read(cx)
            .sign_in
            .as_ref()
            .filter(|login| {
                show_steps
                    && login.from_checklist
                    && steps
                        .iter()
                        .any(|step| checklist_provider(step, &entry.agent) == Some(login.agent))
            })
            .map(sign_in_card::Shown::of);
        if show_steps {
            let blocked = sign_in_blocked(entry, local, access);
            section = section.child(self.render_steps(entry, &steps, blocked, checklist_card.as_ref(), cx));
        }
        if let Some(agent) = plan_agent(&entry.agent) {
            section = section.children(accounts::section(
                self,
                agent,
                entry,
                access,
                checklist_card.is_some(),
                cx,
            ));
        }
        // Every agent's section is a place a setup line opens ("Coding agents ›"). Claude's and Codex's anchors are
        // `CLAUDE_ROUTING` and `CODEX_ROUTING`.
        let anchor = SessionAgent::parse(&entry.agent).map(crate::agents::setup::anchor);
        match anchor {
            Some(anchor) => self.ctx.settings_fields.field(anchor, section, None, cx),
            None => section,
        }
    }

    /// Show who owns each step, with an in-app sign-in for supported plans.
    fn render_steps(
        &self,
        entry: &AgentEntry,
        steps: &[SetupStep],
        blocked: Option<SignInBlock>,
        card: Option<&sign_in_card::Shown>,
        cx: &mut Context<Self>,
    ) -> Div {
        let palette = palette(cx);
        let can_connect = blocked.is_none();
        let done = steps.iter().filter(|step| step.done == Some(true)).count();
        let mut rows: Vec<Option<AnyElement>> = Vec::new();
        for (n, step) in steps.iter().enumerate() {
            let finished = step.done == Some(true);
            let provider = checklist_provider(step, &entry.agent);
            let detail = if provider.is_some() && can_connect {
                "Sign in using your browser."
            } else if finished {
                "Done."
            } else if step.who == "admin" {
                "An owner or admin of your organization does this step."
            } else if step.done.is_none() {
                "Nexus cannot check this step. Skip it if you did it already."
            } else {
                "You do this step on this computer."
            };
            let mark = h_flex()
                .flex_none()
                .justify_center()
                .size(px(20.))
                .rounded(px(10.))
                .text_size(px(11.))
                .map(|mark| {
                    if finished {
                        mark.bg(Palette::success().opacity(0.14))
                            .child(ui::icon(Lucide::Check, Palette::success()).size(px(12.)))
                    } else {
                        mark.bg(palette.hover)
                            .text_color(palette.muted)
                            .child((n + 1).to_string())
                    }
                });
            let title_text = match provider {
                Some(agent) if step.id == "plan_login" => {
                    format!("Connect your {} plan", super::subscriptions::vendor(agent))
                }
                _ => step.text.clone(),
            };
            let title = h_flex()
                .gap(px(8.))
                .child(div().text_size(px(13.)).text_color(palette.strong).child(title_text))
                .when(step.who == "admin" && !finished, |title| {
                    title.child(ui::pill("Admin", palette))
                });
            let row = ui::card_row()
                .when(finished, |row| row.opacity(0.6))
                .child(mark)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(2.))
                        .child(title)
                        .child(ui::setting_detail(detail, palette)),
                )
                .map(|row| match provider {
                    Some(agent) if card.is_none() => {
                        let id = SharedString::from(format!("setup-sign-in-{}", agent.as_str()));
                        let label = format!("Sign in with {}", super::subscriptions::vendor(agent));
                        // A sign-in that can't run says why beside it.
                        let why = match blocked {
                            Some(SignInBlock::Install) => Some(crate::setup::locked::waits_on(
                                format!("Install {} first", entry.name),
                                palette,
                            )),
                            Some(SignInBlock::PlansOff) => {
                                Some(crate::setup::locked::reason("Personal plans are off", palette))
                            }
                            None if self.plans.read(cx).busy => {
                                Some(crate::setup::locked::waits_on("Another sign-in is running", palette))
                            }
                            None => None,
                        };
                        if let Some(why) = why {
                            return row.child(
                                h_flex()
                                    .flex_none()
                                    .gap(px(8.))
                                    .child(why)
                                    .child(ui::disabled_button(id, label, palette)),
                            );
                        }
                        let plans = self.plans.clone();
                        row.child(ui::primary_button(id, label, palette).on_click(move |_, _, cx| {
                            let account = super::subscriptions::chosen(&crate::desktop_settings::load(), agent);
                            plans.update(cx, |plans, cx| plans.connect_from_checklist(agent, account, cx));
                        }))
                    }
                    Some(_) => row,
                    None => row.when_some(step.command.as_ref().filter(|_| !finished), |row, command| {
                        row.child(self.command(command, cx))
                    }),
                });
            rows.push(Some(row.into_any_element()));
            if let Some(shown) = card.filter(|shown| provider == Some(shown.agent)) {
                let copied = self.plans.read(cx).copied.clone();
                rows.push(Some(sign_in_card::card(
                    self,
                    shown,
                    copied.as_deref(),
                    false,
                    cx.reduce_motion(),
                    cx,
                )));
            }
        }
        let mut setup = v_flex()
            .child(
                ui::group_head("Finish setup", palette).child(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(format!("{done} of {} done", steps.len())),
                ),
            )
            .child(ui::group(rows, palette))
            .child(ui::footnote(
                "The steps update when you open this page again or press Check again.",
                palette,
            ));
        if has_recipe(&entry.agent)
            && entry.route == Route::Gateway
            && let Some(ctx) = self.account.clone()
        {
            let path = format!("connect/{}", entry.agent);
            setup = setup.child(
                h_flex().mt(px(6.)).child(
                    ui::web_link(
                        SharedString::from(format!("agents-recipe-{}", entry.agent)),
                        format!("Open the setup of {} in the browser", entry.name),
                        palette,
                    )
                    .on_click(move |_, _, cx| ctx.open_web(&path, cx)),
                ),
            );
        }
        setup
    }

    /// A command in monospace with a copy button.
    fn command(&self, command: &str, cx: &mut Context<Self>) -> Div {
        let palette = palette(cx);
        let copied = self.copied.as_deref() == Some(command);
        let text = command.to_string();
        h_flex()
            .flex_none()
            .gap(px(4.))
            .min_w(px(0.))
            .child(
                div()
                    .min_w(px(0.))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .bg(palette.hover)
                    .font_family(ui::MONO)
                    .text_size(px(12.))
                    .text_color(palette.foreground)
                    .truncate()
                    .child(command.to_string()),
            )
            .child(
                super::row::icon_button(
                    SharedString::from(format!("agents-copy-{command}")),
                    if copied { Lucide::Check } else { Lucide::Copy },
                    if copied { "Copied" } else { "Copy" },
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    this.copied = Some(text.clone());
                    cx.notify();
                })),
            )
    }
}

/// Gateway's refusal, for a person. A 403 means the organization or a role stops the change.
fn refusal(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("owner") || lower.contains("admin") || lower.contains("forbidden") {
        format!("Nexus did not save the change. Your organization does not allow it: {error}")
    } else {
        format!("Nexus did not save the change: {error}")
    }
}

impl Render for AgentsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let busy = self.detecting || self.plans.read(cx).checking;
        let problem = self.plans.read(cx).problem.clone();
        let check = if busy {
            h_flex()
                .id("agents-check")
                .flex_none()
                .size(px(ui::CONTROL_HEIGHT))
                .justify_center()
                .opacity(0.5)
                .child(ui::icon(Lucide::RefreshCw, palette.faint))
                .into_any_element()
        } else {
            super::row::icon_button("agents-check", Lucide::RefreshCw, "Check again", palette)
                .on_click(cx.listener(|this, _, _, cx| this.check_all(cx)))
                .into_any_element()
        };
        let column = v_flex()
            .w_full()
            .min_w(px(0.))
            .child(h_flex().justify_end().child(check))
            .when_some(problem, |column, problem| {
                column.child(ui::error_line(problem, palette).mt(px(4.)))
            })
            .when(!crate::desktop_settings::writable(), |column| {
                column.child(ui::footnote(crate::desktop_settings::READ_ONLY, palette))
            });
        let access = match &self.access {
            Load::Loading => return column.child(ui::skeleton(4, palette)),
            Load::Failed(error) => {
                return column
                    .child(ui::failed(error.clone(), palette))
                    .child(h_flex().justify_center().child(
                        ui::button("agents-retry", "Try again", palette).on_click(cx.listener(|this, _, _, cx| {
                            this.access = Load::Loading;
                            this.load(cx);
                            cx.notify();
                        })),
                    ));
            }
            Load::Ready(access) => access.clone(),
        };
        if access.agents.is_empty() {
            return column.child(ui::empty(
                "No coding agents",
                "This Gateway lists no coding agents.",
                palette,
            ));
        }
        let mut column = column;
        for (n, entry) in access.agents.iter().enumerate() {
            column = column.child(self.render_section(entry, &access, n == 0, cx));
        }
        column
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Health, LocalAgent, Login, PlanUsage, Route, SessionAgent, SetupOptions, Value, agent_setup,
        checklist_provider, computer_line, from_label, heading_status, pays_line, plan_locked, read_access,
        route_button, session_local,
    };
    use nexus_client::agent_setup::SetupStep;

    #[test]
    fn checklist_offers_browser_login_only_for_incomplete_managed_plan_steps() {
        let mut step = SetupStep {
            id: "plan_login",
            who: "you",
            text: String::new(),
            command: None,
            done: None,
        };
        for id in ["plan_login", "plan_relogin"] {
            step.id = id;
            for done in [None, Some(false)] {
                step.done = done;
                assert_eq!(checklist_provider(&step, "claude"), Some(SessionAgent::Claude));
                assert_eq!(checklist_provider(&step, "codex"), Some(SessionAgent::Codex));
                assert_eq!(checklist_provider(&step, "antigravity"), None);
            }
        }
        step.done = Some(true);
        assert_eq!(checklist_provider(&step, "claude"), None);
        step.done = Some(false);
        for id in ["install", "nexus_login", "allow_subscriptions"] {
            step.id = id;
            assert_eq!(checklist_provider(&step, "codex"), None);
        }
    }

    #[test]
    fn connect_on_open_starts_one_sign_in_on_the_next_open_only() {
        assert_eq!(super::take_connect_on_open(), None);
        super::connect_on_open(SessionAgent::Codex);
        assert_eq!(super::take_connect_on_open(), Some(SessionAgent::Codex));
        // A later open of the page, from the settings list, starts nothing.
        assert_eq!(super::take_connect_on_open(), None);
    }

    fn answer() -> Value {
        serde_json::from_str(
            r#"{
            "agentSubscriptions": true,
            "agents": [
                {
                    "agent": "claude", "name": "Claude Code", "install": "npm i -g @anthropic-ai/claude-code",
                    "route": "subscription", "from": "preference",
                    "routes": [
                        {"route": "gateway", "access": "byok", "available": true, "provider": "anthropic"},
                        {"route": "subscription", "access": "subscription", "available": true, "plan": "Claude",
                         "vendor": "Anthropic", "login": "claude /login"}
                    ]
                },
                {
                    "agent": "codex", "name": "Codex", "route": "gateway", "from": "default",
                    "routes": [
                        {"route": "gateway", "access": null, "available": false, "provider": "openai"},
                        {"route": "subscription", "available": false, "plan": "ChatGPT",
                         "reason": "Your organization turned subscription mode off."}
                    ]
                },
                {"name": "no id"}
            ]
        }"#,
        )
        .expect("JSON")
    }

    #[test]
    fn local_agent_catalog_preserves_plans_but_cannot_save_gateway_routes() {
        let access = super::local_access();
        assert!(access.unsupported && access.agent_subscriptions);
        assert!(!access.agents.is_empty());
        for entry in access.agents {
            assert_eq!(entry.route, Route::Subscription);
            assert!(
                entry
                    .routes
                    .iter()
                    .any(|route| route.route == Route::Subscription && route.available)
            );
        }
    }

    #[test]
    fn reads_gateways_answer_in_plain_words() {
        let access = read_access(&answer());
        assert!(access.agent_subscriptions);
        assert!(!access.unsupported);
        assert_eq!(access.agents.len(), 2, "an entry with no agent is dropped");
        let claude = &access.agents[0];
        assert_eq!(claude.route, Route::Subscription);
        assert_eq!(from_label(&claude.from), "Your choice");
        assert_eq!(route_button(claude, Route::Gateway), "Nexus");
        assert_eq!(route_button(claude, Route::Subscription), "Claude plan");
        assert_eq!(
            pays_line(claude),
            "Your Claude plan pays. Nexus records each session but does not bill it."
        );
        assert_eq!(plan_locked(claude, &access), None);
        let codex = &access.agents[1];
        assert_eq!(
            pays_line(codex),
            "Your organization's OpenAI key pays. Nobody added the key yet, so Nexus refuses the calls from Codex."
        );
        assert_eq!(
            plan_locked(codex, &access).as_deref(),
            Some("Your organization turned subscription mode off.")
        );
    }

    #[test]
    fn a_gateway_without_routes_gets_the_catalog() {
        let access = read_access(&Value::Null);
        assert!(access.unsupported);
        assert!(access.agents.iter().any(|entry| entry.agent == "claude"));
    }

    /// The desktop runs Antigravity: every agent Gateway lists has a card, as does one this build does not know.
    #[test]
    fn every_listed_agent_has_a_card() {
        let mut answer = answer();
        if let Some(agents) = answer["agents"].as_array_mut() {
            agents.push(serde_json::json!({ "agent": "antigravity", "name": "Antigravity", "routes": [] }));
            agents.push(serde_json::json!({ "agent": "devin", "name": "Devin", "routes": [] }));
        }
        let shown: Vec<String> = read_access(&answer)
            .agents
            .into_iter()
            .map(|entry| entry.agent)
            .collect();
        assert_eq!(shown, ["claude", "codex", "antigravity", "devin"]);
    }

    #[test]
    fn the_computer_line_names_the_version_and_the_login() {
        let access = read_access(&answer());
        let mut local = LocalAgent {
            agent: "claude".into(),
            installed: true,
            path: Some("/bin/claude".into()),
            version: Some("2.1.0".into()),
            login: Some(Login::ApiKey),
            plan: None,
        };
        assert_eq!(
            computer_line(&access.agents[0], Some(&local)),
            (
                "Claude Code is signed in with an API key. The calls bill the API, not your Claude plan.".into(),
                Health::NeedsYou
            )
        );
        local.login = Some(Login::Plan);
        local.plan = Some("Claude Max".into());
        assert_eq!(
            computer_line(&access.agents[0], Some(&local)),
            (
                "Version 2.1.0 is installed and signed in to Claude Max.".into(),
                Health::Fine
            )
        );
        local.installed = false;
        assert_eq!(
            computer_line(&access.agents[0], Some(&local)),
            ("Claude Code is not installed.".into(), Health::Missing)
        );
    }

    fn plan(agent: SessionAgent, account: Option<&str>, signed_in: bool, active: bool) -> PlanUsage {
        PlanUsage {
            agent,
            account: account.map(String::from),
            plan: Some("Claude Team".into()),
            signed_in,
            active,
            ..nexus_workbench::plans::read_codex(None, &serde_json::json!({"account": null}), None)
        }
    }

    fn signed_out(agent: &str) -> LocalAgent {
        LocalAgent {
            agent: agent.into(),
            installed: true,
            path: None,
            version: Some("2.1.323".into()),
            login: Some(Login::SignedOut),
            plan: None,
        }
    }

    /// The agent's own folder is signed out, but new sessions start on a signed-in Active account.
    #[test]
    fn a_signed_in_active_account_finishes_the_plan_step() {
        let access = read_access(&answer());
        let entry = &access.agents[0];
        let plans = [
            plan(SessionAgent::Claude, None, false, false),
            plan(SessionAgent::Claude, Some("a_1"), true, true),
            plan(SessionAgent::Claude, Some("a_2"), true, false),
        ];
        let local = session_local(Some(&signed_out("claude")), &plans);
        assert_eq!(
            computer_line(entry, local.as_ref()),
            (
                "Version 2.1.323 is installed and signed in to Claude Team.".into(),
                Health::Fine
            )
        );
        let steps = agent_setup::setup_steps(
            &entry.raw,
            Route::Subscription,
            SetupOptions {
                local: local.as_ref(),
                signed_in: Some(true),
                agent_subscriptions: Some(true),
                cli_install: None,
            },
        );
        assert_eq!(steps.last().map(|step| step.done), Some(Some(true)));
    }

    #[test]
    fn every_plan_agent_reads_its_own_accounts() {
        for (name, agent) in [
            ("claude", SessionAgent::Claude),
            ("codex", SessionAgent::Codex),
            ("antigravity", SessionAgent::Antigravity),
        ] {
            let other = if agent == SessionAgent::Claude {
                SessionAgent::Codex
            } else {
                SessionAgent::Claude
            };
            let local = signed_out(name);
            // Another agent's login does not count.
            let elsewhere = [plan(other, None, true, false)];
            assert_eq!(session_local(Some(&local), &elsewhere), Some(local.clone()), "{name}");
            // Its own signed-in login does.
            let own = [plan(agent, None, true, false)];
            let read = session_local(Some(&local), &own);
            assert_eq!(read.and_then(|local| local.login), Some(Login::Plan), "{name}");
        }
    }

    #[test]
    fn signed_out_accounts_and_a_missing_agent_change_nothing() {
        let plans = [
            plan(SessionAgent::Claude, None, false, false),
            plan(SessionAgent::Claude, Some("a_1"), false, true),
        ];
        let local = signed_out("claude");
        assert_eq!(session_local(Some(&local), &plans), Some(local.clone()));
        let missing = LocalAgent {
            installed: false,
            ..local
        };
        let signed_in = [plan(SessionAgent::Claude, Some("a_1"), true, true)];
        assert_eq!(session_local(Some(&missing), &signed_in), Some(missing.clone()));
        assert_eq!(session_local(None, &signed_in), None);
    }

    #[test]
    fn the_heading_says_ready_steps_left_or_an_admin() {
        let step = |who: &'static str, done: Option<bool>| SetupStep {
            id: "x",
            who,
            text: "Do it".into(),
            command: None,
            done,
        };
        assert_eq!(heading_status(&[step("you", Some(true))]), ("Ready".into(), true));
        assert_eq!(
            heading_status(&[step("you", Some(false)), step("you", None)]),
            ("2 steps left".into(), false)
        );
        assert_eq!(
            heading_status(&[step("you", Some(true)), step("admin", Some(false))]),
            ("Needs an admin".into(), false)
        );
        let plans_off = SetupStep {
            id: super::plans_off::STEP,
            ..step("admin", Some(false))
        };
        assert_eq!(
            heading_status(&[step("you", Some(true)), plans_off.clone()]),
            ("Plans off".into(), false)
        );
        assert_eq!(
            super::plans_off::without_line(&[step("you", Some(false)), plans_off]).len(),
            1
        );
    }
}
