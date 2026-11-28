//! A new session, as the Nexus app's launch screen (`desktop/src/workbench/new-session.tsx`): the Nexus mark, "What
//! should we build?", and the context row under it (the project, the workspace and where it runs, as quiet chips with
//! menus). The composer follows, with one model chip in its bar (it opens the model panel: the agent and its model,
//! who pays, the reasoning and the permissions), and under it the project's three latest sessions. Enter starts the session and opens it.
//!
//! "No project" is the project menu's first choice (one kind of session): it is a Gateway chat, so the chat's
//! empty state draws this same head. Each pick is remembered per organization, as the Nexus app's
//! `session-defaults.ts` does, in this build's own settings file.

use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::driver::label;
use nexus_workbench::event::{Effort, PermissionMode, SessionAgent, Via};
use nexus_workbench::harness::EVERY_MODE;
use nexus_workbench::project_defaults::{ProjectDefaults, Workspace as WorksIn};
use serde_json::{Value, json};

use super::code_dock::{EFFORTS, MODES, menu_box, menu_label, menu_row, permission_copy};
use super::menu_side::Side;
use super::model_menu::{bottom_fade, maker_mark};
use super::place_links::{LinkNote, MissingRepo};
pub(super) use super::project_menu::ProjectPick;
use super::project_menu::{Badge, pick_row};
use super::{Auth, Place, View, Workspace, glyph};
use crate::code_run::{AgentAccess, NewSession, Payer};
use crate::motion::{self, TweenExt as _};
use crate::palette::Palette;

/// How many of the project's sessions show under the composer.
const RECENTS: usize = 3;

/// The model panel's size; its list scrolls inside it.
const LAUNCH_BODY: f32 = 352.;
const PANEL_WIDTH: f32 = 572.;

/// How far a menu under its chip drops as it opens (`motion::Phase::menu`: negative is down, away from the chip).
const DROP: f32 = -4.;

/// The new session's menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LaunchMenu {
    Project,
    Workspace,
    /// Where it runs: this computer, Cloud, or a server.
    Machine,
    /// The model panel: the agent and its model, who pays, the reasoning and the permissions.
    Model,
    /// The screen's background (`backdrop.rs`).
    Background,
}

/// The model panel's rows use the same policy for clicks, keys and shortcuts.
pub(super) type LaunchRow = super::code_picker::Row<SessionAgent>;

/// The form's choices.
pub(super) struct CodeForm {
    /// The project's id; `None` picks the first one when the form opens.
    pub project: Option<String>,
    pub agent: SessionAgent,
    /// `None` is the agent's default.
    pub model: Option<String>,
    /// `None` is the model's default.
    pub effort: Option<Effort>,
    pub mode: PermissionMode,
    /// Who pays, as picked or remembered for this agent; `None` follows Gateway's route ([`AgentAccess::default_via`]).
    pub via: Option<Via>,
    pub local_subscription: bool,
    /// The organization's API for the next session only, picked when Auto found no room on the plan. Never remembered.
    pub api_once: bool,
    /// Auto picks the agent too ("Auto · any agent"): the form's agent is the one Gateway named last, on Auto.
    pub auto_any: bool,
    pub(super) provider: Option<nexus_workbench::provider::InstanceId>,
    providers: Vec<LocalProviderChoice>,
    provider_load: Option<Task<()>>,
    pub(super) provider_validation: Option<Task<()>>,
    first_provider: Option<FirstProvider>,
    /// Work in a new worktree instead of the project's folder (git projects only).
    pub worktree: bool,
    /// Review a pull request instead: it is checked out in the reviews folder (`review_pick.rs`). Never remembered.
    pub review: super::review_pick::ReviewPick,
    /// The branch a new worktree starts from (`base_pick.rs`). Never remembered: branches come and go.
    pub base: super::base_pick::BasePick,
    /// The draft view (`draft-N`) whose session the runner is making, one at a time. Its answer names it
    /// (`CodeEvent::Created`, `CodeEvent::StartFailed`), so another window's start or another command's failure
    /// leaves it alone.
    pub starting: Option<String>,
    /// Who may pay, once Gateway has answered.
    pub access: Option<AgentAccess>,
    pub blocker_index: usize,
    /// The model panel's fold over the agents that can't run here is open (`missing_agents.rs`).
    pub missing_open: bool,
    pub installing_nexus: bool,
    pub nexus_install_error: Option<String>,
    pub harness: Option<super::harness::RemoteHarness>,
    pub setup_line: nexus_desktop_core::setup_line::State,
    /// The person last picked No project: New session opens a chat (`rememberPlace` in `session-defaults.ts`).
    pub plain: bool,
    /// The organization whose remembered choices the form holds.
    org: Option<String>,
    /// Those choices include `mode` for the shared pick and `agentModes` for agents with fewer modes.
    remembered: Value,
    /// The picked project's own defaults (Settings › Projects), which win over the remembered choices.
    defaults: ProjectDefaults,
}

enum FirstProvider {
    Loading(nexus_providers::InstanceId),
    Selected,
    ChooseAgain(String),
}

struct LocalProviderChoice {
    id: nexus_workbench::provider::InstanceId,
    name: String,
    agents: Vec<SessionAgent>,
    server_models: Option<Vec<String>>,
}

impl Default for CodeForm {
    fn default() -> Self {
        CodeForm {
            project: None,
            plain: false,
            agent: SessionAgent::Claude,
            model: None,
            effort: None,
            mode: nexus_workbench::harness::harness(SessionAgent::Claude).default_mode(),
            via: None,
            local_subscription: false,
            api_once: false,
            auto_any: false,
            provider: None,
            providers: Vec::new(),
            provider_load: None,
            provider_validation: None,
            first_provider: None,
            worktree: true,
            review: Default::default(),
            base: Default::default(),
            starting: None,
            access: None,
            blocker_index: 0,
            missing_open: false,
            installing_nexus: false,
            nexus_install_error: None,
            harness: None,
            setup_line: crate::paths::setup_line(),
            org: None,
            remembered: Value::Null,
            defaults: ProjectDefaults::default(),
        }
    }
}

impl CodeForm {
    pub(super) fn begin_first_provider(&mut self, id: Option<nexus_providers::InstanceId>) {
        self.first_provider = id.map(FirstProvider::Loading);
    }

    pub(super) fn finish_first_provider(&mut self) -> bool {
        if self.first_provider.take().is_none() {
            return false;
        }
        self.provider = None;
        self.take_agent(SessionAgent::Claude);
        true
    }

    pub(super) fn provider_selection_error(&self) -> Option<&str> {
        match &self.first_provider {
            Some(FirstProvider::ChooseAgain(error)) => Some(error),
            _ => None,
        }
    }

    pub(super) fn provider_loading(&self) -> bool {
        matches!(self.first_provider, Some(FirstProvider::Loading(_)))
    }

    pub(super) fn require_provider_reselection(&mut self, error: String) {
        self.provider = None;
        self.first_provider = Some(FirstProvider::ChooseAgain(error));
    }

    fn update_provider_choices(&mut self, choices: Vec<LocalProviderChoice>) {
        self.providers = choices;
        if let Some(FirstProvider::Loading(id)) = &self.first_provider {
            let id = id.clone();
            let eligible = self
                .providers
                .iter()
                .find(|choice| choice.id == id)
                .and_then(|choice| Some((*choice.agents.first()?, choice.server_models.is_some())));
            if let Some((agent, server)) = eligible {
                self.take_agent(agent);
                if server {
                    self.model = None;
                    self.effort = None;
                }
                self.provider = Some(id);
                self.first_provider = Some(FirstProvider::Selected);
            } else {
                self.require_provider_reselection("The previous provider is unavailable.".into());
            }
        } else if self.provider.as_ref().is_some_and(|id| {
            !self
                .providers
                .iter()
                .any(|choice| &choice.id == id && choice.agents.contains(&self.agent))
        }) {
            self.require_provider_reselection("The previous provider is unavailable.".into());
        }
    }

    fn selected_provider(&self) -> Option<&LocalProviderChoice> {
        let selected = self.provider.as_ref()?;
        self.providers.iter().find(|choice| &choice.id == selected)
    }

    fn server_models(&self) -> Option<&[String]> {
        self.selected_provider()?.server_models.as_deref()
    }

    pub(super) fn selected_server_model(&self) -> Option<&str> {
        match (self.server_models()?, self.model.as_deref()) {
            ([model], None) => Some(model),
            (_, selected) => selected,
        }
    }

    pub(super) fn clear_local_provider_choices(&mut self) {
        self.provider = None;
        self.providers.clear();
        self.provider_load = None;
        self.provider_validation = None;
        self.via = None;
    }

    pub(super) fn needs_local_provider(&self) -> bool {
        self.local_subscription && !local_plan_agent(self.agent) && self.provider.is_none()
    }

    pub(super) fn server_model_blocker(&self) -> Option<&'static str> {
        let models = self.server_models()?;
        match (models, self.model.as_deref()) {
            ([], _) => Some("Add a model to this server in Settings."),
            ([_], None) => None,
            (models, Some(model)) if models.iter().any(|saved| saved == model) => None,
            _ => Some("Choose one of this server's models in the model menu."),
        }
    }

    fn pick_catalog_model(&mut self, model: Option<String>) {
        if self.server_models().is_some() {
            self.provider = None;
        }
        self.model = model;
        self.remember_model();
    }

    fn pick_server_model(&mut self, model: &str) {
        if self
            .server_models()
            .is_some_and(|models| models.iter().any(|saved| saved == model))
        {
            self.model = Some(model.to_string());
        }
    }

    /// Take `org`'s remembered choices, over a first run's.
    fn restore(&mut self, org: &str) {
        let remembered = crate::paths::new_session_defaults(org);
        self.project = remembered["project"].as_str().map(String::from);
        self.plain = remembered["plain"] == true;
        self.org = Some(org.to_string());
        self.remembered = remembered;
        self.apply_defaults(ProjectDefaults::default());
        self.auto_any = self.remembered["autoAny"] == true;
        if self.auto_any {
            self.model = Some(super::code_picker::AUTO.into());
        }
    }

    /// The choices a new session in the picked project starts with: the remembered picks, then the project's own
    /// defaults over them (the Nexus app's `startingChoice`). Read again each time the project changes.
    pub(super) fn use_project(&mut self) {
        let defaults = self
            .project
            .as_deref()
            .map(crate::code_run::project_defaults)
            .unwrap_or_default();
        self.apply_defaults(defaults);
    }

    fn apply_defaults(&mut self, defaults: ProjectDefaults) {
        let first = CodeForm::default();
        let remembered = &self.remembered;
        let agent = defaults
            .agent
            .filter(|agent| agent.supported_on_current_platform())
            .or_else(|| remembered["agent"].as_str().and_then(SessionAgent::parse))
            .filter(|agent| agent.supported_on_current_platform())
            .filter(|agent| !self.local_subscription || local_plan_agent(*agent))
            .unwrap_or(if self.local_subscription {
                self.agent
            } else {
                first.agent
            });
        // Another project starts on its own workspace: a pull request picked is the old repository's.
        self.review.clear();
        self.worktree = match defaults.workspace {
            Some(workspace) => workspace == WorksIn::Worktree,
            None => match remembered["workspace"].as_str() {
                Some("folder") => false,
                Some("worktree") => true,
                _ => first.worktree,
            },
        };
        // A project's own agent or model wins over "Auto · any agent"; with none, the remembered Auto stays.
        let explicit = defaults.agent.is_some() || defaults.model.is_some();
        self.defaults = defaults;
        if self.provider.is_none() {
            self.take_agent(agent);
        }
        self.mode = self.saved_mode(self.agent);
        if explicit {
            self.auto_any = false;
        } else if self.auto_any {
            self.model = Some(super::code_picker::AUTO.into());
            self.effort = None;
        }
    }

    /// Switch to `agent`, with the model, effort and payer last picked for it, or the project's model when its default
    /// is this agent (`modelFor`).
    pub(super) fn take_agent(&mut self, agent: SessionAgent) {
        if mode_preference_agent(self.agent) != mode_preference_agent(agent) {
            self.mode = self.saved_mode(agent);
        }
        if self.agent != agent {
            self.provider = None;
        }
        self.agent = agent;
        (self.model, self.effort) = self.saved_pair(agent);
        self.via = self.saved_via(agent);
        self.api_once = false;
    }

    fn saved_mode(&self, agent: SessionAgent) -> PermissionMode {
        let saved = match mode_preference_agent(agent) {
            Some(agent) => &self.remembered["agentModes"][agent.as_str()],
            None => &self.remembered["mode"],
        };
        let preferred = self
            .defaults
            .mode
            .or_else(|| saved.as_str().and_then(PermissionMode::parse));
        mode_for_agent(agent, preferred)
    }

    /// The model and effort `take_agent` restores for `agent`.
    fn saved_pair(&self, agent: SessionAgent) -> (Option<String>, Option<Effort>) {
        let pair = &self.remembered["models"][agent.as_str()];
        let (model, keep_effort) = self.defaults.model_for(agent, pair["model"].as_str().map(String::from));
        let effort = pair["effort"].as_str().and_then(Effort::parse).filter(|_| keep_effort);
        (model, effort)
    }

    /// The payer pick `take_agent` restores for `agent`.
    fn saved_via(&self, agent: SessionAgent) -> Option<Via> {
        self.remembered["via"][agent.as_str()].as_str().and_then(Via::parse)
    }

    /// Who pays as picked for `agent`: the form's pick for its own agent, else the one saved for that agent, which
    /// picking one of its rows restores.
    pub(super) fn via_for(&self, agent: SessionAgent) -> Option<Via> {
        if agent == self.agent {
            self.via
        } else {
            self.saved_via(agent)
        }
    }

    /// Who pays for a row of `agent` in the model panel: the pick its segments mark, the route it runs on, and what
    /// pays on that route. Read from the row's agent, not the picked one, as the two can differ.
    pub(super) fn row_pay(&self, agent: SessionAgent, org: &str) -> RowPay {
        let pick = self.via_for(agent);
        let via = match &self.access {
            // Antigravity has no Gateway route: its plan, or nothing.
            _ if plan_only(agent) => Via::Subscription,
            // Nexus's own agent is no coding agent: it runs on Gateway only.
            _ if agent.coding().is_none() || !self.plans_allowed() => Via::Nexus,
            Some(access) => pick.unwrap_or_else(|| access.default_via(agent)),
            None => Via::Nexus,
        };
        let payer = self
            .access
            .as_ref()
            .map_or(Payer::Checking, |access| access.payer(agent, via, org).0);
        RowPay { pick, via, payer }
    }

    /// Whether this organization has opted out of repeated provider handoff notices.
    pub(super) fn skip_agent_switch_notice(&self) -> bool {
        self.remembered["skipAgentSwitchNotice"].as_bool().unwrap_or(false)
    }

    pub(super) fn remember_agent_switch_notice(&mut self, skip: bool) {
        if !self.remembered.is_object() {
            self.remembered = json!({});
        }
        self.remembered["skipAgentSwitchNotice"] = json!(skip);
        self.remember();
    }

    /// Remember the choices as they are now, for this organization's next new session.
    pub(super) fn remember(&mut self) {
        if !self.remembered.is_object() {
            self.remembered = json!({});
        }
        let agent = self.agent.as_str();
        // What the project's defaults set is not a pick: the remembered choice under it stays (`session-defaults.ts`).
        if self.defaults.mode != Some(self.mode) {
            self.store_mode(self.agent, self.mode);
        }
        let defaults = &self.defaults;
        // A saved provider's agent is scoped to this form, even after a project change.
        if self.provider.is_none() && defaults.agent != Some(self.agent) {
            self.remembered["agent"] = json!(agent);
        }
        let workspace = if self.worktree {
            WorksIn::Worktree
        } else {
            WorksIn::Folder
        };
        if defaults.workspace != Some(workspace) && !self.review.on {
            self.remembered["workspace"] = json!(if self.worktree { "worktree" } else { "folder" });
        }
        self.remembered["project"] = json!(self.project);
        self.remembered["plain"] = json!(self.plain);
        self.remembered["autoAny"] = json!(self.auto_any);
        if defaults.agent != Some(self.agent) || defaults.model != self.model {
            self.remember_pair(self.agent, self.model.clone(), self.effort);
        }
        if !self.remembered["via"].is_object() {
            self.remembered["via"] = json!({});
        }
        self.remembered["via"][agent] = json!(self.via.map(Via::as_str));
        self.save_picks();
    }

    /// An explicit model or effort pick also counts when the project supplied that model.
    fn remember_model(&mut self) {
        self.remember_pair(self.agent, self.model.clone(), self.effort);
        self.remember();
    }

    /// An explicit permission pick also counts when it matches the project's default.
    fn remember_mode(&mut self) {
        self.store_mode(self.agent, self.mode);
        self.remember();
    }

    fn store_mode(&mut self, agent: SessionAgent, mode: PermissionMode) {
        if !self.remembered.is_object() {
            self.remembered = json!({});
        }
        if let Some(agent) = mode_preference_agent(agent) {
            if !self.remembered["agentModes"].is_object() {
                self.remembered["agentModes"] = json!({});
            }
            self.remembered["agentModes"][agent.as_str()] = json!(mode.as_str());
        } else {
            self.remembered["mode"] = json!(mode.as_str());
        }
    }

    fn remember_pair(&mut self, agent: SessionAgent, model: Option<String>, effort: Option<Effort>) {
        if !self.remembered.is_object() {
            self.remembered = json!({});
        }
        if !self.remembered["models"].is_object() {
            self.remembered["models"] = json!({});
        }
        self.remembered["models"][agent.as_str()] = json!({ "model": model, "effort": effort.map(Effort::as_str) });
    }

    /// Save only the open session's explicit picks. Its other settings are not new-session defaults.
    pub(super) fn remember_session_pick(
        &mut self,
        agent: SessionAgent,
        pair: Option<(Option<String>, Option<Effort>)>,
        mode: Option<PermissionMode>,
    ) {
        if let Some((model, effort)) = pair {
            if agent == self.agent {
                self.model = model.clone();
                self.effort = effort;
            }
            self.remember_pair(agent, model, effort);
        }
        if let Some(mode) = mode {
            self.store_mode(agent, mode);
            if mode_preference_agent(agent) == mode_preference_agent(self.agent) {
                self.mode = mode_for_agent(self.agent, Some(mode));
            }
        }
        self.save_picks();
    }

    fn save_picks(&self) {
        if let Some(org) = &self.org {
            crate::paths::save_new_session_defaults(org, self.remembered.clone());
        }
    }

    /// The starred models, in the order they were starred: stored as `<agent>/<model id>`, `<agent>/` for the
    /// agent's default.
    pub(super) fn favorites(&self) -> Vec<LaunchRow> {
        self.remembered["favorites"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|key| {
                let (agent, model) = key.split_once('/')?;
                let agent = SessionAgent::parse(agent).filter(|agent| agent.supported_on_current_platform())?;
                Some(LaunchRow {
                    agent,
                    model: (!model.is_empty()).then(|| model.to_string()),
                })
            })
            .collect()
    }

    /// Star a model, or unstar it, and remember that.
    fn toggle_favorite(&mut self, row: &LaunchRow) {
        let key = format!("{}/{}", row.agent.as_str(), row.model.as_deref().unwrap_or(""));
        if !self.remembered.is_object() {
            self.remembered = json!({});
        }
        let mut keys: Vec<Value> = self.remembered["favorites"].as_array().cloned().unwrap_or_default();
        match keys.iter().position(|stored| stored.as_str() == Some(key.as_str())) {
            Some(at) => {
                keys.remove(at);
            }
            None => keys.push(json!(key)),
        }
        self.remembered["favorites"] = json!(keys);
        self.remember();
    }

    /// Whether the organization lets sessions run on a person's own plan (false until Gateway answers).
    fn plans_allowed(&self) -> bool {
        self.access.as_ref().is_some_and(|access| access.subscriptions)
    }

    /// "Auto · any agent": Gateway picks the agent too, on Auto. Remembered. Auto runs on Gateway, so the pick drops a
    /// saved local provider.
    pub(super) fn pick_auto_any(&mut self) {
        self.provider = None;
        self.auto_any = true;
        self.model = Some(super::code_picker::AUTO.into());
        self.effort = None;
        self.api_once = false;
        self.remember();
    }

    /// The agent Gateway named for "Auto · any agent": the form runs it on Auto, paid as [`CodeForm::across_via`]. Not a
    /// pick of the person's, so nothing of that agent's saved choices, its payer included, replaces Auto.
    pub(super) fn take_auto_agent(&mut self, agent: SessionAgent) {
        if mode_preference_agent(self.agent) != mode_preference_agent(agent) {
            self.mode = self.saved_mode(agent);
        }
        if self.agent != agent {
            self.provider = None;
        }
        self.agent = agent;
        self.model = Some(super::code_picker::AUTO.into());
        self.effort = None;
    }

    /// Who pays when Auto picks the agent: the person's plan first unless the organization turned plans off, or the
    /// person chose its API for this session.
    pub(super) fn across_via(&self) -> Via {
        let plans = self.access.as_ref().is_some_and(|access| access.subscriptions);
        if self.api_once || !plans {
            Via::Nexus
        } else {
            Via::Subscription
        }
    }

    /// Who pays, as the person picks it: their pick replaces a one-session API choice, and is remembered.
    pub(super) fn set_via(&mut self, via: Option<Via>) {
        self.auto_any = false;
        self.via = via;
        self.api_once = false;
        self.remember();
    }

    /// A plan runs its own agent, with that agent's saved model and effort.
    fn pick_plan(&mut self, agent: SessionAgent) {
        self.provider = None;
        if self.provider_selection_error().is_some() {
            self.first_provider = Some(FirstProvider::Selected);
        }
        if self.agent != agent {
            self.take_agent(agent);
        }
        self.via = Some(Via::Subscription);
        self.api_once = false;
        self.remember();
    }

    /// Who pays for the session this form starts: Team API when plans are off, else the pick, else Gateway's route.
    pub fn effective_via(&self) -> Via {
        if self.local_subscription {
            return if self.provider.is_some() {
                Via::Nexus
            } else {
                Via::Subscription
            };
        }
        // Nexus's own agent is no coding agent: it runs on Gateway only.
        if self.agent.coding().is_none() {
            return Via::Nexus;
        }
        // Antigravity has no Gateway route; with plans off, the launch says so.
        if plan_only(self.agent) {
            return Via::Subscription;
        }
        if self.auto_any {
            return self.across_via();
        }
        let Some(access) = self.access.as_ref().filter(|access| access.subscriptions) else {
            return Via::Nexus;
        };
        if self.api_once {
            return Via::Nexus;
        }
        self.via.unwrap_or_else(|| access.default_via(self.agent))
    }
}

