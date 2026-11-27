//! Records provider-owned commands without executing, approving, or replaying them.
use super::*;
use crate::harness::harness;
use crate::{Live, Workbench, event::WbEvent};

impl Terminals {
    pub(super) fn note_routing_failure(&self, thread: &str, command: &str, error: &str) {
        let code = error.split(':').next().unwrap_or(error);
        // Authorization failures and uncertain dispatches are never fallback reasons.
        if matches!(code, "terminal_unavailable" | "terminal_spawn_failed") {
            let mut registry = self.0.lock().unwrap_or_else(|p| p.into_inner());
            if registry.routing_failures.len() >= MAX_ENTRIES {
                registry.routing_failures.clear();
            }
            registry
                .routing_failures
                .insert((thread.into(), command.into()), code.into());
        }
    }
}

impl Workbench {
    pub(crate) fn observe_native_terminal(&self, thread: &str, live: &Live, event: &WbEvent) -> Result<(), String> {
        let (call, parent) = match event {
            WbEvent::Tool { id, parent, name, .. }
                if matches!(name.as_str(), "Bash" | "exec_command" | "shell_command") =>
            {
                (id, parent)
            }
            WbEvent::ToolResult { id, parent, .. } => (id, parent),
            _ => return Ok(()),
        };
        let scope = &live.scope;
        let identity = live.identity.clone().unwrap_or_default();
        let native = self.record(thread)?.native_id;
        let key = format!("native:{native}:{}:{call}", parent.as_deref().unwrap_or(""));
        let prior = self.store().terminal_receipt(scope, &identity, thread, &key)?;
        if let WbEvent::ToolResult { is_error, .. } = event {
            if let Some((_, mut info)) = prior
                && info.ended_at.is_none()
            {
                info.status = if *is_error { "failed" } else { "completed" }.into();
                info.ended_at = Some(crate::now_ms());
                // Tool success is not a reported process exit code. Preserve null.
                self.store().terminal_save(&info, "externalResult")?;
            }
            return Ok(());
        }
        if prior.is_some() {
            return Ok(());
        }
        let WbEvent::Tool { input, .. } = event else {
            return Ok(());
        };
        let Some(command) = input.get("command").and_then(Value::as_str) else {
            return Ok(());
        };
        let injected = live.state().grant.as_ref().is_some_and(|g| g.toolsets.terminal);
        // An agent the app gives no tools runs every command in its own shell, by design: recorded, not a bypass.
        let harness = harness(live.agent);
        let own_shell = !harness.app_tools().offered();
        let code = if own_shell {
            "agent_own_shell".to_string()
        } else if !self.config().terminal {
            "host_unsupported".to_string()
        } else if !injected {
            "tools_not_injected".into()
        } else {
            self.inner
                .terminals
                .0
                .lock()
                .map_err(|_| "terminal_unavailable")?
                .routing_failures
                .remove(&(thread.into(), command.into()))
                .unwrap_or_else(|| "native_execution_unintercepted".into())
        };
        let reason = match code.as_str() {
            "agent_own_shell" => format!(
                "{} has no shared terminal yet, so it runs commands in its own shell.",
                harness.label()
            ),
            "host_unsupported" => "This host does not offer shared terminal execution.".into(),
            "tools_not_injected" => {
                "The runtime did not receive the shared terminal tools. See its startup notice.".into()
            }
            "terminal_unavailable" => "The shared terminal host reported that execution was unavailable.".into(),
            "terminal_spawn_failed" => "The shared executor confirmed that its process failed to start.".into(),
            _ => "The provider bypassed shared terminal routing; the cause is unknown.".into(),
        };
        let mode = self.record(thread)?.summary.permission_mode;
        let now = crate::now_ms();
        let info = Info {
            id: uuid::Uuid::new_v4().to_string(),
            name: command.chars().take(48).collect(),
            interactive: false,
            command: command.into(),
            reason: Some(reason.clone()),
            cwd: self.folder(thread)?.to_string_lossy().into_owned(),
            status: "providerPending".into(),
            owner: "provider".into(),
            lease: 0,
            private: false,
            result_shared: false,
            closed: false,
            exit_code: None,
            agent_cursor: 0,
            input_count: 0,
            restored: false,
            permission_mode: Some(mode),
            decision_source: Some("provider".into()),
            created_at: now,
            started_at: Some(now),
            ended_at: None,
            stop_requested: false,
            agent_written: false,
            deleted: false,
            route: "externalObserved".into(),
            provider_call_id: Some(call.clone()),
            parent_agent_id: parent.clone(),
            bypass_code: Some(code.clone()),
            bypass_reason: Some(reason.clone()),
        };
        self.store().terminal_insert(
            scope,
            &identity,
            thread,
            &key,
            &uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, command.as_bytes()).to_string(),
            &info,
        )?;
        if own_shell {
            return Ok(());
        }
        self.append(
            thread,
            WbEvent::Notice {
                text: format!(
                    "{}{code}): {reason} The provider requested command {call} outside the workbench; Nexus did not rerun it.",
                    super::BYPASS_NOTICE
                ),
            },
        )?;
        Ok(())
    }
}
