//! What the person's role can't do, said where it is (board P11, "Your role can't do this"). A locked control shows a
//! lock and a reason in a few words beside it, never only in a tooltip, and the way on: who may change it gets the fix
//! ("Turn on ›"), anyone else gets Ask, naming who can (`access::decide`). A page the person can't change shows one
//! notice line at its top instead ([`role_notice`]). Never red: nothing failed.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::access::{self, Decision, OrgAccess};
use super::fix::{Ask, Fix};
use super::line::SetupLine;
use super::{FixKey, Fixer, state};
use crate::palette::Palette;

/// The Ask button's height: a step under a page's 28 px controls, as it sits in a row's text.
const PILL: f32 = 24.;

/// A locked control's reason and its way on, built by the call site each frame. `key` names what is blocked
/// (`"share:public"`): it keys the button and its Ask state.
#[derive(Clone)]
pub struct Locked {
    key: FixKey,
    reason: SharedString,
    fix: Option<(SharedString, Fix)>,
    ask: Option<String>,
}

/// What a locked control offers this person.
#[derive(Clone, Debug)]
enum Way {
    /// They may change it: the fix, as a quiet link.
    Fix(SharedString, Fix),
    /// They may not: Ask, naming who can.
    Ask(SharedString, Ask),
    /// Nothing to offer: the reason alone.
    None,
}

impl Locked {
    /// `reason` is four to six words: "Owner only", "Acme turned off public links". Empty shows the way on alone.
    pub fn new(key: impl Into<FixKey>, reason: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            reason: reason.into(),
            fix: None,
            ask: None,
        }
    }

    /// The fix for who may change it ("Turn on", `Fix::open_at(..).needs("org.settings")`). Who lacks its permission
    /// sees Ask in its place.
    pub fn fix(mut self, label: impl Into<SharedString>, fix: Fix) -> Self {
        self.fix = Some((label.into(), fix));
        self
    }

    /// What Ask sends ("Could you turn on public links for pages in Acme?"). Without it, Ask sends the reason.
    pub fn ask(mut self, message: impl Into<String>) -> Self {
        self.ask = Some(message.into());
        self
    }

    fn way(&self, access: &OrgAccess) -> Way {
        let Some((label, fix)) = &self.fix else {
            return Way::None;
        };
        let needs = fix.required();
        match access::decide(needs, access) {
            Decision::Fix => Way::Fix(label.clone(), fix.clone()),
            Decision::Ask { who } => {
                let message = self.ask.clone().unwrap_or_else(|| self.reason.to_string());
                let mut ask = Ask::new(needs, message);
                let label = access::ask_label(&who, needs).into();
                ask.who = who;
                Way::Ask(label, ask)
            }
        }
    }

    /// The lock, the reason and the way on, in one row that sits beside the control.
    pub fn render(&self, fixer: &Fixer, palette: Palette, cx: &App) -> Div {
        let mut row = h_flex().flex_none().gap(px(8.));
        if !self.reason.is_empty() {
            row = row.child(reason(self.reason.clone(), palette));
        }
        let id = |part: &str| ElementId::Name(format!("locked-{part}-{}", self.key).into());
        let (fix, label) = match self.way(&fixer.access(cx)) {
            Way::None => return row,
            Way::Fix(label, fix) => {
                let link = div()
                    .id(id("fix"))
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.strong))
                    .child(format!("{label} ›"));
                return row.child(self.on_press(link, fix, fixer));
            }
            Way::Ask(label, ask) => (Fix::Ask(ask), label),
        };
        if let Some(asked) = state::asked(&self.key, cx) {
            return row.child(
                h_flex()
                    .flex_none()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child(Icon::new(Lucide::Check).size(px(12.)).text_color(palette.muted))
                    .child(asked.label),
            );
        }
        let pill = h_flex()
            .id(id("ask"))
            .flex_none()
            .h(px(PILL))
            .px(px(10.))
            .rounded_full()
            .bg(palette.selected)
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(palette.foreground)
            .cursor_pointer()
            .hover(|style| style.bg(palette.thumb))
            .active(|style| style.opacity(0.8))
            .child(label);
        row.child(self.on_press(pill, fix, fixer))
    }

    fn on_press(&self, button: Stateful<Div>, fix: Fix, fixer: &Fixer) -> Stateful<Div> {
        let key = self.key.clone();
        let fixer = fixer.clone();
        button.on_click(move |_, window, cx| {
            // A locked row can sit inside a clickable one: the press is the button's alone.
            cx.stop_propagation();
            fixer.run(key.clone(), fix.clone(), window, cx)
        })
    }
}

