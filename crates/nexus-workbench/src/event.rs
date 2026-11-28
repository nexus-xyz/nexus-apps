//! What the webview sees: the twin of this crate's `contract.ts`. Field names are camelCase on the wire, and
//! every event is `{ "kind": "…", … }`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The agent a session runs: a coding agent ([`nexus_agents::Agent`], [`SessionAgent::coding`]) or Nexus's own agent,
/// which only sessions have. Its id ([`SessionAgent::as_str`]) is its serde value, on the wire and in stored rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionAgent {
    Claude,
    Codex,
    /// Nexus's own agent (`nexus __agent`, the nexus CLI): any model the organization allows, through Nexus Gateway.
    Nexus,
    /// Google's Antigravity (`agy acp`, [`crate::acp`]): on the person's Google plan only, with no Gateway route.
    Antigravity,
    #[cfg(feature = "cursor")]
    Cursor,
    #[cfg(feature = "opencode")]
    Opencode,
}

impl SessionAgent {
    /// Every agent, in the order the app lists them.
    pub const ALL: [SessionAgent; 4 + cfg!(feature = "cursor") as usize + cfg!(feature = "opencode") as usize] = [
        SessionAgent::Claude,
        SessionAgent::Codex,
        SessionAgent::Nexus,
        SessionAgent::Antigravity,
        #[cfg(feature = "cursor")]
        SessionAgent::Cursor,
        #[cfg(feature = "opencode")]
        SessionAgent::Opencode,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SessionAgent::Claude => "claude",
            SessionAgent::Codex => "codex",
            SessionAgent::Nexus => "nexus",
            SessionAgent::Antigravity => "antigravity",
            #[cfg(feature = "cursor")]
            SessionAgent::Cursor => "cursor",
            #[cfg(feature = "opencode")]
            SessionAgent::Opencode => "opencode",
        }
    }

    pub fn parse(value: &str) -> Option<SessionAgent> {
        match value {
            "claude" => Some(SessionAgent::Claude),
            "codex" => Some(SessionAgent::Codex),
            "nexus" => Some(SessionAgent::Nexus),
            "antigravity" => Some(SessionAgent::Antigravity),
            #[cfg(feature = "cursor")]
            "cursor" => Some(SessionAgent::Cursor),
            #[cfg(feature = "opencode")]
            "opencode" => Some(SessionAgent::Opencode),
            _ => None,
        }
    }

    /// A policy id (`openai/gpt-6.1-terra`) as the agent names the model (`gpt-6.1-terra`): a coding agent's
    /// `LocalAgentT::provider_model_id`, which the host applies at launch, and the id itself for Nexus's own agent. A
    /// ChatGPT plan rejects the prefixed id.
    pub fn native_model(self, id: &str) -> String {
        crate::harness::harness(self).native_model(id)
    }

    /// Which kind of session agent it is. Every variant states its kind, so a new one does not build until it chooses.
    pub fn kind(self) -> SessionKind {
        match self {
            SessionAgent::Claude => SessionKind::Coding(nexus_agents::Agent::Claude),
            SessionAgent::Codex => SessionKind::Coding(nexus_agents::Agent::Codex),
            SessionAgent::Nexus => SessionKind::Nexus,
            SessionAgent::Antigravity => SessionKind::Coding(nexus_agents::Agent::Antigravity),
            #[cfg(feature = "cursor")]
            SessionAgent::Cursor => SessionKind::Coding(nexus_agents::Agent::Cursor),
            #[cfg(feature = "opencode")]
            SessionAgent::Opencode => SessionKind::Coding(nexus_agents::Agent::Opencode),
        }
    }

    /// Whether Nexus offers this agent for local sessions on this platform.
    pub fn supported_on_current_platform(self) -> bool {
        self.coding().is_none_or(|agent| {
            agent
                .entry()
                .session
                .supports_platform(nexus_agents::Platform::current())
        })
    }

    /// Why Nexus refuses to start this agent locally, if it does.
    pub fn local_launch_refusal(self) -> Option<String> {
        self.coding()
            .and_then(|agent| agent.entry().platform_refusal(nexus_agents::Platform::current()))
    }

    /// Who pays when this agent uses a provider it configured outside Nexus.
    pub fn configured_provider(self) -> Option<&'static str> {
        self.coding()
            .and_then(|agent| agent.entry().billing.configured_provider)
    }

    /// The name of the agent's own permission settings, when it controls what asks.
    pub fn permission_settings(self) -> Option<&'static str> {
        self.coding()
            .and_then(|agent| agent.entry().session.permission_settings)
    }

    /// The coding agent it is, whose facts `nexus-agents` holds. None: Nexus's own agent.
    pub fn coding(self) -> Option<nexus_agents::Agent> {
        match self.kind() {
            SessionKind::Coding(coding) => Some(coding),
            SessionKind::Nexus => None,
        }
    }
}

/// What a session agent is: a coding agent of the catalog, or Nexus's own agent. A session agent that is neither, such
/// as a custom ACP agent, needs a new kind and a [`crate::harness::Harness`] of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Coding(nexus_agents::Agent),
    Nexus,
}

impl TryFrom<nexus_agents::Agent> for SessionAgent {
    type Error = nexus_agents::Agent;

    fn try_from(agent: nexus_agents::Agent) -> Result<Self, Self::Error> {
        match agent {
            nexus_agents::Agent::Claude => Ok(SessionAgent::Claude),
            nexus_agents::Agent::Codex => Ok(SessionAgent::Codex),
            nexus_agents::Agent::Antigravity => Ok(SessionAgent::Antigravity),
            #[cfg(feature = "cursor")]
            nexus_agents::Agent::Cursor => Ok(SessionAgent::Cursor),
            #[cfg(feature = "opencode")]
            nexus_agents::Agent::Opencode => Ok(SessionAgent::Opencode),
            #[cfg(not(feature = "cursor"))]
            nexus_agents::Agent::Cursor => Err(agent),
            #[cfg(not(feature = "opencode"))]
            nexus_agents::Agent::Opencode => Err(agent),
        }
    }
}

/// Kept only for the previous desktop app, which still names [`SessionAgent`] `Agent`. It goes with that app. Every
/// other crate names `SessionAgent`.
pub use SessionAgent as Agent;

/// The tool an agent asks the person a question with: an answer, never a permission.
pub const QUESTION_TOOL: &str = "AskUserQuestion";
/// The tool an agent presents its plan with: the person approves the plan, never a permission.
pub const PLAN_TOOL: &str = "ExitPlanMode";

