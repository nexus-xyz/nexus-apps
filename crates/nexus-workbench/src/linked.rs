//! Linked sessions (subsessions): a Code session whose agent starts other sessions, of any agent this build drives,
//! each in a worktree of its own. This is the core without HTTP: the limits, the typed refusals the tools
//! answer with, [`Workbench::start_child`], and what a linked session inherits at every launch; results,
//! waits, messages to a child and the tree's pause ([`results`]); halts and the cascades ([`halts`]) and the
//! spend cap ([`spend`]). The tests in tests/linked.rs,
//! tests/linked_results.rs and tests/linked_halts.rs cover each scenario.

pub mod cross_project;
pub mod halts;
pub mod handoff;
pub mod messages;
pub mod results;
pub mod spend;

use std::collections::{HashMap, HashSet};
use std::sync::MutexGuard;

use serde::{Deserialize, Serialize};

use crate::event::{
    Effort, PermissionMode, Project, Sender, SessionAgent, StoredEvent, ThreadStatus, ThreadSummary, TurnStatus, Via,
    WbEvent,
};
use crate::harness::{effort_under, harness};
use crate::store::ThreadRecord;
use crate::{NewWorktree, ThreadPatch, UpdateActivity, Workbench, git, now_ms, short_id, title_of};
pub use halts::Halt;

/// `ThreadSummary::spawned_by` for a session its parent's agent started (`start_session`).
pub const SPAWNED_BY_AGENT: &str = "agent";
/// `ThreadSummary::spawned_by` for a session the person started in a crew. The desktop apps no longer start crews
/// (design decision D4), but sessions saved before that still carry it.
pub const SPAWNED_BY_CREW: &str = "crew";

/// Why a linked session with no worktree does not run: it would otherwise run in the project's own folder.
pub(crate) const NO_WORKTREE: &str =
    "This linked session has no worktree of its own, so it can't run. Start a new one from its parent.";

/// Why a paused tree refuses: the tools' `paused` message.
pub(crate) const PAUSED: &str = "This tree of subsessions reached the person's limit on automatic turns, empty waits or \
                                 spend. It goes on after the person's next message.";

/// The most a prompt, a title and a retry key may hold, in characters.
const MAX_PROMPT: usize = 32_000;
const MAX_TITLE: usize = 80;
pub(crate) const MAX_KEY: usize = 64;

/// How far up a chain of parents is followed ([`Workbench::ancestors`]). It is also the deepest a subsession may be,
/// whatever the limits say: below it the walk would miss an ancestor, and with it that ancestor's mode, effort and
/// halt.
const MAX_CHAIN: usize = 16;

/// The deepest level a subsession may be at when no depth limit is set ([`MAX_CHAIN`]).
pub const MAX_DEPTH: u32 = MAX_CHAIN as u32;

/// The limits on subsessions. Each one is optional: `None` is no limit, and `Some(n)` refuses or pauses at `n`, with
/// no ceiling. [`Limits::default`] sets none, so Nexus enforces nothing until the person turns limits on in Settings
/// and sets one. The counters behind them count either way. Depth keeps one bound of its own: [`MAX_DEPTH`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Limits {
    /// The deepest level a subsession may be at (the root is 0), at most [`MAX_DEPTH`]. The deepest level gets no
    /// tools.
    pub max_depth: Option<u32>,
    /// Active children per parent: starting, in a turn, waiting on the person, or with a background sub-agent.
    pub max_live_children: Option<u32>,
    /// Active subsessions in the whole app or host, in every organization.
    pub max_live_linked: Option<u32>,
    /// Child worktrees per parent that are still on the disk.
    pub max_child_worktrees: Option<u32>,
    /// Children started in a tree since the person last wrote to its root.
    pub max_starts: Option<u32>,
    /// Automatic turns in a tree (result messages and `message_session`) since the person last wrote to its root.
    pub max_auto_turns: Option<u32>,
    /// Waits that brought no new result, per parent process, since the person last wrote to its root.
    pub max_idle_waits: Option<u32>,
    /// What a tree may spend at list price, on both routes, since the person last wrote to its root, in US dollars.
    pub crew_spend_usd: Option<f64>,
}

impl Limits {
    /// The same limits with a spend cap that is a number: one that is not is no cap, and one below zero is zero.
    pub fn sanitized(self) -> Limits {
        Limits {
            crew_spend_usd: self.crew_spend_usd.filter(|cap| !cap.is_nan()).map(|cap| cap.max(0.0)),
            ..self
        }
    }

    /// The deepest level a subsession may be at: the depth limit, else [`MAX_DEPTH`], and never past it.
    pub fn depth(&self) -> u32 {
        self.max_depth.map_or(MAX_DEPTH, |depth| depth.min(MAX_DEPTH))
    }
}

/// Whether `count` reached `limit`; never with no limit.
pub(crate) fn reached(limit: Option<u32>, count: usize) -> bool {
    limit.is_some_and(|limit| count >= limit as usize)
}

/// This computer's subsession settings, as the desktop app's Settings keep them: whether new sessions get subsessions,
/// and the limits on them. A host that reads no such settings keeps [`Subsessions::default`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Subsessions {
    /// Whether a new session gets the subsession tools when its start does not say: the person's new sessions,
    /// schedules' runs and starts from another device ([`Config::subsessions`](crate::Config::subsessions)). A
    /// session's own switch (`ThreadPatch::orchestration`) changes one session later.
    pub on: bool,
    pub first_start: crate::session_choice::Preference,
    pub limits: Limits,
    /// "Disable cross-agent subsessions": a subsession runs only its parent's agent, on every route
    /// ([`Config::cross_agent_off`](crate::Config::cross_agent_off)). Off by default.
    pub cross_agent_off: bool,
    /// "Enable cross-project subsessions": `start_session` may start a subsession in another project open here
    /// ([`Config::cross_project_on`](crate::Config::cross_project_on), [`cross_project`]). On by default.
    pub cross_project_on: bool,
}

impl Default for Subsessions {
    fn default() -> Subsessions {
        Subsessions {
            on: true,
            first_start: crate::session_choice::Preference::Ask,
            limits: Limits::default(),
            cross_agent_off: false,
            cross_project_on: true,
        }
    }
}

/// Why a tool refused: `{"code", "message"}` in the tool's error result. The code is for the agent to act on, the
/// message for it to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchCode {
    /// The session's orchestration is off.
    Off,
    /// The session is in Plan only.
    PlanMode,
    DepthLimit,
    /// Too many active children of this parent, or active linked sessions in all.
    ChildrenLimit,
    StartsLimit,
    WorktreesLimit,
    /// The tree reached its turn, wait or spend limit; a person message to its root goes on.
    Paused,
    /// The session is not in the organization the app is signed in to.
    OrgMismatch,
    /// The host has no list prices at all, so the tree's spend can't be counted.
    UnpricedModel,
    /// The person turned on "Disable cross-agent subsessions": a subsession runs only its parent's agent.
    AgentNotAllowed,
    /// The project a start names is not open in Nexus, or is archived.
    ProjectNotOpen,
    /// The project a start names is not trusted in Nexus.
    ProjectUntrusted,
    /// The person turned off "Enable cross-project subsessions".
    CrossProjectOff,
    /// A subsession in another project than its parent's tried to start a subsession or to hand off.
    CrossProject,
    /// This app or host makes no worktrees.
    WorktreesUnavailable,
    BadInput,
    /// Not a direct child of the caller.
    NotYourSession,
    Halted,
    /// The child has a turn under way or is starting.
    Busy,
    /// The app could not do it (its store failed, say): not the caller's doing.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchError {
    pub code: OrchCode,
    pub message: String,
}

