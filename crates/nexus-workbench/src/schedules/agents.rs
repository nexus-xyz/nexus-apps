//! Schedules an agent keeps through the app's tools (`schedule_session`, `list_schedules`, `cancel_schedule` in
//! [`crate::orchestra::tools`]). An agent schedules on its own session's project, with its own route (the person's own
//! sign-in for an agent Gateway has no route for), and never above its own permission mode or effort. It lists the project's schedules, and cancels only those an agent added: the
//! person's own stay theirs. The person sees and edits all of them in Settings.

use super::{ScheduleInput, ScheduleView, When};
use crate::Workbench;
use crate::event::{PermissionMode, SessionAgent};
use crate::linked::{OrchCode, OrchError, child_via, cross_agent_refusal, effort_across, mode_refusal};

/// What an agent asks for. Absent fields take the calling session's.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSchedule {
    pub prompt: String,
    pub title: Option<String>,
    pub when: When,
    pub agent: Option<SessionAgent>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub max_runs_per_day: Option<u32>,
    pub catch_up: Option<bool>,
}

impl Workbench {
    /// The calling session, in the signed-in organization.
    fn caller_summary(&self, caller: &str) -> Result<crate::event::ThreadSummary, OrchError> {
        self.reader()
            .thread(caller)?
            .filter(|record| self.in_scope(&record.scope))
            .map(|record| record.summary)
            .ok_or_else(|| {
                OrchError::new(
                    OrchCode::OrgMismatch,
                    "That session is not in the organization Nexus is signed in to.",
                )
            })
    }

    /// Adds a schedule on the caller's project, as the caller's agent asked. A schedule for another agent takes that
    /// agent's default model, the nearest effort it has under the caller's ([`effort_across`]), and its own sign-in
    /// when Gateway has no route for it ([`child_via`]). With "Enable cross-agent subsessions" off, another agent is
    /// refused.
    pub fn schedule_for(&self, caller: &str, request: AgentSchedule) -> Result<ScheduleView, OrchError> {
        let summary = self.caller_summary(caller)?;
        let agent = request.agent.unwrap_or(summary.agent);
        let same_agent = agent == summary.agent;
        if !same_agent && self.config().cross_agent_off {
            return Err(OrchError::new(
                OrchCode::AgentNotAllowed,
                cross_agent_refusal(summary.agent, "schedules only {agent} sessions"),
            ));
        }
        let mode = request
            .permission_mode
            .unwrap_or(summary.permission_mode)
            .lower(summary.permission_mode);
        if let Some(refusal) = mode_refusal(agent, mode, summary.permission_mode) {
            return Err(OrchError::new(OrchCode::BadInput, refusal));
        }
        let input = ScheduleInput {
            project_id: summary.project_id.clone(),
            title: request.title,
            prompt: request.prompt,
            agent,
            // Another agent's model is not the caller's: a Claude model would not run on Codex.
            model: request.model.or_else(|| summary.model.clone().filter(|_| same_agent)),
            permission_mode: mode,
            effort: if same_agent {
                summary.effort
            } else {
                effort_across(agent, None, summary.effort)
            },
            via: child_via(agent, summary.via),
            worktree: true,
            when: request.when,
            enabled: true,
            max_runs_per_day: request.max_runs_per_day,
            catch_up: request.catch_up.unwrap_or(true),
        };
        self.save_schedule(None, input, Some(caller.to_string()))
            .map_err(|message| OrchError::new(OrchCode::BadInput, message))
    }

    /// The schedules on the caller's project.
    pub fn schedules_for(&self, caller: &str) -> Result<Vec<ScheduleView>, OrchError> {
        let project = self.caller_summary(caller)?.project_id;
        Ok(self
            .schedules()?
            .into_iter()
            .filter(|view| view.schedule.project_id == project)
            .collect())
    }

    /// Deletes a schedule an agent added on the caller's project.
    pub fn unschedule_for(&self, caller: &str, id: &str) -> Result<(), OrchError> {
        let project = self.caller_summary(caller)?.project_id;
        let schedule = self
            .schedule_in_scope(id)
            .ok()
            .filter(|schedule| schedule.project_id == project)
            .ok_or_else(|| OrchError::new(OrchCode::BadInput, "No such schedule on this project."))?;
        if schedule.created_by.is_none() {
            return Err(OrchError::new(
                OrchCode::NotYourSession,
                "The person added that schedule: only they can change it, in Settings.",
            ));
        }
        Ok(self.delete_schedule(id)?)
    }
}
