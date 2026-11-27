//! The shared terminal API for authenticated agent tools and the app window.
use super::*;
use crate::Workbench;
use crate::harness::harness;

#[cfg(test)]
#[path = "guarded_tests.rs"]
mod guarded_tests;

impl Workbench {
    /// Native callers pass `human` only after checking the invoking app window. MCP callers always pass false.
    pub async fn terminal(&self, thread: &str, op: &str, args: Value, human: bool) -> Result<Value, String> {
        self.terminal_guarded(thread, op, args, human, &|| true).await
    }

    /// In-process continuation proof. This is deliberately absent from terminal wire arguments.
    pub async fn terminal_guarded(
        &self,
        thread: &str,
        op: &str,
        args: Value,
        human: bool,
        valid: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Value, String> {
        if !valid() {
            return Err("terminal_not_found".into());
        }
        let scope = self.config().scope;
        let identity = self.inner.host.session_choice_identity();
        self.inner.terminals.context(&scope, identity.clone());
        let attempted_command = args["command"].as_str().map(String::from);
        let result = self.terminal_call(thread, op, args, human, valid).await;
        if !valid() {
            return Err("terminal_not_found".into());
        }
        if let (Err(error), Some(command)) = (&result, attempted_command)
            && matches!(op, "run" | "open" | "execute")
        {
            self.inner.terminals.note_routing_failure(thread, &command, error);
        }
        if self.config().scope != scope || self.inner.host.session_choice_identity() != identity {
            return Err("terminal_not_found".into());
        }
        result
    }

    /// Whether a command started here carries the commit trailer hook ([`crate::Host::marks_commits`]). Agents run
    /// `git commit` in these terminals, not in their own process, so the hook their launch set must be here too.
    async fn terminal_marks_commits(&self, scope: &str) -> bool {
        let terminals = &self.inner.terminals;
        if terminals.marks_commits() {
            return true;
        }
        let identity = self.inner.host.session_choice_identity();
        let marked = self.inner.host.marks_commits().await;
        if marked {
            terminals.keep_marks_commits(scope, &identity);
        }
        marked
    }

    async fn terminal_call(
        &self,
        thread: &str,
        op: &str,
        mut args: Value,
        human: bool,
        valid: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Value, String> {
        if !valid() {
            return Err("terminal_not_found".into());
        }
        if !self.config().terminal && !matches!(op, "list" | "read" | "submissions") {
            return Err("terminal_unavailable: This host does not offer shared terminals.".into());
        }
        let scope = self.config().scope;
        let record = self.record(thread)?;
        if record.scope != scope {
            return Err("terminal_not_found".into());
        }
        let mode = record.summary.permission_mode;
        let terminals = &self.inner.terminals;
        if op == "list" {
            let entries = terminals.history(&scope, thread)?;
            return Ok(json!(
                entries
                    .into_iter()
                    .filter(|e| !e.closed && (args["include_deleted"].as_bool() == Some(true) || !e.deleted))
                    .map(|e| if human { e } else { e.for_agent() })
                    .collect::<Vec<_>>()
            ));
        }

        if matches!(op, "run" | "request_user" | "open" | "stage_shell") {
            if op == "stage_shell" && !human {
                return Err("terminal_user_action_required".into());
            }
            let watch = match args.get("watch") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(watch)) => *watch,
                Some(_) => return Err("terminal_invalid_arguments: watch must be true or false.".into()),
            };
            if watch {
                if op != "run" {
                    return Err("terminal_invalid_arguments: watch is supported only on terminal_run.".into());
                }
                if !human && !harness(record.summary.agent).session_features().steers_mid_turn {
                    return Err(watch::unsupported_error());
                }
            }
            let object = args.as_object_mut().ok_or("terminal_invalid_arguments")?;
            object.insert(
                "require_user".into(),
                json!(matches!(op, "request_user" | "stage_shell")),
            );
            if !human {
                object.remove("private");
            }
            object.insert("interactive".into(), json!(op == "open" || op == "stage_shell"));
            if op == "open" || op == "stage_shell" {
                object.insert("command".into(), json!("Interactive shell"));
            }
            let entry = terminals.stage(&scope, thread, self.folder(thread)?, &args)?;
            {
                let mut state = entry.state();
                // A retry reads the receipt. Current permissions cannot rewrite a launched or ended command.
                if state.info.status != "awaitingUser" {
                    if !human && !matches!(state.info.status.as_str(), "starting" | "running") {
                        state.watch.saw_end();
                    }
                    return Ok(json!(if human {
                        state.info.clone()
                    } else {
                        state.info.for_agent()
                    }));
                }
                state.info.permission_mode = Some(mode);
                // Marked when staged, so a command the person approves in Ask first mode keeps it. A shell the agent
                // opens is one the person may type into, so it keeps the person's pager.
                state.info.agent_written = !human && !state.info.interactive;
                state.watch.requested = watch && !human && op == "run";
                state.info.decision_source = Some(
                    if human {
                        "person"
                    } else if mode == PermissionMode::BypassPermissions {
                        "fullAccess"
                    } else if mode == PermissionMode::Auto {
                        "auto"
                    } else {
                        "requiresApproval"
                    }
                    .into(),
                );
                if human && args["private"].as_bool() == Some(true) {
                    state.info.private = true;
                }
                if !human && mode == PermissionMode::Plan {
                    state.info.status = "denied".into();
                    entry.persist(&state, "denied")?;
                    return Err("terminal_plan_mode: Leave Plan mode before running or staging commands.".into());
                }
                entry.persist(&state, "authorized")?;
            }
            if !matches!(op, "request_user" | "stage_shell") && (human || mode.allows_unasked()) {
                let marked = self.terminal_marks_commits(&scope).await;
                if !valid() {
                    return Err("terminal_not_found".into());
                }
                process::start(&entry, human, &self.config().shell, None, marked)?;
            }
            return Ok(json!(entry.view(human)));
        }
        let entry = terminals.find(&scope, thread, &text(&args, "id", 128)?)?;
        if op == "submissions" {
            let info = entry.info();
            if info.private && !human {
                return Err("terminal_private".into());
            }
            let inputs = self.store().terminal_submissions(&info.id)?;
            return Ok(json!({"truncated": info.input_count > inputs.len() as u64, "inputs":inputs}));
        }
        if op == "read" {
            let after = args.get("after").and_then(Value::as_u64).unwrap_or(0);
            let max = args
                .get("max_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(16_384)
                .clamp(1, 65_536) as usize;
            let wait = args.get("wait_ms").and_then(Value::as_u64).unwrap_or(0).min(10_000);
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(wait);
            // Declared before `state`, so it drops after the state lock is released on every return.
            let mut reader = (!human).then(|| watch::ReaderGuard::new(entry.clone()));
            loop {
                {
                    let mut state = entry.state();
                    if !human && state.info.private && !state.info.result_shared {
                        return Err("terminal_private: The person paused output sharing.".into());
                    }
                    let reviewed = !human && state.info.private;
                    let (bytes, next, truncated) = if reviewed {
                        state.reviewed.read(after, max)
                    } else {
                        state.output.read(after, max)
                    };
                    let active = matches!(state.info.status.as_str(), "starting" | "running" | "awaitingUser");
                    if !bytes.is_empty() || !active || tokio::time::Instant::now() >= deadline {
                        if !human {
                            state.info.agent_cursor = next;
                            if !active {
                                state.watch.saw_end();
                            }
                        }
                        let end = if reviewed {
                            state.reviewed.cursor
                        } else {
                            state.output.cursor
                        };
                        return Ok(
                            json!({"terminal":if human {state.info.clone()} else {state.info.for_agent()},"output":String::from_utf8_lossy(&bytes),"bytes":bytes,"next":next,"end":end,"hasMore":next<end,"truncated":truncated,"idleMs":state.idle_ms(),"outputKind":if reviewed {"reviewed_summary"} else if state.info.restored || state.info.route != "workbench" {"history_only"} else {"process_output"}}),
                        );
                    }
                    if let Some(reader) = reader.as_mut() {
                        reader.arm(&mut state);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                if self.config().scope != scope {
                    return Err("terminal_not_found".into());
                }
            }
        }
        if entry.info().route != "workbench" && !matches!(op, "delete" | "restore" | "rename") {
            return Err(
                "terminal_external_process: The provider owns this process. Use its original activity controls.".into(),
            );
        }
        if op == "execute" {
            if !human {
                return Err("terminal_user_action_required".into());
            }
            if args.get("command").is_some() {
                let command = text(&args, "command", 32_000)?;
                let mut state = entry.state();
                if state.info.status != "awaitingUser" {
                    return Err("terminal_already_started".into());
                }
                if args["expected_command"]
                    .as_str()
                    .is_some_and(|expected| expected != state.info.command)
                {
                    return Err("terminal_command_changed: Review the command again.".into());
                }
                if command != state.info.command {
                    state.info.interactive = false;
                    // The person rewrote it: it is their command now.
                    state.info.agent_written = false;
                }
                state.info.command = command;
            }
            if args["private"].as_bool() == Some(true) {
                entry.state().info.private = true;
            }
            entry.state().info.decision_source = Some("person".into());
            let marked = self.terminal_marks_commits(&scope).await;
            if !valid() {
                return Err("terminal_not_found".into());
            }
            process::start(&entry, true, &self.config().shell, args["command"].as_str(), marked)?;
            return Ok(json!(entry.info()));
        }
        let mut state = entry.state();
        match op {
            "input" | "interrupt" => {
                if !human {
                    agent_write(&state, mode, &args)?;
                }
                let data = if op == "interrupt" {
                    "\u{3}".into()
                } else {
                    text(&args, "data", 16_384)?
                };
                if op == "input" && !human && state.info.interactive && !state.info.private {
                    state.info.input_count += 1;
                    if let Some(store) = &entry.journal {
                        store
                            .lock()
                            .map_err(|_| "terminal_history_unavailable")?
                            .terminal_submission(&state.info, &data)?;
                    }
                } else {
                    entry.persist(
                        &state,
                        if op == "interrupt" {
                            "interruptRequested"
                        } else {
                            "inputSubmitted"
                        },
                    )?;
                }
                state
                    .input
                    .as_ref()
                    .ok_or("terminal_not_running")?
                    .try_send(data.into_bytes())
                    .map_err(|_| "terminal_input_busy")?;
            }
            "resize" if human => {
                let cols = args["cols"].as_u64().unwrap_or(80).clamp(2, 500) as u16;
                let rows = args["rows"].as_u64().unwrap_or(24).clamp(1, 300) as u16;
                if let Some(control) = &state.control {
                    let _ = control.send(process::Control::Resize(cols, rows));
                }
            }
            "take" | "return" | "private" | "share" | "decline" | "close" | "rename" | "delete" | "restore"
            | "stop"
                if human =>
            {
                let ended = matches!(
                    state.info.status.as_str(),
                    "declined" | "completed" | "failed" | "interrupted" | "cancelled" | "unknown" | "denied"
                );
                if (op == "delete" && !ended)
                    || (op == "restore" && (!state.info.deleted || state.info.closed))
                    || (op == "decline" && state.info.status != "awaitingUser")
                    || (op == "return" && (state.info.private || !mode.allows_unasked()))
                    || (op == "share" && !state.info.private)
                {
                    return Err("terminal_invalid_state".into());
                }
                if matches!(op, "take" | "return" | "private" | "decline" | "close" | "stop") {
                    state.info.lease += 1;
                }
                match op {
                    "delete" if ended => state.info.deleted = true,
                    "restore" if state.info.deleted && !state.info.closed => state.info.deleted = false,
                    "stop" => {
                        drop(state);
                        entry.stop();
                        return Ok(json!(entry.info()));
                    }
                    "take" => state.info.owner = "user".into(),
                    "rename" => state.info.name = text(&args, "name", 128)?.chars().take(48).collect(),
                    "return" if !state.info.private && mode.allows_unasked() => {
                        state.info.owner = "agent".into();
                    }
                    "private" => {
                        state.info.private = true;
                        state.info.result_shared = false;
                        state.reviewed = Output::default();
                        state.info.owner = "user".into();
                        state.output.bytes.clear();
                    }
                    "share" if state.info.private => {
                        let reviewed = text(&args, "result", 32_000)?;
                        state.reviewed = Output::default();
                        state.reviewed.push(reviewed.as_bytes());
                        state.info.result_shared = true;
                    }
                    "decline" if state.info.status == "awaitingUser" => state.info.status = "declined".into(),
                    "close" => {
                        state.info.closed = true;
                        drop(state);
                        entry.stop();
                        return Ok(json!(entry.info()));
                    }
                    _ => return Err("terminal_invalid_state".into()),
                }
            }
            _ => return Err("terminal_invalid_operation".into()),
        }
        entry.persist(&state, op)?;
        Ok(json!(if human {
            state.info.clone()
        } else {
            state.info.for_agent()
        }))
    }
}