/// Claude Code's `--permission-mode`. On screen: Ask first, Auto, Auto-accept edits, Plan only, Full access. Each agent
/// maps the mode to its own setting. That setting decides what the agent runs without asking. Who answers an ask that
/// still comes: [`crate::driver::answers_ask`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    Auto,
    AcceptEdits,
    Plan,
    BypassPermissions,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::Default => "default",
            PermissionMode::Auto => "auto",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
            PermissionMode::BypassPermissions => "bypassPermissions",
        }
    }

    pub fn parse(value: &str) -> Option<PermissionMode> {
        match value {
            "default" => Some(PermissionMode::Default),
            "auto" => Some(PermissionMode::Auto),
            "acceptEdits" => Some(PermissionMode::AcceptEdits),
            "plan" => Some(PermissionMode::Plan),
            "bypassPermissions" => Some(PermissionMode::BypassPermissions),
            _ => None,
        }
    }

    /// The mode's risk, lowest first: Plan only, Ask first, Auto-accept edits, Auto, Full access. Auto ranks above
    /// Auto-accept edits: its reviewer model may run a command nobody was asked about, while Auto-accept edits still asks
    /// before every command. A linked session never runs above an ancestor by this order. Modes are stored and sent by name, so the declaration order means nothing; an
    /// explicit, exhaustive rank instead of a derived `Ord` makes a new mode fail to compile until it gets its place.
    pub fn rank(self) -> u8 {
        match self {
            PermissionMode::Plan => 0,
            PermissionMode::Default => 1,
            PermissionMode::AcceptEdits => 2,
            PermissionMode::Auto => 3,
            PermissionMode::BypassPermissions => 4,
        }
    }

    /// Whether the mode runs tools without asking first: Auto and Full access. In these modes the workbench answers
    /// Nexus's own agent's permission asks ([`crate::driver::answers_ask`]).
    pub fn allows_unasked(self) -> bool {
        matches!(self, PermissionMode::Auto | PermissionMode::BypassPermissions)
    }

    /// The lower of two modes by [`PermissionMode::rank`].
    pub fn lower(self, other: PermissionMode) -> PermissionMode {
        if other.rank() < self.rank() { other } else { self }
    }
}

pub use nexus_local_agents::models::Effort;

/// How model calls are paid: through Nexus Gateway, or the person's own Claude subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    Nexus,
    Subscription,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Nexus => "nexus",
            Via::Subscription => "subscription",
        }
    }

    pub fn parse(value: &str) -> Option<Via> {
        match value {
            "nexus" => Some(Via::Nexus),
            "subscription" => Some(Via::Subscription),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadStatus {
    Idle,
    Starting,
    Running,
    Approval,
    Interrupted,
    Error,
}

impl ThreadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ThreadStatus::Idle => "idle",
            ThreadStatus::Starting => "starting",
            ThreadStatus::Running => "running",
            ThreadStatus::Approval => "approval",
            ThreadStatus::Interrupted => "interrupted",
            ThreadStatus::Error => "error",
        }
    }

    pub fn parse(value: &str) -> ThreadStatus {
        match value {
            "starting" => ThreadStatus::Starting,
            "running" => ThreadStatus::Running,
            "approval" => ThreadStatus::Approval,
            "interrupted" => ThreadStatus::Interrupted,
            "error" => ThreadStatus::Error,
            _ => ThreadStatus::Idle,
        }
    }

    /// A turn was under way: the agent was working or waiting on the person.
    pub fn busy(self) -> bool {
        matches!(
            self,
            ThreadStatus::Starting | ThreadStatus::Running | ThreadStatus::Approval
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    /// The repository's remote URLs, `origin` first, read with the branch (`git::remotes`): a prompt link names its
    /// project by one of them. Absent: none, or an older host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remotes: Vec<String>,
    /// Where its pull requests live, read with the remotes (`source_host`): the windows name them by it (merge
    /// requests and `!123` on GitLab). Absent: GitHub.
    #[serde(default, skip_serializing_if = "crate::source_host::Host::is_github")]
    pub host: crate::source_host::Host,
    pub added_at: i64,
    /// The project's saved subsession flag. Absent: off. No start reads it: new sessions follow the computer's switch
    /// (`Config::subsessions`). Kept on the wire for older apps.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub orchestration: bool,
    /// Pinned by the person: first in the sidebar and in every project picker. Absent: not pinned, or an older host.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// Archived by the person: out of the pickers and the sidebar, its sessions, settings and worktrees kept. Absent:
    /// not archived, or an older host.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
}

/// A session's own git worktree: where its agent works, its branch, and what the branch started from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    /// What the branch was made from (`origin/main`): what the diff and the pull request compare against.
    pub base: String,
    /// Taken off the disk: the session can be read but no longer run.
    #[serde(default)]
    pub removed: bool,
    /// The session's latest pull request: opened from the app, or found by the watch on the branch.
    #[serde(default)]
    pub pull_request: Option<String>,
    /// Every pull request the session opened, oldest first: the agent may open several, from more than one branch.
    /// An older session or host has only `pull_request`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pull_requests: Vec<String>,
    /// Someone else's pull request, checked out to review: its worktree is under the reviews folder, and the app
    /// neither pushes it nor opens a pull request from it. None for sessions saved before reviews existed.
    #[serde(default)]
    pub review: Option<Review>,
}

impl Worktree {
    /// Every pull request the session opened, oldest first, with an older session's one `pull_request`.
    pub fn pulls(&self) -> Vec<&str> {
        let mut pulls: Vec<&str> = self.pull_requests.iter().map(String::as_str).collect();
        if let Some(latest) = self.pull_request.as_deref()
            && !pulls.contains(&latest)
        {
            pulls.push(latest);
        }
        pulls
    }

    /// Saves `url` as the session's latest pull request, and in its list once.
    pub fn add_pull(&mut self, url: &str) {
        if self.pull_requests.is_empty()
            && let Some(older) = self.pull_request.take()
            && older != url
        {
            // An older session's one link joins the list first, so it keeps its place.
            self.pull_requests.push(older);
        }
        if !self.pull_requests.iter().any(|known| known == url) {
            self.pull_requests.push(url.to_string());
        }
        self.pull_request = Some(url.to_string());
    }
}

impl ThreadSummary {
    /// When the person last acted on it, as [`ThreadSummary::acted_at`]; its creation when an older host sent none.
    pub fn acted(&self) -> i64 {
        self.acted_at.unwrap_or(self.created_at)
    }

    /// Every pull request the session opened, oldest first: its worktree's, or its own in its project's folder.
    pub fn pulls(&self) -> Vec<&str> {
        match &self.worktree {
            Some(worktree) => worktree.pulls(),
            None => self.pull_requests.iter().map(String::as_str).collect(),
        }
    }

    /// The session's latest pull request: its worktree's, or the last one it opened in its project's folder.
    pub fn latest_pull(&self) -> Option<&str> {
        match &self.worktree {
            Some(worktree) => worktree.pull_request.as_deref(),
            None => self.pull_requests.last().map(String::as_str),
        }
    }

    /// Saves `url` as the session's latest pull request, once: on its worktree, else on the session.
    pub fn add_pull(&mut self, url: &str) {
        match self.worktree.as_mut() {
            Some(worktree) => worktree.add_pull(url),
            None => {
                self.pull_requests.retain(|known| known != url);
                self.pull_requests.push(url.to_string());
            }
        }
    }
}