/// A control in the model panel's right pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PanelPick {
    Effort(Option<Effort>),
    Mode(PermissionMode),
}

/// What an open session's update sends: its model, effort and mode, each `None` when unchanged.
pub(super) type SessionUpdate = (Option<Option<String>>, Option<Option<Effort>>, Option<PermissionMode>);

/// Who pays for one row of the model panel ([`CodeForm::row_pay`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RowPay {
    /// The pick its segments mark: `None` is Auto.
    pub pick: Option<Via>,
    /// The route the row would run on.
    pub via: Via,
    /// What pays on that route.
    pub payer: Payer,
}

impl RowPay {
    /// The Who pays segment to mark: the route that runs. A saved plan pick that can't run (plans off, or the Nexus
    /// agent) marks the API, which pays, so the segments and the budget line agree. While the answer is still being
    /// checked, the saved pick stays marked.
    pub(super) fn marked(&self) -> Option<Via> {
        match self.pick {
            Some(Via::Subscription) if self.via == Via::Nexus && self.payer != Payer::Checking => Some(Via::Nexus),
            pick => pick,
        }
    }
}

/// The project New session shows: `pick` while it is one of `projects`, else the first of them (None when there is
/// none). A pick from another organization is never one of them.
fn project_pick(pick: Option<&str>, projects: &[crate::code::CodeProject]) -> Option<String> {
    match pick {
        Some(id) if projects.iter().any(|project| project.id == id) => Some(id.to_string()),
        _ => projects.first().map(|project| project.id.clone()),
    }
}

impl Workspace {
    /// Keep open-session picks in this organization's defaults, even when New session has not opened yet.
    pub(super) fn remember_session_pick(
        &mut self,
        agent: SessionAgent,
        pair: Option<(Option<String>, Option<Effort>)>,
        mode: Option<PermissionMode>,
    ) {
        let org = self
            .account
            .as_ref()
            .map(|account| account.org.clone())
            .or_else(|| crate::desktop_settings::load().org);
        if let Some(org) = org {
            self.code_form.restore(&org);
            self.code_form.remember_session_pick(agent, pair, mode);
        }
    }

