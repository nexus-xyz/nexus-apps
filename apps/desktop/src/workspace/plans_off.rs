//! New session with personal plans off for the organization (`crate::setup::plans_off`): the setup line over the
//! composer, and the start that resumes once an owner or admin turns them on.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::event::{SessionAgent, Via};

use super::{View, Workspace};
use crate::palette::Palette;
use crate::setup::{self, Fix, SetupLine, TerminalStep, plans_off};

/// Whether the form wants the person's own plan while the organization has plans off: an agent that runs on a plan
/// only, or a saved plan pick. `None` while Gateway has not answered.
pub(super) fn wants_off_plan(
    agent: SessionAgent,
    plan_only: bool,
    via: Option<Via>,
    auto_any: bool,
    subscriptions: Option<bool>,
) -> bool {
    subscriptions == Some(false)
        && agent.coding().is_some()
        && (plan_only || (!auto_any && via == Some(Via::Subscription)))
}

/// A send pressed while plans were off: it goes once the plan is picked, and only then.
const SEND: &str = "plans:off:send";

/// Once plans are on, go back to the session and pick the plan, once however many fixes wait on it.
fn wait_for_plans(workspace: WeakEntity<Workspace>, cx: &mut App) {
    if setup::state::waiting(&plans_off::key(), cx) {
        return;
    }
    setup::on_done(plans_off::KEY, cx, move |window, cx| {
        if let Err(error) = workspace.update(cx, |workspace, cx| workspace.plans_turned_on(window, cx)) {
            eprintln!("nexus-desktop-gpui: plans on: {error}");
        }
    });
}

impl Workspace {
    /// Send was pressed while plans off stop the start: hold it, and send it once the plan is picked. False when
    /// something else blocks the start.
    pub(super) fn hold_for_plans(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.plans_off_stops_start() {
            return false;
        }
        let this = cx.entity().downgrade();
        wait_for_plans(this.clone(), cx);
        if !setup::state::waiting(&SEND.into(), cx) {
            setup::on_done(SEND, cx, move |window, cx| {
                let sent = this.update(cx, |workspace, cx| {
                    // Only from the New session it was held in: the composer kept its text.
                    if matches!(workspace.view, View::NewCode) && workspace.composer_send_enabled() {
                        workspace.submit(window, cx);
                    }
                });
                if let Err(error) = sent {
                    eprintln!("nexus-desktop-gpui: plans on: send the held message: {error}");
                }
            });
        }
        true
    }

    /// The form's agent needs a plan, and the organization has plans off.
    pub(super) fn plans_off_blocks(&self) -> bool {
        matches!(self.view, View::NewCode)
            && !self.cloud_ui.on
            && wants_off_plan(
                self.code_form.agent,
                super::new_code::plan_only(self.code_form.agent),
                self.code_form.via,
                self.code_form.auto_any,
                self.code_form.access.as_ref().map(|access| access.subscriptions),
            )
    }

    /// Only an agent that runs on a plan alone cannot start: another agent pays with credits.
    pub(super) fn plans_off_stops_start(&self) -> bool {
        self.plans_off_blocks() && super::new_code::plan_only(self.code_form.agent)
    }

    fn plans_off_line(&self, cx: &mut Context<Self>) -> Option<SetupLine> {
        if !self.plans_off_blocks() {
            return None;
        }
        let agent = self.code_form.agent;
        let org = self.account.as_ref().map_or("", |account| account.org_name.as_str());
        let this = cx.entity().downgrade();
        let plan_only = super::new_code::plan_only(agent);
        let use_credits = (!plan_only).then(|| {
            let this = this.clone();
            Fix::act(move |_, cx| {
                let paid = this.update(cx, |workspace, cx| {
                    setup::forget(&plans_off::key(), cx);
                    setup::forget(&SEND.into(), cx);
                    workspace.code_form.set_via(Some(Via::Nexus));
                    workspace.refresh_auto_route(cx);
                    cx.notify();
                });
                if let Err(error) = paid {
                    eprintln!("nexus-desktop-gpui: use credits: {error}");
                }
            })
        });
        // Turn on opens General's row, and waits there: once it is saved, the session starts on the plan.
        let turn_on = Fix::act(move |window, cx| {
            wait_for_plans(this.clone(), cx);
            let opened = this.update(cx, |workspace, cx| {
                workspace.run_setup_fix(plans_off::key(), plans_off::turn_on(), window, cx)
            });
            if let Err(error) = opened {
                eprintln!("nexus-desktop-gpui: turn on plans: {error}");
            }
        })
        .needs(plans_off::PERMISSION);
        let mut line = plans_off::line(org, use_credits, turn_on);
        if plan_only {
            let name = nexus_workbench::driver::label(agent);
            let plan = crate::pages::subscriptions::vendor(agent);
            line = line.why(format!("{name} runs on your {plan} plan only."));
        }
        Some(line)
    }

