//! The organization switcher's menu (`org-switcher.tsx` in Nexus UI): "Your accounts", Personal first ("Just you"), a
//! hairline, then each company by name with its slug and your role, a check on the one this window is in, then New
//! organization and Join an organization. Picking another asks Gateway for a token there (`POST /v1/orgs/switch`, as
//! the Nexus app does) and loads the app again for it. New and Join open Nexus UI's `/app/select-org` in the browser,
//! as the web's rows do: it makes an organization and lists the invitations and domains you can join.

use crate::workspace::menu_item_hover;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

use super::code_dock::menu_box;
use super::sidebar::ROW_TEXT;
use super::{Workspace, glyph, initials};
use crate::gateway::Verb;
use crate::pages::fetch;
use crate::palette::Palette;

/// The switcher's shortcut as this platform writes it (`main.rs` binds it to `SwitchAccount`).
const SWITCH_HINT: &str = if cfg!(target_os = "macos") {
    "⌃ ⌘ O"
} else {
    "Ctrl Alt O"
};

/// The managed policy pins this computer to its organization: nobody in the app can change it, so the transcript shows
/// it quiet, with a lock (`transcript.rs`).
pub(super) const ONE_ORG: &str = "Your organization manages this computer: it stays in one organization.";

/// One organization of `GET /v1/orgs`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Org {
    pub id: String,
    pub slug: Option<String>,
    pub name: String,
    pub personal: bool,
    pub current: bool,
    pub logo: Option<String>,
    /// Your role there (`owner`, `admin`, `member`), when Gateway says.
    pub role: Option<String>,
    /// Its organization's id at the sign-in provider: Nexus UI's URL segment when it has no slug.
    pub provider_org_id: Option<String>,
}

impl Org {
    /// Its segment in Nexus UI's URLs (`orgSegment`): the slug, else its id at the sign-in provider, else its id.
    pub(super) fn segment(&self) -> &str {
        self.slug
            .as_deref()
            .or(self.provider_org_id.as_deref())
            .unwrap_or(&self.id)
    }

    /// The row's second line, as the web's: "Just you" for Personal, else the segment and your role ("acme · admin").
    pub(super) fn meta(&self) -> String {
        if self.personal {
            return "Just you".into();
        }
        match &self.role {
            Some(role) => format!("{} · {role}", self.segment()),
            None => self.segment().to_string(),
        }
    }

    fn from_json(org: &Value) -> Option<Org> {
        let text = |name: &str| {
            org.get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(String::from)
        };
        Some(Org {
            id: text("id")?,
            slug: text("slug"),
            name: text("name").unwrap_or_else(|| "Organization".into()),
            personal: text("kind").as_deref() == Some("personal"),
            current: org.get("current").and_then(Value::as_bool).unwrap_or(false),
            logo: text("imageUrl"),
            role: text("role"),
            // Gateway sends this field under the provider's own name only (the check in `vendor_names_tests.rs`).
            provider_org_id: text("clerkOrgId"),
        })
    }
}