impl OrchError {
    pub fn new(code: OrchCode, message: impl Into<String>) -> OrchError {
        OrchError {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for OrchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The workbench's own errors are failures, not refusals.
impl From<String> for OrchError {
    fn from(message: String) -> OrchError {
        OrchError::new(OrchCode::Failed, message)
    }
}

/// `start_session`'s input: a new linked session of `agent` that starts on `prompt`. `key` makes a retry return the
/// same child.
#[derive(Debug, Clone)]
pub struct StartChild {
    pub agent: SessionAgent,
    pub prompt: String,
    /// None: named from the prompt, then from the agent's first answer.
    pub title: Option<String>,
    /// None: the parent's model when the agent is the same, else the agent's default.
    pub model: Option<String>,
    /// Never above the parent's.
    pub effort: Option<Effort>,
    /// Never above the parent's; None: the parent's.
    pub permission_mode: Option<PermissionMode>,
    pub key: Option<String>,
    /// Another project open here, as [`crate::project_lookup::find_project`] reads it; None: the parent's
    /// ([`cross_project`]).
    pub project: Option<String>,
}

/// The session a new one is linked to: its parent, who started it (`agent` or `crew`), and its group: the crew's
/// request, or the parent's turn that started it.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub thread_id: String,
    pub spawned_by: String,
    pub group_id: Option<String>,
}

/// A tree's counters since the person last wrote to its root. In memory: they start again with the app.
#[derive(Debug, Default)]
pub(crate) struct Tree {
    starts: u32,
    /// Result messages and `message_session` turns.
    auto_turns: u32,
    /// Waits that brought no new result, by the parent that waited.
    idle_waits: HashMap<String, u32>,
    /// What the tree spent at list price ([`spend`]).
    spend: f64,
    /// The spend reached the cap: the tree is paused, and its automatic turns are interrupted.
    trip: Option<crate::event::SpendTrip>,
    /// The turns and background sub-agents the trip already asked to stop.
    asked: HashSet<String>,
}

/// A keyed child's id: the same parent and key always name the same session. Every name starts with the parent's id
/// (`t_…`), so a crew member's id ([`crew`]) is never one.
pub(crate) fn keyed_id(parent_id: &str, key: &str) -> String {
    let name = format!("{parent_id}/{key}");
    format!("t_{}", uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, name.as_bytes()))
}

fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Plan => "Plan only",
        PermissionMode::Default => "Ask first",
        PermissionMode::Auto => "Auto",
        PermissionMode::AcceptEdits => "Auto-accept edits",
        PermissionMode::BypassPermissions => "Full access",
    }
}

/// [`effort_under`] for a session of `agent` under an ancestor of another agent. A ceiling `agent` has stays the
/// ceiling, for the model's default too. One it lacks (Codex's `none`, `minimal` or `ultra` for Claude Code) becomes
/// its nearest: `low` below its range, and no ceiling above it, so the default stays the default.
pub(crate) fn effort_across(agent: SessionAgent, effort: Option<Effort>, ceiling: Option<Effort>) -> Option<Effort> {
    harness(agent).child_effort(effort, ceiling)
}

fn refuse<T>(code: OrchCode, message: impl Into<String>) -> Result<T, OrchError> {
    Err(OrchError::new(code, message))
}

/// The route a subsession of `agent` runs on: its parent's, except that an agent Gateway has no route for
/// (`billing.gateway` is None, as for Antigravity) runs on the person's own sign-in to that agent.
pub(crate) fn child_via(agent: SessionAgent, parent: Via) -> Via {
    match agent.coding() {
        Some(coding) if coding.entry().billing.gateway.is_none() => Via::Subscription,
        _ => parent,
    }
}

/// The modes `agent` offers at or below `ceiling`, lowest risk first.
fn modes_within(agent: SessionAgent, ceiling: PermissionMode) -> Vec<PermissionMode> {
    let mut modes: Vec<PermissionMode> = harness(agent)
        .offered_modes()
        .iter()
        .copied()
        .filter(|mode| mode.rank() <= ceiling.rank())
        .collect();
    modes.sort_by_key(|mode| mode.rank());
    modes
}

/// Why a session of `agent` can't run in `mode` under an ancestor at `ceiling`: the agent does not offer `mode`, and
/// running it in the agent's own default could be looser. None when it offers `mode`.
pub(crate) fn mode_refusal(agent: SessionAgent, mode: PermissionMode, ceiling: PermissionMode) -> Option<String> {
    if harness(agent).offered_modes().contains(&mode) {
        return None;
    }
    let label = harness(agent).label();
    let within = modes_within(agent, ceiling);
    Some(if within.is_empty() {
        format!(
            "{label} has no permission mode at or below {}, so it can't run under this session.",
            mode_label(ceiling)
        )
    } else {
        let names: Vec<&str> = within.iter().map(|mode| mode.as_str()).collect();
        format!(
            "{label} can't run in {}. The modes it offers here: permission_mode {}.",
            mode_label(mode),
            names.join(" or ")
        )
    })
}

/// Why another agent than `caller`'s is refused when the person turned on "Disable cross-agent subsessions": the
/// caller starts or schedules only its own agent's: `what` is "starts only {agent} subsessions" or "schedules only
/// {agent} sessions" with the agent filled in.
pub(crate) fn cross_agent_refusal(caller: SessionAgent, what: &str) -> String {
    let what = what.replace("{agent}", harness(caller).label());
    format!("The person turned on \"Disable cross-agent subsessions\" in Settings: this session {what}.")
}

/// The agents Gateway has no route for, as people name them: a subsession of one runs on the person's own sign-in
/// ([`child_via`]).
pub(crate) fn plan_only_labels() -> Vec<&'static str> {
    SessionAgent::ALL
        .into_iter()
        .filter(|agent| child_via(*agent, Via::Nexus) == Via::Subscription)
        .map(|agent| harness(agent).label())
        .collect()
}

impl Workbench {
    /// Applies this computer's subsession settings: the switch to sessions made from now on, the limits as
    /// [`Workbench::set_orchestration_limits`] does. Sessions already made keep their own switch.
    pub fn set_subsessions(&self, subsessions: Subsessions) {
        {
            let mut config = self.inner.config.lock().unwrap_or_else(|p| p.into_inner());
            config.cross_agent_off = subsessions.cross_agent_off;
            config.cross_project_on = subsessions.cross_project_on;
            config.subsessions = subsessions.on;
            config.subsessions_first_start = subsessions.first_start;
        }
        self.set_orchestration_limits(subsessions.limits);
    }

