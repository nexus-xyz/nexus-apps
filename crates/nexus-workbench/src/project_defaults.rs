//! A project's own defaults for a new session (Settings › Projects): the agent and model, the permission mode, and
//! folder or worktree. Each one unset follows what the person last picked on the New session screen. They win over
//! the remembered picks for that project only, as the Nexus app's `workbench/session-defaults.ts` says.
//!
//! Kept in the store (`project_defaults`), so every app on this computer that runs Code sessions reads the same ones.
//! A row that no longer reads (written by a later app) is no defaults, not a broken screen.

use serde::{Deserialize, Serialize};

use crate::event::{PermissionMode, SessionAgent};
use crate::{Result, Workbench};

/// Where a new session works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Workspace {
    /// The project's own folder.
    Folder,
    /// A new worktree of it.
    Worktree,
}

/// A project's defaults, in the shape the Nexus app keeps (`ProjectDefaults`). `model` goes with `agent`: a Claude
/// model is no use to Codex, so it is kept only with an agent. None in `model` is the organization's default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<SessionAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
}

impl ProjectDefaults {
    /// As kept: no model without an agent, and a blank model is the organization's default.
    pub fn clean(mut self) -> ProjectDefaults {
        self.model = self
            .agent
            .and(self.model)
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty());
        self
    }

    /// Nothing set: every choice follows the remembered pick.
    pub fn is_empty(&self) -> bool {
        self.agent.is_none() && self.mode.is_none() && self.workspace.is_none()
    }

    /// The model `agent` starts on, and whether the remembered effort still goes with it: the project's model when
    /// its default is this agent, else `remembered` (the one last picked for the agent). The Nexus app's `modelFor`.
    pub fn model_for(&self, agent: SessionAgent, remembered: Option<String>) -> (Option<String>, bool) {
        if self.agent != Some(agent) || self.model == remembered {
            return (remembered, true);
        }
        (self.model.clone(), false)
    }
}

/// Defaults from their stored JSON; none when there is no row or it does not read.
pub fn read(json: Option<String>) -> ProjectDefaults {
    json.and_then(|json| serde_json::from_str::<ProjectDefaults>(&json).ok())
        .map(ProjectDefaults::clean)
        .unwrap_or_default()
}

impl Workbench {
    /// The project's defaults; none when it has none, or its row does not read.
    pub fn project_defaults(&self, project_id: &str) -> Result<ProjectDefaults> {
        Ok(read(self.store().project_defaults(project_id)?))
    }

    /// Sets them ([`ProjectDefaults::clean`]; all unset takes them away) and answers with what was saved.
    pub fn set_project_defaults(&self, project_id: &str, defaults: ProjectDefaults) -> Result<ProjectDefaults> {
        let defaults = defaults.clean();
        let store = self.store();
        if store.project(project_id)?.is_none() {
            return Err("That project is no longer open.".into());
        }
        let json = if defaults.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&defaults).map_err(|e| e.to_string())?)
        };
        store.set_project_defaults(project_id, json.as_deref())?;
        Ok(defaults)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_model_only_with_its_agent() {
        let lone = ProjectDefaults {
            model: Some("gpt-5".into()),
            ..Default::default()
        };
        assert_eq!(lone.clean(), ProjectDefaults::default());
        let blank = ProjectDefaults {
            agent: Some(SessionAgent::Claude),
            model: Some(" ".into()),
            ..Default::default()
        };
        assert_eq!(blank.clean().model, None);
    }

    #[test]
    fn reads_the_nexus_apps_shape() {
        let read: ProjectDefaults =
            serde_json::from_str(r#"{"agent":"codex","model":"gpt-5","mode":"acceptEdits","workspace":"worktree"}"#)
                .expect("reads");
        assert_eq!(read.agent, Some(SessionAgent::Codex));
        assert_eq!(read.mode, Some(PermissionMode::AcceptEdits));
        assert_eq!(read.workspace, Some(Workspace::Worktree));
        assert_eq!(
            serde_json::to_string(&ProjectDefaults::default()).expect("writes"),
            "{}"
        );
    }

    #[test]
    fn the_projects_model_wins_for_its_own_agent() {
        let defaults = ProjectDefaults {
            agent: Some(SessionAgent::Claude),
            model: Some("opus".into()),
            ..Default::default()
        };
        assert_eq!(
            defaults.model_for(SessionAgent::Claude, Some("sonnet".into())),
            (Some("opus".into()), false)
        );
        assert_eq!(
            defaults.model_for(SessionAgent::Claude, Some("opus".into())),
            (Some("opus".into()), true)
        );
        assert_eq!(
            defaults.model_for(SessionAgent::Codex, Some("gpt-5".into())),
            (Some("gpt-5".into()), true)
        );
    }

    #[test]
    fn the_store_keeps_and_drops_them() {
        let store = crate::store::Store::in_memory().expect("a store");
        let dir = std::env::temp_dir();
        let project = store.add_project(&dir, 1).expect("a project");
        store
            .set_project_defaults(&project.id, Some(r#"{"mode":"plan"}"#))
            .expect("saved");
        assert_eq!(
            read(store.project_defaults(&project.id).expect("read")).mode,
            Some(PermissionMode::Plan)
        );
        // A row a later app wrote, that this one can't read, is no defaults.
        store
            .set_project_defaults(&project.id, Some(r#"{"mode":"telepathy"}"#))
            .expect("saved");
        assert_eq!(
            read(store.project_defaults(&project.id).expect("read")),
            ProjectDefaults::default()
        );
        store.remove_project(&project.id).expect("removed");
        assert_eq!(store.project_defaults(&project.id), Ok(None));
    }
}
