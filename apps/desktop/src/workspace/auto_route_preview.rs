//! What Auto would run for the new session, before it starts: Gateway's dry run (`POST /v1/agents/route`) for the
//! form's agent and payer on this computer. The chip names the model, and a plan with no room asks before any paid run.
//! The handlers that change the agent, the payer, the sign-in or the plan reading ask again ([`Workspace::refresh_auto_route`]);
//! render only reads the answer. `leave_org` drops it.
use gpui_kit::*;
use nexus_client::auto_route::Answer;
use nexus_workbench::event::{SessionAgent, Via};

use super::{Auth, View, Workspace};

/// What an answer is for: the sign-in, the agent (`None` when Auto picks it too), the payer and the plan book's change
/// count.
type Key = (Option<String>, Option<SessionAgent>, Via, u64);

/// The chip's words on Auto: "Auto · Opus 5.5", with the agent when Auto picked it ("Auto · Codex · GPT-6.1 Sol"), and
/// "Auto · any agent" while that answer is read.
pub(super) fn chip_text(across: Option<SessionAgent>, routed: Option<&str>) -> String {
    match (across, routed) {
        (Some(agent), Some(model)) => format!(
            "Auto · {} · {}",
            nexus_workbench::driver::label(agent),
            crate::code::model_label(model)
        ),
        (Some(_), None) => "Auto · any agent".into(),
        (None, Some(model)) => format!("Auto · {}", crate::code::model_label(model)),
        (None, None) => "Auto".into(),
    }
}

/// One form's preview: the choices it was asked for, and Gateway's answer once it came. A newer key drops the pending
/// task, so an old answer never lands.
#[derive(Default)]
pub(super) struct AutoRoutePreview {
    key: Option<Key>,
    pub answer: Option<Answer>,
    pub(super) task: Option<Task<()>>,
}

impl AutoRoutePreview {
    /// The model Auto would start, by the agent's own id: the plan's pick, or a paid pick on a launch that pays.
    pub fn model(&self, via: Via) -> Option<&str> {
        match self.answer.as_ref()? {
            Answer::UsePlan { model, .. } => Some(model),
            Answer::AskToPay { model, .. } if via != Via::Subscription => Some(model),
            _ => None,
        }
    }

    /// The paid model Auto would need on a plan launch: `Some` only when the plan has no room for the task.
    pub fn needs_api(&self, via: Via) -> Option<&str> {
        match self.answer.as_ref()? {
            Answer::AskToPay { name, .. } if via == Via::Subscription => Some(name.rsplit('/').next().unwrap_or(name)),
            _ => None,
        }
    }
}

impl Workspace {
    /// "Auto · any agent" is still asking Gateway which agent runs: a send waits, or it would start the agent the form
    /// held before the answer.
    pub(super) fn auto_agent_pending(&self) -> bool {
        self.code_form.auto_any && self.form_on_auto() && self.auto_route.task.is_some()
    }

    /// Whether the new session's form is on Auto for an agent Auto routes on this computer.
    pub(super) fn form_on_auto(&self) -> bool {
        matches!(self.view, View::NewCode)
            && matches!(self.auth, Auth::SignedIn)
            && !self.cloud_ui.on
            && self.harness_machine().is_none()
            && self.code_form.model.as_deref() == Some(super::code_picker::AUTO)
            && super::code_picker::takes_auto(self.code_form.agent)
    }

