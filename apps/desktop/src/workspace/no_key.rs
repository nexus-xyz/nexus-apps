//! Who pays has no key (board P1 of the setup lines): the organization has no key for the agent's provider. The New
//! session strip and a refused chat answer show one setup line, "Acme has no Anthropic key yet.", with "Add key" for
//! who may add one and Ask for everyone else. A send pressed meanwhile is held, and goes once the key lands: the
//! Providers page completes `providers::key_added`, the window reads who pays again, and the start line's key is done.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::event::{SessionAgent, Via};

use super::{View, Workspace};
use crate::pages::{PageId, providers};
use crate::palette::Palette;
use crate::setup::{self, Fix, FixKey, SetupLine, access, state};

/// The New session send that waits for the key.
pub(super) const START: &str = "start:no-key";
/// The refused chat answer's line.
const CHAT: &str = "chat:no-key";
/// Who may add the organization's provider keys.
const MANAGE: &str = "provider_keys.manage";

/// A missing key in the New session strip, as `start_blocker_items` finds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NoKey {
    /// The provider's name, as Gateway and the agent's vendor write it ("Anthropic").
    pub provider: String,
    /// The person's plan that can start the session instead ("Max plan"), when it can.
    pub plan: Option<String>,
}

/// The provider whose key `agent` runs on through Gateway: its plan's vendor sells it; Nexus's own agent runs on
/// Anthropic's.
pub(super) fn provider(agent: SessionAgent) -> &'static str {
    agent
        .coding()
        .map_or("Anthropic", |coding| coding.entry().identity.vendor)
}

/// The line's words: what is missing, naming the account.
pub(super) fn what(org: &str, provider: &str) -> String {
    // The strip says "your organization" before the account is read.
    let org = match org {
        "" | "your organization" => "Your organization",
        org => org,
    };
    format!("{org} has no {provider} key yet.")
}

/// What Ask copies for the owners and admins.
fn ask_message(org: &str, provider: &str) -> String {
    let org = if org.is_empty() { "our organization" } else { org };
    let article = if provider.starts_with(['A', 'E', 'I', 'O', 'U']) {
        "an"
    } else {
        "a"
    };
    format!(
        "Could you add {article} {provider} key for {org} in Nexus Gateway? `nexus keys add {}`",
        provider.to_lowercase()
    )
}

/// Settings › Providers on the provider's row, for who may add a key.
pub(super) fn add_key(provider: &str) -> Fix {
    Fix::open_at(PageId::Providers, providers::anchor(provider)).needs(MANAGE)
}

impl Workspace {
    fn org_label(&self) -> String {
        self.account
            .as_ref()
            .map(|account| account.org_name.clone())
            .unwrap_or_default()
    }

