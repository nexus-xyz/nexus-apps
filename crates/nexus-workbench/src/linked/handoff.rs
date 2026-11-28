//! A session that hands its work to a new one and archives itself (`handoff_session`), and a session that archives one
//! it started, or itself (`archive_session`). The case: "I'm out of Claude usage. Spawn a Codex session,
//! send a handoff there and archive yourself."
//!
//! The new session takes the old one's place: its parent, its children, its unique name and its worktree, which moves
//! to it so a cleanup of the archived session never removes it. It starts empty, with the handoff as its first message,
//! once the old session's turn has ended, so two agents never edit the same folder at once. Each handoff counts as a
//! start in its tree, and a root's counters move to its successor: a chain of handoffs stops at the start limit until
//! the person writes, and a halt on the old session refuses it.

use std::time::Duration;

use super::{MAX_PROMPT, MAX_TITLE, OrchCode, OrchError, refuse, results};
use crate::event::{Effort, HandoffFrom, Sender, SessionAgent, ThreadStatus, ThreadSummary};
use crate::store::ThreadRecord;
use crate::{ThreadPatch, Workbench, now_ms, short_id};

/// How long the new session's first message waits for the old session's turn to end.
const TURN_END_WAIT: Duration = Duration::from_secs(120);

/// `handoff_session`'s input: the new session's agent, its first message, and optionally its title, model and effort.
#[derive(Debug, Clone)]
pub struct HandOff {
    pub agent: SessionAgent,
    pub prompt: String,
    /// None: the old session's title.
    pub title: Option<String>,
    /// None: the old session's model for the same agent, else the agent's default.
    pub model: Option<String>,
    /// Never above the old session's.
    pub effort: Option<Effort>,
}

impl Workbench {
    /// Hands session `caller_id`'s work to a new session of `hand_off.agent` and archives `caller_id`.
    ///
    /// Refused: not in this organization (`org_mismatch`); halted, or under a halted session (`halted`); orchestration
    /// off (`off`); a subsession in another project than its parent's (`cross_project`); Plan only (`plan_mode`); already archived, an empty or overlong prompt or title (`bad_input`); a
    /// paused tree (`paused`); past the tree's starts (`starts_limit`).
    pub async fn hand_off(&self, caller_id: &str, hand_off: HandOff) -> Result<ThreadSummary, OrchError> {
        if self.missing_spend_prices() {
            self.inner.host.ensure_prices().await;
        }
        let (record, caller, children) = self.admit_handoff(caller_id, &hand_off)?;
        let id = record.summary.id.clone();
        let from = Sender::Session {
            thread_id: caller_id.into(),
            title: caller.summary.title.clone(),
        };
        let mut archived = caller.summary;
        archived.updated_at = record.summary.created_at;
        self.on_archived(&archived);
        self.emit_summary(caller_id);
        self.emit_summary(&id);
        for child in children {
            self.emit_summary(&child.summary.id);
        }
        if let Some(parent) = &record.summary.parent_thread_id {
            self.emit_summary(parent);
        }
        let this = self.clone();
        let old = caller_id.to_string();
        let native_id = record.native_id;
        let prompt = hand_off.prompt.trim().to_string();
        tokio::spawn(async move {
            let start = async {
                this.finish_archived_run(&old).await?;
                if this.halt(&old)?.is_some() {
                    this.stop(&id)?;
                    this.end_ended_child(&id);
                    return Ok(());
                }
                this.first_prompt(&id, &native_id, &prompt, Some(from.clone()), vec![])
                    .await
            }
            .await;
            if let Err(reason) = start {
                if let Err(error) = this.fail_start(&id, &native_id, &prompt, Some(from), vec![], reason) {
                    eprintln!("nexus-workbench: {id}: {error}");
                }
                this.child_moved(&id);
            }
        });
        Ok(record.summary)
    }