    /// New session (the sidebar's row, Ctrl+N), as the Nexus app's `new-session.tsx`: the project last picked, else the
    /// first project; a chat when the person last picked No project, or has no project and did not pick Cloud (Cloud
    /// needs no project here: its repositories come from GitHub).
    pub(super) fn start_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_local() {
            self.open_new_code(window, cx);
            return;
        }
        // Before the projects are read (the launch lands as soon as the account is known), "no project" may be wrong:
        // `settle_new_session` picks again once they are, while the screen is still untouched.
        self.new_session_unsettled = !self.code_overview_loaded;
        if self.new_session_plain() {
            self.new_chat(window, cx);
        } else {
            self.open_new_code(window, cx);
        }
    }

    /// Whether New session opens with No project: the person last picked it, or has no project and did not pick Cloud.
    fn new_session_plain(&self) -> bool {
        let org = self.account.as_ref().map(|account| account.org.clone());
        let plain = org.is_some_and(|org| crate::paths::new_session_defaults(&org)["plain"] == true);
        plain || (self.projects.is_empty() && !self.cloud_ui.on)
    }

    /// Once the projects are read, a New session opened before them picks again, so the launch and a later click
    /// land on the same screen. Only while nothing was typed, attached or opened there yet.
    pub(super) fn settle_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.new_session_unsettled || !self.code_overview_loaded {
            return;
        }
        self.new_session_unsettled = false;
        let shown_plain = match self.view {
            View::Chat if self.current.is_none() && self.entries.is_empty() => true,
            View::NewCode => false,
            _ => return,
        };
        let untouched = self.composer.read(cx).value().trim().is_empty()
            && self.draft.is_empty()
            && self.attaching == 0
            && self.overlay.launch_menu().is_none();
        if untouched && shown_plain != self.new_session_plain() {
            cx.defer_in(window, |this, window, cx| {
                // The pick replaces the first one in the history, so Back does not return to a screen never chosen.
                if this.history.len() == this.history_at + 1
                    && matches!(this.history.last(), Some(Place::Chat(None) | Place::NewCode))
                {
                    this.history.pop();
                    this.history_at = this.history_at.saturating_sub(1);
                }
                this.start_new_session(window, cx);
            });
        }
    }

    pub(super) fn open_new_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connection_save_pending(cx) {
            self.local_mode.connection = None;
        }
        if self.is_local()
            && !matches!(self.view, View::NewCode)
            && let Some(draft) = &self.code_form.starting
        {
            let draft = draft.clone();
            if self.code_form.finish_first_provider() {
                self.local_mode.pending_first_provider = Some(draft);
            }
        }
        self.open_new_code_form(window, cx);
    }

    pub(super) fn open_new_code_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.view, View::NewCode) {
            self.restore_kept_start(window, cx);
            self.focus_composer(window, cx);
            cx.notify();
            return;
        }
        self.visit(Place::NewCode);
        self.reset_chat();
        self.view = View::NewCode;
        self.load_diff_comments();
        self.title = SharedString::default();
        let org = self
            .account
            .as_ref()
            .map(|account| account.org.clone())
            .or_else(|| crate::desktop_settings::load().org);
        if !self.is_local()
            && let Some(org) = org
        {
            self.code_form.restore(&org);
        }
        self.code_form.project = project_pick(self.code_form.project.as_deref(), &self.projects);
        self.code_form.use_project();
        self.code_form.starting = None;
        self.code_form.api_once = false;
        self.probe_plans(cx);
        self.probe_readiness(cx);
        self.check_claude_trust(cx);
        self.refresh_auto_route(cx);
        self.refresh_harness(cx);
        if let Some(runner) = &self.runner {
            runner.load_agents();
            runner.load_access();
        }
        self.sync_draft(window, cx);
        // A failed cloud start restores into this target's draft.
        self.restore_kept_start(window, cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// The New session form's project must be one of `projects`. When the list changes under an open form (the
    /// organization's store just read), a pick that is gone gives way to the first project, as opening the form does.
    pub(super) fn repick_new_code_project(&mut self) {
        if !matches!(self.view, View::NewCode) {
            return;
        }
        let pick = project_pick(self.code_form.project.as_deref(), &self.projects);
        if pick != self.code_form.project {
            self.code_form.project = pick;
            self.code_form.use_project();
        }
    }

    /// A later launch: a plain one brings the window forward, a prompt link opens too once the account is known.
    pub(super) fn arrived(&mut self, arrival: crate::links::Arrival, window: &mut Window, cx: &mut Context<Self>) {
        window.activate_window();
        if let crate::links::Arrival::Link(url) = arrival {
            if self.is_local() || self.account.is_some() {
                self.open_launch_link(&url, window, cx);
            } else {
                self.pending_links.push(url);
            }
        }
    }

    pub(super) fn open_launch_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Signed out, the sign-in page shows: the link waits, and opens with the next account (`Event::Account`).
        // Local waits for no sign-in: the link says it needs an account (`open_prompt_link`).
        if !matches!(self.auth, Auth::SignedIn) && !self.is_local() {
            self.pending_links.push(url.to_string());
            return;
        }
        if crate::links::is_agent_setup(url) {
            if matches!(self.auth, Auth::SignedIn) {
                self.open_native(crate::pages::PageId::Agents, window, cx);
            }
            return;
        }
        self.open_prompt_link(url, window, cx);
    }

    /// Opens New session with a prompt link's draft: its prompt, the project with its repository, its agent and model.
    /// It never sends (`openPromptLink` in the Nexus app).
    pub(super) fn open_prompt_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.auth, Auth::SignedIn) {
            if self.is_local() {
                self.show_notice(
                    "Sign in to Nexus, then open the link again.",
                    None,
                    std::time::Duration::from_secs(8),
                    cx,
                );
            } else {
                // It opens after the sign-in by itself, as a launch link does.
                self.pending_links.push(url.to_string());
            }
            return;
        }
        let link = match crate::links::read(url) {
            Ok(link) => link,
            Err(problem) => {
                self.open_new_code(window, cx);
                self.link_note = LinkNote::new(vec![problem.to_string()], None);
                cx.notify();
                return;
            }
        };
        self.open_new_code(window, cx);
        let mut notes = link.notes.clone();
        let mut missing = None;
        if let Some(repo) = &link.repo {
            // Pinned first, then the one worked in last (`projectsFor`): `projects` is in that order already.
            let found = self.projects.iter().find(|project| {
                project
                    .remotes
                    .iter()
                    .filter_map(|remote| crate::links::repo_ref(remote))
                    .any(|remote| &remote == repo)
            });
            match found {
                Some(project) => {
                    self.code_form.project = Some(project.id.clone());
                    self.code_form.plain = false;
                    self.code_form.use_project();
                }
                // The note leads with it, and offers to clone it or to pick a project (`place_links.rs`).
                None => missing = Some(MissingRepo::new(&repo.host, &repo.path)),
            }
        }
        if let Some(agent) = link.agent
            && agent != self.code_form.agent
        {
            self.code_form.take_agent(agent);
        }
        if let Some(model) = link.model {
            self.code_form.model = Some(model);
            self.code_form.effort = None;
        }
        let prompt = link.prompt;
        // The link's project has its own draft: that is the one the link's prompt meets.
        self.sync_draft(window, cx);
        // A different draft already here goes to the stash first, so nothing typed is lost.
        if self.stash_for_link(&prompt, cx) {
            notes.push(format!(
                "Your earlier draft is in the stash ({}).",
                crate::shortcut_label::label(super::stash::STASH_KEY)
            ));
        }
        self.composer
            .update(cx, |state, cx| state.set_value(prompt, window, cx));
        self.link_note = LinkNote::new(notes, missing);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Reads the selected execution host's plans at most once a minute (the Nexus app's 60s refetch).
    pub(super) fn probe_plans(&mut self, cx: &mut Context<Self>) {
        if self.connection_open() {
            return;
        }
        let host = self
            .code_session()
            .and_then(|session| session.machine.clone())
            .or_else(|| {
                if matches!(self.view, View::NewCode) {
                    self.code_form
                        .project
                        .as_ref()
                        .and_then(|id| self.projects.iter().find(|p| &p.id == id))
                        .and_then(|p| p.machine.clone())
                } else {
                    None
                }
            });
        if self.plan_usage_host != host {
            self.plan_usage = None;
            self.plan_waiting.clear();
            self._plan_probe = None;
            self.plan_usage_host = host.clone();
            self.remote_plan_book = Default::default();
        }
        // This computer's plans show the plan book's news: a session's report, or Settings' newer reading.
        let changes = crate::plan_book::changes();
        if self.plan_book_seen != changes {
            self.plan_book_seen = changes;
            if let (None, Some((_, Ok(plans)))) = (&host, &mut self.plan_usage) {
                *plans = crate::plan_book::shown(plans);
            }
        }
        // agy reports no limits during a turn: an Antigravity session's limits are read again once it changed after the
        // last reading started with its turn over. Both times are its host's, so this computer's clock never meets them.
        let idle_antigravity = self
            .code_session()
            .filter(|session| {
                session.agent == nexus_workbench::event::SessionAgent::Antigravity
                    && session.status != crate::code::Status::Running
            })
            .map(|session| session.updated_at);
        let turn_ended = idle_antigravity.is_some_and(|at| self.plan_usage_turn.is_none_or(|read| at > read));
        let fresh = self
            .plan_usage
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < std::time::Duration::from_secs(60) && !turn_ended);
        // Tests never read the real plans: `off_thread` starts an OS thread the test scheduler cannot follow, and its
        // answer would wake a test task from outside, after the test, and abort the whole run.
        if fresh || self._plan_probe.is_some() || cfg!(test) {
            return;
        }
        self.plan_usage_turn = idle_antigravity;
        let local = host.is_none();
        self._plan_probe = Some(cx.spawn(async move |this, cx| {
            let plans = match host {
                Some(host) => {
                    let read = crate::machines::get().call(&host, "wb_plans", serde_json::json!({}));
                    crate::code_run::runtime()
                        .spawn(async move {
                            match tokio::time::timeout(std::time::Duration::from_secs(25), read).await {
                                Ok(Ok(Ok(value))) => serde_json::from_value(value).map_err(|error| error.to_string()),
                                Ok(Ok(Err(error))) => Err(error),
                                _ => Err("The server's plan reading was lost or timed out.".into()),
                            }
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                }
                // Each agent's plan shows as it answers, so a slow or stuck agent holds back none of the others.
                None => {
                    use futures::StreamExt;
                    let (send, mut progress) = futures::channel::mpsc::unbounded();
                    let probe = cx.background_spawn(async move { crate::pages::subscriptions::probe_each(send) });
                    let mut last = None;
                    while let Some(read) = progress.next().await {
                        let open = this.update(cx, |this, cx| {
                            let before = match &this.plan_usage {
                                Some((_, Ok(plans))) => plans.as_slice(),
                                _ => &[],
                            };
                            crate::plan_book::learn(read.fresh());
                            let plans = crate::plan_book::shown(&read.over(before));
                            this.plan_waiting = read.waiting();
                            this.plan_usage = Some((std::time::Instant::now(), Ok(plans)));
                            this.refresh_auto_route(cx);
                            cx.notify();
                        });
                        if open.is_err() {
                            return;
                        }
                        last = Some(read);
                    }
                    probe
                        .await
                        .map(|()| last.map(|read| read.over(&[])).unwrap_or_default())
                }
            };
            let _ = this.update(cx, |this, cx| {
                // An older or limitless reading does not replace newer limits (`plan_book`). A server's plans are its own.
                let plans = plans.map(|plans| {
                    if local {
                        crate::plan_book::read(&plans)
                    } else {
                        this.remote_plan_book.read(&plans);
                        this.remote_plan_book.shown(&plans)
                    }
                });
                this.plan_usage = Some((std::time::Instant::now(), plans));
                this.plan_waiting.clear();
                this._plan_probe = None;
                this.refresh_auto_route(cx);
                cx.notify();
            });
        }));
    }

    /// The selected session or draft's payer, its plan as read, and whether a usage limit already stopped it.
    pub(super) fn composer_plan(&self) -> Option<(nexus_workbench::allowance::Source, PlanRead<'_>, bool)> {
        use nexus_workbench::allowance::{self, Login, Source};
        let session = self.code_session();
        if session.map_or(self.code_form.provider.is_some(), |session| session.saved_provider) {
            return None;
        }
        let (agent, via, connection, login, limited) = match session {
            Some(session) => (
                session.agent,
                session.via,
                session.connection.as_deref(),
                Login::Of(
                    session
                        .connection
                        .as_ref()
                        .map_or(session.account.as_deref(), |c| c.selected_account.as_deref()),
                ),
                session.limited.is_some(),
            ),
            None => (
                self.code_form.agent,
                self.code_form.effective_via(),
                None,
                Login::NewSession,
                false,
            ),
        };
        let source = allowance::source(agent, via, connection)?;
        // A provider plan is not read for Nexus-routed calls, including their loading and failure states.
        let read = match &self.plan_usage {
            _ if source == Source::Nexus => PlanRead::Read(None),
            None => PlanRead::Checking,
            Some((_, Err(error))) => PlanRead::Failed(error),
            Some((_, Ok(plans))) => {
                let email = connection
                    .and_then(|c| c.account.as_ref())
                    .and_then(|a| a.email.as_deref());
                match allowance::plan_for(plans, agent, login, email) {
                    // Its agent has not answered yet; the other agents' plans are already read.
                    None if self.plan_waiting.contains(&agent) => PlanRead::Checking,
                    plan => PlanRead::Read(plan),
                }
            }
        };
        Some((source, read, limited))
    }

    /// The selected session or draft's payer and tightest plan limit.
    fn composer_allowance(
        &self,
    ) -> Option<(
        nexus_workbench::allowance::Source,
        nexus_workbench::allowance::Reading<'_>,
        bool,
    )> {
        use nexus_workbench::allowance::{self, Reading};
        let (source, read, limited) = self.composer_plan()?;
        let model = self.composer_model();
        let reading = match read {
            PlanRead::Checking => Reading::Checking,
            PlanRead::Failed(_) => Reading::Failed,
            PlanRead::Read(plan) => {
                Reading::Read(allowance::limiting_for(plan, model.as_deref(), crate::code::now_ms()))
            }
        };
        Some((source, reading, limited))
    }

    /// The model the selected session runs (as its agent reported it, else as picked), or the new session's pick: an
    /// Antigravity plan's limit is its model's group. None: the agent's default, not known here.
    fn composer_model(&self) -> Option<String> {
        match self.code_session() {
            Some(session) => session
                .connection
                .as_ref()
                .and_then(|connection| connection.model.clone())
                .or_else(|| session.model.clone()),
            None => self.code_form.model.clone(),
        }
    }

    /// The plan usage in the composer's toolbar, after the effort and in its faint ink: a donut of the 5-hour limit
    /// left, and its number. Hover names each shared limit and its reset; a click opens the usage popover in place
    /// (`usage_meter.rs`). A plan it cannot read leaves the donut hollow, and the hover says why.
    pub(super) fn render_usage_donut(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        use nexus_workbench::allowance::{self, LOW_ALLOWANCE, Source};
        let (source, read, _) = self.composer_plan()?;
        let nexus = source == Source::Nexus;
        let now = crate::code::now_ms();
        let model = self.composer_model();
        let shown = match read {
            PlanRead::Read(plan) => five_hour(plan, model.as_deref(), now),
            _ => None,
        };
        let tightest = match read {
            PlanRead::Read(plan) => allowance::limiting_for(plan, model.as_deref(), now),
            _ => None,
        };
        let low = tightest.is_some_and(|tightest| tightest.left <= LOW_ALLOWANCE);
        let hint = if nexus {
            "Nexus Gateway usage for the selected organization. Provider plan allowances are separate.".to_string()
        } else {
            usage_hint(source, read, now)
        };
        let mark = if nexus {
            Icon::new(Lucide::Gauge).size(px(14.)).into_any_element()
        } else {
            usage_donut(shown.map(|shown| shown.left), low, palette).into_any_element()
        };
        // Low, the number says when it comes back: `8% · 41 min`.
        let text = shown.map(|shown| match shown.limit.resets_at {
            Some(at) if low => format!("{}% · {}", shown.left, allowance::resets_in(at, now)),
            _ => format!("{}%", shown.left),
        });
        let open = self.overlay.usage_meter();
        let gauge = h_flex()
            .id("usage-donut")
            .role(Role::Button)
            .aria_label(SharedString::from(hint.clone()))
            .h(px(26.))
            .px(px(4.))
            .gap(px(5.))
            .rounded(px(6.))
            .text_size(px(13.))
            .text_color(palette.faint)
            .cursor_pointer()
            .hover(|style| style.text_color(palette.foreground))
            // The popover says what the tooltip does, so the tooltip rests while it is open.
            .when(!open, |gauge| {
                gauge.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(hint.clone()).build(window, cx)
                })
            })
            .menu_trigger(
                open,
                cx,
                |this| this.overlay.usage_meter(),
                |this, _, cx| this.toggle_usage_meter(cx),
            )
            .child(mark)
            .children(text);
        Some(
            div()
                .relative()
                .child(gauge)
                .children(self.render_usage_meter(palette, cx))
                .into_any_element(),
        )
    }

    /// The usage warning over the composer, on Home and in a session's dock.
    pub(super) fn render_usage_warning(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        use nexus_workbench::allowance::{self, Reading};
        let (source, Reading::Read(Some(remaining)), limited) = self.composer_allowance()? else {
            return None;
        };
        if !allowance::warns(Some(&remaining), limited) {
            return None;
        }
        Some(
            h_flex()
                .w_full()
                .gap(px(8.))
                .p(px(12.))
                .rounded(px(8.))
                .border_1()
                .border_color(palette.border)
                .bg(palette.raised)
                .text_size(px(12.))
                .child(Icon::new(Lucide::CircleAlert).size(px(15.)))
                .child(v_flex().flex_1().child(allowance::warning(source, &remaining)).child(
                    div().text_color(palette.muted).child(allowance::reset_text(
                        &remaining,
                        crate::code::now_ms(),
                        &chrono::Local,
                    )),
                ))
                .child(
                    super::code_dock::action_button("plan-view-usage", None, "View usage", false, palette).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_native(crate::pages::PageId::Subscriptions, window, cx)
                        }),
                    ),
                )
                .into_any_element(),
        )
    }

    /// The session the form describes, with its first message.
    pub(super) fn new_session(
        &self,
        project_id: String,
        text: String,
        attachments: crate::attachments::Draft,
    ) -> NewSession {
        let form = &self.code_form;
        let git = self
            .projects
            .iter()
            .any(|project| project.id == project_id && project.branch.is_some());
        let base = form
            .base
            .for_project(&project_id)
            .filter(|_| form.worktree && !form.review.on);
        NewSession {
            base,
            project_id,
            agent: form.agent,
            model: form
                .selected_server_model()
                .map(str::to_owned)
                .or_else(|| form.model.clone()),
            permission_mode: form.mode.as_str().to_string(),
            effort: form.effort,
            via: form.effective_via(),
            provider: form.provider.clone(),
            worktree: (form.worktree || form.review.on) && git,
            review: form.review.number.filter(|_| form.review.on && git),
            text,
            attachments,
        }
    }

    /// A new session in `project_id`, from its "+" in the sidebar or the project menu.
    pub(super) fn open_new_code_in(&mut self, project_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_code(window, cx);
        self.code_form.project = Some(project_id);
        self.code_form.plain = false;
        self.code_form.use_project();
        self.cloud_ui.on = false;
        self.check_claude_trust(cx);
        self.cloud_ui.missing = None;
        self.cloud_ui.missing_on = None;
        self.code_form.remember();
        self.overlay.close_launch_menu();
        self.sync_draft(window, cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Local has a scope immediately. A signed-in workspace needs its account before it can accept a project.
    pub(super) fn project_scope_ready(&self) -> bool {
        self.is_local() || (self.account.is_some() && self.account_gateway().is_some())
    }

    pub(super) fn show_project_add_failure(&mut self, reason: &str, cx: &mut Context<Self>) {
        self.show_notice(
            format!("Add a project: {reason}"),
            None,
            std::time::Duration::from_secs(8),
            cx,
        );
    }

    pub(super) fn project_add_refused(&mut self, reason: &str, cx: &mut Context<Self>) {
        eprintln!("nexus-desktop-gpui: Add a project: {reason}");
        self.show_project_add_failure(reason, cx);
    }

    /// Pick a folder with the system's picker and add it as a project; a new session in it opens once it is added.
    pub(super) fn add_project(&mut self, cx: &mut Context<Self>) {
        self.overlay.close_launch_menu();
        if !self.project_scope_ready() {
            self.project_add_refused("Wait for the workspace to open before adding a folder.", cx);
            return;
        }
        if self.is_local()
            && let Some((reason, _)) = self.local_code_blocker()
        {
            self.project_add_refused(reason, cx);
            return;
        }
        let Some(runner) = &self.runner else {
            self.refuse_read_only();
            cx.notify();
            return;
        };
        let scope = self.code_scope.clone();
        let guard = runner.source_guard();
        let picked = super::project_dialog::pick_project_folder(cx);
        cx.spawn(async move |this, cx| {
            let picked = match picked.await {
                Ok(None) => return,
                Ok(Some(path)) => Ok(path),
                Err(error) => Err(error),
            };
            if this
                .update(cx, |this, cx| {
                    let error = match picked {
                        Err(error) => error.into(),
                        Ok(_) if !this.project_scope_ready() || this.code_scope != scope || !guard() => {
                            "The active workspace changed while the folder picker was open. Choose the folder again."
                                .into()
                        }
                        Ok(path) => {
                            if this.is_local()
                                && let Some((reason, _)) = this.local_code_blocker()
                            {
                                reason.into()
                            } else if let Some(runner) = &this.runner {
                                runner.add_project(path, Some(cx.entity_id()));
                                return;
                            } else {
                                this.read_only_note()
                            }
                        }
                    };
                    this.project_add_refused(&error, cx);
                })
                .is_err()
            {
                eprintln!("nexus-desktop-gpui: Add a project: The workspace closed while the folder picker was open.");
            }
        })
        .detach();
    }

    pub(super) fn toggle_launch_menu(&mut self, menu: LaunchMenu, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.launch_menu() == Some(menu) {
            self.overlay.close_launch_menu();
        } else {
            self.open_overlay(Overlay::LaunchMenu(menu));
        }
        self.overlay.close_model_menu();
        if self.overlay.launch_menu() == Some(LaunchMenu::Model) {
            self.refresh_harness(cx);
        }
        if self.overlay.launch_menu() == Some(LaunchMenu::Workspace)
            && self.code_form.worktree
            && !self.code_form.review.on
        {
            self.choose_worktree(window, cx);
        }
        if self.overlay.launch_menu() == Some(LaunchMenu::Project) {
            self.drop_idle_menu_clone();
            self.drop_idle_menu_server_folder();
            // A list that failed or had no GitHub sign-in is read again: GitHub may be connected since.
            if !self.is_local() && !matches!(self.github_picks(), Some(Ok(_))) {
                self.load_cloud_repos(window, cx);
            }
            // Each opening starts with an empty search, focused, and the first row lit. `set_value` sends no change
            // event, so the stored query is cleared with the box (`clear_search`).
            self.cloud_ui.cursor = 0;
            self.cloud_ui.scope = Default::default();
            self.cloud_ui.github_all = false;
            let search = self.repo_search().clone();
            super::picker_navigation::clear_search(&mut self.cloud_ui.query, &search, window, cx);
            self.cloud_ui.list_scroll.scroll_to_item(0);
        }
        if self.overlay.launch_menu() == Some(LaunchMenu::Model) {
            // The model menu shares the chat model menu's search: empty, focused, and the cursor on the picked row.
            self.load_local_provider_choices(cx);
            self.prepare_model_search(window, cx);
        } else if self.overlay.launch_menu().is_none() && menu == LaunchMenu::Model {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    pub(super) fn load_local_provider_choices(&mut self, cx: &mut Context<Self>) {
        if !self.is_local() || !self.local_mode.opt_in.enabled() {
            self.code_form.providers.clear();
            return;
        }
        let read = crate::pages::local_providers::read_for_picker(cx);
        self.code_form.provider_load = Some(cx.spawn(async move |this, cx| {
            let choices = read.await.map(|settings| {
                nexus_workbench::provider::ProviderSet::from_settings(&settings, &Default::default(), None, true)
                    .iter()
                    .map(|provider| LocalProviderChoice {
                        id: provider.id().clone(),
                        name: provider.name(),
                        agents: provider.agents(),
                        server_models: settings
                            .servers
                            .iter()
                            .find(|server| &server.id == provider.id())
                            .map(|server| server.models.clone()),
                    })
                    .collect::<Vec<_>>()
            });
            if let Err(error) = this.update(cx, |this, cx| {
                if this.is_local() {
                    match choices {
                        Ok(choices) => this.code_form.update_provider_choices(choices),
                        Err(error) if this.code_form.provider.is_some() || this.code_form.provider_loading() => {
                            this.code_form.require_provider_reselection(error);
                        }
                        Err(error) => eprintln!("nexus-desktop-gpui: local provider picker: {error}"),
                    }
                    cx.notify();
                }
            }) {
                eprintln!("nexus-desktop-gpui: local provider picker closed: {error}");
            }
        }));
    }

    pub(super) fn close_launch_menu(&mut self, cx: &mut Context<Self>) {
        if self.overlay.close_launch_menu().is_some() {
            cx.notify();
        }
    }

    /// A menu's box that a click outside closes.
    pub(super) fn launch_box(
        &self,
        id: &'static str,
        width: f32,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        menu_box(id, width, palette)
            // Over the art, the menu is frosted glass: the art blurred under a tint of the menu's color.
            .when(self.frosted(), |menu| {
                menu.relative().bg(transparent_black()).child(super::backdrop::frost(
                    crate::tokens::radius::CARD,
                    palette.solid().menu.opacity(0.8),
                    None,
                ))
            })
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_launch_menu(cx)))
    }

    /// The launch head (`wb-new-title`): the Nexus AI mark, then the question, centred over the composer.
    /// The same with or without a project, as the Nexus app's `new-session.tsx` and the web's home.
    pub(super) fn render_launch_head(&self, palette: Palette) -> impl IntoElement {
        v_flex()
            .w_full()
            .items_center()
            .child(maker_mark("nexus", 32., palette))
            .child(
                div()
                    .mt(px(8.))
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(palette.foreground)
                    .text_center()
                    .child("What should we build?"),
            )
    }

    /// The where line under the composer: the project, where it runs and the workspace. Who pays is in the model
    /// panel. Each chip opens its menu 8px under it. `plain` is the chat's: no project.
    pub(super) fn render_where_line(&self, plain: bool, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let form = &self.code_form;
        let project = (!plain)
            .then(|| {
                form.project
                    .as_ref()
                    .and_then(|id| self.projects.iter().find(|project| &project.id == id))
            })
            .flatten();
        let open = self.overlay.launch_menu();
        let frosted = self.frosted();
        self.menu_side.track(open);
        let side = self.menu_side.side();
        // A chat started in a plain project names it; a cloud session names its repository (`RepoChip`).
        let plain_project = plain
            .then(|| match &self.groups.pending {
                Some(crate::project_groups::Home::Plain(id)) => {
                    self.groups.groups.projects.iter().find(|project| &project.id == id)
                }
                _ => None,
            })
            .flatten();
        let cloud_repo = (!plain && self.cloud_ui.on)
            .then(|| self.cloud_ui.repo.as_ref())
            .flatten();
        let missing = (!plain && !self.cloud_ui.on)
            .then(|| self.cloud_ui.missing.as_ref())
            .flatten();
        let project_label: SharedString = match (project, plain_project, cloud_repo.or(missing)) {
            (_, Some(named), _) => named.name.clone().into(),
            (_, _, Some(repo)) => repo.name.clone().into(),
            _ if !plain && self.cloud_ui.on => "Pick a repository".into(),
            (Some(project), _, _) => project.name.clone().into(),
            (None, _, _) if plain => "No project".into(),
            (None, _, _) => "Pick a project".into(),
        };
        let project = project.filter(|_| !self.cloud_ui.on && missing.is_none());
        let project_chip = div()
            .relative()
            .child(
                context_chip(
                    "launch-project",
                    if plain { Lucide::MessageCircle } else { Lucide::Folder },
                    project_label,
                    Some(open == Some(LaunchMenu::Project)),
                    frosted,
                    palette,
                )
                .menu_trigger(
                    open == Some(LaunchMenu::Project),
                    cx,
                    |this| this.overlay.launch_menu() == Some(LaunchMenu::Project),
                    |this, window, cx| this.toggle_launch_menu(LaunchMenu::Project, window, cx),
                ),
            )
            .when(open == Some(LaunchMenu::Project), |frame| {
                frame.child(self.menu_side.measure_chip())
            })
            .when_some(
                motion::presence("launch-project-menu", open == Some(LaunchMenu::Project), cx),
                |frame, phase| {
                    let menu = self.menu_side.fit(side, self.render_project_menu(plain, palette, cx));
                    frame.child(menu_beside(
                        side,
                        phase.menu("launch-project-menu", drop_for(side), menu),
                    ))
                },
            );
        let git = project.is_some_and(|project| project.branch.is_some());
        let workspace_chip = git.then(|| {
            let host = self.review_host();
            let label: String = match (form.review.on, form.review.number) {
                (true, Some(number)) => format!("Review {}", host.reference(number)),
                (true, None) => "Review".into(),
                _ if form.worktree => self.worktree_label(),
                _ => "Project folder".into(),
            };
            let icon = if form.review.on {
                Lucide::GitPullRequest
            } else if form.worktree {
                Lucide::GitBranch
            } else {
                Lucide::Folder
            };
            div()
                .relative()
                .child(
                    context_chip(
                        "launch-workspace",
                        icon,
                        label.into(),
                        Some(open == Some(LaunchMenu::Workspace)),
                        frosted,
                        palette,
                    )
                    .menu_trigger(
                        open == Some(LaunchMenu::Workspace),
                        cx,
                        |this| this.overlay.launch_menu() == Some(LaunchMenu::Workspace),
                        |this, window, cx| this.toggle_launch_menu(LaunchMenu::Workspace, window, cx),
                    ),
                )
                .when(open == Some(LaunchMenu::Workspace), |frame| {
                    frame.child(self.menu_side.measure_chip())
                })
                .when_some(
                    motion::presence("launch-workspace-menu", open == Some(LaunchMenu::Workspace), cx),
                    |frame, phase| {
                        let menu = self.menu_side.fit(side, self.render_workspace_menu(palette, cx));
                        frame.child(menu_beside(
                            side,
                            phase.menu("launch-workspace-menu", drop_for(side), menu),
                        ))
                    },
                )
        });
        // Where it runs: the cloud for a chat or a cloud session, the project's server, or this computer.
        let cloud = !plain && self.cloud_ui.on;
        let server = project
            .and_then(|project| project.machine.as_deref())
            .or(missing.and(self.cloud_ui.missing_on.as_deref()))
            .and_then(|machine| self.servers.iter().find(|server| server.id == machine));
        let (machine_icon, machine): (Lucide, SharedString) = match server {
            _ if plain || cloud => (Lucide::Cloud, "Cloud".into()),
            Some(server) => (Lucide::Server, server.name.clone().into()),
            None => (Lucide::Laptop, crate::code::THIS_COMPUTER.into()),
        };
        // A plain session's Cloud opens where it runs too (`new-session.tsx`): a pick there leaves the chat.
        let machine_chip = div()
            .relative()
            .child(
                context_chip(
                    "launch-machine",
                    machine_icon,
                    machine,
                    Some(open == Some(LaunchMenu::Machine)),
                    frosted,
                    palette,
                )
                .menu_trigger(
                    open == Some(LaunchMenu::Machine),
                    cx,
                    |this| this.overlay.launch_menu() == Some(LaunchMenu::Machine),
                    |this, window, cx| this.toggle_launch_menu(LaunchMenu::Machine, window, cx),
                ),
            )
            .when(open == Some(LaunchMenu::Machine), |frame| {
                frame.child(self.menu_side.measure_chip())
            })
            .when_some(
                motion::presence("launch-machine-menu", open == Some(LaunchMenu::Machine), cx),
                |frame, phase| {
                    let menu = self.menu_side.fit(side, self.render_machine_menu(palette, cx));
                    frame.child(menu_beside(
                        side,
                        phase.menu("launch-machine-menu", drop_for(side), menu),
                    ))
                },
            );
        let workspace_chip = workspace_chip.filter(|_| !cloud);
        h_flex()
            .w_full()
            .px(px(4.))
            .gap(px(2.))
            .when(self.composer_layout.control_icons, |line| line.flex_wrap())
            // What and where sit together; the workspace follows.
            .child(project_chip)
            .when(self.account_gateway().is_some(), |row| row.child(machine_chip))
            .when_some(self.account_control_reason(), |row, reason| {
                row.child(
                    h_flex()
                        .id("launch-local-machine")
                        .h(px(28.))
                        .px(px(8.))
                        .gap(px(6.))
                        .text_size(px(13.))
                        .text_color(palette.faint)
                        .aria_label(reason)
                        .tooltip(move |window, cx| Tooltip::new(reason).build(window, cx))
                        .child(glyph(Lucide::Laptop, palette.faint))
                        .child(crate::code::THIS_COMPUTER),
                )
            })
            .children(workspace_chip)
    }

    /// "Runs on" (`MachinePicker`): this computer, Cloud (a machine of its own, made when the session starts), or one of
    /// your servers with its status. A plain session (the chat's head) runs in the cloud, so Cloud is checked there.
    fn render_machine_menu(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let plain = matches!(self.view, View::Chat);
        let cloud = self.cloud_ui.on && !plain;
        let picked_server = self
            .code_form
            .project
            .as_ref()
            .and_then(|id| self.projects.iter().find(|project| &project.id == id))
            .and_then(|project| project.machine.clone())
            .or_else(|| self.cloud_ui.missing.as_ref().and(self.cloud_ui.missing_on.clone()));
        let servers: Vec<_> = self
            .servers
            .iter()
            .map(|server| {
                let id = server.id.clone();
                let hint = format!(
                    "{} · {}",
                    crate::machine_view::status_view(&server.status).label,
                    server.ssh
                );
                menu_row(
                    SharedString::from(format!("launch-machine-{}", server.id)),
                    server.name.clone().into(),
                    Some(hint.into()),
                    !cloud && !plain && picked_server.as_deref() == Some(server.id.as_str()),
                    palette,
                )
                .on_click(cx.listener(move |this, _, window, cx| this.run_on(Some(&id), window, cx)))
            })
            .collect();
        self.launch_box("launch-machine-menu", 256., palette, cx)
            .child(menu_label("Runs on", palette))
            .child(
                menu_row(
                    "launch-machine-here",
                    crate::code::THIS_COMPUTER.into(),
                    Some(
                        if cfg!(target_os = "macos") {
                            "Stops when this Mac sleeps or Nexus quits"
                        } else {
                            "Stops when this computer sleeps or Nexus quits"
                        }
                        .into(),
                    ),
                    !cloud && !plain && picked_server.is_none(),
                    palette,
                )
                .on_click(cx.listener(|this, _, window, cx| this.run_on(None, window, cx))),
            )
            .child(
                menu_row(
                    "launch-machine-cloud",
                    "Cloud".into(),
                    Some("A machine of its own. Keeps running when you close the laptop".into()),
                    cloud || plain,
                    palette,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.leave_plain(window, cx);
                    this.pick_cloud(window, cx)
                })),
            )
            .children(servers)
            .child(div().h(px(8.)))
            .child(
                menu_row("launch-add-server", "Add a server…".into(), None, false, palette).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.overlay.close_launch_menu();
                        this.open_settings(super::SettingsSection::Page(crate::pages::PageId::Servers), window, cx);
                    },
                )),
            )
            .into_any_element()
    }

    /// Run the new session on this computer (None) or on server `server`: its latest project, as a server's + does.
    pub(super) fn run_on(&mut self, server: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_plain(window, cx);
        self.overlay.close_launch_menu();
        // The repository picked stays picked: its copy there, else the offer to clone it there or run it in the cloud.
        let wanted = self
            .cloud_ui
            .on
            .then(|| self.cloud_ui.repo.clone())
            .flatten()
            .or_else(|| self.cloud_ui.missing.clone());
        self.cloud_ui.on = false;
        self.cloud_ui.missing = None;
        self.cloud_ui.missing_on = None;
        if let Some(repo) = wanted {
            let key = super::repo_groups::cloud_repo_key(&repo);
            let copy = self
                .projects
                .iter()
                .find(|project| {
                    project.machine.as_deref() == server && super::repo_groups::repo_key(&project.remotes) == key
                })
                .map(|project| project.id.clone());
            match (copy, server) {
                (Some(copy), _) => return self.open_new_code_in(copy, window, cx),
                (None, _) => {
                    self.code_form.project = None;
                    self.cloud_ui.missing = Some(repo);
                    self.cloud_ui.missing_on = server.map(String::from);
                    cx.notify();
                    return;
                }
            }
        }
        let project = self
            .projects
            .iter()
            .find(|project| project.machine.as_deref() == server)
            .map(|project| project.id.clone());
        match (project, server) {
            (Some(project), _) => self.open_new_code_in(project, window, cx),
            // A server with no project yet: the project menu offers a folder there, or a clone (`ProjectPicker`).
            (None, Some(server)) => {
                self.drop_idle_menu_clone();
                self.drop_idle_menu_server_folder();
                self.open_overlay(Overlay::LaunchMenu(LaunchMenu::Project));
                self.open_server_folder_in_menu(server, false, window, cx);
            }
            (None, None) => self.code_form.project = None,
        }
        cx.notify();
    }

    /// A pick in a plain session's "Runs on" menu: the session is no longer a chat, so New session opens, and the next
    /// one opens there too (`rememberPlace`).
    fn leave_plain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.view, View::Chat) {
            return;
        }
        self.open_new_code(window, cx);
        self.code_form.plain = false;
        self.code_form.remember();
    }

    /// A pick of the person's own project: a chat filed there.
    pub(super) fn pick_own_project(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.close_launch_menu();
        self.code_form.plain = true;
        self.code_form.remember();
        self.new_chat_in(id, window, cx);
        cx.notify();
    }

    /// A pick of a repository's project (`ids`, its copies): in the cloud while Cloud is picked, else its copy where
    /// the session runs, else any copy on this computer or a server, else the cloud, where it already runs.
    pub(super) fn pick_repo_project(&mut self, ids: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let members: Vec<&crate::code::CodeProject> = ids
            .iter()
            .filter_map(|id| self.projects.iter().find(|project| &project.id == id))
            .collect();
        let repo = members
            .iter()
            .find_map(|project| super::repo_groups::cloud_repo_of(project));
        let in_cloud = |project: &&crate::code::CodeProject| {
            project
                .machine
                .as_deref()
                .is_some_and(|id| self.cloud_machines.iter().any(|machine| machine.id == id))
        };
        let plain = matches!(self.view, View::Chat);
        if self.cloud_ui.on
            && !plain
            && let Some(repo) = repo.clone()
        {
            self.pick_repo(repo, window, cx);
            return;
        }
        let here = (!plain && !self.cloud_ui.on)
            .then(|| {
                self.code_form
                    .project
                    .as_deref()
                    .and_then(|id| self.projects.iter().find(|project| project.id == id))
                    .map(|project| project.machine.clone())
            })
            .flatten()
            .unwrap_or(None);
        let copy = members
            .iter()
            .find(|project| !in_cloud(project) && project.machine == here)
            .or_else(|| members.iter().find(|project| !in_cloud(project)))
            .map(|project| project.id.clone());
        match (copy, repo) {
            (Some(copy), _) => self.open_new_code_in(copy, window, cx),
            (None, Some(repo)) => self.pick_github_repo(repo, window, cx),
            (None, None) => {
                if let Some(first) = ids.first() {
                    self.open_new_code_in(first.clone(), window, cx);
                }
            }
        }
    }

    /// A pick from GitHub runs in the cloud, so nothing downloads to this computer. Its project shows in the sidebar
    /// with the session; Clone to this computer stays one click away on the machine menu.
    pub(super) fn pick_github_repo(
        &mut self,
        repo: crate::cloud::CloudRepo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_plain(window, cx);
        self.pick_cloud(window, cx);
        self.cloud_ui.missing = None;
        self.cloud_ui.missing_on = None;
        // Picking it closes the project menu `pick_cloud` opened while no repository was picked.
        self.pick_repo(repo, window, cx);
    }

    /// The project menu's end, for the chosen machine (`ProjectPicker`): on this computer Open a folder (a repository
    /// comes from GitHub above, or from Clone to this computer); on a server, a folder there and, when its host clones,
    /// Clone a repository there. A form picked here opens in the menu. Answers whether a form is open, for the menu's
    /// width.
    pub(super) fn render_project_menu_add(
        &self,
        plain: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> (AnyElement, bool) {
        let picked_machine = (!plain)
            .then(|| self.code_form.project.as_deref())
            .flatten()
            .and_then(|id| self.projects.iter().find(|project| project.id == id))
            .and_then(|project| project.machine.clone());
        let server = self
            .server_folder_in_menu()
            .map(str::to_string)
            .or(picked_machine)
            .and_then(|machine| self.servers.iter().find(|server| server.id == machine));
        if let Some(server) = server {
            if let Some(field) = self.render_server_folder(&server.id, true, palette, cx) {
                return (field, true);
            }
            // A repository reaches a server from GitHub's list: picked, then cloned there from the card it shows.
            let folder_id = server.id.clone();
            let menu = v_flex().child(
                pick_row(
                    "launch-server-folder",
                    Lucide::FolderOpen,
                    format!("Add a folder on {}…", server.name).into(),
                    None,
                    Badge::None,
                    false,
                    palette,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_server_folder_in_menu(&folder_id, false, window, cx)
                })),
            );
            return (menu.into_any_element(), false);
        }
        if let Some(form) = self.render_menu_clone(palette, cx) {
            return (form, true);
        }
        let menu = v_flex().child(
            pick_row(
                "launch-add-project",
                Lucide::FolderOpen,
                "Open a folder…".into(),
                Some("A folder on this computer".into()),
                Badge::None,
                false,
                palette,
            )
            .on_click(cx.listener(|this, _, _, cx| this.add_project(cx))),
        );
        (menu.into_any_element(), false)
    }

    fn render_workspace_menu(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows = [
            (false, "Project folder", "Works in the project's own checkout"),
            (true, "New worktree", "A branch and folder of its own"),
        ]
        .into_iter()
        .map(|(worktree, label, hint)| {
            menu_row(
                SharedString::from(format!("launch-workspace-{worktree}")),
                label.into(),
                Some(hint.into()),
                !self.code_form.review.on && self.code_form.worktree == worktree,
                palette,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if worktree {
                    // The menu stays open on the branches to start from (`base_pick.rs`).
                    this.choose_worktree(window, cx);
                    return;
                }
                this.code_form.worktree = false;
                this.code_form.review.clear();
                this.code_form.remember();
                this.overlay.close_launch_menu();
                cx.notify();
            }))
        })
        .collect::<Vec<_>>();
        // The third choice, as the Nexus app's: someone's pull request, checked out in the reviews folder.
        let review = menu_row(
            "launch-workspace-review",
            format!("Review a {}", self.review_host().request()).into(),
            Some("Checked out in your reviews folder".into()),
            self.code_form.review.on,
            palette,
        )
        .on_click(cx.listener(|this, _, window, cx| this.choose_review(window, cx)));
        self.launch_box("launch-workspace-menu", 340., palette, cx)
            .children(rows)
            .child(review)
            .when(self.code_form.review.on, |menu| {
                menu.child(self.render_review_list(palette, cx))
            })
            .when(self.code_form.worktree && !self.code_form.review.on, |menu| {
                menu.child(self.render_base_list(palette, cx))
            })
            .into_any_element()
    }

    /// The toolbar's right chip on Home: the one model chip, "[who pays] Opus 5.5 High", which opens the model panel.
    /// An agent that can't run here is a row under the models, in the panel's "not set up" fold.
    pub(super) fn render_launch_right(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let form = &self.code_form;
        let compact = self.composer_layout.chat_icons;
        let models = self.agent_catalog(form.agent);
        // The picked model, else the one Default runs ("Opus 5.5 · Default"), else the agent while its models are read.
        let model: SharedString = if let Some(model) = form.selected_server_model() {
            model.to_string()
        } else if form.server_models().is_some() {
            "Choose a server model".into()
        } else {
            match &form.model {
                Some(id) if id == super::code_picker::AUTO => super::auto_route_preview::chip_text(
                    form.auto_any.then_some(form.agent),
                    self.auto_route.model(form.effective_via()),
                ),
                Some(id) => models
                    .iter()
                    .find(|model| &model.id == id)
                    .map_or_else(|| id.clone(), |model| model.name.clone()),
                None => crate::code::default_model_name(&models).map_or_else(
                    || label(form.agent).to_string(),
                    |name| super::code_picker::default_label(Some(&name)),
                ),
            }
        }
        .into();
        // The effort's word after the model, only when it is not Default ("Opus 5.5 High").
        let efforts = self.form_efforts();
        let effort = form.effort.filter(|_| !efforts.is_empty()).map(effort_name);
        let (permissions, _) = permission_copy(form.agent, form.mode);
        let effort_hint = match effort {
            Some(effort) => format!("Effort: {effort}; permissions: {permissions}"),
            None => format!("Permissions: {permissions}"),
        };
        let org = self
            .account
            .as_ref()
            .map(|account| account.org_name.clone())
            .unwrap_or_default();
        let local_provider = form.selected_provider();
        let payer = if self.is_local() {
            if self.failed_provider_draft().is_some() {
                Payer::None
            } else if local_provider.is_some() {
                Payer::Key
            } else if form.provider_selection_error().is_some()
                || form.provider_loading()
                || form.needs_local_provider()
            {
                Payer::None
            } else {
                Payer::Plan
            }
        } else {
            form.access.as_ref().map_or(Payer::Checking, |access| {
                access.payer(form.agent, form.effective_via(), &org).0
            })
        };
        let open = self.overlay.launch_menu();
        // The mark says who pays: the maker's for its plan, a key, or credits. While that is read it is a spinner, and
        // with nothing able to pay a warning. A payer picked by hand reads as bright as the model; Auto, quieter.
        let pinned = self.is_local() || form.via.is_some();
        let mark = match payer {
            Payer::Key if local_provider.is_some_and(|provider| provider.server_models.is_some()) => {
                Icon::new(Lucide::Server).size(px(14.)).into_any_element()
            }
            Payer::Key => Icon::new(Lucide::KeyRound).size(px(14.)).into_any_element(),
            Payer::Credits => Icon::new(Lucide::Wallet).size(px(14.)).into_any_element(),
            // The maker's mark sets its own colour, so Auto is shown by dimming the whole mark.
            Payer::Plan => div()
                .flex_none()
                .when(!pinned, |mark| mark.opacity(0.55))
                .child(maker_mark(agent_maker(form.agent), 14., palette))
                .into_any_element(),
            Payer::Checking => Icon::new(Lucide::LoaderCircle)
                .size(px(14.))
                .text_color(palette.icon)
                .with_animation(
                    "launch-payer-spinner",
                    Animation::new(std::time::Duration::from_millis(1000))
                        .repeat()
                        .with_max_fps(crate::motion::LOOP_FPS),
                    |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
                )
                .into_any_element(),
            // The key source with no key yet: a quiet key, never a red warning. The setup line says the rest.
            Payer::None => Icon::new(Lucide::KeyRound)
                .size(px(14.))
                .text_color(palette.faint)
                .into_any_element(),
        };
        let pay_tip = match payer {
            Payer::Key if let Some(provider) = local_provider => format!("Uses {}", provider.name),
            Payer::None if self.is_local() => "Choose a local provider".into(),
            Payer::Checking => "Checking who pays…".to_string(),
            Payer::None => super::no_key::what(&org, super::no_key::provider(form.agent)),
            _ => format!(
                "Paid by {}{}",
                payer_name(payer, form.agent, &org).unwrap_or_default(),
                if pinned { ", picked by you" } else { "" }
            ),
        };
        let tip = SharedString::from(format!("{} · {model}. {effort_hint}. {pay_tip}", label(form.agent)));
        // One chip, "[mark] Opus 5.5 High": the mark says who pays, and the chip opens the model panel, which also
        // holds who pays, the reasoning and the permissions.
        let model_open = open == Some(LaunchMenu::Model);
        let model_chip = toolbar_chip("launch-model", model_open, compact, palette)
            .min_w_0()
            .flex_shrink(1.)
            .aria_label(tip.clone())
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .menu_trigger(
                model_open,
                cx,
                |this| this.overlay.launch_menu() == Some(LaunchMenu::Model),
                |this, window, cx| this.toggle_launch_menu(LaunchMenu::Model, window, cx),
            )
            .child(mark)
            .when(!compact, |chip| {
                chip.child(div().min_w_0().truncate().child(model))
                    .children(effort.map(|effort| div().flex_none().text_color(palette.faint).child(effort)))
                    .child(
                        Icon::new(Lucide::ChevronDown)
                            .size(px(14.))
                            .text_color(palette.faint)
                            .with_tween(
                                "launch-model-chevron",
                                if model_open { 1. } else { 0. },
                                motion::Tween::new(motion::COLLAPSE, motion::ease_out),
                                |icon, turn| icon.rotate(radians(std::f32::consts::PI * turn)),
                            ),
                    )
            });
        h_flex()
            .h(px(28.))
            .min_w_0()
            .gap(px(4.))
            .child(div().relative().min_w_0().child(model_chip).when_some(
                motion::presence("launch-model-menu", model_open, cx),
                |frame, phase| {
                    let menu = self.render_launch_model_menu(palette, cx);
                    frame.child(menu_under_box(phase.menu("launch-model-menu", DROP, menu), true))
                },
            ))
            .into_any_element()
    }

    /// An agent's models, as it reported them.
    /// The agent's models, without its own "default" entry and the model it runs: the "Default" row is those.
    pub(super) fn agent_models(&self, agent: SessionAgent) -> Vec<nexus_workbench::agent_models::Model> {
        super::model_menu::listed_models(&self.agent_catalog(agent))
    }

    /// The agent's models as it reported them.
    pub(super) fn agent_catalog(&self, agent: SessionAgent) -> Vec<nexus_workbench::agent_models::Model> {
        let info = if matches!(self.view, View::Code(_)) {
            self.picker_harness_info(agent)
        } else {
            self.launch_harness_info(agent)
        };
        info.and_then(|info| info.models.clone()).unwrap_or_default()
    }

    /// A remembered or session model as the picker lists it: an alias the list leaves out because it runs what
    /// "Default" runs (`opus` beside Opus 5.5) is the "Default" row, so that row shows as picked.
    pub(super) fn listed_pick(&self, agent: SessionAgent, model: Option<&str>) -> Option<String> {
        let id = model?;
        let catalog = self.agent_catalog(agent);
        let listed = super::model_menu::listed_models(&catalog);
        let hidden = !listed.iter().any(|row| row.id == id)
            && catalog
                .iter()
                .any(|row| row.id == id && !crate::code::is_default_entry(row));
        (!hidden).then(|| id.to_string())
    }

    /// Whether an agent can be picked: unknown until the agents are read, then only an installed one.
    pub(super) fn agent_installed(&self, agent: SessionAgent) -> bool {
        self.launch_harness_info(agent)
            .map_or_else(|| self.harness_machine().is_none(), |info| info.path.is_some())
    }

    /// The open Code session the model menu picks for, when its chip opened the menu: its agent and model. A session
    /// can switch between Claude Code and Codex after the person confirms its context handoff.
    pub(super) fn session_pick(&self) -> Option<(SessionAgent, Option<String>)> {
        match &self.view {
            View::Code(view) if view.dock.menu == Some(super::code_dock::Menu::Model) => self
                .code_session()
                .map(|session| (session.agent, session.picker_values().model)),
            _ => None,
        }
    }

    fn launch_picker_ready(&self) -> bool {
        (!matches!(self.view, View::Code(_)) || self.session_pick().is_some())
            && !self.picker_harness_waiting()
            && self.picker_harness_error().is_none()
    }

    fn picker_agent_installed(&self, agent: SessionAgent) -> bool {
        if self.cloud_ui.on && matches!(self.view, View::NewCode) {
            return super::harness::offers_agent_on(agent, true);
        }
        self.picker_harness_info(agent).is_some_and(|info| info.path.is_some())
    }

    fn launch_picker_catalog(&self) -> Vec<super::code_picker::Catalog<SessionAgent>> {
        let session_pick = self.session_pick();
        let remote = match session_pick {
            Some(_) => self.picker_machine().is_some(),
            None => self.cloud_ui.on || self.picker_machine().is_some(),
        };
        SessionAgent::ALL
            .into_iter()
            // A session lists its own agent and those it can switch to; the switch refuses any other. Its own agent
            // stays whatever `offers_agent_on` says for the target machine.
            .filter(|agent| {
                !self.is_local()
                    || local_plan_agent(*agent)
                    || session_pick.as_ref().is_some_and(|(current, _)| current == agent)
                    || self
                        .code_form
                        .providers
                        .iter()
                        .any(|choice| choice.agents.contains(agent))
            })
            .filter(|agent| match session_pick {
                Some((current, _)) if *agent == current => true,
                Some((current, _)) => {
                    nexus_workbench::agent_switch::can_switch(current, *agent)
                        && super::harness::offers_agent_on(*agent, remote)
                }
                None => super::harness::offers_agent_on(*agent, remote),
            })
            .map(|agent| super::code_picker::Catalog {
                agent,
                name: label(agent).to_string(),
                installed: self.picker_agent_installed(agent),
                default_name: super::code_picker::default_label(
                    crate::code::default_model_name(&self.agent_catalog(agent)).as_deref(),
                ),
                models: super::code_picker::with_auto(
                    self.session_pick().is_none()
                        && self.account_gateway().is_some()
                        && super::code_picker::takes_auto(agent),
                    self.agent_models(agent)
                        .into_iter()
                        .map(|model| super::code_picker::Model {
                            id: model.id,
                            name: model.name,
                        })
                        .collect(),
                ),
            })
            .collect()
    }

    fn can_pick_launch_row(&self, row: &LaunchRow) -> bool {
        super::code_picker::can_pick(
            &self.launch_picker_catalog(),
            self.launch_picker_ready(),
            self.nexus_session_agent(),
            row,
        )
    }

    /// The open session's agent when it is Nexus's own (no coding agent): its picker lists only that agent's models.
    fn nexus_session_agent(&self) -> Option<SessionAgent> {
        self.session_pick()
            .map(|(agent, _)| agent)
            .filter(|agent| agent.coding().is_none())
    }

    /// The model panel's list, with the same readiness and catalog guards as selection, and how many of its rows are
    /// favorites (listed first, under their own heading).
    pub(super) fn launch_panel(&self) -> (Vec<LaunchRow>, usize) {
        super::code_picker::panel_rows(
            &self.launch_picker_catalog(),
            self.launch_picker_ready(),
            self.nexus_session_agent(),
            &self.code_form.favorites(),
            &self.model_query,
        )
    }

    pub(super) fn launch_rows(&self) -> Vec<LaunchRow> {
        self.launch_panel().0
    }

    /// A row's name: the model's, or for the agent's own default the model it runs ("Opus 5.5 · Default").
    fn row_name(&self, row: &LaunchRow) -> String {
        match &row.model {
            None => super::code_picker::default_label(
                crate::code::default_model_name(&self.agent_catalog(row.agent)).as_deref(),
            ),
            Some(id) if id == super::code_picker::AUTO => "Auto".into(),
            Some(id) => self
                .agent_catalog(row.agent)
                .into_iter()
                .find(|model| &model.id == id)
                .map_or_else(|| id.clone(), |model| model.name),
        }
    }

    /// Pick a row: its agent (with what was last picked for it), then its model, and close the menu.
    pub(super) fn pick_launch_row(&mut self, row: LaunchRow, window: &mut Window, cx: &mut Context<Self>) {
        if !self.select_launch_row(row) {
            return;
        }
        self.refresh_auto_route(cx);
        self.overlay.close_launch_menu();
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Make a row the new session's model, its agent with what was last picked for it, and keep the panel open.
    /// Whether the row could be picked.
    fn select_launch_row(&mut self, row: LaunchRow) -> bool {
        if !self.can_pick_launch_row(&row) {
            return false;
        }
        self.code_form.auto_any = false;
        if self.code_form.agent != row.agent {
            self.code_form.take_agent(row.agent);
        }
        // An effort stays only with the effective model it was picked for.
        let current = self.listed_pick(row.agent, self.code_form.model.as_deref());
        self.code_form.effort =
            super::code_picker::effort_after_pick(current.as_deref(), row.model.as_deref(), self.code_form.effort);
        self.code_form.pick_catalog_model(row.model);
        true
    }

    fn select_launch_options_row(&mut self, row: LaunchRow) {
        if self.code_form.server_models().is_none() || self.code_form.agent != row.agent {
            self.select_launch_row(row);
        }
    }

    /// Whether the panel offers "Auto · any agent": signed in, on this computer, with two or more installed agents that
    /// Auto routes.
    pub(super) fn offers_auto_any(&self) -> bool {
        matches!(self.auth, super::Auth::SignedIn)
            && !self.cloud_ui.on
            && self.harness_machine().is_none()
            && SessionAgent::ALL
                .into_iter()
                .filter(|agent| super::code_picker::takes_auto(*agent) && self.picker_agent_installed(*agent))
                .count()
                >= 2
    }

    /// Pick "Auto · any agent": Gateway picks the agent, the model and who pays, the plan first.
    pub(super) fn pick_auto_any(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.offers_auto_any() {
            return;
        }
        self.code_form.pick_auto_any();
        self.refresh_auto_route(cx);
        self.overlay.close_launch_menu();
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Pick a row of the model menu: for the open session when its chip opened it, else for the new session.
    pub(super) fn pick_model_row(&mut self, row: LaunchRow, window: &mut Window, cx: &mut Context<Self>) {
        let open =
            self.overlay.launch_menu() == Some(LaunchMenu::Model) || self.dock_menu_is(super::code_dock::Menu::Model);
        if !open || !self.can_pick_launch_row(&row) || !self.launch_rows().contains(&row) {
            return;
        }
        if let Some((agent, current)) = self.session_pick() {
            if agent != row.agent {
                self.request_agent_switch(row, None, cx);
                return;
            }
            let current = self.listed_pick(agent, current.as_deref());
            let effort = self.code_session().and_then(|session| session.picker_values().effort);
            let next_effort = super::code_picker::effort_after_pick(current.as_deref(), row.model.as_deref(), effort);
            self.update_session(Some(row.model), Some(next_effort), None, cx);
            self.close_dock_menu(cx);
            self.focus_composer(window, cx);
        } else {
            self.pick_launch_row(row, window, cx);
        }
    }

    /// Open the model menu's search empty and focused, with the cursor on the picked row.
    pub(super) fn prepare_model_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.view, View::Code(_)) {
            self.refresh_picker_harness(cx);
        }
        self.reset_model_search(window, cx);
        let picked = match self.session_pick() {
            Some((agent, model)) => LaunchRow {
                agent,
                model: self.listed_pick(agent, model.as_deref()),
            },
            None => LaunchRow {
                agent: self.code_form.agent,
                model: if self.code_form.server_models().is_some() {
                    None
                } else {
                    self.listed_pick(self.code_form.agent, self.code_form.model.as_deref())
                },
            },
        };
        self.model_cursor = self.launch_rows().iter().position(|row| *row == picked).unwrap_or(0);
        self.model_cursor_on_open = true;
        self.reveal_model_cursor();
    }

    /// Ctrl+1 to Ctrl+9 on the new-session screen: the favorite at that place. Whether the key was one.
    /// The place counts the favorites the panel lists (and badges), not the stored ones, so a favorite whose agent or
    /// model is gone shifts no shortcut.
    pub(super) fn pick_favorite(&mut self, place: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let listed = super::code_picker::listed_favorites(
            &self.launch_picker_catalog(),
            self.launch_picker_ready(),
            &self.code_form.favorites(),
        );
        match listed.get(place) {
            Some(row) => {
                let row = row.clone();
                self.pick_launch_row(row, window, cx);
                true
            }
            None => false,
        }
    }

    /// Close either Code model menu and return keyboard input to its composer.
    pub(super) fn dismiss_code_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.view, View::Code(_)) {
            self.close_dock_menu(cx);
        } else if self.overlay.launch_menu() == Some(LaunchMenu::Model) {
            self.close_launch_menu(cx);
        }
        self.focus_composer(window, cx);
    }

    /// The model panel: the list on the left, Favorites first and then each installed agent's models, and
    /// the highlighted row's settings on the right: a star, who pays, the reasoning and the permissions. A new
    /// session's panel and an open session's share it; a session keeps its agent and has no star or payer.
    pub(super) fn render_launch_model_menu(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if !self.launch_picker_ready() {
            return v_flex()
                .id("launch-model-status")
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.dismiss_code_model_menu(window, cx);
                }))
                .w(px(PANEL_WIDTH))
                .p(px(16.))
                .text_sm()
                .text_color(palette.muted)
                .child(
                    self.picker_harness_error()
                        .unwrap_or("Checking agents and models…")
                        .to_string(),
                )
                .into_any_element();
        }
        let session = self.session_pick();
        let (rows, starred) = self.launch_panel();
        let searching = !self.model_query.trim().is_empty();
        let heading = |text: SharedString| {
            div()
                .flex_none()
                .px(px(8.))
                .pt(px(8.))
                .pb(px(4.))
                .text_size(px(11.5))
                .text_color(palette.faint)
                .child(text)
        };
        let note = |text: &'static str| {
            div()
                .px(px(8.))
                .pb(px(6.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(text)
        };
        let missing = self.missing_panel_rows();
        let mut list = v_flex().p(px(6.)).gap(px(1.));
        // A search that finds only agents that can't run here lists them with no "No model matches." over them.
        if rows.is_empty() && !(searching && !missing.is_empty()) {
            list = list.child(
                div()
                    .py(px(28.))
                    .px(px(12.))
                    .text_center()
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .child(if searching {
                        "No model matches."
                    } else {
                        "No agent is installed here."
                    }),
            );
        } else if session.is_none() && !searching && starred == 0 {
            list = list
                .child(heading("Favorites".into()))
                .child(note("Star a model to keep it here, with a shortcut."));
        }
        // Auto picks the agent too: one row above the agents, for a new session with two or more agents Auto routes.
        if session.is_none() && !searching && self.offers_auto_any() {
            let picked = self.code_form.auto_any;
            list = list.child(heading("Auto".into())).child(
                h_flex()
                    .id("launch-auto-any")
                    .flex_none()
                    .h(px(28.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .hover(|style| super::code_dock::menu_item_hover(style, palette))
                    .on_click(cx.listener(|this, _, window, cx| this.pick_auto_any(window, cx)))
                    .child(Icon::new(Lucide::Sparkles).size(px(13.)).text_color(palette.icon))
                    .child(div().flex_1().min_w(px(0.)).truncate().child("Auto · any agent"))
                    .child(div().w(px(14.)).flex_none().when(picked, |slot| {
                        slot.child(Icon::new(Lucide::Check).size(px(13.)).text_color(palette.foreground))
                    })),
            );
        }
        let mut last = None;
        for (ix, row) in rows.iter().enumerate() {
            let favorite = ix < starred;
            if favorite && ix == 0 {
                list = list.child(heading("Favorites".into()));
            } else if !favorite && last != Some(row.agent) {
                list = list.child(heading(label(row.agent).into()));
            }
            if !favorite {
                last = Some(row.agent);
            }
            let cursor = ix == self.model_cursor;
            let picked = self.launch_row_picked(row, session.as_ref());
            let shortcut = favorite.then_some(ix).filter(|ix| *ix < 9).map(|ix| {
                let key = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };
                format!("{key}{}", ix + 1)
            });
            let pick_row = row.clone();
            list = list.child(
                h_flex()
                    .id(("launch-row", ix))
                    .flex_none()
                    .h(px(28.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .when(cursor, |item| item.bg(palette.menu_hover))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.model_cursor != ix {
                            this.model_cursor = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_model_row(pick_row.clone(), window, cx)))
                    .child(maker_mark(agent_maker(row.agent), 13., palette))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(self.row_name(row)))
                    .when_some(shortcut, |item, shortcut| {
                        item.child(
                            div()
                                .flex_none()
                                .px(px(5.))
                                .rounded(px(4.))
                                .bg(palette.hover)
                                .text_size(px(11.))
                                .text_color(palette.muted)
                                .child(shortcut),
                        )
                    })
                    .child(div().w(px(14.)).flex_none().when(picked, |slot| {
                        slot.child(Icon::new(Lucide::Check).size(px(13.)).text_color(palette.foreground))
                    })),
            );
        }
        // Agents that can't run here follow the models as quiet rows (`missing_agents.rs`).
        if !missing.is_empty() {
            list = list
                .child(div().h(px(6.)).flex_none())
                .children(self.render_missing_rows(&missing, rows.len(), palette, cx));
        }
        let at_missing = self.missing_at_cursor(rows.len(), &missing);
        let enter = match at_missing {
            Some(super::missing_agents::MissingRow::Fold(_)) if self.code_form.missing_open => "↵ close",
            Some(super::missing_agents::MissingRow::Fold(_)) => "↵ open",
            Some(super::missing_agents::MissingRow::Agent(_)) => "↵ install",
            None => "↵ pick",
        };
        let keys = h_flex()
            .flex_none()
            .h(px(28.))
            .px(px(12.))
            .gap(px(10.))
            .border_t_1()
            .border_color(palette.border)
            .text_size(px(11.))
            .text_color(palette.faint)
            .child("↑↓ move")
            .child(enter)
            .child("esc close");
        let detail = match rows.get(self.model_cursor) {
            Some(row) => Some(self.render_model_detail(row.clone(), session.is_some(), palette, cx)),
            None => at_missing.map(|row| self.render_missing_detail(row, palette, cx)),
        };

        v_flex()
            .id("launch-model-menu")
            .occlude()
            .w(px(PANEL_WIDTH))
            .h(px(LAUNCH_BODY))
            .rounded(px(12.))
            .bg(palette.menu)
            // Over the art, the panel is frosted glass, as the other menus.
            .when(self.frosted(), |menu| {
                menu.relative().bg(transparent_black()).child(super::backdrop::frost(
                    12.,
                    palette.solid().menu.opacity(0.8),
                    None,
                ))
            })
            .shadow(palette.pop_shadow(palette.pop_ring()))
            .overflow_hidden()
            .text_color(palette.foreground)
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.dismiss_code_model_menu(window, cx);
            }))
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.))
                    .items_start()
                    .child(
                        v_flex()
                            .w(px(232.))
                            .flex_none()
                            .h_full()
                            .border_r_1()
                            .border_color(palette.border)
                            .child(
                                h_flex()
                                    .flex_none()
                                    .h(px(38.))
                                    .px(px(12.))
                                    .gap(px(8.))
                                    .border_b_1()
                                    .border_color(palette.border)
                                    .child(Icon::new(Lucide::Search).size(px(14.)).text_color(palette.faint))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(13.))
                                            .child(Input::new(&self.model_search).appearance(false)),
                                    ),
                            )
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h(px(0.))
                                    .child(
                                        list.id("launch-model-list")
                                            .size_full()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.model_scroll),
                                    )
                                    .child(bottom_fade(palette.menu)),
                            )
                            .child(keys),
                    )
                    .child(
                        v_flex()
                            .id("launch-model-detail")
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .overflow_y_scroll()
                            .children(detail),
                    ),
            )
            // Its caller draws it through `motion::presence`, which plays the entrance and the exit.
            .into_any_element()
    }

    /// Whether a row is the model the session runs, or the one the new session will.
    fn launch_row_picked(&self, row: &LaunchRow, session: Option<&(SessionAgent, Option<String>)>) -> bool {
        match session {
            Some((agent, _)) if *agent != row.agent => false,
            Some((_, model)) => {
                let raw = |id: &str| id.rsplit('/').next().unwrap_or(id).to_string();
                self.listed_pick(row.agent, model.as_deref()).as_deref().map(raw) == row.model.as_deref().map(raw)
            }
            None => {
                let form = &self.code_form;
                !form.auto_any
                    && form.agent == row.agent
                    && self.listed_pick(form.agent, form.model.as_deref()) == row.model
            }
        }
    }

    /// The model panel's right pane for the highlighted row. A control on a row that is not picked picks it first,
    /// and the panel stays open.
    fn render_model_detail(&self, row: LaunchRow, in_session: bool, palette: Palette, cx: &mut Context<Self>) -> Div {
        let form = &self.code_form;
        let picked = self.launch_row_picked(&row, self.session_pick().as_ref());
        let values = in_session
            .then(|| self.code_session().map(|session| session.picker_values()))
            .flatten();
        let section = |name: &'static str| div().text_size(px(11.5)).text_color(palette.faint).child(name);
        let segment = |id: SharedString, content: AnyElement, on: bool, enabled: bool| {
            h_flex()
                .id(id)
                .flex_1()
                .min_w(px(0.))
                .h(px(26.))
                .px(px(8.))
                .justify_center()
                .rounded(px(6.))
                .text_size(px(12.5))
                .when(on, |button| button.bg(palette.selected).text_color(palette.foreground))
                .when(!on, |button| button.text_color(palette.muted))
                .map(|button| {
                    if enabled {
                        button
                            .cursor_pointer()
                            .when(!on, |button| button.hover(|style| menu_item_hover(style, palette)))
                    } else {
                        button.opacity(0.45)
                    }
                })
                .child(content)
        };
        let track = || h_flex().p(px(2.)).gap(px(2.)).rounded(px(8.)).bg(palette.hover);

        // The head: the maker, the model's name and, for a new session, its star.
        let starred = form.favorites().contains(&row);
        let star_row = row.clone();
        let head = h_flex()
            .gap(px(10.))
            .child(
                div()
                    .flex_none()
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .bg(palette.raised)
                    .child(maker_mark(agent_maker(row.agent), 16., palette)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.row_name(&row)),
            )
            .when(!in_session, |head| {
                head.child(
                    div()
                        .id("launch-detail-star")
                        .flex_none()
                        .p(px(5.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|style| menu_item_hover(style, palette))
                        .tooltip(move |window, cx| {
                            Tooltip::new(if starred {
                                "Remove from favorites"
                            } else {
                                "Add to favorites"
                            })
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.code_form.toggle_favorite(&star_row);
                            this.sync_model_rows();
                            this.reveal_model_cursor();
                            cx.notify();
                        }))
                        // Amber only when starred; otherwise a faint outline.
                        .child(Icon::new(Lucide::Star).size(px(15.)).text_color(if starred {
                            star()
                        } else {
                            palette.faint.opacity(0.6)
                        })),
                )
            });
        let mut pane = v_flex().p(px(14.)).gap(px(12.)).child(head);

        if !in_session && self.is_local() {
            let choices = form
                .providers
                .iter()
                .filter(|provider| provider.agents.contains(&row.agent));
            if choices.clone().next().is_some() {
                let mut providers = v_flex().gap(px(6.)).child(section("Local provider"));
                for choice in choices {
                    let id = choice.id.clone();
                    let row = row.clone();
                    let is_server = choice.server_models.is_some();
                    providers = providers.child(
                        segment(
                            SharedString::from(id.as_str().to_string()).into(),
                            choice.name.clone().into_any_element(),
                            form.provider.as_ref() == Some(&id),
                            true,
                        )
                        .map(|control| super::first_connection::focusable_connection_control(control, palette.strong))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if (this.code_form.provider.as_ref() != Some(&id) || this.failed_provider_draft().is_some())
                                && this.select_launch_row(row.clone())
                            {
                                this.clear_failed_provider_draft();
                                if is_server {
                                    this.code_form.model = None;
                                }
                                this.code_form.provider = Some(id.clone());
                                if this.code_form.provider_selection_error().is_some() {
                                    this.code_form.first_provider = Some(FirstProvider::Selected);
                                }
                            }
                            cx.notify();
                        })),
                    );
                    if form.provider.as_ref() == Some(&choice.id)
                        && let Some(models) = &choice.server_models
                    {
                        for model in models {
                            let model = model.clone();
                            providers = providers.child(
                                segment(
                                    SharedString::from(format!("server-model-{model}")).into(),
                                    model.clone().into_any_element(),
                                    form.selected_server_model() == Some(model.as_str()),
                                    true,
                                )
                                .map(|control| {
                                    super::first_connection::focusable_connection_control(control, palette.strong)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.code_form.pick_server_model(&model);
                                    cx.notify();
                                })),
                            );
                        }
                    }
                }
                pane = pane.child(providers);
            }
        }

        if !in_session && self.is_local() && local_plan_agent(row.agent) {
            let row = row.clone();
            pane = pane.child(
                v_flex().gap(px(6.)).child(section("CLI subscription")).child(
                    segment(
                        "launch-local-plan".into(),
                        plan_payer_label(row.agent).into_any_element(),
                        form.agent == row.agent && form.provider.is_none(),
                        true,
                    )
                    .map(|control| super::first_connection::focusable_connection_control(control, palette.strong))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.select_launch_row(row.clone()) {
                            this.clear_failed_provider_draft();
                            this.code_form.pick_plan(row.agent);
                        }
                        cx.notify();
                    })),
                ),
            );
        } else if !in_session && !self.is_local() && plan_only(row.agent) {
            let plan = plan_payer_label(row.agent);
            let off = (form.access.is_some() && !form.plans_allowed()).then(|| {
                crate::setup::plans_off::off_for(self.account.as_ref().map_or("", |account| &account.org_name))
            });
            pane = pane.child(
                v_flex()
                    .id("launch-via-fixed-plan")
                    .gap(px(6.))
                    .child(section("Who pays"))
                    .child(
                        h_flex()
                            .gap(px(5.))
                            .text_size(px(12.5))
                            .text_color(if off.is_some() {
                                palette.muted
                            } else {
                                palette.foreground
                            })
                            .when(off.is_some(), |row| {
                                row.child(Icon::new(Lucide::Lock).size(px(12.)).text_color(palette.icon))
                            })
                            .child(match &off {
                                Some(off) => format!("{plan} · {off}"),
                                None => plan,
                            }),
                    ),
            );
        } else if !in_session && !self.is_local() {
            let org = self
                .account
                .as_ref()
                .map(|account| account.org_name.clone())
                .unwrap_or_default();
            let plans = form.plans_allowed();
            let row_pay = form.row_pay(row.agent, &org);
            let (pick, payer) = (row_pay.marked(), row_pay.payer);
            // The organization's choice is named as the budget it counts toward (`payer_name`), never "key" or
            // "credits".
            let api = payer_name(Payer::Credits, row.agent, &org).unwrap_or_default();
            // What Auto picks now, as its tooltip (`auto_row_hint`).
            let auto_hint: SharedString = {
                let resolved = form.access.as_ref().map(|access| {
                    let auto = access.default_via(row.agent);
                    access.payer(row.agent, auto, &org).0
                });
                auto_row_hint(resolved, plans, row.agent, &org).into()
            };
            // A locked plan says why under the track, never in a tooltip only.
            let coding = row.agent.coding().is_some();
            let installed = coding && self.agent_installed(row.agent);
            let plan_reason = if !coding {
                None
            } else if !plans {
                Some(self.plan_off_reason(&plan_payer_label(row.agent), &org, palette))
            } else if !installed {
                Some(self.plan_install_reason(row.agent, palette, cx))
            } else {
                None
            };
            let pay = |id: &'static str, name: AnyElement, on: bool, enabled: bool, pick: Option<Via>| {
                let row = row.clone();
                segment(id.into(), name, on, enabled).when(enabled, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        this.select_launch_row(row.clone());
                        match pick {
                            Some(Via::Subscription) => this.code_form.pick_plan(row.agent),
                            via => {
                                this.code_form.provider = None;
                                this.code_form.set_via(via);
                            }
                        }
                        this.refresh_auto_route(cx);
                        cx.notify();
                    }))
                })
            };
            let mut choices = track().child(
                div()
                    .id("launch-via-auto-tip")
                    .flex_1()
                    .min_w(px(0.))
                    .tooltip(move |window, cx| Tooltip::new(auto_hint.clone()).build(window, cx))
                    .child(pay(
                        "launch-via-auto",
                        "Auto".into_any_element(),
                        pick.is_none(),
                        true,
                        None,
                    )),
            );
            // A plan choice for a coding agent; Nexus's own agent has no plan.
            if coding {
                let plan = plan_payer_label(row.agent);
                let name: AnyElement = if plans {
                    plan.clone().into_any_element()
                } else {
                    h_flex()
                        .gap(px(4.))
                        .justify_center()
                        .child(Icon::new(Lucide::Lock).size(px(11.)))
                        .child(plan.clone())
                        .into_any_element()
                };
                choices = choices.child(
                    div()
                        .id("launch-via-plan-tip")
                        .flex_1()
                        .min_w(px(0.))
                        .when(plans && installed, |tip| {
                            let hint: SharedString = format!("Runs with {}", label(row.agent)).into();
                            tip.tooltip(move |window, cx| Tooltip::new(hint.clone()).build(window, cx))
                        })
                        .child(pay(
                            "launch-via-plan",
                            name,
                            pick == Some(Via::Subscription),
                            plans && installed,
                            Some(Via::Subscription),
                        )),
                );
            }
            choices = choices.child(pay(
                "launch-via-nexus",
                api.into_any_element(),
                pick == Some(Via::Nexus),
                true,
                Some(Via::Nexus),
            ));
            let budget = match payer {
                Payer::Key | Payer::Credits => Some(div().text_color(palette.faint).child(format!(
                    "Counts toward {}'s budget",
                    if org.is_empty() {
                        "your organization"
                    } else {
                        org.as_str()
                    }
                ))),
                // The key source stays, with its reason; who may add a key gets the way to it.
                Payer::None => Some(self.no_key_reason(row.agent, palette, cx)),
                Payer::Checking => Some(div().text_color(palette.faint).child("Checking who pays…")),
                Payer::Plan => None,
            };
            pane = pane.child(
                v_flex()
                    .gap(px(6.))
                    .child(
                        h_flex().justify_between().child(section("Who pays")).child(
                            div()
                                .id("launch-link-plan")
                                .text_size(px(11.5))
                                .text_color(palette.muted)
                                .cursor_pointer()
                                .hover(|style| style.text_color(palette.foreground))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_launch_menu(cx);
                                    this.open_native(crate::pages::PageId::Subscriptions, window, cx);
                                }))
                                .child("Link a plan…"),
                        ),
                    )
                    .child(choices)
                    .children(plan_reason)
                    .children(budget.map(|line| line.text_size(px(12.)))),
            );
        }

        // Reasoning: Default and the model's own levels.
        let efforts = self.row_efforts(row.agent, row.model.as_deref());
        if !efforts.is_empty() {
            let current = match (&values, picked) {
                (Some(values), true) => values.effort,
                (Some(_), false) => self.panel_session_model(&row).1.flatten(),
                (None, _) => self.launch_row_effort(&row),
            };
            let buttons = std::iter::once((None, "Default"))
                .chain(efforts.iter().map(|&(effort, name)| (Some(effort), name)))
                .map(|(effort, name)| {
                    let row = row.clone();
                    segment(
                        SharedString::from(format!("launch-effort-{}", effort.map_or("default", Effort::as_str))),
                        name.into_any_element(),
                        current == effort,
                        true,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if in_session && this.session_pick().is_some_and(|(agent, _)| agent != row.agent) {
                            this.request_agent_switch(row.clone(), effort, cx);
                        } else if in_session {
                            let (model, effort, mode) = this.panel_session_update(&row, PanelPick::Effort(effort));
                            this.update_session_from_panel(model, effort, mode, cx);
                        } else {
                            this.select_launch_options_row(row.clone());
                            this.code_form.effort = effort;
                            if this.code_form.server_models().is_none() {
                                this.code_form.remember_model();
                            }
                            cx.notify();
                        }
                    }))
                });
            pane = pane.child(
                v_flex()
                    .gap(px(6.))
                    .child(section("Reasoning"))
                    .child(track().flex_wrap().children(buttons)),
            );
        }

        // Speed: an open session's fast mode, on a model that has one (`effort::has_fast`).
        if let Some(session) = self
            .code_session()
            .filter(|session| in_session && session.agent == row.agent)
            && super::effort::has_fast(
                row.agent,
                session.via,
                &self.agent_catalog(row.agent),
                row.model.as_deref(),
            )
        {
            let fast = session.fast_shown();
            let off = session
                .connection
                .as_ref()
                .and_then(|connection| connection.fast_off.as_deref());
            let hint = super::code_dock::fast_hint(row.agent, fast.then_some(off).flatten());
            let buttons = [(false, "Standard"), (true, "Fast")].map(|(on, name)| {
                let row = row.clone();
                segment(
                    SharedString::from(if on {
                        "launch-speed-fast"
                    } else {
                        "launch-speed-standard"
                    }),
                    h_flex()
                        .gap(px(4.))
                        .when(on, |content| content.child(Icon::new(Lucide::Zap).size(px(14.))))
                        .child(name)
                        .into_any_element(),
                    fast == on,
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick_panel_fast(&row, on, cx)))
            });
            pane = pane.child(
                v_flex()
                    .gap(px(6.))
                    .child(section("Speed"))
                    .child(track().children(buttons))
                    .child(div().text_size(px(12.)).text_color(palette.faint).child(hint)),
            );
        }

        // Permissions: one icon per mode, and the picked one's name and meaning under them.
        let mode_agent = self
            .code_session()
            .filter(|_| in_session)
            .map_or(row.agent, |session| session.agent);
        let behavior = nexus_workbench::harness::harness(mode_agent);
        let mode = values.as_ref().map_or(form.mode, |values| values.mode);
        let mode = mode_for_agent(mode_agent, Some(mode));
        let buttons = MODES
            .iter()
            .filter(|(each, ..)| behavior.offered_modes().contains(each))
            .map(|&(each, ..)| {
                let (name, _) = permission_copy(mode_agent, each);
                let on = mode == each;
                segment(
                    SharedString::from(format!("launch-mode-{}", each.as_str())),
                    Icon::new(mode_icon(each))
                        .size(px(15.))
                        .text_color(if on { palette.foreground } else { palette.muted })
                        .into_any_element(),
                    on,
                    true,
                )
                .tooltip(move |window, cx| Tooltip::new(name).build(window, cx))
                .on_click({
                    let row = row.clone();
                    cx.listener(move |this, _, _, cx| this.pick_panel_mode(row.clone(), each, cx))
                })
            });
        let (mode_name, mode_hint) = permission_copy(mode_agent, mode);
        pane.child(
            v_flex()
                .gap(px(6.))
                .child(section("Permissions"))
                .child(track().children(buttons))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .child(div().flex_none().child(mode_name))
                        .child(div().min_w(px(0.)).text_color(palette.faint).child(mode_hint)),
                ),
        )
    }

    /// A Permissions pick in the model panel. For a new session it picks the highlighted row first, as Who pays and
    /// Reasoning do. An open session takes the mode alone ([`Workspace::panel_session_update`]).
    pub(super) fn pick_panel_mode(&mut self, row: LaunchRow, mode: PermissionMode, cx: &mut Context<Self>) {
        if self.session_pick().is_some() {
            let (model, effort, mode) = self.panel_session_update(&row, PanelPick::Mode(mode));
            self.update_session_from_panel(model, effort, mode, cx);
        } else {
            self.select_launch_options_row(row);
            self.code_form.mode = mode;
            self.code_form.remember_mode();
            cx.notify();
        }
    }

    /// A Speed pick in an open session's model panel. Fast mode belongs to a model that has it, so on a row that is not
    /// the session's model it moves the session there too, as Reasoning does. The panel stays open.
    fn pick_panel_fast(&mut self, row: &LaunchRow, on: bool, cx: &mut Context<Self>) {
        let (model, effort) = self.panel_session_model(row);
        if model.is_some() {
            self.update_session(model, effort, None, cx);
        }
        self.set_session_fast(on, cx);
        if let View::Code(view) = &mut self.view {
            view.dock.menu = Some(super::code_dock::Menu::Model);
        }
        cx.notify();
    }

    /// What an open session's panel control sends. A Reasoning level belongs to the highlighted row's model, so it
    /// moves the session to that row with it. A Permissions mode is the session's, not a model's, so it goes alone:
    /// the pane follows the pointer, which can cross other rows on its way there.
    pub(super) fn panel_session_update(&self, row: &LaunchRow, pick: PanelPick) -> SessionUpdate {
        match pick {
            PanelPick::Effort(effort) => (self.panel_session_model(row).0, Some(effort), None),
            PanelPick::Mode(mode) => (None, None, Some(mode)),
        }
    }

    /// The effort a new session would keep if this row were picked (`select_launch_row`): its agent's saved one when
    /// the row is another agent's, and only while the effective model stays the same.
    pub(super) fn launch_row_effort(&self, row: &LaunchRow) -> Option<Effort> {
        let form = &self.code_form;
        let (model, effort) = if form.agent == row.agent {
            (form.model.clone(), form.effort)
        } else {
            form.saved_pair(row.agent)
        };
        let current = self.listed_pick(row.agent, model.as_deref());
        super::code_picker::effort_after_pick(current.as_deref(), row.model.as_deref(), effort)
    }

    /// The model and effort an open session's panel control sends with its own value: nothing when the highlighted
    /// row is the session's model, else that row's model and the effort `effort_after_pick` keeps for it.
    pub(super) fn panel_session_model(&self, row: &LaunchRow) -> (Option<Option<String>>, Option<Option<Effort>>) {
        let Some((agent, current)) = self.session_pick() else {
            return (None, None);
        };
        if agent != row.agent {
            return (None, None);
        }
        if self.launch_row_picked(row, Some(&(agent, current.clone()))) {
            return (None, None);
        }
        let current = self.listed_pick(agent, current.as_deref());
        let effort = self.code_session().and_then(|session| session.picker_values().effort);
        let effort = super::code_picker::effort_after_pick(current.as_deref(), row.model.as_deref(), effort);
        (Some(row.model.clone()), Some(effort))
    }

    /// A reasoning or permissions pick in an open session's panel: the session takes it and the panel stays open.
    fn update_session_from_panel(
        &mut self,
        model: Option<Option<String>>,
        effort: Option<Option<Effort>>,
        mode: Option<PermissionMode>,
        cx: &mut Context<Self>,
    ) {
        self.update_session(model, effort, mode, cx);
        if let View::Code(view) = &mut self.view {
            view.dock.menu = Some(super::code_dock::Menu::Model);
        }
        cx.notify();
    }

    /// The picked model's efforts; none for the default model, whose effort is the agent's own.
    pub(super) fn form_efforts(&self) -> Vec<(Effort, &'static str)> {
        self.row_efforts(self.code_form.agent, self.code_form.model.as_deref())
    }

    /// A model's efforts: what its agent reports for it (for Default, what Default runs), else the agent's known
    /// levels.
    pub(super) fn row_efforts(&self, agent: SessionAgent, model: Option<&str>) -> Vec<(Effort, &'static str)> {
        let catalog = self.agent_catalog(agent);
        let reported = match model {
            Some(id) => catalog.iter().find(|entry| entry.id == id),
            // Default takes what its model does (the Nexus app's chip reads "· Default", not the permission mode).
            None => catalog.iter().find(|entry| crate::code::is_default_entry(entry)),
        }
        .map(|entry| entry.efforts.clone())
        .unwrap_or_else(|| {
            super::code_dock::effort::options(agent, &catalog, model)
                .into_iter()
                .map(|(effort, _)| effort)
                .collect()
        });
        EFFORTS
            .iter()
            .copied()
            .filter(|(effort, _)| reported.contains(effort))
            .collect()
    }

    /// The latest sessions, the picked project's first, under the where line: "Pick up where you left off". Plain rows,
    /// no card: the status mark, the title, "{project} · {agent}" and the age.
    pub(super) fn render_recents(&self, palette: Palette, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let current = self.code_form.project.as_deref();
        let mut sessions: Vec<_> = self
            .projects
            .iter()
            .flat_map(|project| {
                project
                    .sessions
                    .iter()
                    .map(move |session| (project.id.as_str(), project.name.as_str(), session))
            })
            .collect();
        if sessions.is_empty() {
            return None;
        }
        sessions.sort_by(|(a_project, _, a), (b_project, _, b)| {
            (Some(*b_project) == current)
                .cmp(&(Some(*a_project) == current))
                .then_with(|| crate::code::list_order(a, b))
        });
        let now = crate::code::now_ms();
        let rows = sessions.into_iter().take(RECENTS).map(|(_, project, session)| {
            let id = session.id.clone();
            h_flex()
                .id(SharedString::from(format!("recent-{}", session.id)))
                .h(px(32.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(6.))
                .text_size(px(13.))
                .line_height(px(18.))
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .on_click(cx.listener(move |this, _, window, cx| this.open_code(&id, window, cx)))
                .child(
                    div()
                        .flex_none()
                        .size(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(super::sidebar::status_mark(&session.id, session.status, palette)),
                )
                .child(
                    div()
                        .flex_shrink(1.)
                        .min_w(px(0.))
                        .truncate()
                        .text_color(palette.foreground)
                        .child(session.title.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(format!("{project} · {}", label(session.agent))),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(crate::code::age(session.updated_at, now)),
                )
        });
        let head = h_flex()
            .h(px(28.))
            .px(px(8.))
            .justify_between()
            .text_size(px(12.))
            .text_color(palette.faint)
            .child("Pick up where you left off")
            .child(
                div()
                    .id("recents-all")
                    .cursor_pointer()
                    .text_color(palette.muted)
                    .hover(|style| style.text_color(palette.foreground))
                    .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx)))
                    .child("All sessions"),
            );
        // On a background, the list sits on frosted glass (`backdrop.rs`) so it reads over the art.
        let frosted = self.frosted();
        Some(
            v_flex()
                .w_full()
                .when(frosted, |list| {
                    list.relative()
                        .p(px(6.))
                        .rounded(px(16.))
                        .border_1()
                        .border_color(palette.border)
                        .child(super::backdrop::frost(
                            16.,
                            crate::frost::Look::now(&palette).surface,
                            Some(palette.border),
                        ))
                })
                .child(head)
                .children(rows),
        )
    }

    pub(super) fn render_new_code(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        // The art behind the window marks this screen, and its "Background" button is last, over the scrolling column,
        // so it takes its clicks.
        let (art, background) = self.render_backdrop(palette, cx);
        let root = v_flex().relative().size_full().child(art);
        let root = root.child(
            div().id("new-code").size_full().overflow_y_scroll().child(
                v_flex()
                    .min_h_full()
                    .w_full()
                    .max_w(px(self.composer_width()))
                    .mx_auto()
                    .justify_center()
                    .px(px(24.))
                    .py(px(48.))
                    .child(self.render_launch_head(palette))
                    // On Cloud, the setup card or the repository picker stands in for the composer until they are done.
                    .child(
                        div()
                            .mt(px(20.))
                            .w_full()
                            .child(match self.render_cloud_instead(palette, cx) {
                                Some(instead) => instead,
                                // Who pays is in the model panel, so the plan's usage stays by the composer.
                                None => v_flex()
                                    .w_full()
                                    .gap(px(8.))
                                    .children(self.render_usage_warning(palette, cx))
                                    .child(self.render_composer(palette, cx))
                                    .into_any_element(),
                            }),
                    )
                    .child(
                        div()
                            .mt(px(8.))
                            .w_full()
                            .child(self.render_where_line(false, palette, cx)),
                    )
                    .children(
                        self.render_setup_line(palette, cx)
                            .map(|line| div().mt(px(12.)).child(line)),
                    )
                    .children(self.link_note.as_ref().map(|note| {
                        home_note(
                            note.text.clone(),
                            self.render_link_note_actions(note, palette, cx),
                            palette,
                        )
                    }))
                    .children(
                        self.render_home_read_only(palette, cx)
                            .map(|line| div().mt(px(12.)).child(line)),
                    )
                    .children(self.render_recents(palette, cx).map(|recents| {
                        div()
                            .mt(px(if self.offer_setup_line() { 32. } else { 40. }))
                            .child(recents)
                    }))
                    .children(self.render_starters(palette, cx).map(|starters| {
                        div()
                            .mt(px(if self.offer_setup_line() { 32. } else { 40. }))
                            .child(starters)
                    })),
            ),
        );
        root.child(background)
    }
}

