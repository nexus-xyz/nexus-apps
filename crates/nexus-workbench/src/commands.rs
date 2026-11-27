//! Typed session commands. These never pass through ordinary prompt submission.
use super::*;

pub const FEATURE: &str = "session-commands";

pub(crate) fn native_name(text: &str) -> Option<&str> {
    let name = text.trim_start().strip_prefix('/')?.split_whitespace().next()?;
    (!name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || b"_:-".contains(&c))).then_some(name)
}

impl Workbench {
    pub(crate) fn send_native_command(
        &self,
        thread_id: &str,
        name: &str,
        text: &str,
        images: Vec<Image>,
    ) -> Result<()> {
        if name.eq_ignore_ascii_case("compact") {
            return Err("Use the typed Compact conversation action.".into());
        }
        let live = self
            .live(thread_id)
            .ok_or("Resume the agent before running a discovered command.")?;
        {
            let state = live.state();
            if !state.idle() || !state.background.is_empty() {
                return Err("Wait for the current turn before running a discovered command.".into());
            }
            let commands = state
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.commands.as_ref());
            if harness::harness(live.agent)
                .session_features()
                .native_commands
                .is_none()
                || !state.initialized
                || state.done
                || state.stop.is_some()
                || !commands.is_some_and(|commands| commands.iter().any(|command| command == name))
            {
                return Err("That command is not available in this process.".into());
            }
        }
        let user = self.append(
            thread_id,
            WbEvent::User {
                text: text.into(),
                images: images.clone(),
                from: None,
            },
        )?;
        let say = Say::User {
            text: text.into(),
            images,
        };
        if !self.begin_turn_if(thread_id, &live, true, Some(say), Some(user.seq), true, true, false)? {
            // The process stopped or got busy after the checks above, so the command never ran. Its message leaves the
            // conversation as a taken-back one does; the composer keeps the draft and shows this error.
            // A failed write is logged, not returned: the reason below is the one the person can act on.
            if let Err(failed) = self.append(thread_id, WbEvent::QueueCancel { seq: user.seq }) {
                eprintln!("nexus-workbench: {thread_id}: {failed}");
            }
            return Err("The command's agent process stopped. Resume it and select the command again.".into());
        }
        // The person's own command: the session moves to the top of its project, as a message does.
        self.person_acted(thread_id)?;
        Ok(())
    }

    /// Compact one initialized, quiet process. No process is launched and no user prompt is logged.
    pub async fn compact(&self, thread_id: &str, instructions: Option<String>) -> Result<()> {
        let live = self.live(thread_id).ok_or("Resume the agent before compacting.")?;
        if let Some(reason) = harness::harness(live.agent)
            .session_features()
            .compact_instructions_refusal
            && instructions.as_ref().is_some_and(|text| !text.is_empty())
        {
            return Err(reason.into());
        }
        let request = live.next_request();
        let (tx, rx) = oneshot::channel();
        {
            let mut state = live.state();
            if !state.initialized
                || !state.idle()
                || !state.background.is_empty()
                || !state.changes.is_empty()
                || state.owed > 0
            {
                return Err("Wait for the turn, approvals and queued work to finish.".into());
            }
            if !state
                .capabilities
                .as_ref()
                .is_some_and(|capabilities| capabilities.compact)
            {
                return Err("This runtime has not reported compaction support.".into());
            }
            state.task_asks.insert(request.clone(), tx);
        }
        let say = Say::Compact {
            request: request.clone(),
            instructions,
        };
        // Lined up with its turn: a message sent from then on goes after it.
        if !self.begin_turn_if(thread_id, &live, false, Some(say), None, false, true, false)? {
            live.state().task_asks.remove(&request);
            return Err("The agent is no longer idle.".into());
        }
        match tokio::time::timeout(TASK_ASK_WAIT, rx).await {
            Ok(Ok(result)) => result.map(|_| ()),
            Ok(Err(_)) => Err("The agent stopped before answering the compaction request.".into()),
            Err(_) => {
                live.state().task_asks.remove(&request);
                Err(
                    "Compaction did not answer in time. It may still be running; do not retry until the turn ends."
                        .into(),
                )
            }
        }
    }
}
