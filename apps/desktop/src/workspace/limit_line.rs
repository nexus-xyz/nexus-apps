//! A budget or a limit is reached (board P5 of the setup lines), in the window: one amber line over the composer, in
//! the New session strip, over a dock card and in place of a refused chat answer's red error (`crate::limit_stop` has
//! the words). Who may raise it lands on its row in Usage › Limits; everyone else asks, or picks another payer they
//! have. A send pressed meanwhile is held, and goes once the budget is raised or the payer switched.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::session_readiness::Budget;
use nexus_workbench::event::Via;

use super::{View, Workspace};
use crate::limit_stop::{self, Level, Stop};
use crate::palette::Palette;
use crate::setup::{self, Fix, FixKey, SetupLine, state};

/// The send held while the budget is used up.
pub(super) const HELD: &str = "limit:send";
const STRIP: &str = "limit:start";
const COMPOSER: &str = "limit:composer";
const CARD: &str = "limit:card";
const CHAT: &str = "limit:chat";

impl Workspace {
    fn limit_org(&self) -> String {
        self.account
            .as_ref()
            .map(|account| account.org_name.clone())
            .unwrap_or_default()
    }

    /// The used-up budget that blocks a send now. `/v1/spend` names no level: the organization's for those who see its
    /// whole spend, and a member asks anyway.
    pub(super) fn budget_stop(&self) -> Option<Stop> {
        self.budget_line()?;
        Some(Stop::Budget {
            level: Level::Organization,
            used_up: true,
            resumes: self.budget_day(),
        })
    }

    /// The line of `stop`. Its fix waits for the raise first: then the person comes back, and a held send or the
    /// refused answer (`chat`) goes on.
    fn limit_line(
        &self,
        key: &'static str,
        stop: &Stop,
        other: Option<(String, Fix)>,
        chat: bool,
        cx: &mut Context<Self>,
    ) -> SetupLine {
        let this = cx.entity().downgrade();
        limit_stop::line(key, stop, &self.limit_org(), other, move |fix| {
            let needs = fix.required();
            let raise = Fix::act(move |window, cx| {
                let fix = fix.clone();
                let ran = this.update(cx, |workspace, cx| {
                    workspace.wait_for_raise(chat, cx);
                    workspace.run_setup_fix(key.into(), fix, window, cx);
                });
                if let Err(error) = ran {
                    eprintln!("nexus-desktop-gpui: limit: raise: {error}");
                }
            });
            match needs {
                Some(permission) => raise.needs(permission),
                None => raise,
            }
        })
    }

    /// The New session strip's line, with the person's plan as another payer when it can start the session.
    pub(super) fn render_start_limit(
        &self,
        plan: Option<String>,
        pager: Option<Div>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let stop = self.budget_stop().unwrap_or(Stop::Budget {
            level: Level::Organization,
            used_up: true,
            resumes: None,
        });
        let other = plan.map(|plan| {
            let this = cx.entity().downgrade();
            let pick = Fix::act(move |_, cx| {
                let picked = this.update(cx, |workspace, cx| {
                    workspace.code_form.set_via(Some(Via::Subscription));
                    workspace.limit_check(cx);
                    cx.notify();
                });
                if let Err(error) = picked {
                    eprintln!("nexus-desktop-gpui: limit: use the plan: {error}");
                }
            });
            (format!("Use {plan}"), pick)
        });
        let line = self.limit_line(STRIP, &stop, other, false, cx);
        let fixer = self.fixer(cx);
        v_flex()
            .px(px(8.))
            .pt(px(8.))
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(palette.muted)
            .children(pager.map(|pager| h_flex().justify_end().child(pager)))
            .child(line.render(&fixer, palette, cx))
            .into_any_element()
    }

    /// Over the composer of a chat or a Code session while the budget is used up. The New session strip says it there.
    pub(super) fn render_limit_line(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let line = matches!(self.view, View::Chat | View::Code(_))
            .then(|| self.budget_stop())
            .flatten()
            .map(|stop| self.limit_line(COMPOSER, &stop, None, false, cx));
        let fixer = self.fixer(cx);
        setup::slot("composer-limit", line, &fixer, palette, cx)
            .map(|line| div().px(px(8.)).pt(px(8.)).child(line).into_any_element())
    }