/// Agents with every mode share a preference. A smaller set gets its own pick.
fn mode_preference_agent(agent: SessionAgent) -> Option<SessionAgent> {
    let offered = nexus_workbench::harness::harness(agent).offered_modes();
    EVERY_MODE.iter().any(|mode| !offered.contains(mode)).then_some(agent)
}

fn mode_for_agent(agent: SessionAgent, picked: Option<PermissionMode>) -> PermissionMode {
    let behavior = nexus_workbench::harness::harness(agent);
    picked
        .filter(|mode| behavior.offered_modes().contains(mode))
        .unwrap_or_else(|| behavior.default_mode())
}

pub(super) fn local_plan_agent(agent: SessionAgent) -> bool {
    matches!(agent, SessionAgent::Claude | SessionAgent::Codex)
}

pub(super) fn plan_only(agent: SessionAgent) -> bool {
    agent
        .coding()
        .is_some_and(|agent| agent.entry().billing.gateway.is_none())
        && nexus_workbench::cloud_agent::cloud_agent(agent)
            .refused(Via::Nexus)
            .is_some()
}

/// Which maker's mark stands for an agent.
pub(super) fn agent_maker(agent: SessionAgent) -> &'static str {
    match agent {
        SessionAgent::Codex => "openai",
        SessionAgent::Claude => "anthropic",
        SessionAgent::Antigravity => "google",
        SessionAgent::Cursor => "cursor",
        SessionAgent::Opencode => "opencode",
        _ => "nexus",
    }
}

