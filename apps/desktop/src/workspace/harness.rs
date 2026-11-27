//! New-session harness readiness and the native Nexus installer.
use gpui_kit::prelude::*;
use nexus_workbench::api::AgentInfo;
use nexus_workbench::event::SessionAgent;

use super::{View, Workspace};

pub(super) struct RemoteHarness {
    machine: String,
    agents: Vec<AgentInfo>,
    error: Option<String>,
    checking: bool,
    /// Distinguishes requests when navigation returns to the same server before an earlier reply arrives.
    check_started: Option<std::time::Instant>,
}

impl RemoteHarness {
    fn waiting(&self) -> bool {
        self.checking && self.agents.is_empty()
    }

    fn accepts_reply(&self, machine: &str, started: std::time::Instant) -> bool {
        self.machine == machine && self.check_started == Some(started)
    }
}

/// Why an agent can't run sessions where a new session starts, or that it is ready: the model panel's right pane
/// shows it for a row under the models (`missing_agents.rs`).
pub(super) fn readiness(found: Option<&AgentInfo>, where_: &str) -> String {
    match found {
        None => "Checking…".into(),
        Some(info) if info.path.is_none() && info.problem.is_some() => "Installed, but can't run sessions".into(),
        Some(info) if info.path.is_none() => format!("Not installed on {where_}"),
        Some(info) => info.version.as_ref().map_or_else(
            || "Installed".into(),
            |version| format!("Installed v{}", version.trim_start_matches('v')),
        ),
    }
}

/// Whether `agent` can run on the selected target (`remote` = Cloud or a remote server, `false` = this computer).
pub(super) fn offers_agent_on(agent: SessionAgent, remote: bool) -> bool {
    if remote {
        !nexus_workbench::harness::harness(agent).session_features().local_only
    } else {
        agent.supported_on_current_platform()
    }
}

impl Workspace {
    pub(super) fn harness_machine(&self) -> Option<&str> {
        self.code_form
            .project
            .as_ref()
            .and_then(|id| self.projects.iter().find(|project| &project.id == id))
            .and_then(|project| project.machine.as_deref())
    }

    /// Whether a new session here can run `agent`. On a cloud or remote machine, any agent that is not `local_only`
    /// is offered even when the local OS does not run it directly (e.g. OpenCode on macOS targeting Cloud). Locally,
    /// the agent must be supported on this computer's platform.
    pub(super) fn offers_agent(&self, agent: SessionAgent) -> bool {
        offers_agent_on(agent, self.cloud_ui.on || self.harness_machine().is_some())
    }

    /// The picker follows the open session's machine, or the new session's selected project.
    pub(super) fn picker_machine(&self) -> Option<String> {
        let session = match &self.view {
            View::Code(view) => Some(
                self.code_session()
                    .and_then(|session| session.machine.clone())
                    .or_else(|| self.machine_source.machine_of(&view.id)),
            ),
            View::NewCode => None,
            _ => return None,
        };
        super::code_picker::target_machine(session.is_some(), session.flatten(), self.harness_machine())
    }

    pub(super) fn picker_harness_info(&self, agent: SessionAgent) -> Option<&AgentInfo> {
        match self.picker_machine() {
            Some(machine) => self
                .code_form
                .harness
                .as_ref()
                .filter(|snapshot| snapshot.machine == machine)
                .and_then(|snapshot| snapshot.agents.iter().find(|info| info.agent == agent)),
            None => self.code_agents.iter().find(|info| info.agent == agent),
        }
    }

    pub(super) fn picker_harness_waiting(&self) -> bool {
        self.picker_machine().is_some_and(|machine| {
            self.code_form
                .harness
                .as_ref()
                .filter(|snapshot| snapshot.machine == machine)
                .is_none_or(|snapshot| snapshot.waiting())
        })
    }

    pub(super) fn picker_harness_error(&self) -> Option<&str> {
        let machine = self.picker_machine()?;
        self.code_form
            .harness
            .as_ref()
            .filter(|snapshot| snapshot.machine == machine)
            .and_then(|snapshot| snapshot.error.as_deref())
    }

