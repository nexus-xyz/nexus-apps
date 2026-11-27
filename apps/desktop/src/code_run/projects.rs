//! Settings' commands on the projects (the Nexus app's `wb_project_*`, `wb_session_start_preference*` and
//! `wb_worktree_cleanup_run`): a project's setup script, its commands, its pin and archive, its session-start choice,
//! and Clean up now. Only the runner hands one out, so only the store's owner changes them. The rules are
//! `nexus_workbench`'s, shared with the Nexus app.

use std::sync::Arc;

use nexus_desktop_core::settings::Settings;
use nexus_remote::cleanup::LastCleanup;
use nexus_workbench::claude_trust::ClaudeTrust;
use nexus_workbench::project_defaults::ProjectDefaults;
use nexus_workbench::scripts::ProjectScript;

use super::{Answer, GpuiHost, Sweeps, ask, sweep_once};
use nexus_remote::engine::Engine;

/// The project's new-session defaults as the store on this computer keeps them, for the New session form. Read from
/// the store, not the runner, so it answers whichever app owns it; none for a server's project or a store that does
/// not open.
pub fn project_defaults(project: &str) -> ProjectDefaults {
    let Ok(Some(store)) = crate::local_store::open() else {
        return ProjectDefaults::default();
    };
    store
        .project_defaults(project)
        .map(nexus_workbench::project_defaults::read)
        .unwrap_or_default()
}

#[derive(Clone)]
pub struct Projects {
    pub(super) api: Engine,
    pub(super) runtime: tokio::runtime::Handle,
    pub(super) host: Arc<GpuiHost>,
    pub(super) sweeps: Arc<Sweeps>,
    pub(super) settings: Arc<dyn Fn() -> Settings + Send + Sync>,
}