    /// Over a dock card, which hides the composer: the budget's line in place of the red note.
    pub(super) fn render_card_limit(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stop = self.budget_stop()?;
        let line = self.limit_line(CARD, &stop, None, false, cx);
        let fixer = self.fixer(cx);
        Some(line.render(&fixer, palette, cx).into_any_element())
    }

    /// A refused chat answer's line, in place of its red error: the stop's fix and, under the last answer, Try again.
    pub(super) fn chat_limit_line(&self, stop: &Stop, last: bool, cx: &mut Context<Self>) -> SetupLine {
        let line = self.limit_line(CHAT, stop, None, true, cx);
        if !last {
            return line;
        }
        let this = cx.entity().downgrade();
        line.fix(
            "Try again",
            Fix::act(move |_, cx| {
                if let Err(error) = this.update(cx, |workspace, cx| workspace.retry(cx)) {
                    eprintln!("nexus-desktop-gpui: limit: try again: {error}");
                }
            }),
        )
    }

    /// Send was pressed while the budget is used up: hold it, and send it once the budget is raised or the payer
    /// switched. False when no budget blocks.
    pub(super) fn hold_for_limit(&mut self, cx: &mut Context<Self>) -> bool {
        if self.budget_line().is_none() {
            return false;
        }
        let held: FixKey = HELD.into();
        if !state::waiting(&held, cx) {
            let this = cx.entity().downgrade();
            state::on_done(held, cx, move |window, cx| {
                // The composer kept the text the person sent; nothing else goes.
                let sent = this.update(cx, |workspace, cx| {
                    if matches!(workspace.view, View::Chat | View::Code(_) | View::NewCode) {
                        workspace.submit(window, cx);
                    }
                });
                if let Err(error) = sent {
                    eprintln!("nexus-desktop-gpui: limit: send the held message: {error}");
                }
            });
        }
        true
    }

    /// Once a limit is saved: back from the Settings the fix opened, and read the budget again, which `limit_check`
    /// follows. `chat`: ask the refused answer again.
    fn wait_for_raise(&mut self, chat: bool, cx: &mut Context<Self>) {
        let raised: FixKey = limit_stop::RAISED.into();
        if state::waiting(&raised, cx) {
            return;
        }
        let this = cx.entity().downgrade();
        state::on_done(raised, cx, move |window, cx| {
            let resumed = this.update(cx, |workspace, cx| {
                if workspace.in_settings_for_fix() {
                    workspace.back_to_app(window, cx);
                }
                workspace.recheck_readiness(cx);
                if chat && matches!(workspace.view, View::Chat) && workspace.chat_waits_for_limit() {
                    workspace.retry(cx);
                }
            });
            if let Err(error) = resumed {
                eprintln!("nexus-desktop-gpui: limit: raised: {error}");
            }
        });
    }

    /// The budget was read again, or another payer picked: once nothing blocks, the held send goes. Deferred, because
    /// the send updates this workspace.
    pub(super) fn limit_check(&mut self, cx: &mut Context<Self>) {
        let held: FixKey = HELD.into();
        if !state::waiting(&held, cx)
            || !limit_lifted(
                self.readiness_current(),
                &self.readiness.budget,
                self.budget_line().is_some(),
            )
        {
            return;
        }
        let window = self.window_handle;
        cx.defer(move |cx| {
            if let Err(error) = window.update(cx, |_, window, cx| setup::done(&held, window, cx)) {
                eprintln!("nexus-desktop-gpui: limit: lifted: {error}");
            }
        });
    }

    /// The last chat answer was refused for a budget or a limit.
    fn chat_waits_for_limit(&self) -> bool {
        self.entries
            .last()
            .and_then(|entry| entry.error.as_ref())
            .is_some_and(|error| limit_stop::stop_of(error).is_some())
    }
}

/// Whether the budget no longer blocks: read again for this account and not used up, or used up and no longer paying
/// through Gateway (another payer). An unknown or another account's budget lifts nothing.
fn limit_lifted(current: bool, budget: &Budget, blocks: bool) -> bool {
    current && !blocks && !matches!(budget, Budget::Unknown)
}

#[cfg(test)]
#[path = "limit_line_tests.rs"]
mod tests;
