//! The Issues view when Linear is missing (`issues_view.rs`): not a load failure, but what the view needs. Connect
//! Linear starts the connectors page's OAuth in place (`pages::oauth_request`); where Gateway has no OAuth for it, it
//! opens Settings › Connections on Linear's row. The view re-reads the connections until the account lands, then
//! `setup::done` loads the issues by themselves. A Linear the organization keeps off asks its admins (`policy.manage`).

use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use crate::connector_gap::{CONNECT_OWN, Gap, MANAGE};
use crate::gateway::Verb;
use crate::pages::{PageId, ui};
use crate::palette::Palette;
use crate::setup::access::{self, Decision};
use crate::setup::{self, Fix, FixKey, SetupLine, Tone};

/// The connector the view reads.
const CONNECTOR: &str = "linear";
/// The blocked action: the Issues view's read.
const KEY: &str = "issues:linear";
/// How often, and how many times, the view re-reads the connections while the browser is open: five minutes.
const EVERY: Duration = Duration::from_secs(3);
const TRIES: usize = 100;

/// The headline, the line under it and the button, for someone who can connect Linear themself.
pub(super) fn connect_words(gap: Gap) -> (&'static str, &'static str, &'static str) {
    if gap == Gap::Expired {
        (
            "Reconnect Linear to see your issues",
            "Linear stopped accepting your saved sign-in.",
            "Reconnect Linear",
        )
    } else {
        (
            "Connect Linear to see your issues",
            "Nexus reads the Linear issues assigned to you.",
            "Connect Linear",
        )
    }
}