impl Projects {
    pub fn phone_invitation_begin(&self) -> Answer<nexus_workbench::computer::Invitation> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.phone_invitation_begin().await
        })
    }
    pub fn phone_invitation_poll(&self, invite_id: String) -> Answer<nexus_workbench::computer::LinkStatus> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.phone_invitation_poll(invite_id).await
        })
    }
    pub fn phone_invitation_approve(
        &self,
        invite_id: String,
        device_id: String,
        code: String,
    ) -> Answer<nexus_workbench::computer::LinkStatus> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.phone_invitation_approve(invite_id, device_id, code).await
        })
    }
    pub fn phone_invitation_cancel(&self, invite_id: String) -> Answer<()> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.phone_invitation_cancel(invite_id).await
        })
    }
    pub fn computer_scope(&self) -> Option<(String, String)> {
        let marker = nexus_desktop_core::secrets::marker(&self.host.token()?);
        Some((self.host.scope(), marker))
    }

    pub fn computer_info(&self) -> Answer<nexus_workbench::computer::Info> {
        ask(&self.api, &self.runtime, |api| async move { api.computer_info().await })
    }

    pub fn computer_join_begin(&self, link: String) -> Answer<nexus_workbench::computer::JoinStatus> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.computer_join_begin(link).await
        })
    }

    pub fn computer_join_poll(&self, invite_id: String) -> Answer<nexus_workbench::computer::JoinStatus> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.computer_join_poll(invite_id).await
        })
    }

    pub fn computer_join_allow(&self, invite_id: String) -> Answer<nexus_workbench::session_sync::control::Settings> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.computer_join_allow(invite_id).await
        })
    }

    pub fn computer_list(&self) -> Answer<serde_json::Value> {
        ask(&self.api, &self.runtime, |api| async move { api.computer_list().await })
    }

    pub fn computer_request(
        &self,
        computer_id: String,
        request_id: String,
        operation: nexus_workbench::computer::Operation,
    ) -> Answer<nexus_workbench::computer::CallResult> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.computer_request(computer_id, request_id, operation).await
        })
    }

    pub fn computer_request_get(
        &self,
        computer_id: String,
        request_id: String,
    ) -> Answer<nexus_workbench::computer::CallResult> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.computer_request_get(computer_id, request_id).await
        })
    }

    pub fn synced_thread_page(&self, thread_id: String) -> Answer<serde_json::Value> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.synced_thread_page(thread_id, None, None, Some(100)).await
        })
    }

    pub fn synced_command(
        &self,
        thread_id: String,
        request_id: String,
        action: nexus_workbench::session_sync::control::Action,
    ) -> Answer<serde_json::Value> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.synced_command(thread_id, request_id, action).await
        })
    }

    pub fn synced_command_get(&self, thread_id: String, request_id: String) -> Answer<serde_json::Value> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.synced_command_get(thread_id, request_id).await
        })
    }

    pub fn code_control(&self) -> Answer<nexus_workbench::session_sync::control::Settings> {
        ask(
            &self.api,
            &self.runtime,
            move |api| async move { api.code_control().await },
        )
    }

    pub fn set_code_control(&self, enabled: bool) -> Answer<nexus_workbench::session_sync::control::Settings> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.set_code_control(enabled).await
        })
    }

    /// Set the setup script (blank takes it away); answers with what was saved.
    pub fn set_setup(&self, project: String, script: String) -> Answer<Option<String>> {
        let script = Some(script.trim().to_string()).filter(|script| !script.is_empty());
        ask(&self.api, &self.runtime, move |api| async move {
            api.project_setup_set(project, script).await
        })
    }

    /// Replace the commands; the workbench refuses a list it could not run and says why.
    pub fn set_scripts(&self, project: String, scripts: Vec<ProjectScript>) -> Answer<Vec<ProjectScript>> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.project_scripts_set(project, scripts).await
        })
    }

    /// Pin or unpin, archive or bring back; None leaves that one as it is.
    pub fn update(&self, project: String, pinned: Option<bool>, archived: Option<bool>) -> Answer<()> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.project_update(project, pinned, archived).await
        })
    }

    pub fn session_start_overrides(&self, clear: bool) -> Answer<u32> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.session_start_overrides(clear).await
        })
    }

    /// The project's new-session defaults.
    pub fn defaults(&self, project: String) -> Answer<ProjectDefaults> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.project_defaults(project).await
        })
    }

    /// Set them; all unset takes them away.
    pub fn set_defaults(&self, project: String, defaults: ProjectDefaults) -> Answer<ProjectDefaults> {
        ask(&self.api, &self.runtime, move |api| async move {
            api.project_defaults_set(project, defaults).await
        })
    }

    /// Whether the person trusts the project's own Claude settings, and what they would turn on.
    pub fn claude_trust(&self, project: String) -> Answer<ClaudeTrust> {
        ask(&self.api, &self.runtime, move |api| async move {
            let value = api
                .call("wb_project_claude_trust", serde_json::json!({ "projectId": project }))
                .await?;
            serde_json::from_value(value).map_err(|error| error.to_string())
        })
    }

    /// Trust them or not; answers with what was saved.
    pub fn set_claude_trust(&self, project: String, trusted: bool) -> Answer<ClaudeTrust> {
        ask(&self.api, &self.runtime, move |api| async move {
            let args = serde_json::json!({ "projectId": project, "trusted": trusted });
            let value = api.call("wb_project_claude_trust_set", args).await?;
            serde_json::from_value(value).map_err(|error| error.to_string())
        })
    }

    /// The agents on this computer and their models, for the defaults' model choice.
    pub fn agent_models(&self) -> Answer<Vec<nexus_workbench::api::AgentInfo>> {
        ask(
            &self.api,
            &self.runtime,
            move |api| async move { api.agent_models().await },
        )
    }

    /// Clean up now: a sweep with the rules saved, here and on each connected server (the Nexus app's
    /// `wb_worktree_cleanup_run`).
    pub fn clean_up_now(&self) -> Answer<LastCleanup> {
        let (host, sweeps, settings) = (self.host.clone(), self.sweeps.clone(), self.settings.clone());
        ask(&self.api, &self.runtime, move |api| async move {
            let rules = tokio::task::spawn_blocking(move || settings().worktree_cleanup.unwrap_or_default())
                .await
                .unwrap_or_default();
            if !rules.any() {
                return Err("Turn on a cleanup rule first.".into());
            }
            Ok(sweep_once(&api, &host, &sweeps, &rules).await)
        })
    }

    /// Every worktree on this computer: the sessions' and the ones no session names (`wb_worktrees`).
    pub fn worktrees(&self) -> Answer<Vec<nexus_workbench::cleanup::Listed>> {
        ask(
            &self.api,
            &self.runtime,
            move |api| async move { api.worktrees().await },
        )
    }

    /// Takes one listed worktree off the disk, unless that would lose work: a session's (`wb_worktree_remove`, the
    /// session stays, readable), or one no session names (`wb_worktree_remove_orphan`). Branches stay.
    pub fn remove_worktree(&self, listed: nexus_workbench::cleanup::Listed) -> Answer<()> {
        ask(&self.api, &self.runtime, move |api| async move {
            match listed.thread_id {
                Some(thread_id) => api.worktree_remove(&thread_id).await.map(|_| ()),
                None => api.worktree_remove_orphan(&listed.path).await,
            }
        })
    }

    /// What the last sweep did, by the hour or by Clean up now; None before the first.
    pub fn last_cleanup(&self) -> Option<LastCleanup> {
        self.sweeps.last()
    }
}
