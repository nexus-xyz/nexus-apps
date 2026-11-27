//! Settings › MCP servers › Outside access (the Nexus app's `OutsideAccessSettings`, `outside-access.tsx`): the switch
//! that lets scripts, the `nexus` CLI and other agents use Code sessions through a socket only this person can open,
//! and the clients approved, each with its scope, its last use and Revoke. One card: the head says Off or On and how
//! many clients, the switch is its first row, and the footnote says where clients connect. Privacy and MCP servers
//! both show it. Only this build's runner serves the socket (`outside.rs`), so the section shows only while this build
//! owns the Code sessions store.

use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_mcp::outside::grants::Scope;

use super::ui;
use crate::outside::{OutsideAccess, View};
use crate::palette::Palette;

/// What the switch's line says. It keeps one length on and off; the state is in the head (`state`), and the socket in
/// the footnote (`footnote`).
fn note(view: &View) -> &'static str {
    if view.supported {
        "Scripts, the nexus CLI and other agents can list, start, read, message and stop your Code sessions."
    } else {
        "Not on Windows yet."
    }
}

/// The state in the card's head: Off, or On and how many clients it approved. On means the socket listens; a switch
/// turned on whose socket failed reads Off, and its problem shows under the card.
fn state(view: &View) -> String {
    if !(view.enabled && view.listening) {
        return "Off".into();
    }
    match view.grants.len() {
        0 => "On".into(),
        1 => "On · 1 client".into(),
        clients => format!("On · {clients} clients"),
    }
}

/// The note under the card: where clients connect while it listens, and who may open it otherwise.
fn footnote(view: &View) -> Option<String> {
    if !view.supported {
        return None;
    }
    Some(
        match view.socket.as_deref().filter(|_| view.enabled && view.listening) {
            Some(socket) => format!("Clients connect through {socket}. Each one asks you first."),
            None => "Only your user account can open the connection. Each client asks you first.".into(),
        },
    )
}

fn scope_text(scope: Scope) -> &'static str {
    match scope {
        Scope::Read => "Read only",
        Scope::Act => "Read and act",
    }
}

/// When a client last used its grant (`ago`), `now` in seconds since the epoch.
fn used(last: Option<i64>, now: i64) -> String {
    let Some(last) = last else {
        return "Never used".into();
    };
    let minutes = (now - last).max(0) / 60;
    match minutes {
        0 => "Used just now".into(),
        1..60 => format!("Used {minutes} min ago"),
        60..1440 => format!("Used {} h ago", minutes / 60),
        _ => format!("Used {} days ago", minutes / 1440),
    }
}

/// The section, or None while this build does not own the store. `problem` is the last switch or Revoke error.
pub fn render<T: 'static>(
    outside: Option<Arc<OutsideAccess>>,
    problem: Option<SharedString>,
    on_problem: impl Fn(&mut T, Option<SharedString>) + Copy + 'static,
    palette: Palette,
    cx: &mut Context<T>,
) -> Option<Div> {
    let outside = outside?;
    let view = outside.view();
    let enabled = view.enabled;
    let writable = crate::desktop_settings::writable() && view.supported;
    let switch = ui::switch("outside-access", view.enabled, !writable, palette);
    let switch = if writable {
        let access = outside.clone();
        switch
            .on_click(cx.listener(move |this, _, _, cx| {
                let error = access.set_enabled(!enabled).err().map(SharedString::from);
                on_problem(this, error);
                cx.notify();
            }))
            .into_any_element()
    } else {
        switch.into_any_element()
    };
    let on = view.enabled && view.listening;
    let head = ui::group_head("Outside access", palette).child(ui::status(
        state(&view),
        if on { Palette::success() } else { palette.icon },
        palette,
    ));
    let mut rows = vec![Some(
        ui::setting_row("Let other apps use Code sessions", note(&view), switch, palette).into_any_element(),
    )];
    if view.grants.is_empty() {
        rows.push(Some(
            ui::card_row()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(palette.muted)
                                .child("No approved clients"),
                        )
                        .child(ui::setting_detail("A client shows here once you approve it.", palette)),
                )
                .into_any_element(),
        ));
    }
    let now = nexus_desktop_mcp::outside::grants::now_s();
    for grant in &view.grants {
        let access = outside.clone();
        let id = grant.id.clone();
        let revoke = ui::button(
            SharedString::from(format!("outside-revoke-{}", grant.id)),
            "Revoke",
            palette,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            let error = access.revoke(&id).err().map(SharedString::from);
            on_problem(this, error);
            cx.notify();
        }));
        rows.push(Some(
            ui::card_row()
                .child(ui::icon_tile(Lucide::SquareTerminal, palette))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(palette.strong)
                                .truncate()
                                .child(grant.client.clone()),
                        )
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(ui::pill(scope_text(grant.scope), palette))
                                .child(ui::setting_detail(used(grant.last_used_at, now), palette)),
                        ),
                )
                .child(div().flex_none().child(revoke))
                .into_any_element(),
        ));
    }
    let problem = problem
        .or_else(|| view.problem.clone().map(SharedString::from))
        .map(|problem| ui::error_line(problem, palette).mt(px(8.)));
    Some(
        v_flex()
            .child(head)
            .child(ui::group(rows, palette))
            .children(problem)
            .children(footnote(&view).map(|text| ui::footnote(text, palette))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui_kit::*` brings GPUI's own `test` attribute: the standard one is meant here.
    use core::prelude::v1::test;

    #[test]
    fn says_when_each_client_last_used_its_grant() {
        assert_eq!(used(None, 1_000), "Never used");
        assert_eq!(used(Some(990), 1_000), "Used just now");
        assert_eq!(used(Some(1_000 - 5 * 60), 1_000), "Used 5 min ago");
        assert_eq!(used(Some(0), 2 * 3600 + 59), "Used 2 h ago");
        assert_eq!(used(Some(0), 3 * 86_400), "Used 3 days ago");
    }

    #[test]
    fn the_head_and_footnote_name_the_socket_and_clients_only_while_it_listens() {
        let view = View {
            supported: true,
            enabled: true,
            listening: true,
            socket: Some("/run/user/1/xyz.nexus.desktop/sessions.sock".into()),
            problem: None,
            grants: Vec::new(),
        };
        assert_eq!(state(&view), "On");
        assert!(footnote(&view).expect("footnote").contains("sessions.sock"));
        let off = View {
            listening: false,
            ..view.clone()
        };
        assert_eq!(state(&off), "Off");
        assert!(!footnote(&off).expect("footnote").contains("sessions.sock"));
        assert_eq!(note(&off), note(&view), "the switch's line keeps one length on and off");
        let windows = View {
            supported: false,
            ..view
        };
        assert_eq!(note(&windows), "Not on Windows yet.");
        assert_eq!(footnote(&windows), None);
    }

    #[test]
    fn the_head_counts_approved_clients_while_it_listens() {
        let grant = |id: &str| nexus_desktop_mcp::outside::grants::GrantView {
            id: id.into(),
            client: "nexus CLI".into(),
            scope: Scope::Read,
            created_at: 0,
            last_used_at: None,
        };
        let mut view = View {
            supported: true,
            enabled: true,
            listening: true,
            socket: None,
            problem: None,
            grants: vec![grant("a")],
        };
        assert_eq!(state(&view), "On · 1 client");
        view.grants.push(grant("b"));
        assert_eq!(state(&view), "On · 2 clients");
        view.enabled = false;
        assert_eq!(state(&view), "Off");
    }
}
