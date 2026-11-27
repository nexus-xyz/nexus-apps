//! Personal plans are off for the organization: the one setup line New session and Settings › Coding agents show, and
//! its fix, the "Agents on personal plans" row of Settings › General. Board P2 of the "Desktop · Setup that blocks an
//! action" page.

use gpui_kit::assets::IconName as Lucide;

use super::{Fix, FixKey, SetupLine};
use crate::pages::PageId;

/// The blocked action: a start, or a pick, on the person's own plan.
pub const KEY: &str = "plans:off";
/// The row of Settings › General that turns personal plans on (`pages/general.rs`).
pub const ANCHOR: &str = "personal-agent-plans";
/// What General checks before it saves the row (`PUT /v1/orgs/settings`).
pub const PERMISSION: &str = "org.settings";

pub fn key() -> FixKey {
    KEY.into()
}

/// The organization's name for a line, or its plain stand-in.
fn org_or_yours(org: &str) -> &str {
    if org.trim().is_empty() {
        "Your organization"
    } else {
        org
    }
}

/// "Off for Acme": why a plan choice is locked, shown beside it.
pub fn off_for(org: &str) -> String {
    if org.trim().is_empty() {
        "Off for your organization".into()
    } else {
        format!("Off for {org}")
    }
}

/// Open General on the row, for who may change it.
pub fn turn_on() -> Fix {
    Fix::open_at(PageId::General, ANCHOR).needs(PERMISSION)
}

/// "Acme turned off personal plans." with Use credits (when another payer can run the agent) and Turn on. Who can't
/// turn them on sees Ask, naming who can. `turn_on` is [`turn_on`], or a step that also waits to resume.
pub fn line(org: &str, use_credits: Option<Fix>, turn_on: Fix) -> SetupLine {
    let org = org_or_yours(org);
    let mut line = SetupLine::new(KEY, Lucide::Lock, format!("{org} turned off personal plans."));
    if let Some(fix) = use_credits {
        line = line.fix("Use credits", fix);
    }
    line.fix("Turn on", turn_on).ask(
        format!("Could you turn on Agents on personal plans for {org}? It is in Nexus, Settings › General."),
        "turn them on",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_name_the_organization() {
        assert_eq!(off_for("Acme"), "Off for Acme");
        assert_eq!(off_for(" "), "Off for your organization");
        assert_eq!(org_or_yours(""), "Your organization");
    }

    #[test]
    fn turn_on_opens_the_row_for_who_may_change_it() {
        let Fix::Open { page, focus, needs } = turn_on() else {
            panic!("Turn on opens General");
        };
        assert_eq!(page, PageId::General);
        assert_eq!(focus, Some(ANCHOR));
        assert_eq!(needs, Some(PERMISSION));
    }
}