/// The organizations of `GET /v1/orgs` the switcher lists: with switching off, only the pinned one (`managed.rs`).
fn listed_orgs(policy: &nexus_desktop_core::policy::Policy, answer: &Value) -> Vec<Org> {
    answer
        .get("orgs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|org| crate::managed::listed(policy, org))
        .filter_map(Org::from_json)
        .collect()
}

/// Personal first, then the companies by name: the order the menu shows (`orderOrgs`).
fn order(mut orgs: Vec<Org>) -> Vec<Org> {
    orgs.sort_by(|a, b| {
        b.personal
            .cmp(&a.personal)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    orgs
}

/// The organizations of a `GET /v1/orgs` answer, as the menu lists them.
pub(super) fn ordered_orgs(answer: &Value) -> Vec<Org> {
    order(listed_orgs(crate::managed::policy(), answer))
}

/// What the switcher's menu holds while it is open.
pub(super) enum OrgMenu {
    Loading,
    Ready(Vec<Org>),
    Failed(SharedString),
}

impl Workspace {
    /// Open the switcher (and read the list again), or close it.
    pub(super) fn toggle_org_menu(&mut self, cx: &mut Context<Self>) {
        if self.overlay.close_org_menu().is_some() {
            cx.notify();
            return;
        }
        self.open_overlay(Overlay::OrgMenu(OrgMenu::Loading));
        self.load_orgs(cx);
        // The other accounts' Assistants are read again as it opens (`account_states.rs`).
        self.poll_accounts_soon(cx);
        cx.notify();
    }

    /// Read the organizations into the open switcher, or into the palette's `@` scope; dropped if neither is open.
    pub(super) fn load_orgs(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let request = account_gateway.request(Verb::Get, "/v1/orgs".into(), None);
        fetch(cx, request, |this, answer, _| {
            let read = match answer {
                Ok(list) => {
                    let orgs = ordered_orgs(&list);
                    this.accounts.orgs = orgs.clone();
                    OrgMenu::Ready(orgs)
                }
                Err(error) => OrgMenu::Failed(format!("Couldn't load your organizations: {error}").into()),
            };
            if let Some(menu) = this.overlay.org_menu_mut() {
                *menu = read;
            } else if let Some(Some(orgs)) = this.overlay.palette_mut() {
                *orgs = read;
            }
        });
    }

    /// After a sign-in, move to the organization the policy pins, unless the person is in it (the Nexus app's
    /// `move_to_pinned_org`). The switch signs in again, and then the person is there, so this ends.
    pub(super) fn move_to_pinned_org(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if crate::managed::policy().organization.is_none() {
            return;
        }
        let request = account_gateway.request(Verb::Get, "/v1/orgs".into(), None);
        fetch(cx, request, |this, answer, _| {
            let moved = answer.and_then(|orgs| crate::managed::pinned_target(crate::managed::policy(), &orgs));
            match moved {
                Ok(Some(id)) => {
                    if let Some(account) = this.account_gateway() {
                        account.switch_org(id);
                    }
                }
                Ok(None) => {}
                Err(error) => this.notice = Some(error.into()),
            }
        });
    }

    pub(super) fn switch_org(&mut self, org: &Org, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        self.overlay.close_org_menu();
        if org.current {
            cx.notify();
            return;
        }
        if !crate::managed::policy().may_switch_to(&org.id, org.slug.as_deref()) {
            self.notice = Some(ONE_ORG.into());
            cx.notify();
            return;
        }
        // The Assistant view open now opens on the new account, and ⌘[ comes back (A24·g).
        self.before_switch();
        // The other organization's sessions and account come with `SignedIn`; until then, nothing of this one shows.
        // The account lands the window on its last place there, else New session (`last_place::landing`).
        self.new_chat(window, cx);
        // Nor its projects, payer, chats or lists (`org_scope.rs`): New session picks from them before the new store's
        // read lands.
        self.leave_org();
        let leaving = self.account.as_ref().map(|account| account.org.clone());
        account_gateway.switch_org_keeping(
            org.id.clone(),
            leaving,
            Some(super::account_states::key_of(org).to_string()),
        );
        cx.notify();
    }

    /// The open switcher, `top` points under the top of the row that holds its button and `left` points in from it.
    pub(super) fn render_org_menu(
        &self,
        left: f32,
        top: f32,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // The palette's `@` scope reads the organizations through this menu's state; the menu stays hidden then.
        if self.overlay.palette().is_some() {
            return None;
        }
        let menu = self.overlay.org_menu()?;
        let mut body = menu_box("org-menu", 248., palette)
            .text_size(px(ROW_TEXT))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.overlay.close_org_menu();
                cx.notify();
            }));
        // One title over every account, so Personal reads once (`.org-menu-title`).
        body = body.child(
            div()
                .px(px(8.))
                .pt(px(4.))
                .pb(px(2.))
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.faint)
                .child("Your accounts"),
        );
        let switching = crate::managed::policy().allow_organization_switching;
        body = match menu {
            OrgMenu::Loading => body.child(div().px(px(8.)).py(px(6.)).text_color(palette.muted).child("Loading…")),
            OrgMenu::Failed(error) => body.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_color(palette.muted)
                    .child(error.clone()),
            ),
            OrgMenu::Ready(orgs) => {
                let mut column = v_flex().gap(px(1.));
                for (index, org) in orgs.iter().enumerate() {
                    // A hairline between Personal and the companies (`.org-menu-group + .org-menu-group`).
                    if index > 0 && orgs[index - 1].personal && !org.personal {
                        column = column.child(menu_hairline(palette));
                    }
                    column = column.child(self.org_row(index, org, palette, cx));
                }
                if !switching {
                    column = column.child(
                        div()
                            .px(px(8.))
                            .pt(px(6.))
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(format!("{}.", crate::managed::SET_BY_ORG)),
                    );
                }
                body.child(column)
            }
        };
        // The switcher's key (A24·f), on the last line.
        body = body.child(menu_hairline(palette)).child(
            h_flex()
                .h(px(32.))
                .px(px(8.))
                .gap(px(8.))
                .text_size(px(12.))
                .text_color(palette.muted)
                .child(div().flex_1().child("Switch account"))
                .child(super::search::keycaps(SWITCH_HINT, false, palette)),
        );
        // New organization and Join an organization (`.org-menu-foot`); not where the policy pins one organization.
        if switching {
            body = body
                .child(menu_hairline(palette))
                .child(self.select_org_row("org-new", Lucide::Plus, "New organization", palette, cx))
                .child(self.select_org_row("org-join", Lucide::Building2, "Join an organization", palette, cx));
        }
        Some(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .child(
                    deferred(
                        anchored()
                            .anchor(Anchor::TopLeft)
                            .snap_to_window_with_margin(px(8.))
                            .child(body),
                    )
                    .with_priority(1),
                )
                .into_any_element(),
        )
    }

    /// An account: its mark, its name over its meta line (`.org-menu-org`), and a check on the one on screen.
    fn org_row(&self, index: usize, org: &Org, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let mark = match (&org.logo, org.personal) {
            (_, true) => div()
                .size(px(MARK))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(palette.selected)
                .child(glyph(Lucide::User, palette.icon).size(px(13.)))
                .into_any_element(),
            (Some(url), false) => super::logo::logo(url.clone(), MARK)
                .rounded(px(6.))
                .bg(white())
                .into_any_element(),
            (None, false) => div()
                .size(px(MARK))
                .flex_none()
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .bg(palette.selected)
                .text_size(px(9.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(initials(&org.name))
                .into_any_element(),
        };
        let label: SharedString = if org.personal {
            "Personal".into()
        } else {
            org.name.clone().into()
        };
        let picked = org.clone();
        h_flex()
            .id(("org-row", index))
            .min_h(px(40.))
            .py(px(4.))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_color(if org.current {
                palette.strong
            } else {
                palette.foreground
            })
            .hover(|style| menu_item_hover(style, palette))
            .child(mark)
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .line_height(relative(1.3))
                    .child(div().truncate().child(label))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(palette.muted)
                            .child(org.meta()),
                    ),
            )
            // Each account's Assistant (A24·f): live for the one on screen, the last read for the others.
            .children(self.account_badge(org, palette))
            .when(org.current, |row| {
                row.child(glyph(Lucide::Check, palette.strong).size(px(14.)))
            })
            .on_click(cx.listener(move |this, _, window, cx| this.switch_org(&picked, window, cx)))
    }

    /// New organization or Join an organization: both open Nexus UI's `/app/select-org` in the browser, as the web's do.
    fn select_org_row(
        &self,
        id: &'static str,
        icon: Lucide,
        label: &'static str,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(id)
            .h(px(30.))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_color(palette.foreground)
            .hover(|style| menu_item_hover(style, palette))
            .child(glyph(icon, palette.icon).size(px(15.)))
            .child(label)
            .on_click(cx.listener(|this, _, _, cx| {
                this.overlay.close_org_menu();
                crate::external_link::open(&select_org_url(&this.gateway.site_url()), cx);
                cx.notify();
            }))
    }
}

