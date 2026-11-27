//! The connectors, on two pages that each hold one person's settings:
//!
//! - [`Side::Mine`], Settings › Connections: the person's own accounts. Only they see and change them.
//! - [`Side::Org`], Settings › Integrations, for owners and admins: who in the organization can use each connector,
//!   the organization's own account, and what agents may change. It applies to everyone.
//!
//! Each group is one settings card, as on This computer. A row is the connector's mark, its name and line, and one
//! status in a few words. Opening it shows the action first (Connect, or who can use it), then what agents can do as
//! two counts, Read and Change, with the list one click away. A filter over the cards finds a connector by name.
//!
//! The per-module write grants and chat's per-connector operations stay in the browser: a row links there.

use std::collections::HashMap;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Map, Value, json};

use super::{Ctx, Load, Verb, fetch, palette, read, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// What a refusal for a missing role reads as.
const ADMINS_ONLY: &str = "Only organization admins can change this.";

/// Whose settings a connectors page holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The person's own accounts: Settings › Connections.
    Mine,
    /// The organization's rules: Settings › Integrations.
    Org,
}

/// Where each side opens in Settings, for the link from the other.
pub(super) const MINE_ADDRESS: &str = "/settings/integrations#connectors";
pub(super) const ORG_ADDRESS: &str = "/settings/connector-rules";

pub(super) fn build_locked(window: &mut Window, cx: &mut App) -> AnyView {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter connectors"));
    ui::locked_page(cx, move |palette| {
        view::locked_toolbar(&input, palette).into_any_element()
    })
}

pub fn build(ctx: Ctx, side: Side, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter connectors"));
        let filter_changes = cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut page = ConnectorsPage {
            ctx,
            side,
            abilities: HashMap::new(),
            abilities_open: None,
            me: Load::Loading,
            catalog: Load::Loading,
            connections: Load::Loading,
            availability: None,
            chat: None,
            open: None,
            inputs: HashMap::new(),
            busy: None,
            confirm: None,
            notice: None,
            refresh_errors: HashMap::new(),
            filter,
            _filter_changes: filter_changes,
            unfolded: Vec::new(),
            key_form: None,
            _oauth_poll: Task::ready(()),
        };
        page.load(cx);
        page
    });
    Some(view.into())
}

/// Start `connector`'s OAuth for `scope` (`user` or `org`): Gateway answers with the vendor's consent page
/// ([`authorize_url`]), which the caller opens in the browser. This page's Connect and a view that misses a connection
/// (`workspace/issues_view.rs`) start it the same way.
pub(crate) fn oauth_request(
    gateway: &crate::workspace::gateway_access::AccountGateway,
    connector: &str,
    scope: &'static str,
) -> crate::backend::Request {
    let path = format!("/v1/connections/{}/oauth", segment(connector));
    gateway.request(Verb::Post, path, Some(json!({ "scope": scope })))
}

/// The consent page in [`oauth_request`]'s answer.
pub(crate) fn authorize_url(answer: Result<Value, String>) -> Result<String, String> {
    answer.and_then(|answer| string(&answer, "authorizeUrl").ok_or_else(|| "Gateway sent no sign-in page.".to_string()))
}

/// How many accounts `GET /v1/connections` lists for `connector` that calls can use now, the person's and the
/// organization's: one to reconnect or a suspended one does not count.
pub(crate) fn usable_accounts(answer: &Value, connector: &str) -> usize {
    parse_connections(answer)
        .iter()
        .filter(|item| item.connector == connector && !item.needs_reconnect && !item.suspended)
        .count()
}

/// The Settings anchor of a connector's row on both connectors pages (`Fix::open_at`), for the connectors a setup line
/// opens. Add one here when a new line needs it.
pub(crate) fn row_anchor(connector: &str) -> Option<&'static str> {
    match connector {
        "linear" => Some("connector-linear"),
        _ => None,
    }
}

/// Discover (`assistant_views/discover.rs`) reads its `short_line` too.
pub(super) mod cards;
mod mine_view;
mod model;
mod org_view;
mod shelves;
mod state;
#[cfg(test)]
mod tests;
mod view;

#[cfg(test)]
#[path = "connectors_policy_tests.rs"]
mod policy_tests;

use cards::*;
// `list` is also a gpui_kit name: say which one the page means.
#[allow(unused_imports)]
use model::list;
use model::*;
use shelves::*;
use state::*;
use view::{block, field_box, notice_line};
