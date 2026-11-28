//! Cross-project subsessions: `start_session` with `project` starts a linked subsession in another project open on this
//! computer. The child keeps its parent (`parent_thread_id`), so results, waits, messages, stops, the halt cascade, the
//! spend cap and the depth limit work as for any child. It is a leaf: it gets no subsession tools and can't hand off,
//! so one agent's choice of project never reaches further.
//!
//! The agent picks the project, not the person, and the project's setup script runs in the child's worktree. So the
//! project must be trusted (`claude_trust`), whatever the agent, and the person can turn the feature off in Settings.

use super::{OrchCode, OrchError, refuse};
use crate::Workbench;
use crate::event::{PermissionMode, Project, ThreadSummary};
use crate::project_lookup::{find_project, open_project};

/// Why a cross-project subsession can't start one of its own.
pub(crate) const NO_START: &str =
    "This subsession runs in another project than its parent's, so it can't start subsessions of its own.";

/// Why a cross-project subsession can't hand off.
pub(crate) const NO_HANDOFF: &str =
    "This subsession runs in another project than its parent's, so it can't hand off its work.";

/// Why turning orchestration on is refused for a cross-project subsession.
pub(crate) const NO_ORCHESTRATION: &str =
    "This session runs in another project than its parent's, so its agent can't start sessions of its own.";

/// Why another project is refused while the person turned the feature off.
const OFF: &str = "The person turned off \"Enable cross-project subsessions\", so this session starts subsessions only \
                   in its own project. The person can turn it on in Settings › Code › Subsessions.";

/// Why a project nobody trusted is refused.
fn untrusted(project: &Project) -> String {
    format!(
        "The project {name} is not trusted in Nexus, so a subsession can't start there. The person can trust it in \
         Settings › Computer › Projects › {name}: turn on \"Trust this project's Claude and OpenCode settings\".",
        name = project.name
    )
}

/// Where a child runs: its project, whether that is another than its parent's, and the mode the project's defaults
/// allow there (None: no default).
pub(crate) struct ChildProject {
    pub project: Project,
    pub cross: bool,
    pub mode: Option<PermissionMode>,
}

impl Workbench {
    /// The project a child of `parent` runs in: the one `asked` names, else the parent's. The parent's own project
    /// named is an ordinary child. Another project needs the feature on (`cross_project_off`), checked first so the
    /// refusal names no other project's folder; it must be open and in sight (`project_not_open`) and trusted
    /// (`project_untrusted`).
    pub(crate) fn child_project(&self, parent: &ThreadSummary, asked: Option<&str>) -> Result<ChildProject, OrchError> {
        let own = self
            .store()
            .project(&parent.project_id)?
            .ok_or_else(|| "This session's project is no longer open.".to_string())?;
        let own = ChildProject {
            project: own,
            cross: false,
            mode: None,
        };
        let Some(asked) = asked.map(str::trim).filter(|asked| !asked.is_empty()) else {
            return Ok(own);
        };
        if !self.config().cross_project_on {
            // Only the parent's own project is looked at.
            return match find_project(std::slice::from_ref(&own.project), asked) {
                Ok(_) => Ok(own),
                Err(_) => refuse(OrchCode::CrossProjectOff, OFF),
            };
        }
        let projects = self.store().projects()?;
        let project = match open_project(&projects, asked) {
            Ok(project) => project.clone(),
            Err(message) => return refuse(OrchCode::ProjectNotOpen, message),
        };
        if project.id == parent.project_id {
            return Ok(own);
        }
        if self.store().claude_trust(&project.id)? != Some(true) {
            return refuse(OrchCode::ProjectUntrusted, untrusted(&project));
        }
        let mode = self.project_defaults(&project.id)?.mode;
        Ok(ChildProject {
            project,
            cross: true,
            mode,
        })
    }

    /// Why child `id` must not start in `project`: the project was removed from Nexus, or the child was archived (a
    /// removal archives its project's sessions). None when it may start. Its setup reads this before it makes a
    /// worktree, so a child admitted just before a removal never works out of sight.
    pub(crate) fn start_gone(&self, id: &str, project: &Project) -> crate::Result<Option<String>> {
        if self.store().project(&project.id)?.is_none() {
            return Ok(Some(format!(
                "The project {} was removed from Nexus, so this subsession did not start.",
                project.name
            )));
        }
        let archived = self.reader().thread(id)?.is_some_and(|record| record.summary.archived);
        Ok(archived.then(|| "This subsession was archived before it started.".to_string()))
    }

    /// Removes the project's row and archives its sessions under the trees' lock, which admits children: a start
    /// that names the project either ends before (its child is archived, then stopped by
    /// [`Workbench::stop_cross_project_children`]) or finds no project.
    pub(crate) fn remove_project_row(&self, project_id: &str) -> crate::Result<()> {
        let _admissions = self.trees();
        self.store().remove_project(project_id)
    }

    /// Stops each active subsession in this organization that a session in another project started in `project_id`,
    /// once the person removed that project. Its parent then gets a stopped result and waits no longer on a session out of sight.
    pub(crate) fn stop_cross_project_children(&self, project_id: &str) {
        let linked = match self.store().linked_threads() {
            Ok(linked) => linked,
            Err(error) => {
                eprintln!("nexus-workbench: {project_id}: {error}");
                return;
            }
        };
        let active = linked
            .into_iter()
            .filter(|record| record.summary.cross_project && record.summary.project_id == project_id)
            // Another organization's sessions are not this workbench's to stop.
            .filter(|record| self.in_scope(&record.scope))
            .filter(|record| self.active(record));
        for record in active {
            if let Err(error) = self.stop(&record.summary.id) {
                eprintln!("nexus-workbench: {}: {error}", record.summary.id);
            }
        }
    }
}

#[cfg(test)]
#[path = "cross_project_tests.rs"]
mod tests;
