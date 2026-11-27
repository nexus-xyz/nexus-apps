//! The "This computer" row's fix as a setup line (board P3): Install, or the agent's sign-in, on the row that says what
//! is missing, not only in the checklist below. Install and a terminal sign-in run in the Workbench terminal; when they
//! end well, the page checks this computer again. Never amber text: amber is for a limit.

use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_client::local_agents::{LocalAgent, Login};
use nexus_workbench::event::SessionAgent;

use super::{AgentEntry, AgentsPage};
use crate::agents::setup as agent_setup;
use crate::palette::Palette;
use crate::setup::{self, Fix, SetupLine};

/// What the row's line says and offers, before it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Need {
    Install,
    /// Signed out, or `api_key`: signed in with an API key, which bills the API.
    SignIn {
        api_key: bool,
    },
}

/// What this computer lacks of the agent, when the person can fix it here.
pub(super) fn need(local: Option<&LocalAgent>, plan_route: bool) -> Option<Need> {
    let local = local?;
    if !local.installed {
        return Some(Need::Install);
    }
    if !plan_route {
        return None;
    }
    match local.login {
        Some(Login::SignedOut) => Some(Need::SignIn { api_key: false }),
        Some(Login::ApiKey) => Some(Need::SignIn { api_key: true }),
        _ => None,
    }
}

impl AgentsPage {
    /// The row's setup line, drawn with this page's fixer. `can_sign_in`: the plan is open and Nexus is not signing in.
    pub(super) fn computer_setup(
        &self,
        entry: &AgentEntry,
        need: Need,
        plan: &str,
        can_sign_in: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // The window's fixer comes with the account context, so a signed-out page has no line.
        let account = self.account.as_ref()?;
        let agent = SessionAgent::parse(&entry.agent)?;
        let key = format!("agents:{}", entry.agent);
        let line = match need {
            Need::Install => {
                let command = agent_setup::install_command(agent)?;
                agent_setup::missing_line(key, agent, "this computer", Some(command), false)
            }
            Need::SignIn { api_key } => {
                let what = if api_key {
                    format!("{} is signed in with an API key, not your {plan} plan.", entry.name)
                } else {
                    format!("{} isn't signed in to your {plan} plan.", entry.name)
                };
                if !can_sign_in {
                    SetupLine::new(key, gpui_kit::assets::IconName::KeyRound, what)
                } else {
                    // Claude and Codex sign in on this page, in the checklist's card; the others in the terminal.
                    let here = agent_setup::signs_in_here(agent).then(|| {
                        let plans = self.plans.clone();
                        Fix::act(move |_, cx| {
                            let account = crate::pages::subscriptions::chosen(&crate::desktop_settings::load(), agent);
                            plans.update(cx, |plans, cx| plans.connect_from_checklist(agent, account, cx));
                        })
                    });
                    agent_setup::signed_out_line(key, agent, what, here, false)
                }
            }
        };
        let page = cx.entity().downgrade();
        let fixer = account.fixer.before(move |key, fix, _, cx| {
            if !matches!(fix, Fix::RunInTerminal(_)) {
                return;
            }
            setup::forget(key, cx);
            let page = page.clone();
            setup::on_done(key.clone(), cx, move |_, cx| {
                if let Err(error) = page.update(cx, |page, cx| page.check_all(cx)) {
                    eprintln!("nexus-desktop-gpui: agents: check again: {error}");
                }
            });
        });
        Some(line.render(&fixer, palette, cx).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn local(installed: bool, login: Option<Login>) -> LocalAgent {
        LocalAgent {
            agent: "codex".into(),
            installed,
            path: None,
            version: None,
            login,
            plan: None,
        }
    }

    #[test]
    fn the_row_installs_a_missing_agent_and_signs_in_a_plan_one() {
        assert_eq!(need(None, true), None);
        assert_eq!(need(Some(&local(false, None)), false), Some(Need::Install));
        assert_eq!(
            need(Some(&local(true, Some(Login::SignedOut))), true),
            Some(Need::SignIn { api_key: false })
        );
        assert_eq!(
            need(Some(&local(true, Some(Login::ApiKey))), true),
            Some(Need::SignIn { api_key: true })
        );
        assert_eq!(need(Some(&local(true, Some(Login::Plan))), true), None);
        // Paid by Gateway, the plan's sign-in is not needed.
        assert_eq!(need(Some(&local(true, Some(Login::SignedOut))), false), None);
    }
}