/// A chip of the where line: an icon, a value and, when it opens a menu, a chevron. The label truncates, its tooltip has it whole.
fn context_chip(
    id: &'static str,
    icon: Lucide,
    label: SharedString,
    open: Option<bool>,
    frosted: bool,
    palette: Palette,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .relative()
        .when(frosted, |chip| {
            chip.child(super::backdrop::frost(
                6.,
                palette.solid().raised.opacity(0.6),
                Some(palette.solid().border),
            ))
        })
        .flex_shrink(1.)
        .min_w_0()
        .max_w(px(240.))
        .h(px(28.))
        .px(px(8.))
        .gap(px(6.))
        .rounded(px(6.))
        .text_size(px(13.))
        .line_height(px(18.))
        .text_color(palette.muted)
        .when(open == Some(true), |chip| chip.bg(palette.hover))
        .child(glyph(icon, palette.muted).size(px(14.)))
        .role(if open.is_some() || id == "launch-repo" {
            Role::Button
        } else {
            Role::Label
        })
        .aria_label(label.clone())
        .tooltip({
            let label = label.clone();
            move |window, cx| Tooltip::new(label.clone()).build(window, cx)
        })
        .child(div().min_w_0().truncate().child(label))
        .when_some(open, |chip, open| {
            chip.cursor_pointer().hover(|style| style.bg(palette.hover)).child(
                // The chevron turns over while the menu is open, as the chat composer's model chip's.
                Icon::new(Lucide::ChevronDown)
                    .size(px(14.))
                    .text_color(palette.faint)
                    .with_tween(
                        SharedString::from(format!("{id}-chevron")),
                        if open { 1. } else { 0. },
                        motion::Tween::new(motion::COLLAPSE, motion::ease_out),
                        |icon, turn| icon.rotate(radians(std::f32::consts::PI * turn)),
                    ),
            )
        })
}

