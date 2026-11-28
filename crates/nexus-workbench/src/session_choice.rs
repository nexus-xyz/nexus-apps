//! The person's choice before an agent creates a linked session. Pending requests live only in memory.
//! Remembered choices belong to one sign-in, organization and project; they never grant permission to run work.
use std::collections::HashMap;
use std::sync::MutexGuard;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::event::{PermissionMode, SessionAgent, Via};
use crate::linked::{OrchCode, OrchError, StartChild, keyed_id};
use crate::{Result, Workbench};

pub const FEATURE: &str = "session-start-choice";

/// An opaque credential identity, never the credential itself. A new sign-in conservatively asks again.
pub fn sign_in_identity(token: &str) -> String {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, token.as_bytes()).to_string()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Preference {
    #[default]
    Ask,
    New,
    Here,
}
impl Preference {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ask" => Some(Self::Ask),
            "new" => Some(Self::New),
            "here" => Some(Self::Here),
            _ => None,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::New => "new",
            Self::Here => "here",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartChoice {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub agent: SessionAgent,
    /// Who pays for the child: Gateway, or the person's own plan for `agent`. None from an older host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<Via>,
    pub model: Option<String>,
    pub permission_mode: PermissionMode,
    pub workspace: String,
    pub can_remember: bool,
}

impl StartChoice {
    /// Who pays, as the start card says it: "your ChatGPT plan" or "Nexus". None from an older host.
    pub fn payer(&self) -> Option<String> {
        match self.via? {
            Via::Subscription => self
                .agent
                .coding()
                .map(|agent| format!("your {} plan", agent.entry().identity.plan)),
            Via::Nexus => Some("Nexus".to_string()),
        }
    }
}

/// Keyboard answers for a focused session-start card. Menus and composition keep their keys.
#[derive(Default)]
pub struct ChoiceKey<'a> {
    pub key: &'a str,
    pub control: bool,
    pub command: bool,
    pub alt: bool,
    pub shift: bool,
    pub composing: bool,
    pub handled: bool,
}
pub fn shortcut(key: ChoiceKey<'_>) -> Option<Preference> {
    if key.composing || key.handled || key.alt || key.shift {
        return None;
    }
    match key.key {
        "escape" if !key.control && !key.command => Some(Preference::Here),
        "enter" if key.control || key.command => Some(Preference::New),
        _ => None,
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn the_card_names_who_pays_for_the_child() {
        let choice = |agent, via| StartChoice {
            id: "c".into(),
            title: "Review".into(),
            prompt: "Review".into(),
            agent,
            via,
            model: None,
            permission_mode: PermissionMode::Default,
            workspace: "/repo".into(),
            can_remember: true,
        };
        let plan = |agent| format!("your {} plan", crate::accounts::plan(agent));
        assert_eq!(
            choice(SessionAgent::Codex, Some(Via::Subscription)).payer(),
            Some(plan(SessionAgent::Codex))
        );
        assert_ne!(plan(SessionAgent::Codex), plan(SessionAgent::Claude));
        assert_eq!(
            choice(SessionAgent::Claude, Some(Via::Nexus)).payer().as_deref(),
            Some("Nexus")
        );
        assert_eq!(choice(SessionAgent::Claude, None).payer(), None);
        assert_eq!(choice(SessionAgent::Nexus, Some(Via::Subscription)).payer(), None);
    }

    #[test]
    fn escape_or_modified_enter_answers_the_card() {
        assert_eq!(
            shortcut(ChoiceKey {
                key: "escape",
                ..Default::default()
            }),
            Some(Preference::Here)
        );
        assert_eq!(
            shortcut(ChoiceKey {
                key: "enter",
                control: true,
                ..Default::default()
            }),
            Some(Preference::New)
        );
        assert_eq!(
            shortcut(ChoiceKey {
                key: "enter",
                command: true,
                ..Default::default()
            }),
            Some(Preference::New)
        );
        assert_eq!(
            shortcut(ChoiceKey {
                key: "enter",
                ..Default::default()
            }),
            None
        );
        assert_eq!(
            shortcut(ChoiceKey {
                key: "escape",
                control: true,
                ..Default::default()
            }),
            None
        );
    }
    #[test]
    fn composition_menus_and_other_modifiers_keep_the_key() {
        for key in [
            ChoiceKey {
                composing: true,
                ..Default::default()
            },
            ChoiceKey {
                handled: true,
                ..Default::default()
            },
            ChoiceKey {
                shift: true,
                ..Default::default()
            },
            ChoiceKey {
                alt: true,
                ..Default::default()
            },
        ] {
            assert_eq!(shortcut(ChoiceKey { key: "escape", ..key }), None);
        }
    }
}
struct Pending {
    parent: String,
    scope: String,
    identity: Option<String>,
    project: String,
    grant: String,
    choice: StartChoice,
    answer: watch::Sender<Option<Preference>>,
}
#[derive(Default)]
pub(crate) struct Choices {
    pending: HashMap<String, Pending>,
}

/// Dropping a cancelled call removes its card. A restart cannot resurrect an unanswered request.
struct OpenChoice {
    workbench: Workbench,
    parent: String,
    id: String,
}
impl Drop for OpenChoice {
    fn drop(&mut self) {
        self.workbench.choices().pending.remove(&self.id);
        self.workbench.emit_summary(&self.parent);
    }
}
fn cancelled() -> OrchError {
    OrchError::new(
        OrchCode::Halted,
        "The session-start request ended. Ask again if the work is still needed.",
    )
}
fn here() -> Value {
    json!({"choice": "here", "status": "continue_here", "message": "The person chose Do it here. Continue this task in the current session. No child session was created."})
}

impl Workbench {
    fn choices(&self) -> MutexGuard<'_, Choices> {
        self.inner.session_choices.lock().unwrap_or_else(|p| p.into_inner())
    }
    pub(crate) fn pending_session_choices(&self, parent: &str) -> Vec<StartChoice> {
        let mut choices: Vec<_> = self
            .choices()
            .pending
            .values()
            .filter(|p| {
                p.parent == parent && self.in_scope(&p.scope) && p.identity == self.inner.host.session_choice_identity()
            })
            .map(|p| p.choice.clone())
            .collect();
        choices.sort_by(|a, b| a.id.cmp(&b.id));
        choices
    }
    pub fn session_start_preference(&self, project: &str) -> Result<Preference> {
        self.project(project)?;
        let config = self.config();
        let Some(identity) = self.inner.host.session_choice_identity() else {
            return Ok(config.subsessions_first_start);
        };
        let saved = self.store().start_preference(&config.scope, &identity, project)?;
        Ok(saved
            .as_deref()
            .and_then(Preference::parse)
            .unwrap_or(config.subsessions_first_start))
    }

    /// Counts or clears project choices saved by this sign-in in the current organization.
    pub fn session_start_overrides(&self, clear: bool) -> Result<u32> {
        let Some(identity) = self.inner.host.session_choice_identity() else {
            return Ok(0);
        };
        self.store()
            .start_preference_overrides(&self.config().scope, &identity, clear)
    }

    pub fn set_session_start_preference(&self, project: &str, preference: Preference) -> Result<Preference> {
        self.project(project)?;
        let identity = self
            .inner
            .host
            .session_choice_identity()
            .ok_or("Sign in before saving a session preference.")?;
        self.store()
            .set_start_preference(&self.config().scope, &identity, project, preference.name())?;
        Ok(preference)
    }
    pub fn answer_session_start(&self, parent: &str, request: &str, choice: Preference, remember: bool) -> Result<()> {
        if choice == Preference::Ask {
            return Err("Choose New session or Do it here.".into());
        }
        let choices = self.choices();
        let pending = choices
            .pending
            .get(request)
            .filter(|p| {
                p.parent == parent
                    && self.in_scope(&p.scope)
                    && p.identity == self.inner.host.session_choice_identity()
                    && self.holds_grant(parent, &p.grant)
            })
            .ok_or("That session-start request is no longer waiting.")?;
        if pending.answer.borrow().is_some() {
            return Err("That request already has an answer.".into());
        }
        if remember {
            let identity = pending
                .identity
                .as_deref()
                .ok_or("Sign in before saving a session preference.")?;
            self.store()
                .set_start_preference(&pending.scope, identity, &pending.project, choice.name())?;
        }
        pending.answer.send_replace(Some(choice));
        Ok(())
    }

    /// Used by the authenticated MCP start tool. The original start_child admission checks still apply after consent.
    pub(crate) async fn request_session_start(
        &self,
        parent: &str,
        mut child: StartChild,
        grant: &str,
        identity: Option<String>,
    ) -> std::result::Result<Value, OrchError> {
        let key = child.key.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if key.is_empty() || key.chars().count() > crate::linked::MAX_KEY {
            return Err(OrchError::new(
                OrchCode::BadInput,
                format!("A key has 1 to {} characters.", crate::linked::MAX_KEY),
            ));
        }
        child.key = Some(key.clone());
        let gate = self.creation_gate(&format!("choice:{parent}:{key}"));
        let _gate = gate.lock().await;
        if !self.holds_grant(parent, grant) {
            return Err(cancelled());
        }
        let record = self.record(parent)?;
        if !self.in_scope(&record.scope) {
            return Err(cancelled());
        }
        if identity != self.inner.host.session_choice_identity() {
            return Err(cancelled());
        }
        if let Some(ref identity) = identity
            && self.store().chose_here(&record.scope, identity, parent, &key)?
        {
            return Ok(here());
        }
        // A keyed child already exists: a lost reply never causes another choice or another child.
        if let Some(existing) = self.reader().thread(&keyed_id(parent, &key))? {
            let runs_as = self.runs_as(&existing.summary);
            return Ok(
                json!({"choice":"new", "session_id":existing.summary.id, "branch":existing.summary.worktree.map(|w|w.branch), "status":existing.summary.status, "runsAs":runs_as}),
            );
        }
        // A fresh app has no catalog until one is asked for: load it, as `start_child` does, so the first child is not
        // refused for want of prices.
        if self.missing_spend_prices() {
            self.inner.host.ensure_prices().await;
        }
        let (preview, project) =
            self.admit_child(&record, child.clone(), None, None, &self.config().orchestration, false)?;
        // A later permission increase cannot silently exceed the mode the person saw on this card.
        // A later decrease still clamps it again in start_child.
        child.permission_mode = Some(preview.summary.permission_mode);
        child.model = preview.summary.model.clone();
        // The choice is about the parent's work: its project's preference, also for a child in another project.
        let preference = self.session_start_preference(&record.summary.project_id)?;
        let choice = if preference == Preference::Ask {
            let id = uuid::Uuid::new_v4().to_string();
            let (answer, mut received) = watch::channel(None);
            self.choices().pending.insert(
                id.clone(),
                Pending {
                    parent: parent.into(),
                    scope: record.scope.clone(),
                    identity: identity.clone(),
                    project: record.summary.project_id.clone(),
                    grant: grant.into(),
                    choice: StartChoice {
                        id: id.clone(),
                        title: preview.summary.title,
                        prompt: child.prompt.clone(),
                        agent: child.agent,
                        via: Some(preview.summary.via),
                        model: preview.summary.model,
                        permission_mode: preview.summary.permission_mode,
                        workspace: project.path,
                        can_remember: identity.is_some(),
                    },
                    answer,
                },
            );
            let _open = OpenChoice {
                workbench: self.clone(),
                parent: parent.into(),
                id,
            };
            self.emit_summary(parent);
            let mut check = tokio::time::interval(Duration::from_millis(100));
            loop {
                tokio::select! {
                    changed = received.changed() => {
                        if changed.is_err() { return Err(cancelled()); }
                        let answer = *received.borrow_and_update();
                        if let Some(answer) = answer { break answer; }
                    }
                    _ = check.tick() => {
                        if !self.holds_grant(parent, grant) || !self.in_scope(&record.scope) || identity != self.inner.host.session_choice_identity() || self.start_refusal(parent)?.is_some() { return Err(cancelled()); }
                    }
                }
            }
        } else {
            preference
        };
        if !self.holds_grant(parent, grant)
            || !self.in_scope(&record.scope)
            || identity != self.inner.host.session_choice_identity()
        {
            return Err(cancelled());
        }
        if choice == Preference::Here {
            if let Some(identity) = identity {
                self.store().remember_here(&record.scope, &identity, parent, &key)?;
            }
            return Ok(here());
        }
        self.start_child(parent, child).await.map(|summary| {
            let runs_as = self.runs_as(&summary);
            json!({"choice":"new", "session_id":summary.id, "branch":summary.worktree.map(|w|w.branch), "status":"starting", "runsAs":runs_as})
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Store;

    #[test]
    fn preferences_and_here_receipts_persist_without_crossing_sign_ins_scopes_or_projects() {
        let path = std::env::temp_dir().join(format!("nexus-session-choice-{}.sqlite", uuid::Uuid::new_v4()));
        {
            let store = Store::open(&path).expect("valid test fixture");
            store
                .set_start_preference("gw|org", "person-a", "project-a", "new")
                .expect("valid test fixture");
            store
                .remember_here("gw|org", "person-a", "parent", "retry")
                .expect("valid test fixture");
        }
        {
            let store = Store::open(&path).expect("valid test fixture");
            assert_eq!(
                store
                    .start_preference("gw|org", "person-a", "project-a")
                    .expect("valid test fixture"),
                Some("new".into())
            );
            for (scope, person, project) in [
                ("gw|other", "person-a", "project-a"),
                ("other|org", "person-a", "project-a"),
                ("gw|org", "person-b", "project-a"),
                ("gw|org", "person-a", "project-b"),
            ] {
                assert_eq!(
                    store
                        .start_preference(scope, person, project)
                        .expect("valid test fixture"),
                    None
                );
            }
            assert!(
                store
                    .chose_here("gw|org", "person-a", "parent", "retry")
                    .expect("valid test fixture")
            );
            assert!(
                !store
                    .chose_here("gw|org", "person-b", "parent", "retry")
                    .expect("valid test fixture")
            );
            assert!(
                !store
                    .chose_here("gw|other", "person-a", "parent", "retry")
                    .expect("valid test fixture")
            );
            store
                .set_start_preference("gw|org", "person-a", "project-a", "ask")
                .expect("valid test fixture");
            assert_eq!(
                store
                    .start_preference("gw|org", "person-a", "project-a")
                    .expect("valid test fixture"),
                Some("ask".into())
            );
        }
        let _ = std::fs::remove_file(path);
    }
}