    /// Applies this computer's subsession limits at once, to the trees at work too. A spend trip ends when the cap is
    /// gone or above what the tree spent. A tree the new limits no longer pause goes on: its sessions show it, and
    /// its pending results are offered (on the async runtime, when this runs on one). A tree they pause shows it.
    pub fn set_orchestration_limits(&self, limits: Limits) {
        let limits = limits.sanitized();
        let changed: Vec<(String, bool)> = {
            let mut trees = self.trees();
            let before: HashSet<String> = trees
                .keys()
                .filter(|root| self.paused_in(&trees, root))
                .cloned()
                .collect();
            self.inner
                .config
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .orchestration = limits;
            let mut released = HashSet::new();
            for (root, tree) in trees.iter_mut() {
                if tree.trip.is_some() && limits.crew_spend_usd.is_none_or(|cap| tree.spend < cap) {
                    tree.trip = None;
                    tree.asked.clear();
                    released.insert(root.clone());
                }
            }
            // A released trip shows too when another limit still pauses the tree: its chip no longer names the cap.
            trees
                .keys()
                .filter_map(|root| {
                    let was = before.contains(root);
                    let unpaused = was && !self.paused_in(&trees, root);
                    let shown = unpaused || released.contains(root) || (!was && self.paused_in(&trees, root));
                    shown.then(|| (root.clone(), unpaused))
                })
                .collect()
        };
        for (root, unpaused) in changed {
            self.emit_tree(&root);
            if unpaused && let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let this = self.clone();
                runtime.spawn_blocking(move || this.offer_tree(&root));
            }
        }
    }

    /// Offers the pending results of every session in the tree of `root` ([`Workbench::offer_results`]).
    fn offer_tree(&self, root: &str) {
        let below = self.descendants(root).unwrap_or_else(|error| {
            eprintln!("nexus-workbench: {root}: {error}");
            vec![]
        });
        self.offer_results(root);
        for record in below {
            self.offer_results(&record.summary.id);
        }
    }

    fn trees(&self) -> MutexGuard<'_, HashMap<String, Tree>> {
        self.inner.trees.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// [`Workbench::tree_paused`] under a lock of the trees already held.
    fn paused_in(&self, trees: &HashMap<String, Tree>, root: &str) -> bool {
        let limits = self.config().orchestration;
        trees.get(root).is_some_and(|tree| {
            tree.trip.is_some()
                || reached(limits.max_auto_turns, tree.auto_turns as usize)
                || tree
                    .idle_waits
                    .values()
                    .any(|n| reached(limits.max_idle_waits, *n as usize))
        })
    }

    /// Starts a linked session of `parent_id` (`start_session`): its row is saved and returned at once, with no
    /// worktree; a task then makes its worktree from the default branch and sends it `child.prompt` from the parent.
    /// A start that fails before the agent runs ends with a failed turn, so a wait on the child ends.
    ///
    /// Refused, in this order: a parent not in this workbench's organization (`org_mismatch`), with orchestration off
    /// (`off`), in another project than its own parent's (`cross_project`), or in Plan only (`plan_mode`); a bad
    /// prompt, title or key (`bad_input`); past the depth (`depth_limit`), the parent's active children or every
    /// active linked session (`children_limit`), the parent's worktrees (`worktrees_limit`); a paused tree (`paused`)
    /// or past the tree's starts (`starts_limit`); another agent than the parent's with cross-agent subsessions off
    /// (`agent_not_allowed`); a `project` other than its own with cross-project subsessions off (`cross_project_off`),
    /// one that is not open or is archived (`project_not_open`) or one not trusted (`project_untrusted`); a mode the agent
    /// does not offer (`bad_input`); no list prices at all (`unpriced_model`); no worktrees here
    /// (`worktrees_unavailable`). A halt on the parent or on any session above it is refused too (`halted`), after its
    /// organization, and checked again just before the row is saved.
    ///
    /// A child in another project ([`cross_project`]) runs in its worktree, with its setup script, and in a mode no
    /// higher than that project's default mode.
    ///
    /// The child has the parent's organization and route (the person's own sign-in for an agent Gateway has no route
    /// for, [`child_via`]), the parent's account when it runs the same agent (else that agent's own login), and the
    /// asked mode and effort brought under its parent's and every ancestor's.
    /// With `key`, a retry returns the child the first call started.
    pub async fn start_child(&self, parent_id: &str, child: StartChild) -> Result<ThreadSummary, OrchError> {
        let limits = self.config().orchestration;
        let Some(parent) = self.reader().thread(parent_id)?.filter(|p| self.in_scope(&p.scope)) else {
            return refuse(
                OrchCode::OrgMismatch,
                "That session is not in the organization Nexus is signed in to.",
            );
        };
        // Its background sub-agents call between its turns too, and a halt holds them as well (U7).
        if let Some(refusal) = self.start_refusal(parent_id)? {
            return refuse(OrchCode::Halted, refusal);
        }
        let id = match &child.key {
            Some(key) if key.is_empty() || key.chars().count() > MAX_KEY => {
                return refuse(OrchCode::BadInput, format!("A key has 1 to {MAX_KEY} characters."));
            }
            Some(key) => Some(keyed_id(parent_id, key)),
            None => None,
        };
        if self.missing_spend_prices() {
            self.inner.host.ensure_prices().await;
        }
        // One child per key, a retry after a lost answer too.
        let gate = id.as_deref().map(|id| self.creation_gate(id));
        let _guard = match &gate {
            Some(gate) => Some(gate.lock().await),
            None => None,
        };
        if let Some(id) = &id
            && let Some(existing) = self.reader().thread(id)?
        {
            if existing.summary.parent_thread_id.as_deref() != Some(parent_id) {
                return refuse(OrchCode::BadInput, "That key already names another session.");
            }
            return Ok(existing.summary);
        }
        let prompt = child.prompt.clone();
        let group_id = self.turn_under_way(parent_id);
        let (record, project) = self.admit_child(&parent, child, id, group_id, &limits, true)?;
        let summary = record.summary.clone();
        let from = Sender::Session {
            thread_id: parent.summary.id.clone(),
            title: parent.summary.title.clone(),
        };
        self.launch_child(record, project, prompt, Some(from), vec![]);
        Ok(summary)
    }

    /// Shows a child [`Workbench::admit_in`] saved, and sets it up in the background: its worktree, then `prompt` from
    /// `from` (None: the person's, a crew's first prompt).
    fn launch_child(
        &self,
        record: ThreadRecord,
        project: Project,
        prompt: String,
        from: Option<Sender>,
        images: Vec<crate::event::Image>,
    ) {
        let native_id = record.native_id.clone();
        if self.in_scope(&record.scope) {
            self.inner
                .host
                .emit(crate::Emit::Thread(Box::new(record.summary.clone())));
        }
        let this = self.clone();
        let id = record.summary.id;
        tokio::spawn(async move { this.set_up_child(id, native_id, project, prompt, from, images).await });
    }

    /// The id of the turn under way in session `thread_id`'s live run: the group of the children its agent starts in
    /// that turn, so one fan-out gets one compare card. None with no turn under way: a call from a background
    /// sub-agent between turns starts a child in no group.
    fn turn_under_way(&self, thread_id: &str) -> Option<String> {
        let live = self.live(thread_id)?;
        let state = live.state();
        state.turns.front().map(|turn| turn.id.clone())
    }

    /// `start_child`'s checks and its row, under the trees' lock: two starts at once count each other, and a Stop or a
    /// Delete above the parent, which writes its halts under that lock ([`halts`]), either finds the child below or
    /// halted it first. `group_id` is the parent's turn under way ([`Workbench::turn_under_way`]); `persist` false (a
    /// session choice's preview) saves no row and counts no start.
    pub(crate) fn admit_child(
        &self,
        parent: &ThreadRecord,
        child: StartChild,
        id: Option<String>,
        group_id: Option<String>,
        limits: &Limits,
        persist: bool,
    ) -> Result<(ThreadRecord, Project), OrchError> {
        let mut trees = self.trees();
        let link = Link {
            thread_id: parent.summary.id.clone(),
            spawned_by: SPAWNED_BY_AGENT.into(),
            group_id,
        };
        self.admit_in(&mut trees, parent, child, id, limits, link, persist)
    }

    /// [`Workbench::admit_child`] under a lock of the trees already held, for a child `link` names: its parent's
    /// agent's, or one of the person's crew ([`crew`]), which may run either agent on the person's plan. With
    /// `persist` false (a session choice's preview), the row is neither saved nor counted as a start.
    #[allow(clippy::too_many_arguments)]
    fn admit_in(
        &self,
        trees: &mut HashMap<String, Tree>,
        parent: &ThreadRecord,
        child: StartChild,
        id: Option<String>,
        limits: &Limits,
        link: Link,
        persist: bool,
    ) -> Result<(ThreadRecord, Project), OrchError> {
        let p = &parent.summary;
        if !p.orchestration {
            return refuse(
                OrchCode::Off,
                "This session can't start other sessions: its orchestration is off.",
            );
        }
        if p.cross_project {
            return refuse(OrchCode::CrossProject, cross_project::NO_START);
        }
        // The parent and every session above it, nearest first: a child runs under all of them.
        let mut chain = vec![parent.clone()];
        chain.extend(self.ancestors(parent)?);
        let parent_mode = self.mode_ceiling(&chain).unwrap_or_else(|| self.mode_in_force(parent));
        if parent_mode == PermissionMode::Plan {
            return refuse(OrchCode::PlanMode, "A session in Plan only can't start other sessions.");
        }
        let prompt = child.prompt.trim();
        if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT {
            return refuse(
                OrchCode::BadInput,
                format!("A prompt has 1 to {MAX_PROMPT} characters."),
            );
        }
        if prompt.starts_with(results::RESULTS_HEADER) {
            return refuse(OrchCode::BadInput, results::FORGED);
        }
        let title = child.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if title.is_some_and(|t| t.chars().count() > MAX_TITLE) {
            return refuse(
                OrchCode::BadInput,
                format!("A title has at most {MAX_TITLE} characters."),
            );
        }
        let depth = p.depth + 1;
        if depth > limits.depth() {
            return refuse(
                OrchCode::DepthLimit,
                format!("Subsessions go {} levels deep at most.", limits.depth()),
            );
        }
        // Each count is read only for a limit the person set.
        if let Some(max) = limits.max_live_children {
            let children = self.store().children(&p.id)?;
            if reached(Some(max), children.iter().filter(|c| self.active(c)).count()) {
                return refuse(
                    OrchCode::ChildrenLimit,
                    format!("{max} of this session's subsessions are already at work. Wait for one to finish."),
                );
            }
        }
        if let Some(max) = limits.max_live_linked {
            let linked = self.store().linked_threads()?;
            if reached(Some(max), linked.iter().filter(|c| self.active(c)).count()) {
                return refuse(
                    OrchCode::ChildrenLimit,
                    format!("{max} subsessions are already at work on this computer. Wait for one to finish."),
                );
            }
        }
        if let Some(max) = limits.max_child_worktrees {
            // A child still starting gets its worktree soon.
            let worktrees = self
                .store()
                .children(&p.id)?
                .iter()
                .filter(|c| match &c.summary.worktree {
                    Some(worktree) => !worktree.removed,
                    None => c.summary.status.busy(),
                })
                .count();
            if reached(Some(max), worktrees) {
                return refuse(
                    OrchCode::WorktreesLimit,
                    format!("This session's subsessions have {worktrees} worktrees. Remove some before starting more."),
                );
            }
        }
        let root = chain
            .last()
            .map_or_else(|| p.id.clone(), |root| root.summary.id.clone());
        if self.paused_in(trees, &root) {
            return refuse(OrchCode::Paused, PAUSED);
        }
        if let Some(max) = limits.max_starts
            && reached(Some(max), trees.get(&root).map_or(0, |tree| tree.starts) as usize)
        {
            return refuse(
                OrchCode::StartsLimit,
                format!(
                    "This tree started {max} subsessions since the person last wrote. It goes on after their next \
                     message."
                ),
            );
        }
        // Any agent, on either route, unless the person turned cross-agent subsessions off. A child of another agent runs
        // on that agent's own login ([`child_via`], `account` below).
        if child.agent != p.agent && self.config().cross_agent_off {
            return refuse(
                OrchCode::AgentNotAllowed,
                cross_agent_refusal(p.agent, "starts only {agent} subsessions"),
            );
        }
        let place = self.child_project(p, child.project.as_deref())?;
        // Another project's default mode is a ceiling too: the agent picked the project, not the person.
        let ceiling = place.mode.map_or(parent_mode, |mode| parent_mode.lower(mode));
        let mode = child.permission_mode.unwrap_or(ceiling).lower(ceiling);
        if let Some(refusal) = mode_refusal(child.agent, mode, ceiling) {
            return refuse(OrchCode::BadInput, refusal);
        }
        let model = child
            .model
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
            // The parent's model, for its agent's own children: a crew member left blank runs its agent's default.
            .or_else(|| {
                p.model
                    .clone()
                    .filter(|_| child.agent == p.agent && link.spawned_by == SPAWNED_BY_AGENT)
            });
        // A model with no list price starts. Refusing it would block Codex's default model, which Gateway picks at launch
        // and the host can't name here; instead the spend cap counts its calls at the catalog's highest price
        // ([`spend`]). With no catalog at all, the cap could count nothing.
        if self.missing_spend_prices() {
            return refuse(
                OrchCode::UnpricedModel,
                "Nexus has no list prices yet, so it can't count this crew's spend. Try again once it is signed in.",
            );
        }
        if self.config().worktrees.is_none() {
            return refuse(
                OrchCode::WorktreesUnavailable,
                "Linked sessions need worktrees, and they aren't available here.",
            );
        }
        let project = place.project;

        // Read again: the parent's row was read before this lock, and a halt may have landed since.
        if let Some(refusal) = self.start_refusal(&p.id)? {
            return refuse(OrchCode::Halted, refusal);
        }
        let now = now_ms();
        let record = ThreadRecord {
            summary: ThreadSummary {
                name: None,
                id: id.unwrap_or_else(|| format!("t_{}", short_id())),
                project_id: project.id.clone(),
                title: title.map_or_else(|| title_of(prompt), |t| t.to_string()),
                agent: child.agent,
                model,
                permission_mode: mode,
                effort: self.effort_under_all(child.agent, child.effort, &chain),
                fast: false,
                via: child_via(child.agent, p.via),
                provider: None,
                // The parent's plan account, for the same agent only: a Claude account is no Codex login.
                account: p.account.clone().filter(|_| child.agent == p.agent),
                status: ThreadStatus::Starting,
                cost_usd: 0.0,
                nexus_sessions: vec![],
                context_tokens: None,
                context_window: None,
                created_at: now,
                updated_at: now,
                listed_at: Some(now),
                acted_at: Some(now),
                archived: false,
                pinned: false,
                worktree: None,
                pull_requests: Vec::new(),
                pending: None,
                capabilities: None,
                parent_thread_id: Some(link.thread_id),
                source_thread_id: None,
                side_chat: false,
                spawned_by: Some(link.spawned_by),
                group_id: link.group_id,
                depth,
                cross_project: place.cross,
                // Depth gates the tools at each launch (`arm`, `Limits::depth`), so a change to the limits reaches
                // this child too.
                orchestration: true,
                waits_on: vec![],
                pending_results: 0,
                start_choices: vec![],
                halt: None,
                crew_paused: false,
                crew_tripped: false,
                crew_spend_trip: None,
                in_background: false,
                background: vec![],
                background_total: 0,
                at_depth_limit: false,
                handoff_from: None,
                limited: None,
                aside: Default::default(),
            },
            native_id: uuid::Uuid::new_v4().to_string(),
            native_started: false,
            // The parent's organization, never the one the app is in now.
            scope: parent.scope.clone(),
            fork_from: None,
            seed: None,
            automatic_title: title.is_none(),
            delivered_seq: 0,
            halt: None,
        };
        if persist {
            self.store().insert_thread(&record)?;
            self.register_session_sync(&record.summary.id, self.inherited_sync_identity(parent).as_ref());
            trees.entry(root).or_default().starts += 1;
        }
        Ok((record, project))
    }

    /// The child's worktree, its setup script, then its first prompt from `from` (None: the person's). Any failure before its agent
    /// runs ends it with a failed turn: a project removed or a child archived before its worktree too
    /// ([`Workbench::start_gone`]). `native_id` tells this child from a session saved under its id after it was
    /// deleted (a keyed retry's), which this setup never touches. A child stopped, halted or deleted meanwhile keeps its
    /// worktree and sends nothing; one whose tree paused meanwhile ends as stopped.
    async fn set_up_child(
        &self,
        id: String,
        native_id: String,
        project: Project,
        prompt: String,
        from: Option<Sender>,
        images: Vec<crate::event::Image>,
    ) {
        let made = async {
            let _update_activity = UpdateActivity::begin(self)?;
            if let Some(reason) = self.start_gone(&id, &project)? {
                return Err(reason);
            }
            let worktree = self
                .make_worktree(&project, &id, &prompt, NewWorktree::default())
                .await?;
            let mut ours = false;
            let mut ended = false;
            let saved = self.change(&id, |record| {
                ours = record.native_id == native_id;
                if ours {
                    record.summary.worktree = Some(worktree.clone());
                    // Archived meanwhile: its project was removed, say.
                    ended = record.summary.status != ThreadStatus::Starting
                        || record.halt.is_some()
                        || record.summary.archived;
                }
            });
            let summary = match saved {
                Ok(summary) if ours => summary,
                // The session went meanwhile: its worktree goes too.
                gone => {
                    let repo = std::path::PathBuf::from(&project.path);
                    let _ = tokio::task::spawn_blocking(move || {
                        git::discard_worktree(&repo, std::path::Path::new(&worktree.path), &worktree.branch)
                    })
                    .await;
                    return Err(gone.err().unwrap_or_else(|| format!("No session {id}.")));
                }
            };
            if ended {
                return Ok(false);
            }
            self.begin_setup(&summary, &project.path)?;
            Ok::<_, String>(true)
        }
        .await;
        let sent = match made {
            Ok(true) => {
                self.first_prompt(&id, &native_id, &prompt, from.clone(), images.clone())
                    .await
            }
            Ok(false) => {
                self.end_ended_child(&id);
                return;
            }
            Err(error) => Err(error),
        };
        if let Err(reason) = sent {
            if let Err(error) = self.fail_start(&id, &native_id, &prompt, from, images, reason) {
                eprintln!("nexus-workbench: {id}: {error}");
            }
            // Its failed turn is a result: a wait on it ends.
            self.child_moved(&id);
        }
    }

    /// The child's first prompt, once its setup script ended: sent, unless it was stopped, halted or deleted meanwhile,
    /// a session above it was halted (it ends as stopped, if the cascade did not end it already) or its tree paused (it
    /// ends as stopped). From its last
    /// check until its agent is registered it holds [`halts::first_prompt_gate`], which Delete waits for.
    async fn first_prompt(
        &self,
        id: &str,
        native_id: &str,
        prompt: &str,
        from: Option<Sender>,
        images: Vec<crate::event::Image>,
    ) -> crate::Result<()> {
        // Its place in line ahead of any message the person sent it meanwhile; the script has ended once it is ours.
        let _in_line = self.wait_for_setup(id).await;
        let gate = self.creation_gate(&halts::first_prompt_gate(id));
        let _gate = gate.lock().await;
        let Some(record) = self.reader().thread(id)?.filter(|record| record.native_id == native_id) else {
            return Err(format!("No session {id}."));
        };
        // A halt above it holds it too: the person interrupted its parent while it was being made, say.
        let stopped = record.summary.status != ThreadStatus::Starting
            || record.halt.is_some()
            || self.ancestors(&record)?.iter().any(|above| above.halt.is_some());
        if stopped || !self.in_scope(&record.scope) || self.tree_paused(&self.root_of(&record)?) {
            self.end_ended_child(id);
            return Ok(());
        }
        // Never the person's turn, even when it is their words: a halt or the crew's spend cap holds it ([`crew`]).
        self.send_with(id, prompt, images, from, false, crate::SendContext::default())
            .await
    }

    /// A child that will not run its first prompt ends as stopped, if nothing ended it yet, and its parent hears of it.
    fn end_ended_child(&self, id: &str) {
        if let Err(error) = self.end_unstarted(id) {
            eprintln!("nexus-workbench: {id}: {error}");
        }
        self.child_moved(id);
    }

    /// A child whose start failed before its agent ran: its prompt (if not logged yet) and a failed turn saying why,
    /// so the child is over. Nothing when its log already ends a turn after its first message (a failed launch wrote
    /// that itself), and nothing once the child is deleted: each write checks first, so a deleted child's id gets no
    /// rows that a keyed retry under it would inherit.
    fn fail_start(
        &self,
        id: &str,
        native_id: &str,
        prompt: &str,
        from: Option<Sender>,
        images: Vec<crate::event::Image>,
        reason: String,
    ) -> crate::Result<()> {
        let ours = || -> crate::Result<Option<ThreadRecord>> {
            Ok(self.reader().thread(id)?.filter(|record| record.native_id == native_id))
        };
        let Some(record) = ours()? else {
            return Ok(());
        };
        let events = self.reader().events_after(id, 0)?;
        // Rows from before the child was saved are a deleted session's under the same id, not its own.
        let own = |stored: &StoredEvent| stored.at >= record.summary.created_at;
        let first = events
            .iter()
            .position(|stored| own(stored) && matches!(stored.event, WbEvent::User { .. }));
        if let Some(first) = first
            && events[first..]
                .iter()
                .any(|stored| matches!(stored.event, WbEvent::TurnEnd { .. }))
        {
            return Ok(());
        }
        if first.is_none() {
            if ours()?.is_none() {
                return Ok(());
            }
            self.append(
                id,
                WbEvent::User {
                    text: prompt.into(),
                    images,
                    from,
                },
            )?;
        }
        let turn_id = format!("turn_{}", short_id());
        if ours()?.is_none() {
            return Ok(());
        }
        self.append(
            id,
            WbEvent::TurnStart {
                turn_id: turn_id.clone(),
            },
        )?;
        if ours()?.is_none() {
            return Ok(());
        }
        self.append(
            id,
            WbEvent::TurnEnd {
                turn_id,
                status: TurnStatus::Error,
                cost_usd: None,
                duration_ms: 0,
                error: Some(reason),
                cursor: None,
            },
        )?;
        self.set_status(id, ThreadStatus::Error)?;
        Ok(())
    }

    /// Active: starting, in a turn, waiting on the person, or with a background sub-agent still at work. A background
    /// shell (a dev server) does not count.
    fn active(&self, record: &ThreadRecord) -> bool {
        record.summary.status.busy()
            || self.live(&record.summary.id).is_some_and(|live| {
                let state = live.state();
                !state.done && (!state.turns.is_empty() || !state.pending.is_empty() || state.book.agents_running())
            })
    }

    /// The mode the session runs in: the lower of the saved one and one its agent has not answered yet.
    fn mode_in_force(&self, record: &ThreadRecord) -> PermissionMode {
        let saved = record.summary.permission_mode;
        match self.pending(&record.summary.id).and_then(|p| p.permission_mode) {
            Some(pending) => saved.lower(pending),
            None => saved,
        }
    }

    /// The effort the session runs at: the saved one brought under one its agent has not answered yet
    /// ([`effort_under`]).
    fn effort_in_force(&self, record: &ThreadRecord) -> Option<Effort> {
        let saved = record.summary.effort;
        match self.pending(&record.summary.id).and_then(|p| p.effort) {
            Some(pending) => effort_under(saved, pending),
            None => saved,
        }
    }

    /// The highest mode a session under all of `ancestors` may run in: the lowest in force among them. None without
    /// ancestors.
    fn mode_ceiling(&self, ancestors: &[ThreadRecord]) -> Option<PermissionMode> {
        ancestors
            .iter()
            .map(|ancestor| self.mode_in_force(ancestor))
            .reduce(PermissionMode::lower)
    }

    /// The effort of a session of `agent` brought under every one of `ancestors`, nearest first: [`effort_under`] an
    /// ancestor of the same agent, [`effort_across`] one of another.
    fn effort_under_all(
        &self,
        agent: SessionAgent,
        effort: Option<Effort>,
        ancestors: &[ThreadRecord],
    ) -> Option<Effort> {
        ancestors.iter().fold(effort, |effort, ancestor| {
            let ceiling = self.effort_in_force(ancestor);
            if ancestor.summary.agent == agent {
                effort_under(effort, ceiling)
            } else {
                effort_across(agent, effort, ceiling)
            }
        })
    }

    /// The session's parent, its parent's parent, and so on, nearest first, as far as their rows still exist. A row
    /// that can't be read fails it: a clamp or a limit never lifts on a store error.
    fn ancestors(&self, record: &ThreadRecord) -> crate::Result<Vec<ThreadRecord>> {
        let mut chain: Vec<ThreadRecord> = Vec::new();
        let mut next = record.summary.parent_thread_id.clone();
        while let Some(id) = next.take() {
            // A cycle (only a damaged store has one) ends the chain at its first repeat.
            if chain.len() >= MAX_CHAIN || id == record.summary.id || chain.iter().any(|a| a.summary.id == id) {
                break;
            }
            let Some(parent) = self.reader().thread(&id)? else {
                break;
            };
            next = parent.summary.parent_thread_id.clone();
            chain.push(parent);
        }
        Ok(chain)
    }

    /// A person message to `record`'s session: its halt and its descendants' cascade halts are cleared, and on a tree's
    /// root, the tree's counters and spend start again and its pause ends. One to a linked session does not reset them,
    /// so every answer the person gives a child doesn't buy the crew a new budget. When its parents can't be read, the
    /// counters stay.
    pub(crate) fn person_wrote(&self, record: &ThreadRecord) {
        // Its halt goes, and the cascade's below it ([`halts`]).
        let cleared = self.person_clears_below(record);
        let unpaused = match self.ancestors(record) {
            Ok(ancestors) if ancestors.is_empty() => {
                let mut trees = self.trees();
                let paused = self.paused_in(&trees, &record.summary.id);
                trees.remove(&record.summary.id);
                paused
            }
            Ok(_) => false,
            Err(error) => {
                eprintln!("nexus-workbench: {}: {error}", record.summary.id);
                false
            }
        };
        // What changed below it shows (its own shows with its turn): every session when the tree's pause ended, else
        // only the ones whose halt went. A message that changed nothing below sends nothing.
        if unpaused {
            if let Ok(below) = self.descendants(&record.summary.id) {
                for below in below {
                    self.emit_summary(&below.summary.id);
                }
            }
        } else {
            for id in cleared {
                self.emit_summary(&id);
            }
        }
    }

    /// [`Workbench::person_clears`], and the sessions below `record`'s whose cascade halt it cleared: the ones that
    /// had one before and have none after. A row that can't be read counts as not cleared.
    fn person_clears_below(&self, record: &ThreadRecord) -> Vec<String> {
        let halted: Vec<String> = self
            .descendants(&record.summary.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|below| Halt::of(below) == Some(Halt::Parent))
            .map(|below| below.summary.id)
            .collect();
        if let Err(error) = self.person_clears(record) {
            eprintln!("nexus-workbench: {}: {error}", record.summary.id);
        }
        halted
            .into_iter()
            .filter(|id| matches!(self.halt(id), Ok(None)))
            .collect()
    }

    /// The record with its mode and effort brought down to every ancestor's, saved when they change: a linked session
    /// never resumes above a parent that was lowered while it was not running.
    pub(crate) fn clamp_to_ancestors(&self, record: ThreadRecord) -> crate::Result<ThreadRecord> {
        if record.summary.parent_thread_id.is_none() {
            return Ok(record);
        }
        let ancestors = self.ancestors(&record)?;
        let mode = self
            .mode_ceiling(&ancestors)
            .map_or(record.summary.permission_mode, |ceiling| {
                record.summary.permission_mode.lower(ceiling)
            });
        // A mode the agent does not offer would run in its own default, which may be looser: the highest one it offers
        // below, or no run at all.
        let mode = match mode_refusal(record.summary.agent, mode, mode) {
            None => mode,
            Some(refusal) => *modes_within(record.summary.agent, mode).last().ok_or(refusal)?,
        };
        let effort = self.effort_under_all(record.summary.agent, record.summary.effort, &ancestors);
        if mode == record.summary.permission_mode && effort == record.summary.effort {
            return Ok(record);
        }
        let summary = self.change(&record.summary.id, |saved| {
            saved.summary.permission_mode = mode;
            saved.summary.effort = effort;
        })?;
        Ok(ThreadRecord {
            summary: ThreadSummary {
                pending: None,
                capabilities: None,
                ..summary
            },
            ..record
        })
    }

    /// The highest mode `thread_id` may run in under its ancestors ([`Self::mode_ceiling`]); None for a session with no
    /// ancestors.
    pub(crate) fn mode_ceiling_of(&self, thread_id: &str) -> crate::Result<Option<PermissionMode>> {
        let record = self.record(thread_id)?;
        Ok(self.mode_ceiling(&self.ancestors(&record)?))
    }

    /// Refuses `mode` for a linked session above any of its ancestors'.
    pub(crate) fn check_mode(&self, thread_id: &str, mode: PermissionMode) -> crate::Result<()> {
        let record = self.record(thread_id)?;
        crate::driver::check_mode(record.summary.agent, mode)?;
        let Some(ceiling) = self.mode_ceiling(&self.ancestors(&record)?) else {
            return Ok(());
        };
        if mode.rank() > ceiling.rank() {
            return Err(format!(
                "This session can't run above the sessions it was started under: one is in {}. Raise that session's \
                 mode first.",
                mode_label(ceiling)
            ));
        }
        Ok(())
    }

    /// Refuses turning orchestration on for a session at the deepest level ([`Limits::depth`]): its agent would get
    /// no tools (`arm`), and a restart for them would only end its work.
    pub(crate) fn check_orchestration(&self, thread_id: &str) -> crate::Result<()> {
        let record = self.record(thread_id)?;
        if record.summary.cross_project {
            return Err(cross_project::NO_ORCHESTRATION.into());
        }
        let deepest = self.config().orchestration.depth();
        if record.summary.depth >= deepest {
            return Err(format!(
                "This session is at the deepest level subsessions go ({deepest}), so its agent can't start sessions \
                 of its own."
            ));
        }
        Ok(())
    }

    /// Refuses `effort` for a linked session above what its ancestors allow ([`effort_under`]). Back to the model's
    /// default too, under an ancestor with an effort of its own: the default may be higher.
    pub(crate) fn check_effort(&self, thread_id: &str, effort: Option<Effort>) -> crate::Result<()> {
        let record = self.record(thread_id)?;
        let ancestors = self.ancestors(&record)?;
        let agent = record.summary.agent;
        if self.effort_under_all(agent, effort, &ancestors) == effort {
            return Ok(());
        }
        Err(match self.effort_under_all(agent, Some(Effort::Max), &ancestors) {
            Some(top) => format!(
                "This session can't run at a higher effort than the sessions it was started under: one is at {} \
                 effort. Raise that session's effort first.",
                top.as_str()
            ),
            None => "This session can't run at a higher effort than the sessions it was started under: one is at \
                     the model's default. Pick low or medium, or the default."
                .into(),
        })
    }

    /// Brings every descendant of the session down under it: to `mode` when it runs above it, and with `effort`, to
    /// the effort its ancestors allow now. Its saved values at once, a running agent's through the usual change (saved
    /// when the agent accepts it; the next launch clamps it either way).
    pub(crate) fn lower_descendants(&self, thread_id: &str, mode: Option<PermissionMode>, effort: bool) {
        let children = match self.store().children(thread_id) {
            Ok(children) => children,
            Err(error) => {
                eprintln!("nexus-workbench: {thread_id}: {error}");
                return;
            }
        };
        for child in children {
            let id = &child.summary.id;
            let mut patch = ThreadPatch::default();
            if let Some(mode) = mode
                && self.mode_in_force(&child).rank() > mode.rank()
            {
                // The highest mode its agent offers at or below. An agent with none (OpenCode, which offers only Ask
                // first, under Plan only) stops, halted, and its next launch refuses to run (`clamp_to_ancestors`).
                match modes_within(child.summary.agent, mode).last() {
                    Some(within) => patch.permission_mode = Some(*within),
                    None => {
                        if let Err(error) = self.stop(id) {
                            eprintln!("nexus-workbench: {id}: {error}");
                        }
                        continue;
                    }
                }
            }
            if effort {
                match self.ancestors(&child) {
                    Ok(ancestors) => {
                        let now = self.effort_in_force(&child);
                        let under = self.effort_under_all(child.summary.agent, now, &ancestors);
                        if under != now {
                            patch.effort = Some(under);
                        }
                    }
                    Err(error) => eprintln!("nexus-workbench: {id}: {error}"),
                }
            }
            // Its own update lowers its descendants in what it changes; the rest goes on from here.
            let rest_mode = mode.filter(|_| patch.permission_mode.is_none());
            let rest_effort = effort && patch.effort.is_none();
            if (patch.permission_mode.is_some() || patch.effort.is_some())
                && let Err(error) = self.update(id, patch)
            {
                eprintln!("nexus-workbench: {id}: {error}");
            }
            if rest_mode.is_some() || rest_effort {
                self.lower_descendants(id, rest_mode, rest_effort);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default sets no limit, a set value keeps its size, and depth stops only where the ancestor walk stops.
    #[test]
    fn a_limit_is_optional_and_has_no_ceiling() {
        let none = Limits::default();
        assert_eq!(
            none,
            Limits {
                max_depth: None,
                max_live_children: None,
                max_live_linked: None,
                max_child_worktrees: None,
                max_starts: None,
                max_auto_turns: None,
                max_idle_waits: None,
                crew_spend_usd: None,
            }
        );
        assert_eq!(none.depth(), MAX_DEPTH);
        for count in [0, 1, 1_000_000] {
            assert!(!reached(none.max_starts, count), "no limit is never reached");
        }
        let wild = Limits {
            max_depth: Some(9),
            max_live_children: Some(99),
            max_live_linked: Some(500),
            max_child_worktrees: Some(1_000),
            max_starts: Some(10_000),
            max_auto_turns: Some(u32::MAX),
            max_idle_waits: Some(99),
            crew_spend_usd: Some(1e9),
        };
        assert_eq!(wild.sanitized(), wild, "no value is cut to a ceiling");
        assert_eq!(wild.depth(), 9);
        assert!(!reached(wild.max_starts, 9_999) && reached(wild.max_starts, 10_000));
        let deep = Limits {
            max_depth: Some(40),
            ..Limits::default()
        };
        assert_eq!(deep.depth(), MAX_DEPTH, "the ancestor walk bounds depth");
        let odd = Limits {
            crew_spend_usd: Some(f64::NAN),
            ..Limits::default()
        };
        assert_eq!(odd.sanitized().crew_spend_usd, None);
        let negative = Limits {
            crew_spend_usd: Some(-5.0),
            ..Limits::default()
        };
        assert_eq!(negative.sanitized().crew_spend_usd, Some(0.0));
    }

    #[test]
    fn codes_are_snake_case_on_the_wire() {
        let error = OrchError::new(OrchCode::UnpricedModel, "no price");
        assert_eq!(
            serde_json::to_value(&error).ok(),
            Some(serde_json::json!({"code": "unpriced_model", "message": "no price"}))
        );
        for (code, wire) in [
            (OrchCode::Off, "off"),
            (OrchCode::PlanMode, "plan_mode"),
            (OrchCode::DepthLimit, "depth_limit"),
            (OrchCode::ChildrenLimit, "children_limit"),
            (OrchCode::StartsLimit, "starts_limit"),
            (OrchCode::WorktreesLimit, "worktrees_limit"),
            (OrchCode::Paused, "paused"),
            (OrchCode::OrgMismatch, "org_mismatch"),
            (OrchCode::AgentNotAllowed, "agent_not_allowed"),
            (OrchCode::ProjectNotOpen, "project_not_open"),
            (OrchCode::ProjectUntrusted, "project_untrusted"),
            (OrchCode::CrossProjectOff, "cross_project_off"),
            (OrchCode::CrossProject, "cross_project"),
            (OrchCode::WorktreesUnavailable, "worktrees_unavailable"),
            (OrchCode::BadInput, "bad_input"),
            (OrchCode::NotYourSession, "not_your_session"),
            (OrchCode::Halted, "halted"),
            (OrchCode::Busy, "busy"),
            (OrchCode::Failed, "failed"),
        ] {
            assert_eq!(serde_json::to_value(code).ok(), Some(serde_json::json!(wire)));
        }
    }

    #[test]
    fn a_key_names_the_same_child_of_the_same_parent_only() {
        let a = keyed_id("t_parent", "build");
        assert_eq!(a, keyed_id("t_parent", "build"));
        assert_ne!(a, keyed_id("t_other", "build"));
        assert_ne!(a, keyed_id("t_parent", "build2"));
        assert!(a.starts_with("t_") && uuid::Uuid::parse_str(&a[2..]).is_ok());
    }

    #[test]
    fn ranks_modes_by_risk_not_by_declaration() {
        use PermissionMode::*;
        assert_eq!(BypassPermissions.lower(Plan), Plan);
        assert_eq!(Default.lower(AcceptEdits), Default);
        assert_eq!(AcceptEdits.lower(BypassPermissions), AcceptEdits);
        assert!(Plan.rank() < Default.rank() && Default.rank() < AcceptEdits.rank());
        // Auto may run a command nobody was asked about: under an Auto-accept edits parent, a child asks again.
        assert_eq!(Auto.lower(AcceptEdits), AcceptEdits);
        assert_eq!(Auto.lower(BypassPermissions), Auto);
    }

    #[test]
    fn the_models_default_effort_never_lifts_a_session_above_its_ancestors() {
        use Effort::*;
        assert!(Low < Medium && Xhigh < Max);
        assert_eq!(effort_under(Some(Max), Some(Low)), Some(Low));
        assert_eq!(effort_under(Some(Low), Some(High)), Some(Low));
        // Under an explicit effort, the model's default is that effort.
        assert_eq!(effort_under(None, Some(Low)), Some(Low));
        // Under the model's default, a lower effort only.
        assert_eq!(effort_under(Some(Low), None), Some(Low));
        assert_eq!(effort_under(Some(Medium), None), Some(Medium));
        assert_eq!(effort_under(Some(High), None), None);
        assert_eq!(effort_under(Some(Max), None), None);
        assert_eq!(effort_under(None, None), None);
    }

    #[test]
    fn an_ancestor_of_another_agent_caps_the_default_with_an_effort_both_have() {
        use Effort::*;
        use SessionAgent::*;
        for agent in [Claude, Codex] {
            for shared in [Low, Medium, High, Xhigh, Max] {
                assert_eq!(effort_across(agent, None, Some(shared)), Some(shared));
            }
        }
        // A Claude Code lead at low: a Codex member on its default runs at low, as one that picks high does.
        assert_eq!(effort_across(Codex, None, Some(Low)), Some(Low));
        assert_eq!(effort_across(Codex, Some(High), Some(Low)), Some(Low));
        assert_eq!(effort_across(Claude, None, Some(Medium)), Some(Medium));
        // Codex's none and minimal are below Claude Code's range: its lowest, low.
        assert_eq!(effort_across(Claude, None, Some(Disabled)), Some(Low));
        assert_eq!(effort_across(Claude, Some(Max), Some(Minimal)), Some(Low));
        // Codex's ultra is above it: no ceiling, and the default stays the default.
        assert_eq!(effort_across(Claude, None, Some(Ultra)), None);
        assert_eq!(effort_across(Claude, Some(Max), Some(Ultra)), Some(Max));
        // Under an ancestor on its default, as for the same agent.
        assert_eq!(effort_across(Codex, Some(High), None), None);
        assert_eq!(effort_across(Codex, Some(Low), None), Some(Low));
    }

    /// Antigravity's levels are low to xhigh: a ceiling below them is low, and one above them is no ceiling.
    #[test]
    fn an_antigravity_subsession_takes_the_nearest_level_it_has() {
        use Effort::*;
        use SessionAgent::Antigravity;
        for shared in [Low, Medium, High, Xhigh] {
            assert_eq!(effort_across(Antigravity, None, Some(shared)), Some(shared));
            assert_eq!(effort_across(Antigravity, Some(Xhigh), Some(shared)), Some(shared));
        }
        assert_eq!(effort_across(Antigravity, None, Some(Disabled)), Some(Low));
        assert_eq!(effort_across(Antigravity, Some(High), Some(Minimal)), Some(Low));
        for above in [Max, Ultra] {
            assert_eq!(effort_across(Antigravity, None, Some(above)), None);
            assert_eq!(effort_across(Antigravity, Some(High), Some(above)), Some(High));
        }
    }

    /// A mode the agent does not offer is refused with the modes it offers at or below the ceiling, lowest risk first;
    /// the agents that offer every mode are never refused.
    #[test]
    fn a_subsession_never_runs_in_a_mode_its_agent_does_not_offer() {
        use PermissionMode::*;
        let every = [Plan, Default, AcceptEdits, Auto, BypassPermissions];
        #[cfg(feature = "cursor")]
        {
            assert_eq!(modes_within(SessionAgent::Cursor, Auto), [Plan, AcceptEdits]);
            assert_eq!(modes_within(SessionAgent::Cursor, Default), [Plan]);
            assert_eq!(
                mode_refusal(SessionAgent::Cursor, Default, Auto).as_deref(),
                Some("Cursor can't run in Ask first. The modes it offers here: permission_mode plan or acceptEdits.")
            );
        }
        #[cfg(feature = "opencode")]
        {
            assert_eq!(modes_within(SessionAgent::Opencode, Plan), []);
            assert_eq!(
                mode_refusal(SessionAgent::Opencode, Plan, Plan).as_deref(),
                Some("OpenCode has no permission mode at or below Plan only, so it can't run under this session.")
            );
        }
        for agent in [
            SessionAgent::Claude,
            SessionAgent::Codex,
            SessionAgent::Nexus,
            SessionAgent::Antigravity,
        ] {
            for mode in every {
                assert_eq!(mode_refusal(agent, mode, mode), None, "{agent:?} {mode:?}");
            }
        }
    }

    /// A subsession keeps its parent's route, except an agent Gateway has no route for: that one runs on the person's
    /// own sign-in, even under a parent on Gateway.
    #[test]
    fn an_agent_without_a_gateway_route_runs_on_the_persons_own_sign_in() {
        let plan_only = plan_only_labels();
        assert!(plan_only.contains(&"Antigravity"), "{plan_only:?}");
        assert!(!plan_only.contains(&"Claude Code") && !plan_only.contains(&"Nexus"));
        for parent in [Via::Nexus, Via::Subscription] {
            assert_eq!(child_via(SessionAgent::Antigravity, parent), Via::Subscription);
            for agent in [SessionAgent::Claude, SessionAgent::Codex, SessionAgent::Nexus] {
                assert_eq!(child_via(agent, parent), parent, "{agent:?}");
            }
        }
    }
}
