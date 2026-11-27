//! The New session blocker strip, with one fix at a time.
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::session_readiness::{self, Blocker, Facts};
use nexus_workbench::event::{SessionAgent, Via};

use super::no_key::NoKey;
use super::{View, Workspace, glyph};
use crate::agents::setup as agent_setup;
use crate::palette::Palette;
use crate::setup::{self, SetupLine};

#[derive(Clone)]
enum Fix {
    Install,
    Connect(SessionAgent),
    Retry(String),
    Here,
    Gateway,
    /// The organization's API for this one session: Auto found no room on the plan.
    ApiOnce,
    Plan,
    Recheck,
    LocalProviders,
    Copy(String),
    /// "Use <email>": the person's choice of plan account (None: the agent's own login).
    UseAccount(SessionAgent, Option<String>),
    /// "Sign in to another account": Settings › Agents, where Add account signs one in.
    OtherAccount,
    /// The answer about the project's own Claude settings and OpenCode config (`claude_trust.rs`).
    Trust(bool),
    /// Not a button: the item is a used-up budget, drawn as its setup line, with the person's plan ("Max plan") when
    /// it can start the session instead (`limit_line.rs`).
    Limit(Option<String>),
    /// Not a button: the item is a missing key, drawn as a setup line (`no_key.rs`).
    NoKey(NoKey),
}

/// The signed-in logins of `agent` in `plans`, one per account email: the choices of "Choose your … account".
fn distinct_logins(plans: &[nexus_workbench::plans::PlanUsage], agent: SessionAgent) -> Vec<(Option<String>, String)> {
    let mut logins: Vec<(Option<String>, String)> = Vec::new();
    for plan in plans.iter().filter(|plan| plan.agent == agent && plan.signed_in) {
        let Some(email) = plan.email.clone().filter(|email| !email.is_empty()) else {
            continue;
        };
        if !logins.iter().any(|(_, seen)| seen.eq_ignore_ascii_case(&email)) {
            logins.push((plan.account.clone(), email));
        }
    }
    logins
}

pub(super) struct Item {
    title: String,
    text: String,
    command: Option<String>,
    actions: Vec<(String, Fix)>,
    /// The agent's own setup, drawn as a setup line in place of the title, text and actions
    /// (`render_start_agent_step`).
    step: Option<AgentStep>,
}

/// The selected agent is not installed or not signed in (board P3).
#[derive(Clone, Debug, PartialEq)]
enum AgentStep {
    /// Nexus's own agent: Install puts the nexus CLI in place from Gateway.
    InstallNexus,
    /// An agent missing from `place`: its install command runs in the Workbench terminal, over ssh for a server.
    /// `settings`: it is this computer's, so "Coding agents ›" shows.
    Install {
        command: String,
        place: String,
        settings: bool,
    },
    /// Not signed in to its plan, or signed in with an API key: Sign in; `team_api` adds Use Team API.
    SignIn { team_api: bool },
    /// A setting only the person can change in a file (agy's API key provider): Check again once changed.
    Change,
}

impl Item {
    /// The missing key this item stands for: the strip draws it as a setup line, with its own fixes.
    pub(super) fn no_key(&self) -> Option<&NoKey> {
        self.actions.iter().find_map(|(_, fix)| match fix {
            Fix::NoKey(no_key) => Some(no_key),
            _ => None,
        })
    }
}