/// A lock and a few words beside a control the person can't use: "Owner only".
pub fn reason(text: impl Into<SharedString>, palette: Palette) -> Div {
    h_flex()
        .flex_none()
        .gap(px(4.))
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(Icon::new(Lucide::Lock).size(px(12.)).text_color(palette.icon))
        .child(text.into())
}

/// Why a button is off, beside it, when it waits on a step rather than a role: "Install Codex first".
pub fn waits_on(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .flex_none()
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(text.into())
}

/// The one notice at the top of a page the person's role can't change: "Owners and admins change providers. Ana and
/// Dev can." with "Ask Ana". The page shows it only to people without `needs`, so it always asks.
pub fn role_notice(
    key: impl Into<FixKey>,
    what: impl Into<SharedString>,
    needs: &'static str,
    message: impl Into<String>,
    access: &OrgAccess,
) -> SetupLine {
    let (label, why, ask) = notice_ask(needs, message.into(), access);
    SetupLine::new(key, Lucide::Lock, what)
        .why(why)
        .fix(label, Fix::Ask(ask))
}

/// The notice's button, its second sentence and its question.
fn notice_ask(needs: &'static str, message: String, access: &OrgAccess) -> (String, String, Ask) {
    let who = access::who_can(needs, access);
    let label = access::ask_label(&who, Some(needs));
    let why = access::can_line(&who, Some(needs), "");
    let mut ask = Ask::new(Some(needs), message);
    ask.who = who;
    (label, why, ask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::PageId;
    use core::prelude::v1::test;
    use serde_json::json;

    fn acme(permissions: &[&str]) -> OrgAccess {
        OrgAccess {
            me: Some("u_me".into()),
            permissions: Some(permissions.iter().map(|each| each.to_string()).collect()),
            managers: access::parse_managers(&json!({"members": [
                {"userId": "u_ana", "name": "Ana Souza", "role": "owner"},
                {"userId": "u_dev", "name": "Dev Patel", "role": "admin"},
            ]})),
        }
    }

    fn public_links() -> Locked {
        Locked::new("share:public", "Acme turned off public links")
            .fix(
                "Turn on",
                Fix::open_at(PageId::General, "pages-public-links").needs("org.settings"),
            )
            .ask("Could you turn on public links for pages in Acme?")
    }

    #[test]
    fn who_may_change_it_gets_the_fix() {
        let Way::Fix(label, Fix::Open { focus, .. }) = public_links().way(&acme(&["org.settings"])) else {
            panic!("an admin turns it on");
        };
        assert_eq!(label.as_ref(), "Turn on");
        assert_eq!(focus, Some("pages-public-links"));
    }

    #[test]
    fn anyone_else_asks_the_people_who_can() {
        let Way::Ask(label, ask) = public_links().way(&acme(&["artifacts.share"])) else {
            panic!("a member asks");
        };
        assert_eq!(label.as_ref(), "Ask Ana");
        assert_eq!(ask.who, ["Ana", "Dev"]);
        assert_eq!(ask.needs, Some("org.settings"));
        assert_eq!(ask.message, "Could you turn on public links for pages in Acme?");
    }

    #[test]
    fn without_a_fix_only_the_reason_shows() {
        assert!(matches!(
            Locked::new("repo:run", "Owner only").way(&acme(&[])),
            Way::None
        ));
    }

    #[test]
    fn without_a_message_ask_sends_the_reason() {
        let locked = Locked::new("connector:jira", "Acme hasn't turned on Jira for you")
            .fix("Turn on", Fix::open(PageId::ConnectorRules).needs("policy.manage"));
        let Way::Ask(_, ask) = locked.way(&acme(&[])) else {
            panic!("a member asks");
        };
        assert_eq!(ask.message, "Acme hasn't turned on Jira for you");
    }

    #[test]
    fn a_role_notice_names_who_can_and_asks_the_first() {
        let (label, why, ask) = notice_ask(
            "policy.manage",
            "Could you turn a provider on for Acme?".into(),
            &acme(&[]),
        );
        assert_eq!(label, "Ask Ana");
        assert_eq!(why, "Ana and Dev can.");
        assert_eq!(ask.needs, Some("policy.manage"));
        let (label, why, _) = notice_ask("org.billing", String::new(), &OrgAccess::default());
        assert_eq!((label.as_str(), why.as_str()), ("Ask an owner", "An owner can."));
    }
}