    pub(super) fn refresh_picker_harness(&mut self, cx: &mut Context<Self>) {
        self.refresh_harness_for(self.picker_machine(), cx);
    }

    pub(super) fn launch_harness_info(&self, agent: SessionAgent) -> Option<&AgentInfo> {
        if matches!(self.view, View::NewCode)
            && let Some(machine) = self.harness_machine()
        {
            return self
                .code_form
                .harness
                .as_ref()
                .filter(|snapshot| snapshot.machine == machine)
                .and_then(|snapshot| snapshot.agents.iter().find(|info| info.agent == agent));
        }
        self.code_agents.iter().find(|info| info.agent == agent)
    }

    pub(super) fn refresh_harness(&mut self, cx: &mut Context<Self>) {
        self.refresh_harness_for(self.harness_machine().map(str::to_owned), cx);
    }

    fn refresh_harness_for(&mut self, machine: Option<String>, cx: &mut Context<Self>) {
        let Some(machine) = machine else {
            if let Some(runner) = &self.runner {
                runner.load_agents();
            } else {
                let answer = crate::code_run::read_agents();
                cx.spawn(async move |this, cx| {
                    if let Ok(agents) = answer.await {
                        let _ = this.update(cx, |this, cx| {
                            this.code_agents = agents;
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
            return;
        };
        if self
            .code_form
            .harness
            .as_ref()
            .is_none_or(|snapshot| snapshot.machine != machine)
        {
            self.code_form.harness = Some(RemoteHarness {
                machine: machine.clone(),
                agents: Vec::new(),
                error: None,
                checking: false,
                check_started: None,
            });
        }
        let started = std::time::Instant::now();
        if let Some(snapshot) = self.code_form.harness.as_mut() {
            if snapshot.checking {
                return;
            }
            snapshot.checking = true;
            snapshot.check_started = Some(started);
        }
        let result = crate::code_run::read_remote_agents(machine.clone(), self.machine_source.clone());
        cx.spawn(async move |this, cx| {
            let result = result
                .await
                .unwrap_or_else(|_| Err("The server's agent check was lost.".into()));
            let _ = this.update(cx, |this, cx| {
                if let Some(snapshot) = this
                    .code_form
                    .harness
                    .as_mut()
                    .filter(|snapshot| snapshot.accepts_reply(&machine, started))
                {
                    snapshot.checking = false;
                    match result {
                        Ok(agents) => {
                            snapshot.agents = agents;
                            snapshot.error = None;
                        }
                        Err(error) => snapshot.error = Some(error),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn install_nexus_harness(&mut self, cx: &mut Context<Self>) {
        if self.code_form.installing_nexus {
            return;
        }
        self.code_form.installing_nexus = true;
        self.code_form.nexus_install_error = None;
        let result = crate::code_run::install_nexus(self.gateway.url().to_string());
        cx.spawn(async move |this, cx| {
            let result = result
                .await
                .unwrap_or_else(|_| Err("The installer stopped before it finished.".into()));
            let _ = this.update(cx, |this, cx| {
                this.code_form.installing_nexus = false;
                match result {
                    Ok(info) => {
                        this.code_agents.retain(|entry| entry.agent != info.agent);
                        this.code_agents.push(info);
                        if let Some(runner) = &this.runner {
                            runner.load_agents();
                        }
                    }
                    Err(error) => this.code_form.nexus_install_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

pub(super) fn install_command(agent: SessionAgent, gateway: &str, ssh: Option<&str>) -> String {
    // A coding agent installs with its entry's command; Nexus's own agent from this Gateway's installer.
    let command = match agent.coding() {
        Some(coding) if ssh.is_none() => coding.local_install_command().to_string(),
        Some(coding) => coding.entry().install.command.to_string(),
        None => format!(
            "curl -fsSL {} | bash",
            nexus_workbench::open::shell_quote(&format!("{}/install.sh", gateway.trim_end_matches('/')))
        ),
    };
    match ssh {
        Some(ssh) => format!(
            "ssh -t {} {}",
            nexus_workbench::open::shell_quote(ssh),
            nexus_workbench::open::shell_quote(&command)
        ),
        None => command,
    }
}

#[cfg(test)]
mod tests {
    use super::{RemoteHarness, install_command, readiness};
    use nexus_workbench::{api::AgentInfo, event::SessionAgent};

    #[test]
    fn remote_catalog_waits_only_while_its_initial_read_is_running() {
        let mut snapshot = RemoteHarness {
            machine: "server".into(),
            agents: Vec::new(),
            error: None,
            checking: true,
            check_started: None,
        };
        assert!(snapshot.waiting());
        snapshot.checking = false;
        assert!(!snapshot.waiting(), "an empty successful reply is settled");
        snapshot.error = Some("offline".into());
        assert!(!snapshot.waiting(), "an error is shown instead of an endless loader");
    }

    #[test]
    fn remote_catalog_ignores_replies_for_another_target_or_an_older_visit() {
        let first = std::time::Instant::now();
        let next = first + std::time::Duration::from_millis(1);
        let snapshot = RemoteHarness {
            machine: "server-a".into(),
            agents: Vec::new(),
            error: None,
            checking: true,
            check_started: Some(next),
        };
        assert!(!snapshot.accepts_reply("server-b", next));
        assert!(!snapshot.accepts_reply("server-a", first));
        assert!(snapshot.accepts_reply("server-a", next));
    }

    #[test]
    fn remote_agent_catalog_reads_current_and_older_hosts() {
        let older: AgentInfo =
            serde_json::from_value(serde_json::json!({"agent":"nexus","path":null,"version":null})).unwrap();
        assert_eq!(older.models, None);
        let current: AgentInfo = serde_json::from_value(serde_json::json!({"agent":"claude","path":"/server/claude","version":"1","models":[{"id":"opus","name":"Opus","efforts":["high"],"resolvedId":"opus-5"}]})).unwrap();
        assert_eq!(current.models.unwrap()[0].id, "opus");
    }

    #[test]
    fn remote_install_hints_use_the_selected_server() {
        assert_eq!(
            install_command(SessionAgent::Codex, "https://gateway", Some("dev@example")),
            "ssh -t 'dev@example' 'npm install -g @openai/codex'"
        );
        assert!(
            install_command(SessionAgent::Nexus, "https://gateway/", Some("dev@example"))
                .contains("https://gateway/install.sh")
        );
        let info = AgentInfo {
            agent: SessionAgent::Nexus,
            path: None,
            version: None,
            models: None,
            problem: None,
        };
        assert_eq!(readiness(Some(&info), "Build server"), "Not installed on Build server");
    }

    #[test]
    fn readiness_distinguishes_missing_old_and_ready_runtimes() {
        let mut info = AgentInfo {
            agent: SessionAgent::Nexus,
            path: None,
            version: None,
            models: None,
            problem: None,
        };
        assert_eq!(readiness(None, "this computer"), "Checking…");
        assert_eq!(
            readiness(Some(&info), "this computer"),
            "Not installed on this computer"
        );
        info.problem = Some("The JavaScript CLI cannot run sessions".into());
        assert_eq!(
            readiness(Some(&info), "this computer"),
            "Installed, but can't run sessions"
        );
        info.path = Some("/fake/nexus".into());
        info.version = Some("v1.2.3".into());
        assert_eq!(readiness(Some(&info), "this computer"), "Installed v1.2.3");
    }

    #[test]
    fn cloud_and_remote_targets_offer_cloud_capable_agents_including_opencode() {
        use super::offers_agent_on;

        // Cloud / remote targets offer all non-local-only agents regardless of the local OS.
        assert!(offers_agent_on(SessionAgent::Claude, true));
        assert!(offers_agent_on(SessionAgent::Codex, true));
        assert!(offers_agent_on(SessionAgent::Nexus, true));
        assert!(offers_agent_on(SessionAgent::Opencode, true));
        assert!(!offers_agent_on(SessionAgent::Antigravity, true));
        assert!(!offers_agent_on(SessionAgent::Cursor, true));

        // Local targets match the local platform's support.
        for agent in SessionAgent::ALL {
            assert_eq!(offers_agent_on(agent, false), agent.supported_on_current_platform());
        }
    }
}
