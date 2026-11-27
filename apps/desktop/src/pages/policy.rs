//! Settings › Model policy (`ui/.../settings/model-policy`, `settings/org/model-policy.tsx`): which models people and
//! agents can call through the Gateway. The organization's rule, then Claude Code's and Codex's, each with All / Only
//! these / All except and its default model, or as allow and block patterns. Above them, the operator controls in
//! force (pause, limit, override); below, one sentence that asks Gateway the decision it enforces.
//!
//! A team's Models section (Teams › a team, `teams/[slug]/page.tsx`) is this view for one team (`team_view`): the
//! team's operator controls (`/v1/policies/teams/{slug}/controls`) and its allowed models
//! (`/v1/policies/teams/{slug}`). Its owners and admins, and the leads of the team or a team above it, change them; only
//! owners and admins override a model.

use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

use super::Ctx;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

pub(super) fn build_locked(cx: &mut App) -> AnyView {
    super::ui::locked_page(cx, render::locked)
}

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| PolicyPage::new(ctx, window, cx));
    Some(view.into())
}

/// The team a policy view edits, for Teams › a team.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TeamScope {
    pub slug: String,
    pub name: String,
    pub id: String,
    /// Its ancestors, top level first: slug and name.
    pub path: Vec<(String, String)>,
    /// Its members, for a control on a person: user id and name.
    pub members: Vec<(String, String)>,
}

/// A team's Models section: its operator controls and its allowed models.
pub(super) fn team_view(ctx: Ctx, team: TeamScope, window: &mut Window, cx: &mut App) -> Entity<PolicyPage> {
    cx.new(|cx| PolicyPage::with(ctx, Some(team), window, cx))
}

mod card;
mod controls;
mod model;
mod page;
mod render;
mod test_panel;
mod widgets;

use model::*;
pub(super) use page::PolicyPage;
use widgets::*;

#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod tests;
