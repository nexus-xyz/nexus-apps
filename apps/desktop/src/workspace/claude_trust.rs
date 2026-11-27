//! The New session question about a project's own Claude settings or OpenCode config (`nexus_workbench::claude_trust`).
//! Until the person trusts the project, its Claude and OpenCode sessions load only the person's own settings. The
//! question shows once, as the first item of the setup strip (`start_blockers.rs`), only for Claude and OpenCode and
//! only when the chosen agent's project settings would turn something on. Trust or Don't trust answers it for good; the project's settings page changes it later.
//!
//! The answer is read from the project's own runner (this computer's, or its server's) when New session opens or the
//! project changes. A runner that does not know the command (an older server) asks nothing. A send before the read
//! answers is not held: that session starts with only the person's settings, and its transcript says why.

use std::collections::HashMap;

use gpui_kit::*;
use nexus_workbench::claude_trust::ClaudeTrust;
use nexus_workbench::event::SessionAgent;
use serde_json::{Value, json};

use super::{View, Workspace};

/// Each project's last read answer, by project id.
#[derive(Default)]
pub(super) struct Asks {
    read: HashMap<String, ClaudeTrust>,
    /// The project being read now.
    reading: Option<(String, Task<()>)>,
    /// An answer being kept.
    saving: Option<Task<()>>,
}

/// The read's answer: a runner that refuses the command (an older one) asks nothing.
fn read_answer(answer: Result<Value, String>) -> ClaudeTrust {
    answer
        .and_then(|value| serde_json::from_value(value).map_err(|error| error.to_string()))
        .unwrap_or_else(|error| {
            eprintln!("nexus-desktop: could not read the project's Claude settings answer: {error}");
            ClaudeTrust::default()
        })
}

/// The setup item's title and text for `trust` and `agent`, when the question waits.
pub(super) fn question(trust: &ClaudeTrust, agent: SessionAgent) -> Option<(String, String)> {
    let found = trust.found_for(agent).filter(|_| trust.asks_for(agent))?;
    let name = crate::agents::ui(agent).short_name();
    Some((
        format!("Trust this project's {name} settings?"),
        format!(
            "It would turn on {}. Until you trust it, {name} uses only your own settings here. You can change this \
             later in the project's settings.",
            found.summary()
        ),
    ))
}

impl Workspace {
    /// The New session form's local project, when its own settings matter: a Claude or OpenCode session that does not
    /// run on Cloud.
    fn claude_trust_project(&self) -> Option<&str> {
        let limited = ClaudeTrust::default().found_for(self.code_form.agent).is_some();
        (matches!(self.view, View::NewCode) && !self.cloud_ui.on && limited)
            .then_some(())
            .and(self.code_form.project.as_deref())
    }

    /// Reads the New session project's answer again (its folder may have changed since).
    pub(super) fn check_claude_trust(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.code_form.project.clone().filter(|_| !self.cloud_ui.on) else {
            return;
        };
        if self
            .claude_trust
            .reading
            .as_ref()
            .is_some_and(|(reading, _)| *reading == project)
        {
            return;
        }
        let Some(runs) = self.runs(&project) else {
            return;
        };
        let answer = runs.call("wb_project_claude_trust", json!({ "projectId": project }));
        let id = project.clone();
        let task = cx.spawn(async move |this, cx| {
            let trust = read_answer(answer.await.unwrap_or_else(|_| Err("the answer was lost".into())));
            this.update(cx, |this, cx| {
                this.claude_trust.reading = None;
                this.claude_trust.read.insert(id, trust);
                cx.notify();
            })
            .ok();
        });
        self.claude_trust.reading = Some((project, task));
    }

    /// The question's title and text for the New session project, if it waits for the person.
    pub(super) fn claude_trust_question(&self) -> Option<(String, String)> {
        question(
            self.claude_trust.read.get(self.claude_trust_project()?)?,
            self.code_form.agent,
        )
    }

    /// Trust or Don't trust, from the setup strip.
    pub(super) fn answer_claude_trust(&mut self, trusted: bool, cx: &mut Context<Self>) {
        let Some(project) = self.claude_trust_project().map(String::from) else {
            return;
        };
        let Some(runs) = self.runs(&project) else {
            self.refuse_read_only();
            return;
        };
        if self.claude_trust.saving.is_some() {
            return;
        }
        let answer = runs.call(
            "wb_project_claude_trust_set",
            json!({ "projectId": project, "trusted": trusted }),
        );
        self.claude_trust.saving = Some(cx.spawn(async move |this, cx| {
            let saved = answer
                .await
                .unwrap_or_else(|_| Err("The answer was lost.".into()))
                .and_then(|value| serde_json::from_value::<ClaudeTrust>(value).map_err(|error| error.to_string()));
            this.update(cx, |this, cx| {
                this.claude_trust.saving = None;
                match saved {
                    Ok(trust) => {
                        this.claude_trust.read.insert(project, trust);
                    }
                    Err(error) => this.composer_note = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use nexus_workbench::claude_trust::Found;

    fn found() -> Found {
        Found {
            hooks: 3,
            allowed: 2,
            mcp_servers: 1,
            other: false,
            extras: 0,
            plugins: 0,
        }
    }

    #[test]
    fn asks_only_an_unanswered_project_with_settings_that_matter() {
        let waiting = ClaudeTrust {
            trusted: None,
            found: found(),
            ..ClaudeTrust::default()
        };
        let (title, text) = question(&waiting, SessionAgent::Claude).expect("a question");
        assert_eq!(title, "Trust this project's Claude settings?");
        assert!(
            text.contains("3 hooks, 2 pre-approved commands, 1 MCP server"),
            "{text}"
        );
        for answered in [Some(true), Some(false)] {
            let trust = ClaudeTrust {
                trusted: answered,
                found: found(),
                ..ClaudeTrust::default()
            };
            assert_eq!(question(&trust, SessionAgent::Claude), None);
        }
        assert_eq!(question(&ClaudeTrust::default(), SessionAgent::Claude), None);
        // Only the agents whose project settings are limited are asked.
        assert_eq!(question(&waiting, SessionAgent::Codex), None);
    }

    #[test]
    fn asks_opencode_about_its_own_config() {
        let waiting = ClaudeTrust {
            trusted: None,
            found: Found::default(),
            opencode: Found {
                mcp_servers: 1,
                plugins: 2,
                ..Found::default()
            },
        };
        assert_eq!(question(&waiting, SessionAgent::Claude), None);
        let (title, text) = question(&waiting, SessionAgent::Opencode).expect("a question");
        assert_eq!(title, "Trust this project's OpenCode settings?");
        assert!(text.contains("1 MCP server, 2 plugins or custom tools"), "{text}");
        assert!(text.contains("OpenCode uses only your own settings"), "{text}");
        let trusted = ClaudeTrust {
            trusted: Some(true),
            ..waiting
        };
        assert_eq!(question(&trusted, SessionAgent::Opencode), None);
    }

    #[test]
    fn a_runner_that_refuses_the_read_asks_nothing() {
        assert_eq!(read_answer(Err("Unknown command".into())), ClaudeTrust::default());
        let read = read_answer(Ok(json!({
            "trusted": null,
            "found": { "hooks": 3, "allowed": 2, "mcpServers": 1, "other": false }
        })));
        assert!(read.asks());
    }
}
