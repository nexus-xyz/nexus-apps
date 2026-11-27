//! Where a project's settings live, for Settings › Worktrees and the project editor (`project_editor.rs`): the Code
//! sessions' store on this computer, through the runner's [`Projects`], or a server's host, through its relay
//! (`wb_project_*` over `machines.call`). Both answer the same calls with the same rules (`nexus_workbench::api`).

use std::sync::Arc;

use futures::future::BoxFuture;
use nexus_workbench::api::AgentInfo;
use nexus_workbench::claude_trust::ClaudeTrust;
use nexus_workbench::project_defaults::ProjectDefaults;
use nexus_workbench::scripts::ProjectScript;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::code_run::{Answer, Projects};
use crate::machines::Machines;

/// A call's answer, or why it failed in words for the page.
pub type Reply<T> = BoxFuture<'static, Result<T, String>>;

const STOPPED: &str = "The Code sessions stopped before they answered.";
const LOST: &str = "The server's answer was lost.";
/// What an older host's refusal of a call it does not know reads as.
pub const OLDER_HOST: &str = "This server's Nexus host can't change this. Update it in Settings › Servers.";

/// The project's home: this computer's store, or server `machine`.
#[derive(Clone)]
pub enum ProjectHost {
    Here(Projects),
    Server { machines: Arc<Machines>, machine: String },
}

fn here<T: Send + 'static>(answer: Answer<T>) -> Reply<T> {
    Box::pin(async move { answer.await.unwrap_or_else(|_| Err(STOPPED.into())) })
}

/// A host's error as the page says it: an unknown call is an older host.
fn plain(error: String) -> String {
    if error.starts_with("Unknown command") {
        OLDER_HOST.into()
    } else {
        crate::machine_view::plain_error(&error)
    }
}

/// `call` on server `machine`, its answer read as `T`.
pub fn on_server<T: DeserializeOwned + Send + 'static>(
    machines: &Machines,
    machine: &str,
    call: &'static str,
    args: Value,
) -> Reply<T> {
    let answered = machines.call(machine, call, args);
    Box::pin(async move {
        let value = answered.await.map_err(|_| LOST.to_string())?.map_err(plain)?;
        serde_json::from_value(value).map_err(|error| format!("The server answered something else: {error}"))
    })
}

impl ProjectHost {
    fn there<T: DeserializeOwned + Send + 'static>(&self, call: &'static str, args: Value) -> Option<Reply<T>> {
        match self {
            ProjectHost::Here(_) => None,
            ProjectHost::Server { machines, machine } => Some(on_server(machines, machine, call, args)),
        }
    }

    /// The same call here or there; `here` is the runner's.
    fn ask<T: DeserializeOwned + Send + 'static>(
        &self,
        call: &'static str,
        args: Value,
        local: impl FnOnce(&Projects) -> Answer<T>,
    ) -> Reply<T> {
        match self {
            ProjectHost::Here(projects) => here(local(projects)),
            ProjectHost::Server { .. } => self
                .there(call, args)
                .unwrap_or_else(|| Box::pin(async { Err(LOST.into()) })),
        }
    }

    /// The same call, whose answer the page does not need.
    fn ask_unit(&self, call: &'static str, args: Value, local: impl FnOnce(&Projects) -> Answer<()>) -> Reply<()> {
        match self {
            ProjectHost::Here(projects) => here(local(projects)),
            ProjectHost::Server { .. } => {
                let reply = self.ask::<Value>(call, args, |_| unreachable!("a server's call"));
                Box::pin(async move { reply.await.map(drop) })
            }
        }
    }

    pub fn defaults(&self, project: String) -> Reply<ProjectDefaults> {
        let args = json!({ "projectId": project });
        self.ask("wb_project_defaults", args, |projects| projects.defaults(project))
    }

    pub fn set_defaults(&self, project: String, defaults: ProjectDefaults) -> Reply<ProjectDefaults> {
        let args = json!({ "projectId": project, "defaults": defaults });
        self.ask("wb_project_defaults_set", args, |projects| {
            projects.set_defaults(project, defaults)
        })
    }

    /// The agents where the project's sessions run, with their models.
    pub fn agent_models(&self) -> Reply<Vec<AgentInfo>> {
        self.ask("wb_agent_models", json!({}), Projects::agent_models)
    }

    pub fn update(&self, project: String, pinned: Option<bool>, archived: Option<bool>) -> Reply<()> {
        let args = json!({ "projectId": project, "pinned": pinned, "archived": archived });
        self.ask_unit("wb_project_update", args, |projects| {
            projects.update(project, pinned, archived)
        })
    }

    /// Set the setup script (blank takes it away); answers with what was saved.
    pub fn set_setup(&self, project: String, script: String) -> Reply<Option<String>> {
        let kept = Some(script.trim().to_string()).filter(|script| !script.is_empty());
        let args = json!({ "projectId": project, "script": kept });
        self.ask("wb_project_setup_set", args, |projects| {
            projects.set_setup(project, script)
        })
    }

    pub fn set_scripts(&self, project: String, scripts: Vec<ProjectScript>) -> Reply<Vec<ProjectScript>> {
        let args = json!({ "projectId": project, "scripts": scripts });
        self.ask("wb_project_scripts_set", args, |projects| {
            projects.set_scripts(project, scripts)
        })
    }

    /// Whether the person trusts the project's own Claude settings, and what they would turn on.
    pub fn claude_trust(&self, project: String) -> Reply<ClaudeTrust> {
        let args = json!({ "projectId": project });
        self.ask("wb_project_claude_trust", args, |projects| {
            projects.claude_trust(project)
        })
    }

    /// Trust them or not; answers with what was saved.
    pub fn set_claude_trust(&self, project: String, trusted: bool) -> Reply<ClaudeTrust> {
        let args = json!({ "projectId": project, "trusted": trusted });
        self.ask("wb_project_claude_trust_set", args, |projects| {
            projects.set_claude_trust(project, trusted)
        })
    }
}

#[cfg(test)]
#[path = "project_host_tests.rs"]
mod tests;