impl Workspace {
    pub(super) fn start_blocker_items(&self) -> Vec<Item> {
        if !matches!(self.view, View::NewCode) {
            return Vec::new();
        }
        if self.is_local() && (self.code_form.provider_loading() || self.code_form.provider_validation.is_some()) {
            return vec![Item {
                title: "Loading your local provider…".into(),
                text: String::new(),
                command: None,
                actions: Vec::new(),
                step: None,
            }];
        }
        if let Some(error) = self
            .failed_provider_draft()
            .or(self.code_form.provider_selection_error())
        {
            return vec![Item {
                title: "Choose a local provider".into(),
                text: format!("{error} Choose a provider in the model menu."),
                command: None,
                actions: vec![("Local providers".into(), Fix::LocalProviders)],
                step: None,
            }];
        }
        if self.code_form.needs_local_provider() {
            return vec![Item {
                title: "Choose a local provider".into(),
                text: "Select a saved provider in the model menu.".into(),
                command: None,
                actions: Vec::new(),
                step: None,
            }];
        }
        if let Some(text) = self.code_form.server_model_blocker() {
            return vec![Item {
                title: "Choose a server model".into(),
                text: text.into(),
                command: None,
                actions: Vec::new(),
                step: None,
            }];
        }
        let agent = self.code_form.agent;
        let name = nexus_workbench::driver::label(agent);
        let found = self.launch_harness_info(agent);
        if self.is_local() && self.code_form.provider.is_some() && found.is_none() {
            return vec![Item {
                title: format!("Checking whether {name} is installed"),
                text: String::new(),
                command: None,
                actions: vec![("Check again".into(), Fix::Recheck)],
                step: None,
            }];
        }
        let server = self
            .harness_machine()
            .and_then(|id| self.servers.iter().find(|server| server.id == id));
        let remote = self.harness_machine().is_some();
        let where_ = server.map_or("this computer", |server| server.name.as_str());
        let plan = crate::pages::subscriptions::vendor(agent);
        let plan_only = super::new_code::plan_only(agent);
        let local_only = nexus_workbench::harness::harness(agent).session_features().local_only;
        let organization = self
            .account
            .as_ref()
            .map_or("your organization", |account| account.org_name.as_str());
        let via = self.code_form.effective_via();
        let local = self
            .readiness
            .local
            .as_ref()
            .and_then(|local| local.as_ref().ok())
            .and_then(|local| local.iter().find(|local| local.agent == agent.as_str()));
        // A signed-in extra account (Claude or Codex) is a plan login even when the agent's own folder is signed out.
        let another_account = matches!(&self.plan_usage, Some((_, Ok(plans))) if plans.iter().any(|plan| plan.agent == agent && plan.account.is_some() && plan.signed_in));
        let checks_login = agent
            .coding()
            .is_none_or(|agent| agent.entry().sign_in.status_args.is_some());
        let login_needed = via == Via::Subscription && checks_login && !remote && !self.cloud_ui.on;
        let login_blocks = session_readiness::plan_login_blocks(
            login_needed,
            self.readiness.local_checking.is_some() && self.readiness.local.is_none(),
            matches!(self.readiness.local, Some(Err(_))),
            another_account,
            local,
        );
        let cloud_plan = self.cloud_ui.on && crate::cloud::words::plan_refused(agent, via);
        let use_plan = nexus_workbench::harness::harness(agent).plan_payer().is_some()
            && !cloud_plan
            && found.is_some_and(|agent| agent.path.is_some())
            && self
                .code_form
                .access
                .as_ref()
                .is_some_and(|access| access.plan_available(agent));
        let no_key =
            via == Via::Nexus
                && self.code_form.access.as_ref().is_some_and(|access| {
                    matches!(access.payer(agent, via, organization).0, crate::code_run::Payer::None)
                });
        let facts = Facts {
            agent_missing: !self.cloud_ui.on && found.is_some_and(|agent| agent.path.is_none()),
            offline: server
                .is_some_and(|server| !matches!(server.status, nexus_remote::machine::Status::Connected { .. })),
            cloud_plan,
            login_blocks,
            login_installed: local.is_some_and(|local| local.installed),
            no_key,
            budget: self.budget_line().is_some(),
        };
        // An agent that signs in on its own (agy) says how, in place of Nexus's sign-in.
        let api_key = local.is_some_and(|local| local.login == Some(nexus_client::local_agents::Login::ApiKey));
        let signs_in_itself = crate::agents::ui(agent).signs_in_itself(api_key, name, plan);
        let lost = self.readiness.lost_claude.as_ref().filter(|_| {
            agent == SessionAgent::Claude
                && local.is_some_and(|local| local.login == Some(nexus_client::local_agents::Login::SignedOut))
        });
        let mut items: Vec<Item> = session_readiness::blockers(facts).into_iter().map(|kind| {
            match kind {
                // Nexus's own agent (no coding agent) installs from Gateway with one press.
                Blocker::Install if agent.coding().is_none() && !remote => Item {
                    title: found.and_then(|agent| agent.problem.as_ref()).map_or_else(|| format!("{name} isn't installed on {where_}."), |_| "The nexus CLI here can't run Nexus's agent.".into()),
                    text: self.code_form.nexus_install_error.clone().or_else(|| found.and_then(|agent| agent.problem.clone()))
                        .unwrap_or_else(|| "Install puts the native nexus CLI in place from your Gateway.".into()),
                    command: None,
                    actions: Vec::new(),
                    step: Some(AgentStep::InstallNexus),
                },
                // The Workbench runs the install where the person sees it, over ssh for a server.
                Blocker::Install => {
                    let command = super::harness::install_command(agent, self.gateway.url(), server.map(|server| server.ssh.as_str()));
                    Item {
                        title: format!("{name} isn't installed on {where_}."),
                        text: String::new(),
                        command: Some(command.clone()),
                        actions: Vec::new(),
                        step: Some(AgentStep::Install { command, place: where_.to_string(), settings: !remote }),
                    }
                }
                Blocker::Offline => {
                    let server = server.expect("offline fact requires a server");
                    let view = crate::machine_view::status_view(&server.status);
                    Item { title: format!("{} is {}", server.name, view.label.to_lowercase()),
                        text: "Sessions on this server are paused, not lost. They pick up where they stopped once it is back.".into(),
                        command: None, actions: vec![("Retry".into(), Fix::Retry(server.id.clone())), ("Run on this computer".into(), Fix::Here)], step: None }
                }
                Blocker::CloudPlan if plan_only => Item { title: format!("{name} doesn't run on Cloud"),
                    text: format!("It runs on your {plan} plan only, and that plan's sign-in stays on this computer."),
                    command: None, actions: vec![("Run on this computer".into(), Fix::Here)], step: None },
                Blocker::CloudPlan => Item { title: format!("{name} on Cloud runs on Team API"),
                    text: format!("Your {plan} plan can't sign in on a cloud machine, so its sign-in stays on this computer."),
                    command: None, actions: vec![("Run on this computer".into(), Fix::Here), ("Use Team API".into(), Fix::Gateway)], step: None },
                // agy signs in on its own first start: `agy` runs in the Workbench terminal. Its API key setting is
                // a file only the person changes.
                Blocker::PlanLogin if signs_in_itself.is_some() => {
                    let (title, text) = signs_in_itself.clone().unwrap_or_default();
                    let step = if api_key { AgentStep::Change } else { AgentStep::SignIn { team_api: false } };
                    Item { title: format!("{title}."), text, command: None, actions: Vec::new(), step: Some(step) }
                }
                // A login that was signed in and is not now names its account, and offers the sign-in again; Nexus does
                // not move to another account by itself.
                Blocker::PlanLogin if lost.is_some() => Item {
                    title: format!("{} is signed out of {name}", lost.map_or("", String::as_str)),
                    text: format!("Its saved sign-in no longer works: {name} refused it, or it signed out elsewhere. Nexus did not move to another account."),
                    command: None,
                    actions: vec![("Sign in again".into(), Fix::Connect(agent)), ("Check again".into(), Fix::Recheck), ("Use Team API".into(), Fix::Gateway)],
                    step: None,
                },
                // Without an account there is no Team API to offer.
                Blocker::PlanLogin => Item {
                    title: if api_key {
                        format!("{name} is signed in with an API key, not your {plan} plan.")
                    } else { format!("{name} isn't signed in to your {plan} plan.") },
                    text: String::new(),
                    command: None,
                    actions: Vec::new(),
                    step: Some(AgentStep::SignIn { team_api: !plan_only && !self.is_local() }),
                },
                Blocker::NoKey => {
                    let provider = super::no_key::provider(agent);
                    // "Use Max plan": the plan's name as the payer chip says it.
                    let plan = use_plan.then(|| self.code_form.access.as_ref().map(|access| access.payer(agent, Via::Subscription, organization).1)).flatten();
                    let no_key = NoKey { provider: provider.to_string(), plan };
                    Item { title: super::no_key::what(organization, provider), text: String::new(), command: None,
                        actions: vec![(String::new(), Fix::NoKey(no_key))], step: None }
                }
                Blocker::Budget => {
                    // "Use Max plan": the plan's name as the payer chip says it.
                    let plan = use_plan.then(|| self.code_form.access.as_ref().map(|access| access.payer(agent, Via::Subscription, organization).1)).flatten();
                    Item { title: self.budget_line().unwrap_or_default(), text: String::new(), command: None,
                        actions: vec![(String::new(), Fix::Limit(plan))], step: None }
                }
            }
        }).collect();
        if local_only && remote {
            items.insert(
                0,
                Item {
                    title: format!("{name} runs on this computer only"),
                    text: format!("Its {plan} sign-in stays on this computer. Pick another agent for {where_}."),
                    command: None,
                    actions: vec![("Run on this computer".into(), Fix::Here)],
                    step: None,
                },
            );
        }
        // More than one login of the agent on this computer, and the person chose none: they pick, Nexus never does.
        if via == Via::Subscription
            && !remote
            && !self.cloud_ui.on
            && !crate::pages::subscriptions::chosen_by_person(agent)
            && let Some((_, Ok(plans))) = &self.plan_usage
        {
            let logins = distinct_logins(plans, agent);
            // One login another organization uses: this organization does not take it over without asking.
            let claimed = logins
                .iter()
                .find(|(account, _)| crate::pages::subscriptions::claimed_elsewhere(agent, account))
                .map(|(_, email)| email.clone());
            if logins.len() > 1 || claimed.is_some() {
                let text = match &claimed {
                    Some(email) => format!(
                        "Another organization uses {email}. {organization} does not take it over: pick the account its sessions use."
                    ),
                    None => format!(
                        "This computer has {} {plan} logins. Pick the one new sessions use. Nexus never switches it for you.",
                        logins.len()
                    ),
                };
                let mut actions: Vec<(String, Fix)> = logins
                    .into_iter()
                    .map(|(account, email)| (format!("Use {email}"), Fix::UseAccount(agent, account)))
                    .collect();
                actions.push(("Sign in to another account".into(), Fix::OtherAccount));
                items.insert(
                    0,
                    Item {
                        title: if claimed.is_some() {
                            format!("Which {plan} account does {organization} use?")
                        } else {
                            format!("Choose your {plan} account")
                        },
                        text,
                        command: None,
                        actions,
                        step: None,
                    },
                );
            }
        }
        if let Some((title, text)) = self.claude_trust_question() {
            items.insert(
                0,
                Item {
                    title,
                    text,
                    command: None,
                    actions: vec![
                        ("Trust".into(), Fix::Trust(true)),
                        ("Don't trust".into(), Fix::Trust(false)),
                    ],
                    step: None,
                },
            );
        }
        if let Some(model) = self.form_on_auto().then(|| self.auto_route.needs_api(via)).flatten() {
            let model = crate::code::model_label(model);
            items.push(Item {
                title: format!("Your {plan} plan has no room for this task"),
                text: format!(
                    "Auto would run {model} on {organization}'s API. It runs only if you choose it, for this session \
                     only."
                ),
                command: None,
                actions: vec![(format!("Run on {organization}'s API"), Fix::ApiOnce)],
                step: None,
            });
        }
        // With plans off, Antigravity has no route at all: the setup line over the composer says so
        // (`plans_off.rs`), and nothing else is worth fixing first.
        if self.plans_off_stops_start() {
            items.clear();
        }
        items
    }

