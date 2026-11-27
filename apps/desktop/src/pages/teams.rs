//! Settings › Teams: the organization's teams as a tree, who leads each and how many people are in it. Owners and
//! admins create teams, see who is pending (added by email before joining) and cancel them. A team opens here too:
//! its place in the tree, its child teams, members and pending members; owners and admins add and remove people,
//! rename it, move it and delete it; anyone in it makes it their default. It copies Nexus UI's
//! `ui/src/components/nexus-ui/settings/org/teams.tsx` and `app/app/[org]/(settings)/teams/[slug]/page.tsx`.
//!
//! A team's Models section is the policy view for one team (`policy::team_view`): its operator controls and its
//! allowed models. Its spend over 30 days (`GET /v1/analytics/usage?team=…&groupBy=person`) shows as a figure and per
//! member, to whoever may read it.

use std::collections::HashSet;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

use super::{Ctx, Load, PageId, Verb, fetch, palette, policy, read, read_in, ui, usage};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| TeamsPage::new(ctx, window, cx));
    Some(view.into())
}

mod layout;
mod model;
mod state;
#[cfg(test)]
mod tests;
mod view;
pub(super) use view::build_locked;

use layout::*;
use model::*;
use state::*;