    fn admit_handoff(
        &self,
        caller_id: &str,
        hand_off: &HandOff,
    ) -> Result<(ThreadRecord, ThreadRecord, Vec<ThreadRecord>), OrchError> {
        let gate = self.settings_gate(caller_id);
        let _settings = gate.lock().unwrap_or_else(|p| p.into_inner());
        let mut trees = self.trees();
        let Some(caller) = self.reader().thread(caller_id)?.filter(|c| self.in_scope(&c.scope)) else {
            return refuse(
                OrchCode::OrgMismatch,
                "That session is not in the organization Nexus is signed in to.",
            );
        };
        if let Some(refusal) = self.start_refusal(caller_id)? {
            return refuse(OrchCode::Halted, refusal);
        }
        let c = &caller.summary;
        if !c.orchestration {
            return refuse(
                OrchCode::Off,
                "This session can't hand off its work: its orchestration is off.",
            );
        }
        if c.cross_project {
            return refuse(OrchCode::CrossProject, super::cross_project::NO_HANDOFF);
        }
        if c.archived {
            return refuse(OrchCode::BadInput, "This session is already archived.");
        }
        let mut chain = vec![caller.clone()];
        chain.extend(self.ancestors(&caller)?);
        let mode = self.mode_ceiling(&chain).unwrap_or_else(|| self.mode_in_force(&caller));
        if mode == crate::PermissionMode::Plan {
            return refuse(OrchCode::PlanMode, "A session in Plan only can't hand off its work.");
        }
        let prompt = hand_off.prompt.trim();
        if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT {
            return refuse(
                OrchCode::BadInput,
                format!("A handoff has 1 to {MAX_PROMPT} characters."),
            );
        }
        if prompt.starts_with(results::RESULTS_HEADER) {
            return refuse(OrchCode::BadInput, results::FORGED);
        }
        let title = hand_off.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if title.is_some_and(|t| t.chars().count() > MAX_TITLE) {
            return refuse(
                OrchCode::BadInput,
                format!("A title has at most {MAX_TITLE} characters."),
            );
        }
        if self.missing_spend_prices() {
            return refuse(
                OrchCode::UnpricedModel,
                "Nexus has no list prices yet, so it can't count this crew's spend. Try again once it is signed in.",
            );
        }
        let limits = self.config().orchestration;
        let root = chain
            .last()
            .map_or_else(|| c.id.clone(), |root| root.summary.id.clone());
        let same_agent = hand_off.agent == c.agent;
        let now = now_ms();
        let record = ThreadRecord {
            summary: ThreadSummary {
                id: format!("t_{}", short_id()),
                title: title.map_or_else(|| c.title.clone(), str::to_string),
                agent: hand_off.agent,
                model: hand_off
                    .model
                    .clone()
                    .map(|model| model.trim().to_string())
                    .filter(|model| !model.is_empty())
                    .or_else(|| c.model.clone().filter(|_| same_agent)),
                permission_mode: mode,
                effort: self.effort_under_all(hand_off.agent, hand_off.effort.or(c.effort), &chain),
                fast: false,
                // The old session's account only for the same agent: a Claude account is no Codex login.
                account: c.account.clone().filter(|_| same_agent),
                status: ThreadStatus::Starting,
                cost_usd: 0.0,
                nexus_sessions: vec![],
                context_tokens: None,
                context_window: None,
                created_at: now,
                updated_at: now,
                listed_at: Some(now),
                archived: false,
                pinned: c.pinned,
                pull_requests: Vec::new(),
                pending: None,
                capabilities: None,
                source_thread_id: None,
                side_chat: false,
                waits_on: vec![],
                pending_results: 0,
                start_choices: vec![],
                halt: None,
                crew_paused: false,
                crew_tripped: false,
                in_background: false,
                background: vec![],
                background_total: 0,
                at_depth_limit: false,
                limited: None,
                aside: Default::default(),
                handoff_from: Some(HandoffFrom {
                    thread_id: c.id.clone(),
                    title: c.title.clone(),
                    agent: c.agent,
                }),
                // Its place: the same project, parent, depth and orchestration, the name and the worktree.
                ..c.clone()
            },
            native_id: uuid::Uuid::new_v4().to_string(),
            native_started: false,
            scope: caller.scope.clone(),
            fork_from: None,
            seed: None,
            automatic_title: false,
            delivered_seq: 0,
            halt: None,
        };
        let id = record.summary.id.clone();
        if self.paused_in(&trees, &root) {
            return refuse(OrchCode::Paused, super::PAUSED);
        }
        if let Some(max) = limits.max_starts
            && super::reached(Some(max), trees.get(&root).map_or(0, |tree| tree.starts) as usize)
        {
            return refuse(
                OrchCode::StartsLimit,
                format!("This crew reached its limit of {max} starts. Write to its lead to continue."),
            );
        }
        let identity = self.inherited_sync_identity(&caller);
        let children = self.store().replace_for_handoff(&caller, &record, identity.as_ref())?;
        let key = if root == caller_id {
            let counters = trees.remove(&root).unwrap_or_default();
            trees.insert(id.clone(), counters);
            id
        } else {
            root
        };
        trees.entry(key).or_default().starts += 1;
        Ok((record, caller, children))
    }

    // Let the current tool return and the agent finish its turn. Reap the owner before reusing its checkout,
    // including when a background agent outlives the turn or the provider never produces a turn end.
    async fn finish_archived_run(&self, thread_id: &str) -> crate::Result<()> {
        let Some(live) = self.live(thread_id) else {
            return Ok(());
        };
        let until = tokio::time::Instant::now() + TURN_END_WAIT;
        while !live.state().turns.is_empty() && tokio::time::Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.stop_live(&live, crate::StopReason::Stop, None);
        let mut exited = live.exited.clone();
        tokio::time::timeout(TURN_END_WAIT, exited.wait_for(|done| *done))
            .await
            .map_err(|_| "The previous session still owns its checkout.".to_string())?
            .map(|_| ())
            .map_err(|_| "Nexus lost track of the previous session.".to_string())
    }

    /// Archives session `caller_id` itself (`target` None), or a session it started, which stops first, with no halt.
    /// The person can restore either. True when the archived session was running.
    ///
    /// Refused: not in this organization (`org_mismatch`); orchestration off (`off`); not a session it started
    /// (`not_your_session`).
    pub fn archive_session(&self, caller_id: &str, target: Option<&str>) -> Result<bool, OrchError> {
        let caller = self.orchestrating_parent(caller_id)?;
        let id = match target.filter(|target| *target != caller_id) {
            None => caller.summary.id.clone(),
            Some(target) => {
                let child = self.direct_child(caller_id, target)?;
                if !self.in_scope(&child.scope) {
                    return refuse(
                        OrchCode::OrgMismatch,
                        "That session is not in the organization Nexus is signed in to.",
                    );
                }
                child.summary.id
            }
        };
        let running = self.is_live(&id);
        if id != caller_id {
            self.stop_tree(&id, None)?;
        }
        self.update(
            &id,
            ThreadPatch {
                archived: Some(true),
                ..ThreadPatch::default()
            },
        )?;
        if id == caller_id && running {
            let this = self.clone();
            tokio::spawn(async move {
                if let Err(error) = this.finish_archived_run(&id).await {
                    eprintln!("nexus-workbench: {id}: {error}");
                }
            });
        }
        Ok(running)
    }
}