/// A where-line menu beside its 28px chip, above everything: under it, or over it when `side` says so
/// (`menu_side.rs`).
fn menu_beside(side: Side, menu: impl IntoElement) -> impl IntoElement {
    let gap = px(super::menu_side::GAP);
    let (top, anchor, offset) = if side.above {
        (px(0.), Anchor::BottomLeft, -gap)
    } else {
        (px(28.), Anchor::TopLeft, gap)
    };
    div().absolute().left_0().top(top).child(
        deferred(
            anchored()
                .anchor(anchor)
                .offset(point(px(0.), offset))
                .snap_to_window_with_margin(px(super::menu_side::MARGIN))
                .child(menu),
        )
        .with_priority(1),
    )
}

/// How far a menu travels as it opens: away from its chip, so down under it and up over it.
fn drop_for(side: Side) -> f32 {
    if side.above { -DROP } else { DROP }
}

/// A favorite's star.
fn star() -> Hsla {
    rgb(0xf5a524).into()
}

/// The permission mode's icon in the effort menu's row.
pub(super) fn mode_icon(mode: PermissionMode) -> Lucide {
    match mode {
        PermissionMode::Default => Lucide::Lock,
        PermissionMode::AcceptEdits => Lucide::Pencil,
        PermissionMode::Auto => Lucide::ShieldCheck,
        PermissionMode::Plan => Lucide::Map,
        PermissionMode::BypassPermissions => Lucide::ShieldOff,
    }
}

