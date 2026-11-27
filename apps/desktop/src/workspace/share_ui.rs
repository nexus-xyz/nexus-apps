//! The pieces the chat's and the page's Share sheets draw with: the sheet's ring and the link
//! card's shadow, its icon and secondary buttons, a checkbox, and a blocked audience's lock, reason and way on.

use crate::pages::PageId;
use crate::palette::Palette;
use crate::setup::{Fix, FixKey, Fixer, Locked};
use crate::workspace::glyph;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// The sheet's border: the plain border in dark, a softer one in light.
pub(super) fn ring(palette: Palette) -> Hsla {
    if palette.dark {
        palette.border
    } else {
        palette.border.opacity(0.8)
    }
}

/// The crafted content card's soft shadow, under the share link card.
pub(super) fn card_shadow() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: hsla(0., 0., 0., 0.04),
            offset: point(px(0.), px(1.)),
            blur_radius: px(2.),
            spread_radius: px(0.),
            inset: false,
        },
        BoxShadow {
            color: hsla(0., 0., 0., 0.12),
            offset: point(px(0.), px(6.)),
            blur_radius: px(16.),
            spread_radius: px(-10.),
            inset: false,
        },
    ]
}

/// What a blocked audience says at its row's right, and the organization setting that governs it.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Blocked {
    /// The reason in a few words, beside the lock: "Acme turned off public links".
    pub(super) short: String,
    /// The General row that turns it on (`Fix::open_at`).
    pub(super) anchor: &'static str,
    pub(super) label: &'static str,
    /// What a member's Ask sends.
    pub(super) message: String,
}

/// The policy's reason (`Audience::blocked`, `ShareState::blocked`) read for the row.
pub(super) fn blocked(reason: &str, org: &str) -> Blocked {
    let (short, anchor, label, message) = if reason.contains("public links for pages") {
        (
            format!("{org} turned off public links"),
            "pages-public-links",
            "Turn on",
            format!("Could you turn on public links for pages in {org}?"),
        )
    } else if reason.contains("turned off") {
        (
            format!("{org} turned off outside sharing"),
            "sharing-settings",
            "Turn on",
            format!("Could you turn on sharing outside {org}?"),
        )
    } else if reason.contains("named people") {
        (
            "Named people only".into(),
            "sharing-settings",
            "Change",
            format!("Could you allow public links in {org}?"),
        )
    } else if reason.contains("owners and admins") {
        (
            "Owners and admins only".into(),
            "sharing-settings",
            "Change",
            format!("Could you let everyone in {org} share outside it?"),
        )
    } else {
        (reason.into(), "sharing-settings", "Change", reason.into())
    };
    Blocked {
        short,
        anchor,
        label,
        message,
    }
}

/// A blocked audience's lock, reason and way on, in place of its check: who may change the organization's setting
/// gets "Turn on ›" to its row in General; anyone else asks. `close` shuts the sheet first, so Settings opens clear.
pub(super) fn blocked_row_end(
    kind: &str,
    reason: &str,
    org: &str,
    close: impl Fn(&mut Window, &mut App) + 'static,
    fixer: &Fixer,
    palette: Palette,
    cx: &App,
) -> Div {
    let Blocked {
        short,
        anchor,
        label,
        message,
    } = blocked(reason, org);
    let opener = fixer.clone();
    let key: FixKey = format!("share:{kind}").into();
    let open_key = key.clone();
    let turn_on = Fix::act(move |window, cx| {
        close(window, cx);
        opener.run(open_key.clone(), Fix::open_at(PageId::General, anchor), window, cx);
    })
    .needs("org.settings");
    Locked::new(key, short)
        .fix(label, turn_on)
        .ask(message)
        .render(fixer, palette, cx)
}

pub(super) fn icon_button(id: &'static str, icon: Lucide, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|style| style.bg(palette.menu_hover))
        .child(glyph(icon, palette.icon).size(px(16.)))
}

/// A secondary button on the sheet: 28 tall on the `control` fill (the menu hover tone until the shell's token lands).
pub(super) fn secondary(
    id: &'static str,
    icon: Option<Lucide>,
    label: &'static str,
    palette: Palette,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .h(px(28.))
        .px(px(10.))
        .gap(px(6.))
        .rounded(px(6.))
        .bg(palette.menu_hover)
        .text_color(palette.foreground)
        .cursor_pointer()
        .hover(|style| style.bg(palette.selected))
        .children(icon.map(|icon| glyph(icon, palette.icon).size(px(14.))))
        .child(label)
}

pub(super) fn checkbox(checked: bool, palette: Palette) -> impl IntoElement {
    div()
        .flex_none()
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .when(checked, |box_| {
            box_.bg(palette.foreground)
                .child(glyph(Lucide::Check, palette.primary_foreground).size(px(12.)))
        })
        .when(!checked, |box_| box_.border_1().border_color(palette.icon))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use nexus_desktop_core::sharing::Audience;
    use serde_json::json;

    #[test]
    fn each_reason_names_its_setting_in_a_few_words() {
        let pages = blocked("Your organization has turned off public links for pages.", "Acme");
        assert_eq!(pages.short, "Acme turned off public links");
        assert_eq!((pages.anchor, pages.label), ("pages-public-links", "Turn on"));
        assert_eq!(pages.message, "Could you turn on public links for pages in Acme?");

        let policy = json!({ "artifacts.external": "named", "artifacts.external.who": "admins" });
        let named = Audience::Public
            .blocked(&policy, "member")
            .expect("public links are off");
        assert_eq!(blocked(named, "Acme").short, "Named people only");
        let admins = Audience::External
            .blocked(&policy, "member")
            .expect("members can't share outside");
        let admins = blocked(admins, "Acme");
        assert_eq!(
            (admins.short.as_str(), admins.anchor),
            ("Owners and admins only", "sharing-settings")
        );
        let off = Audience::External
            .blocked(&json!({ "artifacts.external": "off" }), "owner")
            .expect("outside sharing is off");
        assert_eq!(blocked(off, "Acme").short, "Acme turned off outside sharing");
    }

    #[test]
    fn every_short_reason_fits_beside_the_row() {
        for reason in [
            "Your organization has turned off public links for pages.",
            "Your organization has turned off sharing outside it.",
            "Your organization allows sharing with named people only, not public links.",
            "Only owners and admins can share outside your organization.",
        ] {
            let words = blocked(reason, "Acme").short.split_whitespace().count();
            assert!((2..=6).contains(&words), "{reason}: {words} words");
        }
    }
}