impl Workspace {
    fn issues_org_words(&self) -> String {
        match self.account.as_ref().map(|account| account.org_name.trim()) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => "your organization".into(),
        }
    }

    /// The empty state for a missing Linear: its logo, what the view needs, and the one way to get it.
    pub(super) fn render_issues_missing(&self, gap: Gap, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let fixer = self.fixer(cx);
        let logo = match crate::assets::connector(CONNECTOR) {
            Some((path, color)) => ui::mark_tile(path, color, palette),
            None => ui::letter_tile("Linear", palette),
        };
        let state = v_flex().w_full().items_center().pt(px(48.)).gap(px(8.)).child(logo);
        let connects = matches!(gap, Gap::NotConnected | Gap::Expired)
            && access::decide(Some(CONNECT_OWN), &fixer.access(cx)) == Decision::Fix;
        if !connects {
            // Who can't connect, a Linear the organization keeps off, or one this Gateway lacks: the setup line says
            // what is missing, and asks who can.
            let line = self.issues_missing_line(gap, cx);
            return state
                .children(
                    setup::slot("issues-setup", Some(line), &fixer, palette, cx)
                        .map(|line| div().mt(px(8.)).w_full().max_w(px(560.)).child(line)),
                )
                .into_any_element();
        }
        let (headline, why, label) = connect_words(gap);
        let action = if self.issues.connecting {
            self.render_issues_waiting(palette, cx)
        } else {
            ui::primary_button("issues-connect", label, palette)
                .on_click(cx.listener(|this, _, window, cx| this.connect_linear(window, cx)))
                .into_any_element()
        };
        state
            .child(
                div()
                    .mt(px(4.))
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child(headline),
            )
            .child(div().text_size(px(13.)).text_color(palette.muted).child(why))
            .child(div().mt(px(8.)).child(action))
            .into_any_element()
    }

    /// The setup line for who can't connect Linear here, a Linear kept off, or one this Gateway doesn't offer.
    fn issues_missing_line(&self, gap: Gap, cx: &mut Context<Self>) -> SetupLine {
        let org = self.issues_org_words();
        let this = cx.entity().downgrade();
        let connect = Fix::act(move |window, cx| {
            if let Err(error) = this.update(cx, |this, cx| this.connect_linear(window, cx)) {
                eprintln!("nexus-desktop-gpui: connect Linear: {error}");
            }
        });
        match gap {
            Gap::NotConnected | Gap::Expired => {
                SetupLine::new(KEY, Lucide::Link, format!("You can't connect Linear in {org} yet."))
                    .fix("Connect Linear", connect.needs(CONNECT_OWN))
                    .ask("Could you connect Linear for me in Nexus?", "connect it")
                    .main()
            }
            Gap::Off => {
                let rules = match crate::pages::row_anchor(CONNECTOR) {
                    Some(anchor) => Fix::open_at(PageId::ConnectorRules, anchor),
                    None => Fix::open(PageId::ConnectorRules),
                };
                SetupLine::new(KEY, Lucide::Lock, format!("Linear isn't on for you in {org}."))
                    .fix("Turn on", rules.needs(MANAGE))
                    .ask(
                        "Could you turn on the Linear connector for me in Nexus? It's in Settings › Integrations.",
                        "turn it on",
                    )
                    .main()
            }
            Gap::NotOffered => SetupLine::new(KEY, Lucide::Info, "This Gateway doesn't offer Linear yet.")
                .why("Whoever runs it can turn it on.")
                .tone(Tone::Info),
        }
    }

    /// While the browser is open: a still line (it waits on the person), and Try again to open it again.
    fn render_issues_waiting(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap(px(10.))
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .child(ui::icon(Lucide::Globe, palette.icon))
                    .child("Waiting for Linear in your browser…"),
            )
            .child(
                ui::ghost_button("issues-connect-again", "Try again", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.connect_linear(window, cx))),
            )
            .into_any_element()
    }

    /// A real load failure: the red ✕, Gateway's words, and Try again.
    pub(super) fn render_issues_failed(
        &self,
        error: SharedString,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity().downgrade();
        let again = Fix::act(move |_, cx| {
            let reloaded = this.update(cx, |this, cx| {
                this.issues.list = None;
                this.load_issues(cx);
                cx.notify();
            });
            if let Err(error) = reloaded {
                eprintln!("nexus-desktop-gpui: issues: {error}");
            }
        });
        let line = SetupLine::new("issues:read", Lucide::X, "Your issues didn't load.")
            .tone(Tone::Failed)
            .why(error)
            .fix("Try again", again);
        line.render(&self.fixer(cx), palette, cx).into_any_element()
    }

    /// Start Linear's OAuth in the browser, as Settings › Connections does; without OAuth here, open Linear's row there.
    /// Then wait for the account, and load the issues when it lands.
    pub(super) fn connect_linear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Connecting is an account action: signed out, there is no account to connect Linear to.
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let key: FixKey = KEY.into();
        // One wait at a time: a second press replaces the first.
        setup::forget(&key, cx);
        let this = cx.entity().downgrade();
        setup::on_done(key.clone(), cx, move |_, cx| {
            let reloaded = this.update(cx, |this, cx| {
                this.issues.connecting = false;
                this.load_issues(cx);
                cx.notify();
            });
            if let Err(error) = reloaded {
                eprintln!("nexus-desktop-gpui: issues after Linear connected: {error}");
            }
        });
        self.issues.connecting = true;
        let start = crate::pages::oauth_request(&account_gateway, CONNECTOR, "user");
        self.issues.watch = cx.spawn_in(window, async move |this, cx| {
            let url = crate::pages::authorize_url(start.await);
            let started = this.update_in(cx, |this, window, cx| match url {
                Ok(url) => crate::external_link::open(&url, cx),
                // No OAuth here (a key form), or Gateway refused it: Linear's row in Settings › Connections.
                Err(error) => {
                    eprintln!("nexus-desktop-gpui: Linear OAuth: {error}");
                    let fix = match crate::pages::row_anchor(CONNECTOR) {
                        Some(anchor) => Fix::open_at(PageId::Connectors, anchor),
                        None => Fix::open(PageId::Connectors),
                    };
                    this.run_setup_fix(KEY.into(), fix, window, cx);
                }
            });
            if started.is_err() {
                return;
            }
            for _ in 0..TRIES {
                cx.background_executor().timer(EVERY).await;
                // A sign-out while waiting ends the wait: the account Linear would join is gone.
                let Ok(Some(read)) = this.update(cx, |this, _| {
                    this.account_gateway()
                        .map(|gateway| gateway.request(Verb::Get, "/v1/connections".into(), None))
                }) else {
                    return;
                };
                if let Ok(answer) = read.await
                    && crate::pages::usable_accounts(&answer, CONNECTOR) > 0
                {
                    if let Err(error) = cx.update(|window, cx| setup::done(&KEY.into(), window, cx)) {
                        eprintln!("nexus-desktop-gpui: Linear connected: {error}");
                    }
                    return;
                }
            }
            // Five minutes and no account: the view offers Connect again.
            let gave_up = this.update(cx, |this, cx| {
                this.issues.connecting = false;
                setup::forget(&KEY.into(), cx);
                cx.notify();
            });
            if let Err(error) = gave_up {
                eprintln!("nexus-desktop-gpui: wait for Linear: {error}");
            }
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn the_first_words_say_what_is_missing() {
        assert_eq!(
            connect_words(Gap::NotConnected),
            (
                "Connect Linear to see your issues",
                "Nexus reads the Linear issues assigned to you.",
                "Connect Linear"
            )
        );
        assert_eq!(connect_words(Gap::Expired).2, "Reconnect Linear");
    }
}