    fn plan_login_waiting(&self) -> bool {
        matches!(self.view, View::NewCode)
            && !self.cloud_ui.on
            && self.harness_machine().is_none()
            && self.code_form.effective_via() == Via::Subscription
            && self.readiness.local.is_none()
            && self.readiness.local_checking.is_some()
            && !(crate::agents::ui(self.code_form.agent).extra_account_starts()
                && matches!(&self.plan_usage, Some((_, Ok(plans))) if plans.iter().any(|plan| plan.agent == self.code_form.agent && plan.account.is_some() && plan.signed_in)))
    }

    pub(super) fn start_blocked(&self) -> bool {
        self.plan_login_waiting()
            || self.auto_agent_pending()
            || self.plans_off_stops_start()
            || !self.start_blocker_items().is_empty()
    }

    pub(super) fn render_start_blockers(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let items = self.start_blocker_items();
        if items.is_empty() {
            let waiting = if self.plan_login_waiting() {
                Some("Checking your plan sign-in…")
            } else if self.auto_agent_pending() {
                Some("Auto is choosing the agent…")
            } else {
                None
            };
            return waiting.map(|text| {
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child(text)
                    .into_any_element()
            });
        }
        let index = self.code_form.blocker_index.min(items.len() - 1);
        let item = &items[index];
        if let Some(step) = &item.step {
            return Some(self.render_start_agent_step(item, step, index, items.len(), palette, cx));
        }
        let pager = self.blocker_pager(index, items.len(), cx);
        // A used-up budget is a setup line: the amber dot, the account, and Raise limit or Ask (`limit_line.rs`).
        if let Some((_, Fix::Limit(plan))) = item.actions.first() {
            return Some(self.render_start_limit(plan.clone(), pager, palette, cx));
        }
        // A missing key is a setup line: what is missing, naming the account, and its fixes on the right.
        if let Some(no_key) = item.no_key().cloned() {
            let line = self.no_key_line(&no_key, cx);
            let fixer = self.fixer(cx);
            return Some(
                v_flex()
                    .px(px(8.))
                    .pt(px(8.))
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .children(pager.map(|pager| h_flex().justify_end().child(pager)))
                    .child(line.render(&fixer, palette, cx))
                    .into_any_element(),
            );
        }
        let head = h_flex()
            .gap(px(8.))
            .child(glyph(Lucide::Info, palette.muted).size(px(16.)))
            .child(div().flex_1().text_color(palette.foreground).child(item.title.clone()))
            .children(pager);
        let actions = item.actions.iter().cloned().enumerate().map(|(index, (label, fix))| {
            div()
                .id(("blocker-action", index))
                .cursor_pointer()
                .rounded(px(6.))
                .px(px(6.))
                .py(px(3.))
                .text_color(palette.foreground)
                .hover(|style| style.bg(palette.hover))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.fix_start(fix.clone(), window, cx)))
        });
        let mut body = v_flex()
            .px(px(14.))
            .py(px(10.))
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(palette.muted)
            .border_b_1()
            .border_color(palette.border)
            .child(head)
            .when(!item.text.is_empty(), |body| {
                body.child(super::message_text::MessageText::new("blocker-text", item.text.clone()))
            })
            .child(h_flex().gap(px(8.)).flex_wrap().children(actions));
        if let Some(command) = &item.command {
            let command = command.clone();
            let copy = command.clone();
            body = body.child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .font_family("monospace")
                            .child(command),
                    )
                    .child(
                        div()
                            .id("blocker-copy-command")
                            .cursor_pointer()
                            .child("Copy")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.fix_start(Fix::Copy(copy.clone()), window, cx)
                            })),
                    ),
            );
        }
        Some(body.into_any_element())
    }

    /// "‹ 2 of 3 ›" when more than one item blocks.
    fn blocker_pager(&self, index: usize, len: usize, cx: &mut Context<Self>) -> Option<Div> {
        if len < 2 {
            return None;
        }
        let previous = index.saturating_sub(1);
        let next = (index + 1).min(len - 1);
        let arrow = |id: &'static str, label: &'static str, glyph: &'static str, to: usize, cx: &mut Context<Self>| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .cursor_pointer()
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx))
                .child(glyph)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.code_form.blocker_index = to;
                    cx.notify();
                }))
        };
        Some(
            h_flex()
                .gap(px(8.))
                .child(arrow("blocker-previous", "Previous setup item", "‹", previous, cx))
                .child(format!("{} of {}", index + 1, len))
                .child(arrow("blocker-next", "Next setup item", "›", next, cx)),
        )
    }

    /// The selected agent's install or sign-in, as a setup line with its command and Copy under it. When the terminal
    /// step ends well, the start checks again, so the line goes and Send turns on with the message still there.
    fn render_start_agent_step(
        &self,
        item: &Item,
        step: &AgentStep,
        index: usize,
        len: usize,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let agent = self.code_form.agent;
        let this = cx.entity().downgrade();
        let act = |fix: Fix| {
            let this = this.clone();
            setup::Fix::act(move |window, cx| {
                let fix = fix.clone();
                if let Err(error) = this.update(cx, |workspace, cx| workspace.fix_start(fix, window, cx)) {
                    eprintln!("nexus-desktop-gpui: start setup: {error}");
                }
            })
        };
        let key = format!("start:new:{}", agent.as_str());
        let mut command = None;
        let line = match step {
            AgentStep::InstallNexus => {
                let label = if self.code_form.installing_nexus {
                    "Installing…"
                } else {
                    "Install"
                };
                SetupLine::new(key, Lucide::SquareTerminal, item.title.clone())
                    .why(item.text.clone())
                    .fix(label, act(Fix::Install))
            }
            AgentStep::Install {
                command: install,
                place,
                settings,
            } => {
                command = Some(install.clone());
                agent_setup::missing_line(key, agent, place, Some(install), *settings)
            }
            AgentStep::SignIn { team_api } => {
                command = (!agent_setup::signs_in_here(agent))
                    .then(|| agent_setup::login_command(agent).map(str::to_owned))
                    .flatten();
                let line = agent_setup::signed_out_line(key, agent, item.title.clone(), None, true);
                if *team_api {
                    line.fix("Use Team API", act(Fix::Gateway))
                } else {
                    line
                }
            }
            AgentStep::Change => SetupLine::new(key, Lucide::KeyRound, item.title.clone())
                .why(item.text.clone())
                .fix("Check again", act(Fix::Recheck))
                .fix("Coding agents ›", agent_setup::open_agent(agent)),
        };
        let recheck: super::agent_setup::Resume = Rc::new(move |window, cx| {
            if let Err(error) = this.update(cx, |workspace, cx| workspace.fix_start(Fix::Recheck, window, cx)) {
                eprintln!("nexus-desktop-gpui: start setup: {error}");
            }
        });
        // A sign-in in Settings ends there: the next look reads this computer's sign-ins again.
        let readiness = cx.entity().downgrade();
        let fixer = self.resuming_fixer(recheck, cx).before(move |_, fix, _, cx| {
            if matches!(fix, setup::Fix::SignIn(_))
                && let Err(error) = readiness.update(cx, |workspace, _| workspace.readiness.local_checked = None)
            {
                eprintln!("nexus-desktop-gpui: start setup: {error}");
            }
        });
        v_flex()
            .px(px(14.))
            .py(px(10.))
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(palette.muted)
            .border_b_1()
            .border_color(palette.border)
            .children(
                self.blocker_pager(index, len, cx)
                    .map(|pager| h_flex().justify_end().child(pager)),
            )
            .child(super::agent_setup::agent_card(
                line.main(),
                command.as_deref(),
                &fixer,
                palette,
                cx,
            ))
            .into_any_element()
    }

    fn fix_start(&mut self, fix: Fix, window: &mut Window, cx: &mut Context<Self>) {
        match fix {
            Fix::Install => self.install_nexus_harness(cx),
            // Settings › Agents opens the browser for the selected profile and offers Cancel while it waits.
            Fix::Connect(agent) => {
                crate::pages::connect_on_open(agent);
                self.open_settings(super::SettingsSection::Page(crate::pages::PageId::Agents), window, cx);
            }
            Fix::Retry(machine) => {
                if let Err(error) = crate::machines::get().reconnect(&machine) {
                    self.composer_note = Some(error.into());
                }
            }
            Fix::Here => self.run_on(None, window, cx),
            Fix::Gateway => self.code_form.set_via(Some(Via::Nexus)),
            Fix::ApiOnce => self.code_form.api_once = true,
            Fix::Plan => self.code_form.set_via(Some(Via::Subscription)),
            Fix::LocalProviders => self.open_settings(super::SettingsSection::LocalProviders, window, cx),
            Fix::Recheck => {
                self.plan_usage = None;
                self.probe_plans(cx);
                self.recheck_readiness(cx);
            }
            Fix::Copy(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.composer_note = Some("Copied".into());
            }
            Fix::OtherAccount => {
                self.open_settings(super::SettingsSection::Page(crate::pages::PageId::Agents), window, cx);
            }
            Fix::UseAccount(agent, account) => {
                let saving = crate::pages::subscriptions::choose(agent, account);
                cx.spawn(async move |this, cx| {
                    let saved = saving.await;
                    let _ = this.update(cx, |this, cx| {
                        match saved {
                            Ok(settings) => {
                                this.settings_saved(&settings);
                                this.plan_usage = None;
                                this.probe_plans(cx);
                                this.recheck_readiness(cx);
                            }
                            Err(error) => this.composer_note = Some(error.into()),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            Fix::Trust(trusted) => self.answer_claude_trust(trusted, cx),
            // Its setup line runs its own fixes.
            Fix::Limit(_) => {}
            Fix::NoKey(_) => {}
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentStep, Fix};
    use crate::code_run::AgentAccess;
    use crate::palette::{Palette, ThemeChoice};
    use crate::workspace::{View, Workspace, machine_source::MachineSource};
    use nexus_client::local_agents::{LocalAgent, Login};
    use nexus_workbench::{
        api::AgentInfo,
        event::{SessionAgent, Via},
    };
    use std::sync::Arc;

    /// Auto on a plan with no room: the form asks before any paid run, and the organization's API applies to the next
    /// session only.
    #[gpui_kit::test]
    fn auto_with_no_plan_room_offers_the_api_for_one_session(cx: &mut gpui_kit::TestAppContext) {
        let _agents = crate::code_run::fixture_agents(Vec::new());
        cx.update(gpui_kit::init);
        let (gateway, _gateway_events) = crate::gateway::Gateway::start();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            Workspace::with_machine_source(
                Arc::new(gateway),
                events,
                arrivals,
                false,
                MachineSource::isolated(),
                window,
                cx,
            )
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.auth = crate::workspace::Auth::SignedIn;
            workspace.view = View::NewCode;
            workspace.cloud_ui.on = false;
            workspace.code_form.project = None;
            workspace.code_form.agent = SessionAgent::Claude;
            workspace.code_form.model = Some(crate::workspace::code_picker::AUTO.into());
            workspace.code_form.via = Some(Via::Subscription);
            workspace.code_form.access = Some(AgentAccess {
                subscriptions: true,
                answer: None,
            });
            workspace.code_agents = vec![AgentInfo {
                agent: SessionAgent::Claude,
                path: Some("/fixture/claude".into()),
                version: None,
                models: None,
                problem: None,
            }];
            workspace.readiness.local = Some(Ok(vec![LocalAgent {
                agent: "claude".into(),
                installed: true,
                path: Some("/fixture/claude".into()),
                version: None,
                login: Some(Login::Plan),
                plan: None,
            }]));
            // No answer yet: nothing blocks.
            assert!(workspace.start_blocker_items().is_empty());
            workspace.auto_route.answer = Some(nexus_client::auto_route::Answer::AskToPay {
                model: "claude-sonnet-5-5".into(),
                name: "anthropic/claude-sonnet-5.5".into(),
                access: Some("byok".into()),
                effort: None,
            });
            let items = workspace.start_blocker_items();
            assert_eq!(items.len(), 1);
            assert!(items[0].title.contains("no room"), "{}", items[0].title);
            assert!(items[0].text.contains("Sonnet 5.5"), "{}", items[0].text);
            assert!(matches!(items[0].actions.as_slice(), [(_, Fix::ApiOnce)]));
            assert!(!workspace.composer_send_enabled());

            workspace.fix_start(Fix::ApiOnce, window, cx);
            assert_eq!(workspace.code_form.effective_via(), Via::Nexus);
            assert!(workspace.start_blocker_items().is_empty());
            // Never remembered: the saved pick is still the plan.
            assert_eq!(workspace.code_form.via, Some(Via::Subscription));
            // Another agent, or the next session, asks again.
            workspace.code_form.take_agent(SessionAgent::Claude);
            assert!(!workspace.code_form.api_once);
            // Picking the plan again replaces the one-session API choice.
            workspace.fix_start(Fix::ApiOnce, window, cx);
            workspace.fix_start(Fix::Plan, window, cx);
            assert!(!workspace.code_form.api_once);
            assert_eq!(workspace.code_form.effective_via(), Via::Subscription);
            // Leaving the organization drops the old sign-in's preview.
            workspace.leave_org();
            assert!(workspace.auto_route.answer.is_none());

            // "Auto · any agent" still asking which agent runs: the send waits for the answer.
            workspace.auth = crate::workspace::Auth::SignedIn;
            workspace.view = View::NewCode;
            workspace.code_form.access = Some(AgentAccess {
                subscriptions: true,
                answer: None,
            });
            workspace.code_form.pick_auto_any();
            assert!(!workspace.start_blocked());
            workspace.auto_route.task = Some(gpui_kit::Task::ready(()));
            assert!(workspace.auto_agent_pending());
            assert!(workspace.start_blocked());
            workspace.auto_route.task = None;
            assert!(!workspace.start_blocked());
        });
    }

    #[gpui_kit::test]
    fn a_plan_login_blocker_disables_send_until_the_snapshot_is_signed_in(cx: &mut gpui_kit::TestAppContext) {
        let _agents = crate::code_run::fixture_agents(Vec::new());
        cx.update(gpui_kit::init);
        let (gateway, _gateway_events) = crate::gateway::Gateway::start();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            Workspace::with_machine_source(
                Arc::new(gateway),
                events,
                arrivals,
                false,
                MachineSource::isolated(),
                window,
                cx,
            )
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.view = View::NewCode;
            workspace.cloud_ui.on = false;
            workspace.code_form.project = None;
            workspace.code_form.agent = SessionAgent::Claude;
            workspace.code_form.via = Some(Via::Subscription);
            workspace.code_form.access = Some(AgentAccess {
                subscriptions: true,
                answer: None,
            });
            workspace.plan_usage = None;
            workspace.code_agents = vec![AgentInfo {
                agent: SessionAgent::Claude,
                path: Some("/fixture/claude".into()),
                version: None,
                models: None,
                problem: None,
            }];
            let mut local = LocalAgent {
                agent: "claude".into(),
                installed: true,
                path: Some("/fixture/claude".into()),
                version: None,
                login: Some(Login::SignedOut),
                plan: None,
            };
            workspace.readiness.local = Some(Ok(vec![local.clone()]));
            workspace
                .composer
                .update(cx, |input, cx| input.set_value("keep this draft", window, cx));
            workspace.composer_empty = false;

            let items = workspace.start_blocker_items();
            assert_eq!(items.len(), 1);
            assert!(items[0].title.contains("isn't signed in"));
            // Claude signs in from Settings › Coding agents, and Team API stays on offer.
            assert_eq!(items[0].step, Some(AgentStep::SignIn { team_api: true }));
            let palette = Palette::for_choice(ThemeChoice::Dark, gpui_kit::WindowAppearance::Dark);
            assert!(workspace.render_start_blockers(palette, cx).is_some());
            assert!(!workspace.composer_send_enabled());
            workspace.submit_code("keep this draft".into(), false, window, cx);
            assert_eq!(
                workspace.composer_note.as_deref(),
                Some("Resolve the session setup item above before sending.")
            );
            assert_eq!(workspace.composer.read(cx).value().as_ref(), "keep this draft");

            // A lost login: it names its account and offers its sign-in again, not another account.
            workspace.readiness.lost_claude = Some("ana@acme.dev".into());
            let items = workspace.start_blocker_items();
            assert_eq!(items.len(), 1);
            assert!(
                items[0].title.starts_with("ana@acme.dev is signed out of"),
                "{}",
                items[0].title
            );
            assert!(
                items[0].text.contains("did not move to another account"),
                "{}",
                items[0].text
            );
            assert_eq!(items[0].actions[0].0, "Sign in again");
            assert!(matches!(items[0].actions[0].1, Fix::Connect(SessionAgent::Claude)));
            workspace.readiness.lost_claude = None;

            local.login = Some(Login::Plan);
            workspace.readiness.local = Some(Ok(vec![local]));
            assert!(workspace.start_blocker_items().is_empty());

            // Two Claude logins on this computer and no choice yet: New session asks, one action per email.
            let login = |account: Option<&str>, email: &str| {
                nexus_workbench::plans::read_claude(
                    account.map(String::from),
                    &serde_json::json!({"account": {"email": email, "subscriptionType": "Claude Max"}}),
                    None,
                )
            };
            workspace.plan_usage = Some((
                std::time::Instant::now(),
                Ok(vec![
                    login(None, "ana@acme.dev"),
                    login(Some("a_1"), "ana@home.dev"),
                    // The same person twice is one choice.
                    login(Some("a_2"), "ANA@acme.dev"),
                ]),
            ));
            let items = workspace.start_blocker_items();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].title, "Choose your Claude account");
            let labels: Vec<&str> = items[0].actions.iter().map(|(label, _)| label.as_str()).collect();
            assert_eq!(
                labels,
                ["Use ana@acme.dev", "Use ana@home.dev", "Sign in to another account"]
            );
            assert!(matches!(
                &items[0].actions[1].1,
                Fix::UseAccount(SessionAgent::Claude, Some(id)) if id == "a_1"
            ));
            assert!(!workspace.composer_send_enabled(), "it starts on nobody's guess");
            // One login: nothing to choose.
            workspace.plan_usage = Some((std::time::Instant::now(), Ok(vec![login(None, "ana@acme.dev")])));
            assert!(workspace.start_blocker_items().is_empty());
            // One login that another organization uses is asked for, not taken over.
            let mut settings = nexus_desktop_core::settings::Settings::default();
            settings.remember_org_account("acme", "claude", None);
            crate::pages::subscriptions::note_chosen(&settings);
            let items = workspace.start_blocker_items();
            assert_eq!(items.len(), 1);
            assert!(
                items[0].title.starts_with("Which Claude account does "),
                "{}",
                items[0].title
            );
            assert!(
                items[0].text.contains("Another organization uses ana@acme.dev"),
                "{}",
                items[0].text
            );
            crate::pages::subscriptions::note_chosen(&Default::default());
            workspace.plan_usage = None;
            assert!(workspace.render_start_blockers(palette, cx).is_none());
            assert!(workspace.composer_send_enabled());

            // Antigravity: its own plan's words, no Team API fix, and nothing at all with plans off.
            workspace.code_form.agent = SessionAgent::Antigravity;
            workspace.code_agents.push(AgentInfo {
                agent: SessionAgent::Antigravity,
                path: Some("/fixture/agy".into()),
                version: None,
                models: None,
                problem: None,
            });
            let agy = LocalAgent {
                agent: "antigravity".into(),
                installed: true,
                path: Some("/fixture/agy".into()),
                version: None,
                login: Some(Login::ApiKey),
                plan: None,
            };
            workspace.readiness.local = Some(Ok(vec![agy]));
            let items = workspace.start_blocker_items();
            assert!(!items.is_empty());
            assert!(items[0].title.contains("Google AI plan"), "{}", items[0].title);
            assert!(items[0].text.contains("modelProvider"), "{}", items[0].text);
            // Its API key setting is a file only the person changes: no sign-in, no Team API.
            assert_eq!(items[0].step, Some(AgentStep::Change));
            assert!(
                items
                    .iter()
                    .flat_map(|item| &item.actions)
                    .all(|(_, fix)| !matches!(fix, Fix::Gateway))
            );
            // Signed out, agy signs in with its own command in the Workbench terminal.
            workspace.readiness.local = Some(Ok(vec![LocalAgent {
                agent: "antigravity".into(),
                installed: true,
                path: Some("/fixture/agy".into()),
                version: None,
                login: Some(Login::SignedOut),
                plan: None,
            }]));
            let items = workspace.start_blocker_items();
            assert_eq!(items[0].step, Some(AgentStep::SignIn { team_api: false }));
            assert!(workspace.render_start_blockers(palette, cx).is_some());
            workspace.code_form.access = Some(AgentAccess {
                subscriptions: false,
                answer: None,
            });
            // Plans off: the setup line says so in place of the strip, and the start waits.
            assert!(workspace.start_blocker_items().is_empty());
            assert!(workspace.plans_off_stops_start());
            assert!(workspace.render_plans_off(palette, cx).is_some());
            assert!(!workspace.composer_send_enabled());
            // A send pressed now is held, with no note, until plans are on and the plan is picked.
            workspace.composer_note = None;
            workspace.submit_code("keep this draft".into(), false, window, cx);
            assert_eq!(workspace.composer_note, None);
            assert!(crate::setup::state::waiting(&"plans:off:send".into(), cx));
            assert!(crate::setup::state::waiting(&crate::setup::plans_off::key(), cx));
            // Without a held send, plans on only picks the plan: the composer keeps its text, unsent.
            crate::setup::forget(&"plans:off:send".into(), cx);
            crate::setup::forget(&crate::setup::plans_off::key(), cx);
            workspace.plans_turned_on(window, cx);
            assert!(
                workspace
                    .code_form
                    .access
                    .as_ref()
                    .is_some_and(|access| access.subscriptions)
            );
            assert!(!workspace.plans_off_stops_start());
            assert_eq!(workspace.composer.read(cx).value().as_ref(), "keep this draft");
            assert!(workspace.code_form.starting.is_none(), "nothing was sent");
            workspace.code_form.access = Some(AgentAccess {
                subscriptions: false,
                answer: None,
            });

            // Antigravity signed in to its plan, with Cloud picked: its plan's sign-in stays here, so it does not start
            // there.
            workspace.code_form.access = Some(AgentAccess {
                subscriptions: true,
                answer: None,
            });
            workspace.readiness.local = Some(Ok(vec![LocalAgent {
                agent: "antigravity".into(),
                installed: true,
                path: Some("/fixture/agy".into()),
                version: None,
                login: Some(Login::Plan),
                plan: None,
            }]));
            assert!(workspace.start_blocker_items().is_empty());
            workspace.cloud_ui.on = true;
            let items = workspace.start_blocker_items();
            assert_eq!(items.len(), 1);
            assert!(items[0].title.ends_with("doesn't run on Cloud"), "{}", items[0].title);
            assert!(!workspace.composer_send_enabled());
        });
    }
}
