//! Cross-project subsessions: `start_session` with `project` starts a linked subsession in another project open on this
//! computer. The child keeps its parent (`parent_thread_id`), so results, waits, messages, stops, the halt cascade, the
//! spend cap and the depth limit work as for any child. It may start subsessions of its own; depth keeps counting from
//! the tree's root. It can't hand off yet.
//!
//! The agent picks the project, not the person, and the project's setup script runs in the child's worktree. So the
//! project must be trusted (`claude_trust`), whatever the agent, and the person can turn the feature off in Settings.
//! Each hop is checked again: a child in project B that names project C is checked against C. A child in B that
//! starts one in B is an ordinary child of its parent, with `cross_project` false.

use super::{OrchCode, OrchError, refuse};
use crate::Workbench;
use crate::event::{PermissionMode, Project, ThreadSummary};
use crate::project_lookup::{find_project, in_sight, open_project};

/// Why a cross-project subsession can't hand off.
pub(crate) const NO_HANDOFF: &str =
    "This subsession runs in another project than its parent's, so it can't hand off its work.";

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
    ///
    /// `root_project` is the project of the tree's root. A parent outside it got there by a cross-project start, so a
    /// child in the parent's own project gets the same three checks: the person may have turned the setting off,
    /// archived the project or stopped trusting it since, and the child's worktree runs that project's setup script.
    pub(crate) fn child_project(
        &self,
        parent: &ThreadSummary,
        asked: Option<&str>,
        root_project: &str,
    ) -> Result<ChildProject, OrchError> {
        let own = self
            .store()
            .project(&parent.project_id)?
            .ok_or_else(|| "This session's project is no longer open.".to_string())?;
        let own = ChildProject {
            project: own,
            cross: false,
            mode: None,
        };
        let outside_root = parent.project_id != root_project;
        let stay = |own: ChildProject| {
            if outside_root {
                self.check_hop(&own.project)?;
            }
            Ok(own)
        };
        let Some(asked) = asked.map(str::trim).filter(|asked| !asked.is_empty()) else {
            return stay(own);
        };
        if !self.config().cross_project_on {
            // Only the parent's own project is looked at.
            return match find_project(std::slice::from_ref(&own.project), asked) {
                Ok(_) => stay(own),
                Err(_) => refuse(OrchCode::CrossProjectOff, OFF),
            };
        }
        let projects = self.store().projects()?;
        let project = match open_project(&projects, asked) {
            Ok(project) => project.clone(),
            Err(message) => return refuse(OrchCode::ProjectNotOpen, message),
        };
        if project.id == parent.project_id {
            return stay(own);
        }
        self.check_hop(&project)?;
        let mode = self.project_defaults(&project.id)?.mode;
        Ok(ChildProject {
            project,
            cross: true,
            mode,
        })
    }

    /// The checks every start outside the root's project passes: the feature on (`cross_project_off`), the project
    /// in sight (`project_not_open`) and trusted (`project_untrusted`).
    fn check_hop(&self, project: &Project) -> Result<(), OrchError> {
        if !self.config().cross_project_on {
            return refuse(OrchCode::CrossProjectOff, OFF);
        }
        if let Err(message) = in_sight(project) {
            return refuse(OrchCode::ProjectNotOpen, message);
        }
        if self.store().claude_trust(&project.id)? != Some(true) {
            return refuse(OrchCode::ProjectUntrusted, untrusted(project));
        }
        Ok(())
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
                eprintln!("nexus-workbench: could not read the subsessions of a removed project: {error}");
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