/// The accounts' marks: 22 px, as `.org-avatar`.
const MARK: f32 = 22.;

/// A hairline across the menu between its groups.
fn menu_hairline(palette: Palette) -> impl IntoElement {
    div().my(px(4.)).h(px(1.)).bg(palette.border)
}

/// Nexus UI's page that makes an organization and lists the ones you can join (`SELECT_ORG_URL`).
fn select_org_url(site: &str) -> String {
    format!("{}/app/select-org", site.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::{Org, listed_orgs, order, select_org_url};
    use serde_json::json;

    #[test]
    fn each_row_says_what_the_web_row_says() {
        let list = json!([
            { "id": "p", "name": "Ada", "kind": "personal", "role": "owner" },
            { "id": "t1", "slug": "nexus-xyz", "name": "Nexus Labs", "kind": "company", "role": "admin" },
            { "id": "t2", "clerkOrgId": "org_2a", "name": "Acme", "kind": "company", "role": "member" },
            { "id": "t3", "name": "Bare", "kind": "company" },
        ]);
        let metas: Vec<_> = list
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Org::from_json)
            .map(|org| org.meta())
            .collect();
        assert_eq!(metas, ["Just you", "nexus-xyz · admin", "org_2a · member", "t3"]);
    }

    #[test]
    fn new_and_join_open_the_web_page_that_does_both() {
        assert_eq!(select_org_url("https://nexus.inc"), "https://nexus.inc/app/select-org");
        assert_eq!(select_org_url("https://nexus.inc/"), "https://nexus.inc/app/select-org");
    }

    #[test]
    fn personal_first_then_by_name() {
        let list = json!([
            { "id": "t2", "name": "zeta", "kind": "company" },
            { "id": "t1", "name": "Acme", "kind": "company", "current": true, "imageUrl": "https://x/logo.png" },
            { "id": "p", "name": "Ada", "kind": "personal" },
        ]);
        let orgs = order(list.as_array().unwrap().iter().filter_map(Org::from_json).collect());
        let ids: Vec<_> = orgs.iter().map(|org| org.id.as_str()).collect();
        assert_eq!(ids, ["p", "t1", "t2"]);
        assert!(orgs[1].current && orgs[1].logo.is_some());
    }

    #[test]
    fn a_pinned_organization_is_the_only_one_listed() {
        use nexus_desktop_core::policy::{Value as Pref, key};
        let answer = json!({ "orgs": [
            { "id": "p", "name": "Me", "kind": "personal", "current": true },
            { "id": "t1", "slug": "acme", "name": "Acme", "kind": "company" },
        ]});
        let pinned = crate::managed::tests::policy_of(
            &[
                (key::ORGANIZATION, Pref::Text("acme".into())),
                (key::ALLOW_ORGANIZATION_SWITCHING, Pref::Bool(false)),
            ],
            None,
        );
        let ids: Vec<_> = listed_orgs(&pinned, &answer).into_iter().map(|org| org.id).collect();
        assert_eq!(ids, ["t1"]);
        assert_eq!(
            listed_orgs(&crate::managed::tests::policy_of(&[], None), &answer).len(),
            2
        );
    }
}