/// The pull request a review session checked out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub number: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub id: String,
    pub project_id: String,
    pub title: String,
    /// The session's unique name (session_name.rs): a person, the CLI or `start_session` sets it, and nothing makes
    /// one up. Claude Code gets it as `--name`, so its cross-session tools (ListAgents, SendMessage) reach this
    /// session by it. Absent: no name; a session without one writes the same bytes as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub agent: SessionAgent,
    pub model: Option<String>,
    pub permission_mode: PermissionMode,
    /// None: the model's default. Sessions saved before effort existed read as None.
    #[serde(default)]
    pub effort: Option<Effort>,
    /// Claude Code's fast mode (`fastMode`): faster output at a higher price, on the models that have it
    /// (`fast_mode::supports`). False on sessions saved before it existed; a session without it writes the same bytes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fast: bool,
    pub via: Via,
    /// The configured local provider, when this session chose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<nexus_providers::InstanceId>,
    /// The agent's extra account a session on a subscription runs on (`Workbench::add_account`); None: the agent's own
    /// login. Sessions saved before accounts existed read as None.
    #[serde(default)]
    pub account: Option<String>,
    pub status: ThreadStatus,
    pub cost_usd: f64,
    pub nexus_sessions: Vec<String>,
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Sidebar activity: creation, completion, or a request for attention. None from older hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_at: Option<i64>,
    /// When the person last acted on it: made it, or sent it a message. Agent work, a finished turn, an approval, an
    /// error, a rename and usage never change it. The sidebar orders a project's sessions by it. None from
    /// older hosts: read it as `created_at` ([`ThreadSummary::acted`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acted_at: Option<i64>,
    pub archived: bool,
    /// Held at the top of its project in the sidebar. Sessions saved before pins existed read as false, and an
    /// unpinned session writes the same bytes as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// None: the session works in its project's folder.
    #[serde(default)]
    pub worktree: Option<Worktree>,
    /// The pull requests a session in its project's folder opened, oldest first: its agent's `gh pr create` printed
    /// them (created_pulls.rs). A session in a worktree keeps them on its worktree. Absent: none, or an older host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pull_requests: Vec<String>,
    /// Changes the running agent has not answered yet; None when there are none. Never stored: only a live agent has
    /// them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    /// What the running agent accepts; None with no live process. Never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Box<AgentCapabilities>>,
    /// The session whose agent (or whose person, for a crew) started this one; absent for a session the person
    /// started (desktop-linked-sessions.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<String>,
    /// A fork's source: the session it was forked from (`Workbench::fork_thread`), which its "Send back to source"
    /// writes to (`Workbench::send_back`). Absent for a session that is not a fork, and for forks made before it was
    /// kept. The source may since have been deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_thread_id: Option<String>,
    /// A side chat (side_chat.rs): a fork of its session (`source_thread_id`) in the same folder, shown beside that
    /// session and never in the sidebar. A session that is not one writes the same bytes as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub side_chat: bool,
    /// Who started it: `agent` (its parent's agent, `start_session`) or `crew` (a crew the person started).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<String>,
    /// The session that handed its work to this one and was archived (`handoff_session`). Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_from: Option<HandoffFrom>,
    /// The group it was started in, with its siblings: a crew's request id, or the id of the parent's turn whose agent
    /// started it (none when a background sub-agent started it between turns).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// 0 for a session the person started, its parent's depth + 1 for a linked one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub depth: u32,
    /// Its agent may start, message, wait for and stop linked sessions of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub orchestration: bool,
    /// The linked sessions its agent's open wait covers (`wait_sessions`). Never stored: only a running agent waits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waits_on: Vec<String>,
    /// Its linked sessions' results it has not read yet: they go with its next message. Never stored: read from the
    /// children's logs and cursors.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending_results: u32,
    /// Unanswered session-start choices. Live only; a restart never starts pending work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub start_choices: Vec<crate::session_choice::StartChoice>,
    /// Why it may run no automatic turn (`ThreadRecord::halt`): `person`, `parent` or `fault`. Absent: none. Filled
    /// from the row when the summary is read; never stored from this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub halt: Option<String>,
    /// Its tree reached a limit (automatic turns, empty waits or spend) since the person last wrote to its root.
    /// Never stored: the counters live in memory.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crew_paused: bool,
    /// Its tree's pause is the spend cap's. Never stored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crew_tripped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crew_spend_trip: Option<SpendTrip>,
    /// Its turn is over and a background sub-agent (or a turn its agent owes) still holds it. Never stored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_background: bool,
    /// What its agent left running past its turn: background sub-agents and commands, oldest first, at most
    /// [`BACKGROUND_LISTED`]. Filled only while no turn runs. Never stored. An older host omits it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background: Vec<BackgroundWork>,
    /// How many things it left running in all: more than `background` lists past [`BACKGROUND_LISTED`]. Never stored.
    /// 0 from an older host: count `background` then.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub background_total: u32,
    /// It is at the deepest level linked sessions go: its agent gets no tools, so its orchestration can't be turned
    /// on. Never stored: the limit is the app's setting.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub at_depth_limit: bool,
    /// The person's plan refused its last turn at a usage limit (`Workbench::continue_at_resets`). Absent: it did not,
    /// or the person has since sent a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limited: Option<LimitStop>,
    /// Snoozed, or settled once its pull request merged (snooze.rs). Stored; an active session writes nothing.
    #[serde(flatten)]
    pub aside: crate::snooze::Aside,
}

/// The list-price estimate at the call that paused a crew.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendTrip {
    pub cap_usd: f64,
    pub spent_usd: f64,
}

/// The fields of a [`ThreadSummary`] that only the running workbench knows (`fill_live_fields`): the store never
/// holds them. A reader of the store keeps them from the workbench's last push of each session and puts them back on
/// what it reads ([`ThreadSummary::take_live`], [`ThreadSummary::with_live`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveFields {
    pub waits_on: Vec<String>,
    pub pending_results: u32,
    pub start_choices: Vec<crate::session_choice::StartChoice>,
    pub crew_paused: bool,
    pub crew_tripped: bool,
    pub crew_spend_trip: Option<SpendTrip>,
    pub in_background: bool,
    pub background: Vec<BackgroundWork>,
    pub background_total: u32,
    pub at_depth_limit: bool,
}

impl ThreadSummary {
    /// Its never-stored fields, as the workbench filled them.
    pub fn take_live(&self) -> LiveFields {
        LiveFields {
            waits_on: self.waits_on.clone(),
            pending_results: self.pending_results,
            start_choices: self.start_choices.clone(),
            crew_paused: self.crew_paused,
            crew_tripped: self.crew_tripped,
            crew_spend_trip: self.crew_spend_trip,
            in_background: self.in_background,
            background: self.background.clone(),
            background_total: self.background_total,
            at_depth_limit: self.at_depth_limit,
        }
    }