/// What the composer knows of the session's plan: still reading, the read failed (and why), or the plan read
/// (None: no signed-in plan for this session's account).
#[derive(Clone, Copy)]
pub(super) enum PlanRead<'a> {
    Checking,
    Failed(&'a str),
    Read(Option<&'a nexus_workbench::plans::PlanUsage>),
}

/// The limit the donut shows: the plan's 5-hour limit, else the tightest one the session's `model` works against. None
/// when the plan cannot be read, as [`nexus_workbench::allowance::limiting_for`] decides.
fn five_hour<'a>(
    plan: Option<&'a nexus_workbench::plans::PlanUsage>,
    model: Option<&str>,
    now: i64,
) -> Option<nexus_workbench::allowance::Allowance<'a>> {
    use nexus_workbench::allowance::{self, Allowance};
    let tightest = allowance::limiting_for(plan, model, now)?;
    plan?
        .limits
        .iter()
        .find(|limit| limit.label == "Session")
        .and_then(|limit| allowance::left(limit).map(|left| Allowance { limit, left }))
        .or(Some(tightest))
}

/// The donut's hover: each shared limit, how much is left and when it resets; or why the plan cannot be read.
fn usage_hint(source: nexus_workbench::allowance::Source, read: PlanRead<'_>, now: i64) -> String {
    use nexus_workbench::allowance::{self, Allowance};
    let name = source.name();
    let plan = match read {
        PlanRead::Checking => return format!("Checking your {name} plan usage…"),
        PlanRead::Failed(error) => return format!("Could not read your {name} plan usage: {error}"),
        PlanRead::Read(None) => return format!("No signed-in {name} plan for this session's account."),
        PlanRead::Read(Some(plan)) => plan,
    };
    if let Some(error) = &plan.error {
        return format!("Could not read your {name} plan usage: {error}");
    }
    if plan
        .limits
        .iter()
        .any(|limit| limit.resets_at.is_some_and(|at| at <= now))
    {
        return format!("A {name} limit just reset. Reading it again…");
    }
    let lines: Vec<String> = plan
        .limits
        .iter()
        // A plan's shared limits, and every group's (Antigravity's model groups).
        .filter(|limit| limit.label == "Session" || limit.label == "Weekly" || !limit.models.is_empty())
        .filter_map(|limit| {
            let left = allowance::left(limit)?;
            let reset = allowance::reset_text(&Allowance { limit, left }, now, &chrono::Local);
            Some(format!(
                "{}: {left}% left. {reset}",
                allowance::limit_label(&limit.label)
            ))
        })
        .collect();
    if lines.is_empty() {
        return if plan.google_cloud {
            nexus_workbench::plans::CLOUD_USAGE_NOTE.into()
        } else {
            format!("Your {name} plan reported no usage limits.")
        };
    }
    lines.join("\n")
}