/// A press on a menu's trigger toggles the menu, with the organization switcher's parts: a trigger with
/// `menu_trigger`, and a menu that closes on a mouse down outside it.
#[cfg(test)]
mod trigger_tests {
    // Not `super::*`: GPUI's prelude has a `test` attribute of its own.
    use gpui_kit::{
        Context, InteractiveElement, IntoElement, Modifiers, MouseButton, MouseDownEvent, ParentElement, Render,
        Styled, TestAppContext, Window, div, px,
    };

    use gpui_kit::prelude::FluentBuilder;

    use crate::workspace::MenuTrigger;

    /// The switcher in small: `open` is `org_menu`, and `loads` counts the `/v1/orgs` requests an opening makes.
    struct Switcher {
        open: bool,
        loads: usize,
    }

    impl Render for Switcher {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let trigger = div()
                .id("org")
                .debug_selector(|| "org".into())
                .size(px(40.))
                // As `toggle_org_menu`: close an open menu, or open it and load the organizations.
                .menu_trigger(
                    self.open,
                    cx,
                    |this| this.open,
                    |this, _, cx| {
                        this.open = !this.open;
                        if this.open {
                            this.loads += 1;
                        }
                        cx.notify();
                    },
                );
            div()
                .size_full()
                .child(trigger)
                .child(
                    div()
                        .id("elsewhere")
                        .debug_selector(|| "elsewhere".into())
                        .size(px(40.)),
                )
                .when(self.open, |root| {
                    root.child(
                        div()
                            .id("org-menu")
                            .occlude()
                            .size(px(40.))
                            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                this.open = false;
                                cx.notify();
                            })),
                    )
                })
        }
    }

    fn click(cx: &mut gpui_kit::VisualTestContext, selector: &'static str) {
        let at = cx.debug_bounds(selector).expect("drawn").center();
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    #[gpui_kit::test]
    fn a_second_click_on_the_switcher_closes_its_menu(cx: &mut TestAppContext) {
        let (switcher, cx) = cx.add_window_view(|_, _| Switcher { open: false, loads: 0 });
        let state = |cx: &mut gpui_kit::VisualTestContext| switcher.read_with(cx, |s, _| (s.open, s.loads));
        click(cx, "org");
        assert_eq!(
            state(cx),
            (true, 1),
            "the first click opens the menu and loads the organizations"
        );
        click(cx, "org");
        assert_eq!(
            state(cx),
            (false, 1),
            "the second click closes it, and does not load again"
        );
        click(cx, "org");
        assert_eq!(state(cx), (true, 2), "a third click opens it again");
        click(cx, "elsewhere");
        assert_eq!(state(cx), (false, 2), "a click outside closes it");
    }
}