    /// Under Who pays, when the organization's key pays and there is none: the key, "No Anthropic key yet", and for who
    /// may add one, "Add ›" to the provider's row. Everyone else reads the strip's line, which asks.
    pub(super) fn no_key_reason(&self, agent: SessionAgent, palette: Palette, cx: &mut Context<Self>) -> Div {
        let provider = provider(agent);
        let can_add = matches!(
            access::decide(Some(MANAGE), &self.fixer(cx).access(cx)),
            access::Decision::Fix
        );
        h_flex()
            .gap(px(6.))
            .text_color(palette.muted)
            .child(Icon::new(Lucide::KeyRound).size(px(12.)).text_color(palette.icon))
            .child(format!("No {provider} key yet"))
            .when(can_add, |reason| {
                reason.child(
                    div()
                        .id("launch-no-key-add")
                        .text_color(palette.foreground)
                        .cursor_pointer()
                        .hover(|style| style.text_color(palette.strong))
                        .child("Add ›")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_launch_menu(cx);
                            this.run_setup_fix(START.into(), add_key(provider), window, cx);
                        })),
                )
            })
    }

    /// The strip's line: "Use Max plan" when a plan can start it, and "Add key".
    pub(super) fn no_key_line(&self, no_key: &NoKey, cx: &mut Context<Self>) -> SetupLine {
        let org = self.org_label();
        let mut line = SetupLine::new(START, Lucide::KeyRound, what(&org, &no_key.provider));
        if let Some(plan) = &no_key.plan {
            let this = cx.entity().downgrade();
            line = line.fix(
                format!("Use {plan}"),
                Fix::act(move |window, cx| {
                    let picked = this.update(cx, |workspace, cx| {
                        workspace.code_form.set_via(Some(Via::Subscription));
                        workspace.key_check(window, cx);
                        cx.notify();
                    });
                    if let Err(error) = picked {
                        eprintln!("nexus-desktop-gpui: no key: use the plan: {error}");
                    }
                }),
            );
        }
        line.fix("Add key", add_key(&no_key.provider))
            .ask(ask_message(&org, &no_key.provider), "add one")
    }

    /// The New session's missing key, if that is what blocks it.
    pub(super) fn start_no_key(&self) -> Option<NoKey> {
        self.start_blocker_items()
            .iter()
            .find_map(|item| item.no_key().cloned())
    }

    /// Send was pressed while the key is missing: hold it, and send it once the key lands. False when something else
    /// blocks the start.
    pub(super) fn hold_for_key(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(no_key) = self.start_no_key() else {
            return false;
        };
        let start: FixKey = START.into();
        if !state::waiting(&start, cx) {
            let this = cx.entity().downgrade();
            state::on_done(start, cx, move |window, cx| {
                // Only from the New session it was held in: the composer kept its text.
                let sent = this.update(cx, |workspace, cx| {
                    if matches!(workspace.view, View::NewCode) {
                        workspace.submit(window, cx);
                    }
                });
                if let Err(error) = sent {
                    eprintln!("nexus-desktop-gpui: no key: send the held message: {error}");
                }
            });
        }
        self.wait_for_key(&no_key.provider, false, cx);
        true
    }

    /// When the Providers page saves `provider`'s key: back from the Settings the fix opened, and read who pays again,
    /// which `key_check` follows. `chat`: retry the refused answer instead.
    fn wait_for_key(&mut self, provider: &str, chat: bool, cx: &mut Context<Self>) {
        let added = providers::key_added(provider);
        if state::waiting(&added, cx) {
            return;
        }
        let this = cx.entity().downgrade();
        state::on_done(added, cx, move |window, cx| {
            let resumed = this.update(cx, |workspace, cx| {
                if workspace.in_settings_for_fix() {
                    workspace.back_to_app(window, cx);
                }
                if let Some(runner) = &workspace.runner {
                    runner.load_access();
                }
                if chat && matches!(workspace.view, View::Chat) && workspace.chat_waits_for_key() {
                    workspace.retry(cx);
                }
            });
            if let Err(error) = resumed {
                eprintln!("nexus-desktop-gpui: no key: key added: {error}");
            }
        });
    }

    /// Who pays was read again, or a plan was picked: once nothing is missing, the held send goes. Deferred, because the
    /// send updates this workspace.
    pub(super) fn key_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start: FixKey = START.into();
        if !state::waiting(&start, cx) || !matches!(self.view, View::NewCode) || self.start_no_key().is_some() {
            return;
        }
        window.defer(cx, move |window, cx| setup::done(&start, window, cx));
    }

    /// The last chat answer was refused for a missing key.
    fn chat_waits_for_key(&self) -> bool {
        self.entries
            .last()
            .and_then(|entry| entry.error.as_ref())
            .is_some_and(|error| crate::pay_notice::missing_key(error).is_some())
    }

    /// A chat answer was refused: when it was for a missing key, it is asked again once the key lands.
    pub(super) fn hold_chat_for_key(&mut self, refusal: &str, cx: &mut Context<Self>) {
        if let Some(provider) = crate::pay_notice::missing_key(refusal) {
            self.wait_for_key(&provider.to_string(), true, cx);
        }
    }

    /// A refused chat answer's line, in place of its red error: "Add key" and, under the last answer, Try again.
    pub(super) fn chat_no_key_line(&self, provider: &str, last: bool, cx: &mut Context<Self>) -> SetupLine {
        let org = self.org_label();
        let mut line = SetupLine::new(CHAT, Lucide::KeyRound, what(&org, provider)).fix("Add key", add_key(provider));
        if last {
            let this = cx.entity().downgrade();
            line = line.fix(
                "Try again",
                Fix::act(move |_, cx| {
                    if let Err(error) = this.update(cx, |workspace, cx| workspace.retry(cx)) {
                        eprintln!("nexus-desktop-gpui: no key: try again: {error}");
                    }
                }),
            );
        }
        line.ask(ask_message(&org, provider), "add one")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn the_line_names_the_account_and_the_provider() {
        assert_eq!(what("Acme", "Anthropic"), "Acme has no Anthropic key yet.");
        assert_eq!(what("", "OpenAI"), "Your organization has no OpenAI key yet.");
        assert_eq!(
            ask_message("Acme", "Anthropic"),
            "Could you add an Anthropic key for Acme in Nexus Gateway? `nexus keys add anthropic`"
        );
        assert!(ask_message("Acme", "Google").starts_with("Could you add a Google key"));
    }

    /// No key: the strip is the setup line, a send is held without the old note, and it goes once a key pays.
    #[gpui_kit::test]
    fn a_send_held_for_a_missing_key_goes_once_the_key_lands(cx: &mut gpui_kit::TestAppContext) {
        use crate::code_run::AgentAccess;
        use crate::palette::ThemeChoice;
        use crate::workspace::machine_source::MachineSource;
        use nexus_workbench::api::AgentInfo;
        use serde_json::json;
        use std::sync::Arc;

        let _agents = crate::code_run::fixture_agents(Vec::new());
        cx.update(gpui_kit::init);
        let (gateway, _gateway_events) = crate::gateway::Gateway::start();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            Workspace::with_machine_source(
                Arc::new(gateway),
                events,
                arrivals,
                false,
                MachineSource::isolated(),
                window,
                cx,
            )
        });
        let access = |gateway: serde_json::Value| AgentAccess {
            subscriptions: false,
            answer: Some(json!({ "agents": [{ "agent": "claude", "routes": [gateway] }] })),
        };
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.auth = crate::workspace::Auth::SignedIn;
            workspace.view = View::NewCode;
            workspace.cloud_ui.on = false;
            workspace.code_form.project = None;
            workspace.code_form.agent = SessionAgent::Claude;
            workspace.code_form.via = Some(Via::Nexus);
            workspace.code_form.access = Some(access(json!({ "route": "gateway", "available": false })));
            workspace.code_agents = vec![AgentInfo {
                agent: SessionAgent::Claude,
                path: Some("/fixture/claude".into()),
                version: None,
                models: None,
                problem: None,
            }];
            let no_key = workspace.start_no_key().expect("the missing key blocks the start");
            assert_eq!(no_key.provider, "Anthropic");
            assert_eq!(no_key.plan, None, "plans are off");
            let palette = Palette::for_choice(ThemeChoice::Dark, gpui_kit::WindowAppearance::Dark);
            assert!(workspace.render_start_blockers(palette, cx).is_some());

            workspace
                .composer
                .update(cx, |input, cx| input.set_value("Profile the decoder", window, cx));
            workspace.composer_empty = false;
            workspace.submit_code("Profile the decoder".into(), false, window, cx);
            assert_eq!(workspace.composer_note, None, "the line is the item: no note under it");
            assert!(state::waiting(&START.into(), cx));
            assert!(state::waiting(&providers::key_added("anthropic"), cx));
            assert_eq!(workspace.composer.read(cx).value().as_ref(), "Profile the decoder");

            // Still no key: nothing goes.
            workspace.key_check(window, cx);
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert!(state::waiting(&START.into(), cx)));
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.code_form.access = Some(access(
                json!({ "route": "gateway", "access": "byok", "available": true }),
            ));
            assert!(workspace.start_no_key().is_none());
            workspace.key_check(window, cx);
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert!(!state::waiting(&START.into(), cx), "the held send went"));
    }

    #[test]
    fn add_key_opens_the_providers_row_for_who_may_manage_keys() {
        let Fix::Open { page, focus, needs } = add_key("Anthropic") else {
            panic!("Add key opens a page");
        };
        assert_eq!(page, PageId::Providers);
        assert_eq!(focus, Some("provider-anthropic"));
        assert_eq!(needs, Some(MANAGE));
    }
}