/// The donut: the share left as an arc over a full track, 2px thick. Low, the arc is the billing amber. Hollow (no
/// arc) when nothing was read.
fn usage_donut(left: Option<i64>, low: bool, palette: Palette) -> impl IntoElement {
    let arc = if low { palette.warning } else { palette.muted };
    gpui_kit::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let radius = 5.5_f32;
            for (fraction, color) in [(1., palette.border), (left.unwrap_or(0) as f32 / 100., arc)] {
                if fraction <= 0. {
                    continue;
                }
                let mut path = gpui_kit::PathBuilder::stroke(px(2.));
                for step in 0..=100 {
                    let angle = -std::f32::consts::FRAC_PI_2 + fraction * std::f32::consts::TAU * step as f32 / 100.;
                    let point =
                        gpui_kit::point(center.x + px(radius * angle.cos()), center.y + px(radius * angle.sin()));
                    if step == 0 {
                        path.move_to(point);
                    } else {
                        path.line_to(point);
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .size(px(14.))
    // A crowded toolbar row would shrink its width and clip the ring's sides.
    .flex_none()
}

/// A one-row note under the where line (a link that found no project, or the Nexus app owning Code sessions), its
/// text on the chips' icon column, with an optional action at the right.
fn home_note(note: SharedString, action: Option<AnyElement>, palette: Palette) -> impl IntoElement {
    h_flex()
        .mt(px(16.))
        .h(px(28.))
        .pl(px(12.))
        .pr(px(4.))
        .gap(px(8.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(13.))
                .line_height(px(18.))
                .text_color(palette.muted)
                .child(note),
        )
        .children(action)
}

/// A chip in the composer's toolbar: 28 tall, the 14 icon and 13 text, `menu-hover` while hovered or open. `icon_only`
/// makes it a 28 square (narrow panes, or a chip with no words).
fn toolbar_chip(id: &'static str, open: bool, icon_only: bool, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .role(Role::Button)
        .flex_none()
        .h(px(28.))
        .gap(px(6.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .line_height(px(18.))
        .text_color(palette.foreground)
        .map(|chip| {
            if icon_only {
                chip.w(px(28.)).justify_center()
            } else {
                chip.px(px(8.))
            }
        })
        .hover(|style| style.bg(palette.menu_hover))
        .when(open, |chip| chip.bg(palette.menu_hover))
}

/// A menu opened from a control in the Home composer's toolbar: 8px under the box (the toolbar sits 10px above its
/// bottom edge), left- or right-aligned to the control.
fn menu_under_box(menu: AnyElement, right: bool) -> impl IntoElement {
    let corner = div().absolute().top(px(28.));
    let corner = if right { corner.right_0() } else { corner.left_0() };
    corner.child(
        deferred(
            anchored()
                .anchor(if right { Anchor::TopRight } else { Anchor::TopLeft })
                .offset(point(px(0.), px(18.)))
                .snap_to_window_with_margin(px(8.))
                .child(menu),
        )
        .with_priority(1),
    )
}

/// An effort's word in the chip and the picker ("XHigh" reads "Extra high", no thinking reads "Off").
fn effort_name(effort: Effort) -> &'static str {
    match effort {
        Effort::Disabled => "Off",
        Effort::Xhigh => "Extra high",
        _ => EFFORTS
            .iter()
            .find(|(level, _)| *level == effort)
            .map_or("Default", |(_, name)| *name),
    }
}

/// Who pays, by name: "Claude plan", "ChatGPT plan", or the organization's budget. Never "Credits" or "key".
/// Subtitle of the Auto row. Before access loads (`None`) it says "checking", like the chip does.
fn auto_row_hint(resolved: Option<Payer>, plans: bool, agent: SessionAgent, org: &str) -> String {
    match resolved.unwrap_or(Payer::Checking) {
        Payer::Checking => "Checking who pays…".into(),
        Payer::None => "Nobody can pay yet".into(),
        payer => {
            let name = payer_name(payer, agent, org).unwrap_or_default();
            if plans {
                format!("Uses {name} now")
            } else {
                format!("{name} pays")
            }
        }
    }
}

fn plan_payer_label(agent: SessionAgent) -> String {
    agent.configured_provider().map_or_else(
        || format!("{} plan", crate::pages::subscriptions::vendor(agent)),
        String::from,
    )
}

/// `None` when nobody is paying: `Payer::None` and `Payer::Checking` must not name a payer that cannot pay.
fn payer_name(payer: Payer, agent: SessionAgent, org: &str) -> Option<String> {
    match payer {
        Payer::None | Payer::Checking => None,
        Payer::Plan => Some(plan_payer_label(agent)),
        _ if org.is_empty() => Some("Team budget".into()),
        _ => Some(format!("{org} budget")),
    }
}

#[cfg(test)]
mod tests {
    fn server_form(models: &[&str]) -> super::CodeForm {
        use super::*;
        let id = nexus_workbench::provider::InstanceId::new("server:test".into());
        CodeForm {
            agent: SessionAgent::Opencode,
            provider: Some(id.clone()),
            providers: vec![LocalProviderChoice {
                id,
                name: "Server".into(),
                agents: vec![SessionAgent::Opencode],
                server_models: Some(models.iter().map(|model| (*model).into()).collect()),
            }],
            ..CodeForm::default()
        }
    }

    #[test]
    fn the_first_provider_choice_is_used_once_and_survives_a_project_pick() {
        use super::*;
        let sample = server_form(&["team/model"]);
        let mut form = CodeForm {
            local_subscription: true,
            ..CodeForm::default()
        };
        form.begin_first_provider(sample.provider);
        assert!(form.provider_loading());
        form.update_provider_choices(sample.providers);
        assert_eq!(form.agent, SessionAgent::Opencode);
        assert_eq!(form.selected_server_model(), Some("team/model"));
        form.apply_defaults(ProjectDefaults {
            agent: Some(SessionAgent::Codex),
            ..Default::default()
        });
        assert_eq!(form.agent, SessionAgent::Opencode);
        assert!(form.finish_first_provider());
        assert!(form.provider.is_none());
        assert_eq!(form.effective_via(), Via::Subscription);
        assert!(!form.finish_first_provider());
    }

    #[test]
    fn a_first_openai_key_does_not_replace_the_subscription_agent() {
        use super::*;
        let id = nexus_workbench::provider::InstanceId::new("key:test".into());
        let mut form = CodeForm {
            local_subscription: true,
            ..CodeForm::default()
        };
        form.begin_first_provider(Some(id.clone()));
        form.update_provider_choices(vec![LocalProviderChoice {
            id,
            name: "OpenAI".into(),
            agents: vec![SessionAgent::Codex],
            server_models: None,
        }]);
        assert_eq!(form.agent, SessionAgent::Codex);
        form.remember(); // The project picker remembers the project.
        assert!(form.finish_first_provider());
        form.apply_defaults(ProjectDefaults::default());
        assert_eq!(form.agent, SessionAgent::Claude);
        form.apply_defaults(ProjectDefaults {
            agent: Some(SessionAgent::Codex),
            ..Default::default()
        });
        assert_eq!(form.agent, SessionAgent::Codex);
        assert_eq!(form.effective_via(), Via::Subscription);
    }

    #[test]
    fn a_new_server_session_needs_a_saved_model_or_a_single_default() {
        let mut empty = server_form(&[]);
        assert!(empty.server_model_blocker().is_some());
        empty.pick_server_model("team/model");
        assert!(empty.model.is_none());

        let mut many = server_form(&["team/model", "team/other"]);
        assert!(many.server_model_blocker().is_some());
        many.pick_server_model("zai/glm-5.3");
        assert!(many.model.is_none());
        assert!(many.server_model_blocker().is_some());
        many.pick_server_model("team/model");
        assert_eq!(many.model.as_deref(), Some("team/model"));
        assert!(many.server_model_blocker().is_none());
        many.providers[0].server_models = Some(vec!["team/other".into()]);
        assert!(many.server_model_blocker().is_some());

        let single = server_form(&["team/model"]);
        assert!(single.model.is_none());
        assert!(single.server_model_blocker().is_none());
    }

    #[test]
    fn auto_any_agent_drops_a_saved_local_provider() {
        // A provider picked signed out stays on the form after sign-in; Auto runs on Gateway, never on that provider.
        let mut form = server_form(&["team/model"]);
        form.pick_auto_any();
        assert!(form.provider.is_none());
        assert_eq!(form.model.as_deref(), Some(super::super::code_picker::AUTO));
    }

    #[test]
    fn a_projects_own_agent_or_model_ends_auto_any_and_none_keeps_it() {
        use super::*;
        let mut form = CodeForm::default();
        form.pick_auto_any();
        form.apply_defaults(ProjectDefaults::default());
        assert!(form.auto_any);
        assert_eq!(form.model.as_deref(), Some(super::super::code_picker::AUTO));
        form.apply_defaults(ProjectDefaults {
            agent: Some(SessionAgent::Codex),
            model: Some("gpt-6.1-sol".into()),
            ..ProjectDefaults::default()
        });
        assert!(!form.auto_any);
        assert_eq!(form.model.as_deref(), Some("gpt-6.1-sol"));
    }

    #[test]
    fn auto_any_agent_runs_the_named_agent_on_auto_and_a_row_pick_ends_it() {
        use super::*;
        let mut form = CodeForm::default();
        form.access = Some(crate::code_run::AgentAccess {
            subscriptions: true,
            answer: None,
        });
        form.via = Some(Via::Nexus);
        form.pick_auto_any();
        assert!(form.auto_any);
        assert_eq!(form.model.as_deref(), Some(super::super::code_picker::AUTO));
        // Plan first, whatever the agent's own saved payer.
        assert_eq!(form.effective_via(), Via::Subscription);
        form.take_auto_agent(SessionAgent::Codex);
        assert_eq!(form.agent, SessionAgent::Codex);
        assert_eq!(form.model.as_deref(), Some(super::super::code_picker::AUTO));
        assert_eq!(form.effective_via(), Via::Subscription);
        // The one-session API choice holds across agents too.
        form.api_once = true;
        assert_eq!(form.effective_via(), Via::Nexus);
        // Plans off: Gateway pays.
        form.api_once = false;
        form.access = Some(crate::code_run::AgentAccess {
            subscriptions: false,
            answer: None,
        });
        assert_eq!(form.effective_via(), Via::Nexus);
        // An explicit payer pick ends Auto across agents.
        form.set_via(Some(Via::Subscription));
        assert!(!form.auto_any);
    }

    #[test]
    fn cursor_starts_in_accept_edits_and_keeps_an_explicit_plan_pick() {
        use super::*;
        let mut form = CodeForm::default();
        assert_eq!(form.mode, PermissionMode::Auto);
        form.take_agent(SessionAgent::Cursor);
        assert_eq!(form.mode, PermissionMode::AcceptEdits);
        assert_eq!(form.effective_via(), Via::Subscription);
        form.mode = PermissionMode::Plan;
        form.remember_mode();
        form.take_agent(SessionAgent::Claude);
        assert_eq!(form.mode, PermissionMode::Auto);
        form.take_agent(SessionAgent::Cursor);
        form.remember_session_pick(SessionAgent::Codex, None, Some(PermissionMode::Default));
        assert_eq!(form.mode, PermissionMode::Plan);
        form.take_agent(SessionAgent::Claude);
        assert_eq!(form.mode, PermissionMode::Default);
        assert_eq!(form.remembered["mode"], PermissionMode::Default.as_str());
        assert_eq!(
            form.remembered["agentModes"][SessionAgent::Cursor.as_str()],
            PermissionMode::Plan.as_str()
        );
    }

    #[test]
    fn auto_row_says_checking_until_access_loads() {
        use super::{Payer, SessionAgent, auto_row_hint};
        assert_eq!(
            auto_row_hint(None, true, SessionAgent::Claude, "Acme"),
            "Checking who pays…"
        );
        assert_eq!(
            auto_row_hint(None, false, SessionAgent::Claude, "Acme"),
            "Checking who pays…"
        );
        assert_eq!(
            auto_row_hint(Some(Payer::Credits), false, SessionAgent::Claude, "Acme"),
            "Acme budget pays"
        );
    }

    #[test]
    fn payer_name_only_names_states_that_can_pay() {
        use super::{Payer, SessionAgent, payer_name};
        assert_eq!(payer_name(Payer::Checking, SessionAgent::Claude, "Acme"), None);
        assert_eq!(payer_name(Payer::None, SessionAgent::Claude, "Acme"), None);
        assert_eq!(
            payer_name(Payer::Plan, SessionAgent::Claude, "Acme").as_deref(),
            Some("Claude plan")
        );
        assert_eq!(
            payer_name(Payer::Plan, SessionAgent::Codex, "Acme").as_deref(),
            Some("ChatGPT plan")
        );
        assert_eq!(
            payer_name(Payer::Key, SessionAgent::Claude, "Acme").as_deref(),
            Some("Acme budget")
        );
        assert_eq!(
            payer_name(Payer::Credits, SessionAgent::Claude, "").as_deref(),
            Some("Team budget")
        );
    }

    use super::*;
    use core::prelude::v1::test;

    fn claude_plan(session: f64, weekly: f64) -> nexus_workbench::plans::PlanUsage {
        use nexus_workbench::plans::{Limit, PlanUsage};
        PlanUsage {
            agent: SessionAgent::Claude,
            account: None,
            email: None,
            plan: Some("Claude Max".into()),
            signed_in: true,
            active: true,
            limits: vec![
                Limit {
                    models: Vec::new(),
                    label: "Session".into(),
                    used_percent: session,
                    resets_at: Some(10_000),
                },
                Limit {
                    models: Vec::new(),
                    label: "Weekly".into(),
                    used_percent: weekly,
                    resets_at: Some(20_000),
                },
            ],
            resets: None,
            error: None,
            google_cloud: false,
            unavailable: false,
            fetched_at: None,
        }
    }

    #[test]
    fn the_donut_shows_the_five_hour_limit_even_when_the_week_is_tighter() {
        let plan = claude_plan(28., 60.);
        let shown = five_hour(Some(&plan), None, 0).expect("a reading");
        assert_eq!((shown.limit.label.as_str(), shown.left), ("Session", 72));
        let mut weekly_only = claude_plan(0., 60.);
        weekly_only.limits.remove(0);
        let shown = five_hour(Some(&weekly_only), None, 0).expect("a reading");
        assert_eq!((shown.limit.label.as_str(), shown.left), ("Weekly", 40));
    }

    #[test]
    fn antigravity_donut_shows_the_session_models_group() {
        use nexus_workbench::allowance::Source;
        use nexus_workbench::plans::{Limit, PlanUsage};
        let group = |label: &str, model: &str, used_percent: f64| Limit {
            models: vec![model.into()],
            label: label.into(),
            used_percent,
            resets_at: Some(10_000),
        };
        let plan = PlanUsage {
            limits: vec![
                group("Weekly · Gemini", "Gemini 3.8 Flash", 4.),
                group("Weekly · Claude and GPT", "Claude Opus 4.6", 50.),
            ],
            ..claude_plan(0., 0.)
        };
        let shown = five_hour(Some(&plan), Some("gemini-3.8-flash-high"), 0).expect("a reading");
        assert_eq!((shown.limit.label.as_str(), shown.left), ("Weekly · Gemini", 96));
        let shown = five_hour(Some(&plan), None, 0).expect("a reading");
        assert_eq!(shown.left, 50);
        let hint = usage_hint(Source::Plan(SessionAgent::Antigravity), PlanRead::Read(Some(&plan)), 0);
        assert!(hint.contains("· Gemini"), "{hint}");
        assert!(hint.contains("· Claude and GPT"), "{hint}");
    }

    #[test]
    fn the_donut_hover_says_why_it_cannot_read_the_plan() {
        use nexus_workbench::allowance::Source;
        let claude = Source::Plan(SessionAgent::Claude);
        assert_eq!(
            usage_hint(claude, PlanRead::Checking, 0),
            "Checking your Claude plan usage…"
        );
        assert_eq!(
            usage_hint(claude, PlanRead::Failed("timed out"), 0),
            "Could not read your Claude plan usage: timed out"
        );
        assert_eq!(
            usage_hint(claude, PlanRead::Read(None), 0),
            "No signed-in Claude plan for this session's account."
        );
        let mut failed = claude_plan(0., 0.);
        failed.error = Some("Sign in again".into());
        assert_eq!(
            usage_hint(claude, PlanRead::Read(Some(&failed)), 0),
            "Could not read your Claude plan usage: Sign in again"
        );
        let plan = claude_plan(28., 60.);
        assert_eq!(
            usage_hint(claude, PlanRead::Read(Some(&plan)), 15_000),
            "A Claude limit just reset. Reading it again…"
        );
        let hint = usage_hint(claude, PlanRead::Read(Some(&plan)), 0);
        assert!(hint.starts_with("5-hour limit: 72% left. Resets in"), "{hint}");
        assert!(hint.contains("\nWeekly limit: 40% left. Resets in"), "{hint}");
    }

    fn project(id: &str) -> crate::code::CodeProject {
        crate::code::CodeProject {
            id: id.into(),
            name: id.into(),
            branch: None,
            pinned: false,
            remotes: Vec::new(),
            sessions: Vec::new(),
            machine: None,
            added_at: 0,
        }
    }

    /// After an organization switch the old projects are gone, so a pick from the old organization is dropped, and
    /// the new organization's first project takes its place once they are read.
    #[test]
    fn a_pick_from_another_organization_gives_way_to_this_ones_first_project() {
        assert_eq!(
            project_pick(Some("a"), &[project("a"), project("b")]).as_deref(),
            Some("a")
        );
        assert_eq!(project_pick(Some("old"), &[]), None);
        assert_eq!(
            project_pick(Some("old"), &[project("x"), project("y")]).as_deref(),
            Some("x")
        );
        assert_eq!(project_pick(None, &[project("x")]).as_deref(), Some("x"));
    }

    /// A form with these remembered picks and no organization, so nothing is written.
    fn form(remembered: Value) -> CodeForm {
        CodeForm {
            remembered,
            ..CodeForm::default()
        }
    }

    /// Gateway's answer: Claude's API runs on the organization's key, Codex's on `codex_gateway`; both plans can run.
    fn access(subscriptions: bool, codex_gateway: Value) -> AgentAccess {
        AgentAccess {
            subscriptions,
            answer: Some(json!({ "agents": [
                { "agent": "claude", "route": "gateway", "from": "default", "routes": [
                    { "route": "gateway", "access": "byok", "available": true },
                    { "route": "subscription", "available": true, "plan": "Claude" }
                ] },
                { "agent": "codex", "route": "gateway", "from": "default", "routes": [
                    codex_gateway,
                    { "route": "subscription", "available": true, "plan": "ChatGPT" }
                ] }
            ] })),
        }
    }

    fn pay(pick: Option<Via>, via: Via, payer: Payer) -> RowPay {
        RowPay { pick, via, payer }
    }

    /// Another agent's row shows that agent's saved payer, which picking the row restores, not the picked agent's.
    #[test]
    fn who_pays_for_a_row_reads_its_own_agents_saved_pick() {
        let byok = json!({ "route": "gateway", "access": "byok", "available": true });
        let mut form = form(json!({ "via": { "claude": "subscription", "codex": "nexus" } }));
        form.access = Some(access(true, byok));
        form.take_agent(SessionAgent::Claude);
        assert_eq!(
            form.row_pay(SessionAgent::Claude, "Acme"),
            pay(Some(Via::Subscription), Via::Subscription, Payer::Plan)
        );
        assert_eq!(
            form.row_pay(SessionAgent::Codex, "Acme"),
            pay(Some(Via::Nexus), Via::Nexus, Payer::Key)
        );
        // The reverse: the picked agent on the API, the other saved on its plan.
        form.via = Some(Via::Nexus);
        form.remembered["via"]["codex"] = json!("subscription");
        assert_eq!(
            form.row_pay(SessionAgent::Codex, "Acme"),
            pay(Some(Via::Subscription), Via::Subscription, Payer::Plan)
        );
        // No saved pick for the row's agent is Auto, even when the picked agent has one.
        form.remembered["via"]["codex"] = Value::Null;
        assert_eq!(
            form.row_pay(SessionAgent::Codex, "Acme"),
            pay(None, Via::Nexus, Payer::Key)
        );
        // What picking the row restores is what the row showed.
        form.take_agent(SessionAgent::Codex);
        assert_eq!(form.via, None);
        form.remembered["via"]["claude"] = json!("subscription");
        assert_eq!(form.via_for(SessionAgent::Claude), Some(Via::Subscription));
    }

    /// Nexus, plans off, Gateway not answered yet, and an API route that cannot run.
    #[test]
    fn who_pays_for_a_row_without_a_plan_or_an_answer() {
        let byok = json!({ "route": "gateway", "access": "byok", "available": true });
        let mut form = form(json!({ "via": { "claude": "subscription", "nexus": "subscription" } }));
        form.access = Some(access(true, byok.clone()));
        form.take_agent(SessionAgent::Claude);
        // Nexus always runs on credits.
        assert_eq!(
            form.row_pay(SessionAgent::Nexus, "Acme"),
            pay(Some(Via::Subscription), Via::Nexus, Payer::Credits)
        );
        // Plans off: the API pays whatever was picked, and its segment is the one marked, as the budget line says.
        form.access = Some(access(false, byok));
        let plans_off = form.row_pay(SessionAgent::Claude, "Acme");
        assert_eq!(plans_off, pay(Some(Via::Subscription), Via::Nexus, Payer::Key));
        assert_eq!(plans_off.marked(), Some(Via::Nexus));
        // Gateway has not answered: checking, and the saved pick stays marked until it does.
        form.access = None;
        let checking = form.row_pay(SessionAgent::Claude, "Acme");
        assert_eq!(checking, pay(Some(Via::Subscription), Via::Nexus, Payer::Checking));
        assert_eq!(checking.marked(), Some(Via::Subscription));
        // Codex's API cannot run: none pays on it, and Auto goes to the plan.
        let unavailable = json!({ "route": "gateway", "access": null, "available": false });
        form.access = Some(access(true, unavailable));
        form.remembered["via"]["codex"] = json!("nexus");
        assert_eq!(
            form.row_pay(SessionAgent::Codex, "Acme"),
            pay(Some(Via::Nexus), Via::Nexus, Payer::None)
        );
        form.remembered["via"]["codex"] = Value::Null;
        assert_eq!(
            form.row_pay(SessionAgent::Codex, "Acme"),
            pay(None, Via::Subscription, Payer::Plan)
        );
    }

    #[test]
    fn an_unset_workspace_uses_a_worktree() {
        assert!(CodeForm::default().worktree);
        for remembered in [Value::Null, json!({}), json!({ "workspace": "unknown" })] {
            let mut form = form(remembered);
            form.apply_defaults(ProjectDefaults::default());
            assert!(form.worktree);
        }
    }

    #[test]
    fn workspace_picks_and_project_defaults_override_the_first_choice() {
        for (pick, worktree) in [("folder", false), ("worktree", true)] {
            let mut form = form(json!({ "workspace": pick }));
            form.apply_defaults(ProjectDefaults::default());
            assert_eq!(form.worktree, worktree);
            form.apply_defaults(ProjectDefaults {
                workspace: Some(if worktree { WorksIn::Folder } else { WorksIn::Worktree }),
                ..ProjectDefaults::default()
            });
            assert_eq!(form.worktree, !worktree);
            form.apply_defaults(ProjectDefaults::default());
            assert_eq!(form.worktree, worktree);
        }
    }

    #[test]
    fn picking_a_plan_switches_agents_and_preserves_each_models_settings() {
        let mut form = form(json!({
            "models": {
                "claude": { "model": "sonnet", "effort": "high" },
                "codex": { "model": "gpt-5", "effort": "low" }
            },
            "via": { "codex": "nexus" }
        }));
        form.access = Some(AgentAccess {
            subscriptions: true,
            answer: None,
        });
        form.take_agent(SessionAgent::Claude);
        form.pick_plan(SessionAgent::Codex);
        assert_eq!(form.agent, SessionAgent::Codex);
        assert_eq!((form.model.as_deref(), form.effort), (Some("gpt-5"), Some(Effort::Low)));
        assert_eq!(form.effective_via(), Via::Subscription);
        assert_eq!(form.remembered["agent"], "codex");

        form.pick_plan(SessionAgent::Claude);
        assert_eq!(
            (form.model.as_deref(), form.effort),
            (Some("sonnet"), Some(Effort::High))
        );
        assert_eq!(form.effective_via(), Via::Subscription);
        // Re-picking the same plan keeps edits made in the model picker.
        form.model = Some("opus".into());
        form.pick_plan(SessionAgent::Claude);
        assert_eq!(form.model.as_deref(), Some("opus"));
        // Organization policy still wins over a pinned plan.
        form.access.as_mut().unwrap().subscriptions = false;
        assert_eq!(form.effective_via(), Via::Nexus);
    }

    #[test]
    fn a_projects_defaults_win_over_the_remembered_picks() {
        let mut form = form(json!({
            "agent": "claude", "mode": "plan", "workspace": "folder",
            "models": { "claude": { "model": "sonnet", "effort": "high" }, "codex": { "model": "gpt-5", "effort": "low" } }
        }));
        form.apply_defaults(ProjectDefaults {
            agent: Some(SessionAgent::Codex),
            model: Some("gpt-5-codex".into()),
            mode: None,
            workspace: Some(WorksIn::Worktree),
        });
        assert_eq!(form.agent, SessionAgent::Codex);
        // The project's model, on the model's own default effort.
        assert_eq!((form.model.as_deref(), form.effort), (Some("gpt-5-codex"), None));
        // Unset in the project: the remembered pick.
        assert_eq!(form.mode, PermissionMode::Plan);
        assert!(form.worktree);
        // Another agent: its remembered pair, the project's model is not its.
        form.take_agent(SessionAgent::Claude);
        assert_eq!(
            (form.model.as_deref(), form.effort),
            (Some("sonnet"), Some(Effort::High))
        );

        // No defaults: back to the remembered picks.
        form.apply_defaults(ProjectDefaults::default());
        assert_eq!((form.agent, form.worktree), (SessionAgent::Claude, false));
    }

    #[test]
    fn nexus_can_restore_its_model_and_be_saved_as_a_favorite() {
        let mut form =
            form(json!({ "agent": "nexus", "models": {"nexus": {"model": "auto"}}, "favorites": ["nexus/auto"] }));
        form.apply_defaults(ProjectDefaults::default());
        assert_eq!(form.agent, SessionAgent::Nexus);
        assert_eq!(form.model.as_deref(), Some("auto"));
        form.via = Some(Via::Subscription);
        assert_eq!(form.effective_via(), Via::Nexus);
        assert_eq!(
            form.favorites(),
            vec![LaunchRow {
                agent: SessionAgent::Nexus,
                model: Some("auto".into())
            }]
        );
        form.remember();
        assert_eq!(form.remembered["agent"], "nexus");
    }

    #[test]
    fn another_project_drops_the_review_and_a_review_is_not_remembered() {
        let mut form = CodeForm::default();
        form.review.on = true;
        form.review.number = Some(42);
        form.remember();
        assert_eq!(form.remembered["workspace"], Value::Null);
        form.apply_defaults(ProjectDefaults::default());
        assert!(!form.review.on);
        assert_eq!(form.review.number, None);
    }

    #[test]
    fn a_projects_defaults_are_not_remembered_as_picks() {
        let mut form = form(json!({ "agent": "claude", "mode": "plan", "workspace": "folder" }));
        form.apply_defaults(ProjectDefaults {
            agent: Some(SessionAgent::Codex),
            model: Some("gpt-5".into()),
            mode: Some(PermissionMode::AcceptEdits),
            workspace: Some(WorksIn::Worktree),
        });
        form.remember();
        assert_eq!(form.remembered["agent"], "claude");
        assert_eq!(form.remembered["mode"], "plan");
        assert_eq!(form.remembered["workspace"], "folder");
        assert!(form.remembered["models"]["codex"].is_null());
        // A pick of its own is remembered.
        form.mode = PermissionMode::Default;
        form.remember();
        assert_eq!(form.remembered["mode"], "default");
    }

    #[test]
    fn open_session_picks_restore_for_new_sessions_without_copying_unpicked_settings() {
        let saved = json!({
            "agent": "codex", "mode": "plan", "workspace": "worktree", "project": "saved-project",
            "models": {
                "claude": { "model": "sonnet", "effort": "high" },
                "codex": { "model": "gpt-5", "effort": "low" }
            },
            "favorites": ["codex/gpt-5"], "via": { "codex": "subscription" }
        });
        let mut choices = form(saved.clone());
        choices.apply_defaults(ProjectDefaults::default());
        choices.project = Some("saved-project".into());
        choices.remember_session_pick(
            SessionAgent::Codex,
            Some((Some("gpt-6.1".into()), Some(Effort::Ultra))),
            Some(PermissionMode::BypassPermissions),
        );
        // A later favorite or payer change must not overwrite the saved pair with stale form values.
        choices.remember();
        let mut reopened = form(choices.remembered);
        reopened.apply_defaults(ProjectDefaults::default());
        assert_eq!(reopened.agent, SessionAgent::Codex);
        assert_eq!(reopened.model.as_deref(), Some("gpt-6.1"));
        assert_eq!(reopened.effort, Some(Effort::Ultra));
        assert_eq!(reopened.mode, PermissionMode::BypassPermissions);
        for field in ["workspace", "project", "favorites", "via"] {
            assert_eq!(reopened.remembered[field], saved[field]);
        }
        reopened.take_agent(SessionAgent::Claude);
        assert_eq!(reopened.model.as_deref(), Some("sonnet"));
        assert_eq!(reopened.effort, Some(Effort::High));
    }

    #[test]
    fn permission_only_picks_preserve_models_and_explicit_defaults_clear_the_pair() {
        let saved = json!({
            "agent": "codex", "mode": "plan",
            "models": { "codex": { "model": "gpt-5", "effort": "ultra" } }
        });
        let mut choices = form(saved.clone());
        choices.remember_session_pick(SessionAgent::Codex, None, Some(PermissionMode::BypassPermissions));
        assert_eq!(choices.remembered["models"], saved["models"]);
        assert_eq!(choices.remembered["agent"], saved["agent"]);
        choices.remember_session_pick(SessionAgent::Codex, Some((None, None)), None);
        choices.apply_defaults(ProjectDefaults::default());
        assert_eq!((choices.model, choices.effort), (None, None));
        assert_eq!(choices.mode, PermissionMode::BypassPermissions);
    }

    #[test]
    fn explicit_effort_on_a_project_model_is_remembered_and_can_be_cleared() {
        let mut choices = form(json!({ "models": { "codex": { "model": "gpt-5", "effort": "low" } } }));
        let defaults = ProjectDefaults {
            agent: Some(SessionAgent::Codex),
            model: Some("gpt-6.1".into()),
            ..ProjectDefaults::default()
        };
        choices.apply_defaults(defaults.clone());
        assert_eq!(choices.effort, None);
        choices.effort = Some(Effort::Ultra);
        choices.remember_model();
        choices.apply_defaults(defaults.clone());
        assert_eq!(choices.effort, Some(Effort::Ultra));
        choices.effort = None;
        choices.remember_model();
        choices.apply_defaults(defaults);
        assert_eq!(choices.effort, None);
    }

    #[test]
    fn explicit_project_mode_pick_is_remembered_but_project_precedence_stays() {
        let mut choices = form(json!({ "mode": "plan" }));
        let defaults = ProjectDefaults {
            mode: Some(PermissionMode::BypassPermissions),
            ..ProjectDefaults::default()
        };
        choices.apply_defaults(defaults.clone());
        choices.remember_mode();
        choices.apply_defaults(ProjectDefaults::default());
        assert_eq!(choices.mode, PermissionMode::BypassPermissions);
        choices.remember_session_pick(SessionAgent::Codex, None, Some(PermissionMode::Default));
        choices.apply_defaults(defaults);
        assert_eq!(choices.mode, PermissionMode::BypassPermissions);
        choices.apply_defaults(ProjectDefaults::default());
        assert_eq!(choices.mode, PermissionMode::Default);
    }
}

#[cfg(test)]
#[path = "model_picker_tests.rs"]
mod model_picker_tests;

#[cfg(test)]
#[path = "project_picker_tests.rs"]
mod project_picker_tests;

#[cfg(test)]
mod donut_tests {
    use super::{Palette, usage_donut};
    // `gpui_kit::*` brings a `test` attribute of its own: the standard one is named, so `#[gpui_kit::test]` expands.
    use core::prelude::v1::test;
    use gpui_kit::prelude::*;
    use gpui_kit::*;

    /// A toolbar row too narrow for its children: the donut, then a wide neighbour.
    struct Crowded;

    impl Render for Crowded {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
            div()
                .flex()
                .w(px(40.))
                .child(usage_donut(Some(93), false, palette))
                .child(div().debug_selector(|| "after-donut".into()).w(px(200.)).h(px(10.)))
        }
    }

    #[gpui_kit::test]
    fn donut_keeps_its_width_in_a_crowded_row(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| Crowded);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let after = cx.debug_bounds("after-donut").expect("the neighbour is drawn");
        assert_eq!(after.left(), px(14.), "the donut shrank below 14px");
    }
}
