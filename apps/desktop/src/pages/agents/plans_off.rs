//! Settings › Coding agents with personal plans off for the organization: one setup line per agent that has a plan
//! (`crate::setup::plans_off`), in place of the "Needs an admin" step and the refused save.

use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_client::agent_setup::SetupStep;
use nexus_client::agents::Route;

use super::{Access, AgentEntry, AgentsPage};
use crate::setup::{Fix, plans_off};

/// The step the line stands for.
pub(super) const STEP: &str = "allow_subscriptions";

/// Whether the line shows for `entry`: it has a plan, and the organization has plans off, or refused the plan.
pub(super) fn shows(entry: &AgentEntry, access: &Access, refused: bool) -> bool {
    !access.unsupported
        && entry.routes.iter().any(|view| view.route == Route::Subscription)
        && (!access.agent_subscriptions || refused)
}

/// The steps without the one the line stands for.
pub(super) fn without_line(steps: &[SetupStep]) -> Vec<SetupStep> {
    steps.iter().filter(|step| step.id != STEP).cloned().collect()
}

/// Gateway refused the plan because of the organization or a role (`refusal`'s 403 words).
pub(super) fn refused_by_org(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("owner") || lower.contains("admin") || lower.contains("forbidden")
}

impl AgentsPage {
    /// The line under the agent's card: Use credits while the agent is on its plan, and Turn on.
    /// None when signed out: the line names the organization and runs its fix in the account's window.
    pub(super) fn plans_off_line(&self, entry: &AgentEntry, cx: &mut Context<Self>) -> Option<AnyElement> {
        let account = self.account.as_ref()?;
        let palette = super::palette(cx);
        let gateway_runs = entry
            .routes
            .iter()
            .any(|view| view.route == Route::Gateway && view.available);
        let use_credits = (entry.route == Route::Subscription && gateway_runs).then(|| {
            let page = cx.entity().downgrade();
            let agent = entry.agent.clone();
            Fix::act(move |_, cx| {
                if let Err(error) = page.update(cx, |page, cx| page.choose(agent.clone(), Route::Gateway, cx)) {
                    eprintln!("nexus-desktop-gpui: use credits: {error}");
                }
            })
        });
        let line = plans_off::line(&account.org_name, use_credits, plans_off::turn_on());
        Some(
            div()
                .mt(px(8.))
                .child(line.render(&account.fixer, palette, cx))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn only_the_organizations_refusal_turns_into_the_line() {
        assert!(refused_by_org("403 Forbidden"));
        assert!(refused_by_org("only owners and admins can change this"));
        assert!(!refused_by_org("502 Bad Gateway"));
    }
}