    /// Puts `live` back on a summary read from the store. An unanswered start choice asks the person, as
    /// `fill_live_fields` says.
    pub fn with_live(&mut self, live: &LiveFields) {
        self.waits_on = live.waits_on.clone();
        self.pending_results = live.pending_results;
        self.start_choices = live.start_choices.clone();
        self.crew_paused = live.crew_paused;
        self.crew_tripped = live.crew_tripped;
        self.crew_spend_trip = live.crew_spend_trip;
        self.in_background = live.in_background;
        self.background = live.background.clone();
        self.background_total = live.background_total;
        self.at_depth_limit = live.at_depth_limit;
        if !self.start_choices.is_empty() {
            self.status = ThreadStatus::Approval;
        }
    }
}

/// A turn the person's plan stopped at its usage limit, and the plan's own Continue for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitStop {
    /// When the limit resets, epoch milliseconds; None when the agent did not say.
    pub resets_at: Option<i64>,
    /// The session continues by itself at `resets_at`: the person chose "Continue at reset".
    pub scheduled: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Who wrote a `User` row other than the person: one of the app's linked sessions (desktop-linked-sessions.md, "Who
/// wrote a row"). A row without one is the person's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Sender {
    /// The session `thread_id` (its parent, starting it or messaging it), titled `title` when it wrote.
    Session { thread_id: String, title: String },
}

/// The session that handed its work to this one (`handoff_session`), as it was when it did: the sidebar says "from a
/// Claude Code session" with it, and the timeline links back to it, archived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffFrom {
    pub thread_id: String,
    pub title: String,
    pub agent: SessionAgent,
}

/// A background command's latest output, read live (`Workbench::task_output`). Never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOutput {
    /// Its latest output: at most the last 8 KiB.
    pub output: String,
    /// How much it has written in all.
    pub total_bytes: u64,
    /// The agent or the core left earlier output out.
    pub truncated: bool,
}

/// Safe launch attribution. This never contains credentials or MCP configuration values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAttribution {
    pub gateway: Option<String>,
    pub organization: Option<String>,
    pub team: Option<String>,
    /// Gateway's payer classification for this launch, not the billing owner's identity.
    pub access: Option<String>,
    pub configured_mcp: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAccount {
    pub kind: String,
    pub email: Option<String>,
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpHealth {
    pub name: String,
    pub status: String,
    pub detail: Option<String>,
}

/// One process's observations, distinct from the session's requested settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConnection {
    pub generation: String,
    pub via: Option<Via>,
    pub provider: Option<String>,
    pub native_id: Option<String>,
    pub reported_at: i64,
    pub model: Option<String>,
    pub version: Option<String>,
    pub effort: Option<String>,
    pub permissions: Option<String>,
    pub attribution: SessionAttribution,
    pub selected_account: Option<String>,
    pub account: Option<RuntimeAccount>,
    pub mcp: Option<Vec<McpHealth>>,
    pub mcp_unavailable: Option<String>,
    pub skills: Option<Vec<String>>,
    pub plugins: Option<Vec<String>>,
    /// Why Claude Code's fast mode is not serving (its `fast_mode_disabled_reason`, or `cooldown`); None when it is on
    /// or the agent reports nothing about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_off: Option<String>,
}

/// What the running agent process accepts, by task kind ("agent", "shell"): from its version and the options Nexus
/// sent it (`driver::capabilities`). Live only: never stored. A kind a reader does not know is one it cannot control.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// Observations from this exact live process; the event log retains last-reported observations after exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<Box<SessionConnection>>,
    /// This process's reported headless command names. Empty is authoritative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    /// Manual compaction through the typed adapter, independent of task controls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compact: bool,
    /// The agent's version, as it reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The kinds of task `wb_task_stop` stops.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    /// The kinds of foreground task that can move to the background (`wb_task_background`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background: Vec<String>,
    /// The kinds of task whose output can be read live (`wb_task_output`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<String>,
    /// The kinds of task that take a message of their own. None: no sub-agent of either agent does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message: Vec<String>,
    /// The session's Interrupt ends only the lead's turn, and sub-agents and background commands run on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupt_spares_agents: bool,
}

/// A mode, model or effort change sent to the running agent and not answered yet (`Workbench::update`). The session
/// shows it over the saved value until the agent accepts it (then it is saved), refuses it, or ends. An absent field
/// is not pending; a `null` model or effort is a change back to the default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "pending_value")]
    pub model: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "pending_value")]
    pub effort: Option<Option<Effort>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
}

// Absent means unchanged; an explicit null means a pending reset to the default.
fn pending_value<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// An image sent with a message or returned by a tool: its bytes in base64.
// note: kept inline in the event log (up to 20 MB a message, the webview's limit); move them to files beside the
// database if screenshot-heavy sessions load slowly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub media_type: String,
    pub data: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSuggestion {
    #[serde(rename = "type")]
    pub kind: String,
    pub label: String,
    pub raw: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Decision {
    Allow,
    AllowSession,
    Deny,
    /// The agent stopped before the person answered.
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Error,
}

/// Where a background task (a sub-agent, a shell left running) stands, as the agent last said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

/// Where a project's setup script stands in a new worktree ([`crate::setup`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetupStatus {
    Running,
    /// Exited with 0.
    Done,
    /// Exited with another code, or could not start.
    Failed,
    /// Ran past [`crate::setup::TIMEOUT`] and was stopped.
    TimedOut,
    /// Stopped by the person ([`crate::Workbench::stop_setup`]).
    Stopped,
}

/// How many entries [`ThreadSummary::background`] lists. The summary goes to every window and server client, so a
/// session that started hundreds of commands still sends a short list.
pub const BACKGROUND_LISTED: usize = 20;

/// One thing a session's agent left running past its turn (`ThreadSummary::background`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundWork {
    pub kind: BackgroundKind,
    /// A sub-agent's description, or a command's text. Empty when the agent did not say.
    pub title: String,
    /// When this agent process saw it start (ms since 1970). None when it started before the process knew of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackgroundKind {
    /// A sub-agent.
    Agent,
    /// A command.
    Shell,
}

/// One entry of Claude Code's list of background tasks still running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTask {
    pub task_id: String,
    pub task_type: Option<String>,
    pub description: String,
}