    /// The line over the composer, in place: the composer's row reveal adds the motion.
    pub(super) fn render_plans_off(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let line = self.plans_off_line(cx)?;
        let fixer = self.fixer(cx);
        Some(
            div()
                .px(px(8.))
                .pt(px(8.))
                .child(line.render(&fixer, palette, cx))
                .into_any_element(),
        )
    }

    /// Under the launch menu's Who pays track: "Max plan is off for Acme.", beside the locked choice.
    pub(super) fn plan_off_reason(&self, plan: &str, org: &str, palette: Palette) -> AnyElement {
        let off = plans_off::off_for(org);
        h_flex()
            .gap(px(5.))
            .text_size(px(12.))
            .text_color(palette.muted)
            .child(Icon::new(Lucide::Lock).size(px(12.)).text_color(palette.icon))
            .child(format!("{plan} · {off}"))
            .into_any_element()
    }

    /// Under the launch menu's Who pays track: the agent's plan needs the agent here, and Install runs its command in
    /// the Workbench terminal.
    pub(super) fn plan_install_reason(
        &self,
        agent: SessionAgent,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = nexus_workbench::driver::label(agent);
        let ssh = self
            .harness_machine()
            .and_then(|id| self.servers.iter().find(|server| server.id == id))
            .map(|server| server.ssh.clone());
        let command = super::harness::install_command(agent, self.gateway.url(), ssh.as_deref());
        let key: setup::FixKey = format!("launch:install:{}", agent.as_str()).into();
        let running = matches!(setup::state::run_state(&key, cx), Some(setup::Run::Running));
        let step = TerminalStep {
            command,
            cwd: None,
            name: Some(format!("Install {name}")),
        };
        h_flex()
            .gap(px(8.))
            .text_size(px(12.))
            .text_color(palette.muted)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(format!("Install {name} to use this plan.")),
            )
            .child(
                div()
                    .id("launch-plan-install")
                    .flex_none()
                    .text_color(palette.foreground)
                    .when(!running, |link| {
                        link.cursor_pointer()
                            .hover(|style| style.text_color(palette.strong))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_launch_menu(cx);
                                this.run_setup_fix(key.clone(), Fix::RunInTerminal(step.clone()), window, cx);
                            }))
                    })
                    .child(if running { "Installing…" } else { "Install" }),
            )
            .into_any_element()
    }

    /// Gateway's answer says plans are on: a Turn on that waits resumes, wherever it was turned on.
    pub(super) fn plans_access_read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on = self
            .code_form
            .access
            .as_ref()
            .is_some_and(|access| access.subscriptions);
        if on && setup::state::waiting(&plans_off::key(), cx) {
            let key = plans_off::key();
            window.defer(cx, move |window, cx| setup::done(&key, window, cx));
        }
    }

    /// Plans are on now (General saved it): back to the session, on the plan. A send held while they were off goes
    /// then; any other text stays in the composer, unsent.
    pub(super) fn plans_turned_on(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(access) = self.code_form.access.as_mut() {
            access.subscriptions = true;
        }
        if matches!(self.view, View::Settings(_)) {
            self.back_to_app(window, cx);
        } else if let Some(runner) = &self.runner {
            runner.load_access();
        }
        if !matches!(self.view, View::NewCode) {
            // The person left the New session it was held in: it never goes on its own.
            setup::forget(&SEND.into(), cx);
            return;
        }
        let agent = self.code_form.agent;
        if agent.coding().is_some() && !super::new_code::plan_only(agent) {
            self.code_form.set_via(Some(Via::Subscription));
            self.refresh_auto_route(cx);
        }
        let held: setup::FixKey = SEND.into();
        if setup::state::waiting(&held, cx) {
            window.defer(cx, move |window, cx| setup::done(&held, window, cx));
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The glob import of gpui_kit brings its own `test` macro: name the built-in one.
    use core::prelude::v1::test;

    #[test]
    fn a_plan_pick_or_a_plan_only_agent_waits_on_plans() {
        let claude = SessionAgent::Claude;
        let plan = Some(Via::Subscription);
        assert!(wants_off_plan(claude, false, plan, false, Some(false)));
        assert!(wants_off_plan(
            SessionAgent::Antigravity,
            true,
            None,
            false,
            Some(false)
        ));
        // Credits, Auto, plans on, or Gateway not answered yet: nothing waits.
        assert!(!wants_off_plan(claude, false, Some(Via::Nexus), false, Some(false)));
        assert!(!wants_off_plan(claude, false, None, false, Some(false)));
        assert!(!wants_off_plan(claude, false, plan, true, Some(false)));
        assert!(!wants_off_plan(claude, false, plan, false, Some(true)));
        assert!(!wants_off_plan(claude, false, plan, false, None));
    }
}