    /// Ask Gateway again when the sign-in, the form's agent, its payer or the plan's reading changed. Called from the
    /// handlers that change them, never from render. Never in tests.
    pub(super) fn refresh_auto_route(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        if !self.form_on_auto() {
            if self.auto_route.key.is_some() {
                self.auto_route = AutoRoutePreview::default();
            }
            return;
        }
        let Some(account) = self.account_gateway() else {
            self.auto_route = AutoRoutePreview::default();
            return;
        };
        let across = self.code_form.auto_any;
        let via = if across {
            self.code_form.across_via()
        } else {
            self.code_form.effective_via()
        };
        let key: Key = (
            account.marker(),
            (!across).then_some(self.code_form.agent),
            via,
            crate::plan_book::changes(),
        );
        if self.auto_route.key.as_ref() == Some(&key) {
            return;
        }
        let Some(runner) = &self.runner else { return };
        if across {
            let Some(asked) = account.preview_code_route_across(runner, via) else {
                return;
            };
            self.auto_route = AutoRoutePreview {
                key: Some(key.clone()),
                answer: None,
                task: None,
            };
            self.auto_route.task = Some(cx.spawn(async move |this, cx| {
                let answer = asked.await.ok().and_then(Result::ok).flatten();
                let _ = this.update(cx, |this, cx| {
                    if this.auto_route.key.as_ref() != Some(&key) {
                        return;
                    }
                    this.auto_route.task = None;
                    if let Some((agent, answer)) = answer
                        && this.code_form.auto_any
                    {
                        // The form runs the agent Gateway named: the launch then routes its model as for one agent.
                        this.code_form.take_auto_agent(agent);
                        this.auto_route.answer = Some(answer);
                    }
                    cx.notify();
                });
            }));
            return;
        }
        let Some(asked) = account.preview_code_route(runner, self.code_form.agent, via) else {
            return;
        };
        self.auto_route = AutoRoutePreview {
            key: Some(key.clone()),
            answer: None,
            task: None,
        };
        self.auto_route.task = Some(cx.spawn(async move |this, cx| {
            let answer = asked.await.ok().and_then(Result::ok).flatten();
            let _ = this.update(cx, |this, cx| {
                if this.auto_route.key.as_ref() == Some(&key) {
                    this.auto_route.answer = answer;
                    this.auto_route.task = None;
                    cx.notify();
                }
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::AutoRoutePreview;
    use nexus_client::auto_route::Answer;
    use nexus_workbench::event::Via;

    fn preview(answer: Answer) -> AutoRoutePreview {
        AutoRoutePreview {
            answer: Some(answer),
            ..Default::default()
        }
    }

    #[test]
    fn a_plan_pick_names_its_model_on_every_payer() {
        let plan = preview(Answer::UsePlan {
            model: "claude-opus-5-5".into(),
            effort: None,
        });
        assert_eq!(plan.model(Via::Subscription), Some("claude-opus-5-5"));
        assert_eq!(plan.needs_api(Via::Subscription), None);
    }

    #[test]
    fn a_paid_pick_runs_only_on_a_paying_launch_and_asks_on_the_plan() {
        let paid = preview(Answer::AskToPay {
            model: "claude-sonnet-5-5".into(),
            name: "anthropic/claude-sonnet-5.5".into(),
            access: Some("byok".into()),
            effort: None,
        });
        assert_eq!(paid.model(Via::Subscription), None);
        assert_eq!(paid.needs_api(Via::Subscription), Some("claude-sonnet-5.5"));
        assert_eq!(paid.model(Via::Nexus), Some("claude-sonnet-5-5"));
        assert_eq!(paid.needs_api(Via::Nexus), None);
    }

    #[test]
    fn the_chip_names_the_agent_only_when_auto_picked_it() {
        use super::chip_text;
        use nexus_workbench::event::SessionAgent;
        assert_eq!(chip_text(None, None), "Auto");
        assert_eq!(chip_text(None, Some("claude-opus-5-5")), "Auto · Opus 5.5");
        assert_eq!(chip_text(Some(SessionAgent::Codex), None), "Auto · any agent");
        assert_eq!(
            chip_text(Some(SessionAgent::Codex), Some("gpt-6.1-sol")),
            format!(
                "Auto · {} · {}",
                nexus_workbench::driver::label(SessionAgent::Codex),
                crate::code::model_label("gpt-6.1-sol")
            )
        );
    }

    #[test]
    fn no_answer_and_stop_show_nothing() {
        assert_eq!(AutoRoutePreview::default().model(Via::Subscription), None);
        let stop = preview(Answer::Stop {
            reason: "no_offer".into(),
        });
        assert_eq!(stop.model(Via::Nexus), None);
        assert_eq!(stop.needs_api(Via::Subscription), None);
    }
}