/// More about a task than its first reports said: its place in the agent tree and its progress. Every field is absent
/// when not said, so a task event without any is byte for byte what a core before it wrote. A later report's absent
/// field keeps the earlier value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetail {
    /// The task whose agent started this one; absent: the session's own agent did, or the agent did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    /// 1 for a task the session's own agent started, N + 1 for one a depth-N sub-agent started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// The model the task's agent runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The agent's own name for it (a Codex nickname).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its latest action, in the agent's words ("Running Echo the word inner").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// It runs past the call that started it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backgrounded: Option<bool>,
    /// One run of the task: a task that runs again keeps its id and gets a new run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// An update of a running task: it never starts, resumes or renames it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<bool>,
    /// Why it ended: `worker_restart` (the agent), `process_ended` or `app_closed` (Nexus wrote the report).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl TaskDetail {
    /// None when it says nothing, so the event keeps its old bytes.
    pub fn some(self) -> Option<Box<TaskDetail>> {
        (self != TaskDetail::default()).then(|| Box::new(self))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WbEvent {
    User {
        text: String,
        /// Sent to the agent before the text. Messages saved before images existed read as none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<Image>,
        /// Who wrote it, when not the person (a linked session). Messages saved before linked sessions read as none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Sender>,
    },
    Session {
        nexus_session_id: Option<String>,
        native_id: Option<String>,
        model: Option<String>,
        via: Via,
        resumed: bool,
    },
    /// A deliberate provider boundary. Previous native cursors cannot cross it.
    AgentSwitched {
        from: SessionAgent,
        to: SessionAgent,
        text: String,
    },
    TurnStart {
        turn_id: String,
    },
    Text {
        id: String,
        text: String,
        parent: Option<String>,
    },
    Thinking {
        id: String,
        text: String,
        parent: Option<String>,
    },
    Tool {
        id: String,
        name: String,
        input: Value,
        parent: Option<String>,
    },
    ToolResult {
        id: String,
        output: String,
        /// Inline raster images returned by the tool, separate from its capped text.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<Image>,
        is_error: bool,
        truncated: bool,
        /// The sub-agent whose call this answers: its Agent call's id. Absent: the session's own agent, or not said.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    Approval {
        request_id: String,
        tool_use_id: Option<String>,
        tool: String,
        input: Value,
        description: Option<String>,
        suggestions: Vec<PermissionSuggestion>,
        /// The sub-agent that asks: its Agent call's id, or its task id when its call is not known. Absent: the
        /// session's own agent, or not said.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    ApprovalResolved {
        request_id: String,
        decision: Decision,
        message: Option<String>,
    },
    Usage {
        model: String,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
        /// `cache_write_tokens` split by how long the entries live, as Claude Code reports the call's
        /// `usage.cache_creation` ([`CacheWrites`]). Absent: the agent did not say (Codex, the Nexus agent, a log
        /// written before this change).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_writes: Option<CacheWrites>,
        cost_usd: f64,
        /// The sub-agent that made the call: its Agent call's id. Absent: the session's own agent, or a Codex log
        /// written before the core kept it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    TurnEnd {
        turn_id: String,
        status: TurnStatus,
        cost_usd: Option<f64>,
        duration_ms: u64,
        error: Option<String>,
        /// Where the agent's own conversation stood as the turn ended (Claude Code's last message `uuid`, the Codex
        /// turn's id), for a revert to go back to. None: nothing reached the conversation, or logged before reverts.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    /// An agent's report on one of its tasks (a sub-agent or a command it left running): Claude Code's `task_started`,
    /// `task_updated` and `task_notification`, or a Codex child thread's start and end (codex.rs). It started, its
    /// state changed, or it ended. Later events for the same `task_id` replace what earlier ones said. Only the start
    /// and the end carry `tool_use_id`, so `task_id` is what links them.
    Task {
        task_id: String,
        /// The tool call that started the task (the sub-agent's Agent call, the shell's Bash call), when said.
        tool_use_id: Option<String>,
        /// `local_bash`, `local_agent` and so on, when the agent says.
        task_type: Option<String>,
        /// A sub-agent's type ("general-purpose", "Explore"), on its start.
        agent_type: Option<String>,
        status: TaskStatus,
        /// On start, what the task does; on its end, the agent's summary of how it went; empty on an update.
        description: String,
        /// On a sub-agent's end: its tool calls, tokens and time. `total_tokens` is the size of its latest model call
        /// (input, cache and output), not the tokens it spent.
        tool_count: Option<u64>,
        total_tokens: Option<u64>,
        duration_ms: Option<u64>,
        /// Its place in the agent tree and its progress, when the agent says.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<Box<TaskDetail>>,
    },
    /// Claude Code's full list of the background tasks still running (`background_tasks_changed`). A task that leaves
    /// the list has ended, even before its `task` report says how.
    BackgroundTasks {
        tasks: Vec<BackgroundTask>,
    },
    /// A sanitized observation from one process, retained as historical evidence after that process ends.
    Connection {
        connection: Box<SessionConnection>,
    },
    /// The slash commands the agent offers, as it reported them on start (without the leading slash).
    Commands {
        commands: Vec<String>,
    },
    /// The project's setup script in the session's new worktree, run before the agent's first turn. Events with the
    /// same `id` follow it: each adds the `output` since the one before, and the last says how it ended.
    Setup {
        id: String,
        command: String,
        status: SetupStatus,
        output: String,
        exit_code: Option<i32>,
        duration_ms: Option<u64>,
    },
    /// The person took back a message queued behind the turn under way (`seq`: its `User` event): the agent never
    /// gets it. Also logged for a discovered command whose process stopped before it could run (commands.rs).
    QueueCancel {
        seq: i64,
    },
    /// The person rewrote a queued message before the agent got it: the agent gets `text` (and the message's images).
    QueueEdit {
        seq: i64,
        text: String,
    },
    /// The person moved a queued message (`seq`: its `User` event) in the queue: it now goes just before the queued
    /// message `before`, or last when `before` is None. The agent gets the queue in its new order.
    QueueMove {
        seq: i64,
        #[serde(default)]
        before: Option<i64>,
    },
    /// The person sent a queued message into the turn under way rather than after it ("Send now").
    QueueSteer {
        seq: i64,
    },
    /// The agent read the person's message `seq` (its `User` event): it said so when it took the message into its
    /// conversation (`Out::Taken`). A message without one was not confirmed: an older log, or one the agent never got.
    Read {
        seq: i64,
    },
    /// The session's files were put back as they were when the turn ending at event `turn` ended
    /// (`Workbench::revert_to_turn`); `files` changed. The turns between that one and this event are reverted: the agent
    /// no longer has them in its conversation, and they can't be reverted to.
    Revert {
        turn: i64,
        files: u32,
    },
    /// The conversation starts again before this user message. Files remain unchanged.
    Rewind {
        before: i64,
        /// An exact native rewind point. None starts a fresh conversation from the retained transcript.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    Notice {
        text: String,
    },
    Error {
        text: String,
        detail: Option<String>,
    },
}

impl WbEvent {
    /// Bookkeeping the agent reports on its own: it never starts a turn.
    /// The sub-agent whose work this is: its Agent call's id (an approval's may be its task id). None: the session's
    /// own agent's, or not said.
    pub fn parent(&self) -> Option<&str> {
        match self {
            WbEvent::Text { parent, .. }
            | WbEvent::Thinking { parent, .. }
            | WbEvent::Tool { parent, .. }
            | WbEvent::ToolResult { parent, .. }
            | WbEvent::Approval { parent, .. } => parent.as_deref(),
            _ => None,
        }
    }

    pub fn is_report(&self) -> bool {
        matches!(
            self,
            WbEvent::Task { .. }
                | WbEvent::BackgroundTasks { .. }
                | WbEvent::Commands { .. }
                | WbEvent::Connection { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredEvent {
    pub thread_id: String,
    /// From 1 per session, no gaps.
    pub seq: i64,
    pub at: i64,
    pub event: WbEvent,
}

/// Streamed text of an assistant message so far (`wb:delta`): shown, never stored, and replaced by the `Text` event
/// with the same id. Kept off the numbered log so a view's last `seq` never moves backwards.
///
/// `offset` None: `text` is the whole text so far. Some(n): `text` follows the first n UTF-16 units (a JavaScript
/// string's length) of what was sent before, so a long answer is not sent again every 50 ms. The whole text is sent
/// first, and again after the screen reads the session's log (`Workbench::events`), which is how a screen that missed
/// a part asks for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delta {
    pub thread_id: String,
    pub id: String,
    pub text: String,
    /// The sub-agent's tool call this text belongs to; None: the main answer.
    #[serde(default)]
    pub parent: Option<String>,
    /// Left out when None, so a whole text reads as it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

/// One model call's token counts, as Claude Code reports them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// `cache_write_tokens` by lifetime; None when the agent does not say.
    pub cache_writes: Option<CacheWrites>,
}

/// The tokens one call wrote to the prompt cache, by the lifetime of the entries: Anthropic's
/// `usage.cache_creation.ephemeral_5m_input_tokens` and `ephemeral_1h_input_tokens`, which add up to
/// `cache_creation_input_tokens` (https://docs.claude.com/en/docs/build-with-claude/prompt-caching).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWrites {
    #[serde(default)]
    pub five_minute_tokens: u64,
    #[serde(default)]
    pub one_hour_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stored_summary() -> ThreadSummary {
        serde_json::from_value(json!({
            "id": "s", "projectId": "p", "title": "t", "agent": "claude", "model": null, "permissionMode": "default",
            "via": "nexus", "status": "idle", "costUsd": 0, "nexusSessions": [], "contextTokens": null,
            "contextWindow": null, "createdAt": 1, "updatedAt": 1, "archived": false,
        }))
        .expect("a summary")
    }

    /// A session agent's serde value, its id and its parse are one string, in `ALL`'s order. The webview, stored rows
    /// and session sync read these values.
    #[test]
    fn session_agent_serde_value_is_the_id_parse_reads() {
        for agent in SessionAgent::ALL {
            let id = agent.as_str();
            assert_eq!(serde_json::to_value(agent).ok(), Some(json!(id)));
            assert_eq!(serde_json::from_value::<SessionAgent>(json!(id)).ok(), Some(agent));
            assert_eq!(SessionAgent::parse(id), Some(agent));
        }
        assert_eq!(
            SessionAgent::ALL.map(SessionAgent::as_str),
            [
                "claude",
                "codex",
                "nexus",
                "antigravity",
                #[cfg(feature = "cursor")]
                "cursor",
                #[cfg(feature = "opencode")]
                "opencode",
            ]
        );
    }

    /// Each variant's place in [`SessionAgent::ALL`]. The match lists every variant, so a new one does not build here
    /// until it has a place, and `PLACES` counts the arms.
    fn place(agent: SessionAgent) -> usize {
        match agent {
            SessionAgent::Claude => 0,
            SessionAgent::Codex => 1,
            SessionAgent::Nexus => 2,
            SessionAgent::Antigravity => 3,
            #[cfg(feature = "cursor")]
            SessionAgent::Cursor => 4,
            #[cfg(feature = "opencode")]
            SessionAgent::Opencode => 4 + cfg!(feature = "cursor") as usize,
        }
    }
    const PLACES: usize = 4 + cfg!(feature = "cursor") as usize + cfg!(feature = "opencode") as usize;

    /// `ALL` is written by hand: every variant is in it once, at its place.
    #[test]
    fn session_agent_all_lists_every_variant() {
        assert_eq!(SessionAgent::ALL.len(), PLACES);
        for (index, agent) in SessionAgent::ALL.into_iter().enumerate() {
            assert_eq!(place(agent), index);
            assert_eq!(SessionAgent::ALL[place(agent)], agent);
        }
    }

    /// Every coding agent offered in a local session uses the same id as the catalog.
    #[test]
    fn coding_agents_round_trip_through_session_agents() {
        use nexus_agents::Agent::{Cursor, Opencode};

        #[cfg(not(feature = "cursor"))]
        {
            assert_eq!(SessionAgent::try_from(Cursor), Err(Cursor));
            assert_eq!(SessionAgent::parse(Cursor.as_str()), None);
        }
        #[cfg(not(feature = "opencode"))]
        {
            assert_eq!(SessionAgent::try_from(Opencode), Err(Opencode));
            assert_eq!(SessionAgent::parse(Opencode.as_str()), None);
        }
        for coding in nexus_agents::Agent::ALL
            .into_iter()
            .filter(|&agent| cfg!(feature = "cursor") || agent != Cursor)
            .filter(|&agent| cfg!(feature = "opencode") || agent != Opencode)
        {
            let agent = SessionAgent::try_from(coding).expect("local session agent");
            assert_eq!(agent.coding(), Some(coding));
            assert_eq!(agent.as_str(), coding.as_str());
        }
        for agent in SessionAgent::ALL {
            if let Some(coding) = agent.coding() {
                assert_eq!(SessionAgent::try_from(coding), Ok(agent));
            }
        }
        assert_eq!(SessionAgent::Nexus.coding(), None);
    }

    /// Each session agent's kind: the catalog's coding agents, and Nexus's own agent.
    #[test]
    fn every_session_agent_states_its_kind() {
        let expected = |agent| match agent {
            SessionAgent::Claude => SessionKind::Coding(nexus_agents::Agent::Claude),
            SessionAgent::Codex => SessionKind::Coding(nexus_agents::Agent::Codex),
            SessionAgent::Nexus => SessionKind::Nexus,
            SessionAgent::Antigravity => SessionKind::Coding(nexus_agents::Agent::Antigravity),
            #[cfg(feature = "cursor")]
            SessionAgent::Cursor => SessionKind::Coding(nexus_agents::Agent::Cursor),
            #[cfg(feature = "opencode")]
            SessionAgent::Opencode => SessionKind::Coding(nexus_agents::Agent::Opencode),
        };
        for agent in SessionAgent::ALL {
            assert_eq!(agent.kind(), expected(agent), "{agent:?}");
        }
    }

    #[test]
    fn live_fields_go_back_on_a_stored_summary_whole() {
        let mut pushed = stored_summary();
        pushed.waits_on = vec!["child".into()];
        pushed.pending_results = 2;
        pushed.crew_paused = true;
        pushed.crew_tripped = true;
        pushed.crew_spend_trip = Some(SpendTrip {
            cap_usd: 20.0,
            spent_usd: 20.4,
        });
        pushed.in_background = true;
        pushed.background = vec![BackgroundWork {
            kind: BackgroundKind::Agent,
            title: "Explore the repo".into(),
            started_at: Some(5),
        }];
        pushed.background_total = 31;
        pushed.at_depth_limit = true;
        let live = pushed.take_live();
        assert_ne!(live, LiveFields::default());
        let mut read = stored_summary();
        read.with_live(&live);
        assert_eq!(read, pushed);
        // Empty live fields clear what an earlier push put back.
        read.with_live(&LiveFields::default());
        assert_eq!(read, stored_summary());
    }

    #[test]
    fn an_unanswered_start_choice_asks_the_person() {
        let live = LiveFields {
            start_choices: vec![crate::session_choice::StartChoice {
                id: "c".into(),
                title: "Fix CI".into(),
                prompt: "Fix CI".into(),
                agent: SessionAgent::Claude,
                via: None,
                model: None,
                permission_mode: PermissionMode::Default,
                workspace: "here".into(),
                can_remember: false,
            }],
            ..LiveFields::default()
        };
        let mut read = stored_summary();
        read.with_live(&live);
        assert_eq!(read.status, ThreadStatus::Approval);
        assert_eq!(read.take_live(), live);
    }

    #[test]
    fn older_tool_results_default_to_no_images() {
        let old = json!({"kind":"toolResult", "id":"shot", "output":"ok", "isError":false, "truncated":false});
        let event = serde_json::from_value::<WbEvent>(old.clone()).expect("old event");
        assert!(matches!(&event, WbEvent::ToolResult { images, .. } if images.is_empty()));
        assert_eq!(serde_json::to_value(&event).expect("serialized event"), old);
    }

    #[test]
    fn events_match_the_typescript_contract() {
        let event = WbEvent::ToolResult {
            images: vec![],
            id: "t1".into(),
            output: "ok".into(),
            is_error: false,
            truncated: false,
            parent: None,
        };
        assert_eq!(
            serde_json::to_value(&event).ok(),
            Some(json!({"kind": "toolResult", "id": "t1", "output": "ok", "isError": false, "truncated": false}))
        );
        let event = WbEvent::ApprovalResolved {
            request_id: "r".into(),
            decision: Decision::AllowSession,
            message: None,
        };
        assert_eq!(
            serde_json::to_value(&event).ok(),
            Some(json!({"kind": "approvalResolved", "requestId": "r", "decision": "allowSession", "message": null}))
        );
        let event = WbEvent::Task {
            task_id: "b1".into(),
            tool_use_id: Some("t1".into()),
            task_type: None,
            agent_type: None,
            status: TaskStatus::Stopped,
            description: "d".into(),
            tool_count: Some(2),
            total_tokens: None,
            duration_ms: None,
            detail: None,
        };
        assert_eq!(
            serde_json::to_value(&event).ok(),
            Some(
                json!({"kind": "task", "taskId": "b1", "toolUseId": "t1", "taskType": null, "agentType": null,
                "status": "stopped", "description": "d", "toolCount": 2, "totalTokens": null, "durationMs": null})
            )
        );
        let event = WbEvent::BackgroundTasks {
            tasks: vec![BackgroundTask {
                task_id: "b1".into(),
                task_type: Some("local_bash".into()),
                description: "sleep".into(),
            }],
        };
        assert_eq!(
            serde_json::to_value(&event).ok(),
            Some(
                json!({"kind": "backgroundTasks", "tasks": [{"taskId": "b1", "taskType": "local_bash", "description": "sleep"}]})
            )
        );
        assert_eq!(
            serde_json::to_value(PermissionMode::BypassPermissions).ok(),
            Some(json!("bypassPermissions"))
        );
        assert_eq!(serde_json::to_value(Effort::Xhigh).ok(), Some(json!("xhigh")));
    }

    #[test]
    fn agent_tree_fields_match_the_typescript_contract() {
        let event = WbEvent::ToolResult {
            images: vec![],
            id: "t2".into(),
            output: "ok".into(),
            is_error: false,
            truncated: false,
            parent: Some("t1".into()),
        };
        assert_eq!(
            serde_json::to_value(&event).ok(),
            Some(
                json!({"kind": "toolResult", "id": "t2", "output": "ok", "isError": false, "truncated": false, "parent": "t1"})
            )
        );
        let event = WbEvent::Approval {
            request_id: "r".into(),
            tool_use_id: None,
            tool: "Bash".into(),
            input: json!({}),
            description: None,
            suggestions: vec![],
            parent: Some("agent-a1".into()),
        };
        assert_eq!(
            serde_json::to_value(&event).ok(),
            Some(
                json!({"kind": "approval", "requestId": "r", "toolUseId": null, "tool": "Bash", "input": {},
                "description": null, "suggestions": [], "parent": "agent-a1"})
            )
        );
        let event = WbEvent::Usage {
            model: "m".into(),
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cache_writes: None,
            cost_usd: 0.5,
            parent: Some("t1".into()),
        };
        assert_eq!(
            serde_json::to_value(&event).ok(),
            Some(
                json!({"kind": "usage", "model": "m", "inputTokens": 1, "outputTokens": 2, "cacheReadTokens": 0,
                "cacheWriteTokens": 0, "costUsd": 0.5, "parent": "t1"})
            )
        );
        let event = WbEvent::Task {
            task_id: "a2".into(),
            tool_use_id: None,
            task_type: Some("local_agent".into()),
            agent_type: None,
            status: TaskStatus::Running,
            description: String::new(),
            tool_count: None,
            total_tokens: None,
            duration_ms: None,
            detail: TaskDetail {
                parent_task_id: Some("a1".into()),
                depth: Some(2),
                model: Some("m".into()),
                name: Some("Ada".into()),
                activity: Some("Reading x".into()),
                backgrounded: Some(true),
                run_id: Some("run1".into()),
                progress: Some(true),
                reason: Some("worker_restart".into()),
            }
            .some(),
        };
        assert_eq!(
            serde_json::to_value(&event).ok(),
            Some(
                json!({"kind": "task", "taskId": "a2", "toolUseId": null, "taskType": "local_agent",
                "agentType": null, "status": "running", "description": "", "toolCount": null, "totalTokens": null,
                "durationMs": null, "detail": {"parentTaskId": "a1", "depth": 2, "model": "m", "name": "Ada",
                "activity": "Reading x", "backgrounded": true, "runId": "run1", "progress": true,
                "reason": "worker_restart"}})
            )
        );
        // A detail that says nothing is no detail; one field keeps only that field.
        assert_eq!(TaskDetail::default().some(), None);
        let detail = TaskDetail {
            depth: Some(1),
            ..TaskDetail::default()
        };
        assert_eq!(serde_json::to_value(detail.some()).ok(), Some(json!({"depth": 1})));
    }

    #[test]
    fn reads_and_writes_events_logged_before_the_agent_tree_unchanged() {
        for old in [
            r#"{"kind":"toolResult","id":"t1","output":"ok","isError":false,"truncated":false}"#,
            r#"{"kind":"approval","requestId":"r","toolUseId":"t1","tool":"Bash","input":{"command":"ls"},"description":null,"suggestions":[]}"#,
            r#"{"kind":"usage","model":"m","inputTokens":1,"outputTokens":2,"cacheReadTokens":3,"cacheWriteTokens":4,"costUsd":0.25}"#,
            r#"{"kind":"task","taskId":"b1","toolUseId":"t1","taskType":"local_agent","agentType":"Explore","status":"completed","description":"d","toolCount":2,"totalTokens":10,"durationMs":5}"#,
        ] {
            let event: WbEvent = serde_json::from_str(old).expect("an old event still reads");
            assert_eq!(serde_json::to_string(&event).ok().as_deref(), Some(old));
        }
    }

    #[test]
    fn parses_effort_levels() {
        for effort in [
            Effort::Disabled,
            Effort::Minimal,
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
            Effort::Ultra,
        ] {
            assert_eq!(Effort::parse(effort.as_str()), Some(effort));
        }
        assert_eq!(Effort::parse("High"), None);
        assert_eq!(Effort::parse("unknown"), None);
    }

    #[test]
    fn reads_a_summary_saved_before_effort() {
        let summary: ThreadSummary = serde_json::from_value(json!({
            "id": "t", "projectId": "p", "title": "x", "agent": "claude", "model": null,
            "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
            "contextTokens": null, "contextWindow": null, "createdAt": 1, "updatedAt": 1, "archived": false
        }))
        .expect("an old summary still reads");
        assert_eq!(summary.effort, None);
        assert_eq!(summary.pending, None);
    }

    /// Linked sessions add only optional fields: an older reader's JSON still reads, and a session the person started
    /// writes the same bytes as before (desktop-linked-sessions.md, "Data").
    #[test]
    fn linked_fields_are_optional_both_ways() {
        let old = json!({
            "id": "t", "projectId": "p", "title": "x", "agent": "claude", "model": null,
            "permissionMode": "default", "effort": null, "via": "nexus", "account": null, "status": "idle",
            "costUsd": 0.0, "nexusSessions": [], "contextTokens": null, "contextWindow": null, "createdAt": 1,
            "updatedAt": 1, "archived": false, "worktree": null
        });
        let mut summary: ThreadSummary = serde_json::from_value(old.clone()).expect("an old summary still reads");
        assert_eq!(summary.parent_thread_id, None);
        assert_eq!((summary.depth, summary.orchestration), (0, false));
        assert_eq!(serde_json::to_value(&summary).ok(), Some(old));
        summary.parent_thread_id = Some("t_lead".into());
        summary.spawned_by = Some("agent".into());
        summary.group_id = Some("g1".into());
        summary.depth = 1;
        summary.orchestration = true;
        let linked = serde_json::to_value(&summary).unwrap_or_default();
        assert_eq!(
            (
                &linked["parentThreadId"],
                &linked["spawnedBy"],
                &linked["groupId"],
                &linked["depth"],
                &linked["orchestration"]
            ),
            (&json!("t_lead"), &json!("agent"), &json!("g1"), &json!(1), &json!(true))
        );
        // The live fields go out only when they say something, and an older reader's JSON reads without them.
        assert_eq!((linked.get("waitsOn"), linked.get("pendingResults")), (None, None));
        summary.waits_on = vec!["t_a".into()];
        summary.pending_results = 2;
        let live = serde_json::to_value(&summary).unwrap_or_default();
        assert_eq!(
            (&live["waitsOn"], &live["pendingResults"]),
            (&json!(["t_a"]), &json!(2))
        );
        let back: ThreadSummary = serde_json::from_value(live).expect("the live fields read back");
        assert_eq!((back.waits_on, back.pending_results), (vec!["t_a".to_string()], 2));
        // The halt and the crew's pause go out only when set, under the names contract.ts reads.
        assert_eq!(
            (
                linked.get("halt"),
                linked.get("crewPaused"),
                linked.get("crewTripped"),
                linked.get("inBackground")
            ),
            (None, None, None, None)
        );
        summary.halt = Some("person".into());
        summary.crew_paused = true;
        summary.crew_tripped = true;
        summary.crew_spend_trip = Some(SpendTrip {
            cap_usd: 20.0,
            spent_usd: 20.4,
        });
        summary.in_background = true;
        let halted = serde_json::to_value(&summary).unwrap_or_default();
        assert_eq!(halted["crewSpendTrip"], json!({"capUsd": 20.0, "spentUsd": 20.4}));
        assert_eq!(
            (
                &halted["halt"],
                &halted["crewPaused"],
                &halted["crewTripped"],
                &halted["inBackground"]
            ),
            (&json!("person"), &json!(true), &json!(true), &json!(true))
        );
        assert_eq!(linked.get("background"), None);
        summary.background = vec![BackgroundWork {
            kind: BackgroundKind::Shell,
            title: "npm run dev".into(),
            started_at: Some(5),
        }];
        let busy = serde_json::to_value(&summary).unwrap_or_default();
        assert_eq!(
            busy["background"],
            json!([{"kind": "shell", "title": "npm run dev", "startedAt": 5}])
        );
        let back: ThreadSummary = serde_json::from_value(busy).expect("the background list reads back");
        assert_eq!(back.background, summary.background);
        assert_eq!(linked.get("atDepthLimit"), None);
        summary.at_depth_limit = true;
        let deepest = serde_json::to_value(&summary).unwrap_or_default();
        assert_eq!(deepest["atDepthLimit"], json!(true));

        // A message logged before `from` is the person's, and still writes its old bytes.
        let old = r#"{"kind":"user","text":"hi"}"#;
        let event: WbEvent = serde_json::from_str(old).expect("an old message still reads");
        assert!(matches!(&event, WbEvent::User { from: None, .. }));
        assert_eq!(serde_json::to_string(&event).ok().as_deref(), Some(old));
        let event = WbEvent::User {
            text: "Build it".into(),
            images: vec![],
            from: Some(Sender::Session {
                thread_id: "t_lead".into(),
                title: "Lead".into(),
            }),
        };
        assert_eq!(
            serde_json::to_value(&event).ok(),
            Some(
                json!({"kind": "user", "text": "Build it", "from": {"kind": "session", "threadId": "t_lead", "title": "Lead"}})
            )
        );

        let project: Project = serde_json::from_value(json!({
            "id": "p", "name": "repo", "path": "/r", "branch": null, "addedAt": 1
        }))
        .expect("an old project still reads");
        assert!(!project.orchestration);
        assert_eq!(
            serde_json::to_value(&project)
                .ok()
                .and_then(|v| v.get("orchestration").cloned()),
            None
        );
    }

    #[test]
    fn a_pending_change_names_only_what_waits() {
        let pending = Pending {
            permission_mode: Some(PermissionMode::Plan),
            model: Some(None),
            effort: None,
            fast: None,
        };
        // `model: null` is a change back to the default; the effort is not pending, so it is absent.
        assert_eq!(
            serde_json::to_value(&pending).ok(),
            Some(json!({"permissionMode": "plan", "model": null}))
        );
    }
}
