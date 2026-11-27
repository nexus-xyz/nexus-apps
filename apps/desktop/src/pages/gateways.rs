//! Settings › Connection (the Nexus app's `settings/connections.tsx` and `gateway-settings.tsx`): one list of Gateways,
//! the one this build talks to first, then the ones this computer signed in to before, each with its sign-in, and a
//! last row that unfolds to add a new one by its address. The list is the Nexus app's (`settings.json`,
//! `Settings::known_gateways`), with production last.
//!
//! Switch moves the whole app with no restart, as the Nexus app's switch does (`Workspace::switch_gateway`): the
//! Gateway is saved, keeping this one's organization for a switch back, and a new client signs in with the target's
//! keychain entry. A Gateway never signed in to shows the sign-in.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::settings::Settings;

use super::desktop_app::off_thread;
use super::{DeviceCtx, Load, PageId, palette, ui};
use crate::managed::SET_BY_ORG;
use crate::palette::Palette;

/// The add form's left inset: the rows' text, past the row's 16px padding, the 28px icon tile and the 16px gap.
const TEXT_INSET: f32 = 60.;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

pub fn build(ctx: DeviceCtx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("https://nexus-gateway.example.com"));
        let subscription = cx.subscribe_in(
            &address,
            window,
            |this: &mut GatewaysPage, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => this.add(window, cx),
                    // Add and switch turns on once there is an address, and a new address clears the last refusal.
                    InputEvent::Change => {
                        this.add_problem = None;
                        cx.notify();
                    }
                    _ => {}
                }
            },
        );
        // A search that lands on "Add a gateway" finds its field unfolded.
        let adding = ctx.settings_fields.pending_target() == Some("add-gateway");
        let mut page = GatewaysPage {
            ctx,
            known: Load::Loading,
            address,
            adding,
            add_problem: None,
            switch_problem: None,
            _subscription: subscription,
        };
        page.load(cx);
        page
    });
    Some(view.into())
}

/// One Gateway the page offers.
#[derive(Debug, Clone, PartialEq)]
struct Known {
    url: String,
    /// The organization last signed in to there, if the Nexus app remembers it.
    org: Option<String>,
    /// Its keychain entry is there: switching to it needs no sign-in.
    saved_sign_in: bool,
}

/// The Gateways to offer, as (URL, organization), `current` first (`Settings::known_gateways`).
fn known_gateways(settings: &Settings, current: &str) -> Vec<(String, Option<String>)> {
    settings.known_gateways(current)
}

struct GatewaysPage {
    ctx: DeviceCtx,
    known: Load<Vec<Known>>,
    /// "Add a gateway": its address.
    address: Entity<InputState>,
    /// The add row is unfolded.
    adding: bool,
    /// Why the typed address was refused.
    add_problem: Option<SharedString>,
    /// The saved Gateway whose switch failed, and why, shown on its row.
    switch_problem: Option<(String, SharedString)>,
    _subscription: Subscription,
}

impl GatewaysPage {
    fn load(&mut self, cx: &mut Context<Self>) {
        self.known = Load::Loading;
        let current = self.ctx.gateway_url.as_str().trim_end_matches('/').to_string();
        off_thread(
            cx,
            move || {
                // The keychain may be slow to answer (or ask the person on macOS): never on the UI thread.
                known_gateways(&crate::desktop_settings::load(), &current)
                    .into_iter()
                    .map(|(url, org)| Known {
                        saved_sign_in: crate::keychain::has_sign_in(&url),
                        url,
                        org,
                    })
                    .collect::<Vec<_>>()
            },
            |this, known, _| this.known = Load::Ready(known),
        );
    }

    /// Move the app to `url`. On success the workspace starts again on it, and this page closes.
    fn switch(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_problem = None;
        let switch = self.ctx.switch_gateway.clone();
        if let Err(error) = switch(url.clone(), window, cx) {
            self.switch_problem = Some((url, error.into()));
        }
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.address.read(cx).value().trim().to_string();
        if url.is_empty() {
            return;
        }
        self.add_problem = None;
        let switch = self.ctx.switch_gateway.clone();
        if let Err(error) = switch(url, window, cx) {
            self.add_problem = Some(error.into());
        }
        cx.notify();
    }

    /// Unfold or fold the add row. Unfolding puts the cursor in the address field.
    fn toggle_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adding = !self.adding;
        self.add_problem = None;
        if self.adding {
            self.address.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    /// Why this is the Gateway, when it is not the plain choice: the organization's lock, or `NEXUS_URL`.
    fn where_from(&self) -> Option<(Lucide, &'static str)> {
        if locked() {
            Some((
                Lucide::Lock,
                "Your organization manages this computer: it sets the gateway.",
            ))
        } else if std::env::var("NEXUS_URL").is_ok_and(|url| !url.is_empty()) {
            Some((
                Lucide::SquareTerminal,
                "Set by NEXUS_URL when this build started, until you switch.",
            ))
        } else {
            None
        }
    }

    /// "Current": the Gateway this app talks to, with its saved sign-in once the list has read it.
    fn render_current(&self, locked: bool, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let status = match &self.known {
            Load::Ready(known) => known.first().map(Status::of),
            _ => None,
        };
        let (tile, pill, end) = if locked {
            (Lucide::Lock, SET_BY_ORG, None)
        } else {
            (
                Lucide::Server,
                "Current",
                Some(ui::icon(Lucide::Check, palette.strong).size(px(16.)).into_any_element()),
            )
        };
        let row = gateway_row(
            self.ctx.gateway_url.as_str().trim_end_matches('/'),
            tile,
            Some(pill),
            status,
            end,
            palette,
        );
        flush(self.ctx.settings_fields.field("gateway-address", row, None, cx)).into_any_element()
    }

    /// A Gateway signed in to before, with Switch, or why its last switch failed.
    fn render_saved(&self, ix: usize, known: &Known, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let failed = self
            .switch_problem
            .as_ref()
            .filter(|(url, _)| *url == known.url)
            .map(|(_, problem)| problem.clone());
        let label = if failed.is_some() { "Try again" } else { "Switch" };
        let control = if crate::desktop_settings::writable() {
            let url = known.url.clone();
            let button = if failed.is_some() {
                ui::button(("gateway-switch", ix), label, palette)
            } else {
                ui::ghost_button(("gateway-switch", ix), label, palette)
            };
            button
                .on_click(cx.listener(move |this, _, window, cx| this.switch(url.clone(), window, cx)))
                .into_any_element()
        } else {
            ui::disabled_button(("gateway-switch", ix), label, palette).into_any_element()
        };
        let status = match failed {
            Some(problem) => Status::Failed(problem),
            None => Status::of(known),
        };
        gateway_row(&known.url, Lucide::Server, None, Some(status), Some(control), palette)
            .id(("gateway-row", ix))
            .hover(|style| style.bg(palette.hover))
            .into_any_element()
    }

    /// The list's last row: "Add a gateway", which unfolds into the address field and Add and switch.
    fn render_add(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let head = ui::focusable(ui::card_row().id("gateway-add-toggle"), palette)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_adding(window, cx)))
            .child(ui::icon_tile(Lucide::Plus, palette))
            .child(
                ui::setting_lines(
                    "Add a gateway",
                    "Connect to your company's own gateway by its address.",
                    palette,
                )
                .flex_1(),
            )
            .child(ui::icon(
                if self.adding {
                    Lucide::ChevronDown
                } else {
                    Lucide::ChevronRight
                },
                palette.faint,
            ));
        let form = self
            .adding
            .then(|| self.render_add_form(palette, cx).into_any_element());
        let body = v_flex()
            .child(head)
            .child(crate::motion::reveal("gateway-add-form", self.adding, form));
        let focus = self.address.read(cx).focus_handle(cx);
        flush(self.ctx.settings_fields.field("add-gateway", body, Some(focus), cx)).into_any_element()
    }

    fn render_add_form(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let typed = !self.address.read(cx).value().trim().is_empty();
        let add = if typed && crate::desktop_settings::writable() {
            ui::primary_button("gateway-add", "Add and switch", palette)
                .on_click(cx.listener(|this, _, window, cx| this.add(window, cx)))
                .into_any_element()
        } else {
            ui::disabled_button("gateway-add", "Add and switch", palette).into_any_element()
        };
        let cancel = ui::ghost_button("gateway-add-cancel", "Cancel", palette)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_adding(window, cx)));
        let mut field = ui::input_box(&self.address, palette).font_family(ui::MONO);
        if self.add_problem.is_some() {
            field = field.border_1().border_color(palette.destructive);
        }
        let hint: AnyElement = match &self.add_problem {
            Some(problem) => crate::motion::arrive(
                ui::error_line(problem.clone(), palette).text_size(px(12.)),
                ElementId::Name(format!("gateway-add-problem-{problem}").into()),
                4.,
            )
            .into_any_element(),
            None => ui::setting_detail(
                "The app moves to it at once. You sign in there if you have not before.",
                palette,
            )
            .into_any_element(),
        };
        v_flex()
            .pl(px(TEXT_INSET))
            .pr(px(16.))
            .pb(px(16.))
            .gap(px(6.))
            .child(ui::label("Gateway address", palette))
            .child(h_flex().gap(px(8.)).child(field).child(cancel).child(add))
            .child(hint)
    }
}

/// A Gateway's sign-in as its row shows it: a mark and a few words.
#[derive(Debug, Clone, PartialEq)]
enum Status {
    SavedSignIn(Option<String>),
    SignedOut,
    /// Its last switch failed: why.
    Failed(SharedString),
}

impl Status {
    fn of(known: &Known) -> Status {
        if known.saved_sign_in {
            Status::SavedSignIn(known.org.clone())
        } else {
            Status::SignedOut
        }
    }

    fn text(&self) -> SharedString {
        match self {
            Status::SavedSignIn(Some(org)) => format!("Saved sign-in for {org}").into(),
            Status::SavedSignIn(None) => "Saved sign-in".into(),
            Status::SignedOut => "No saved sign-in".into(),
            Status::Failed(problem) => problem.clone(),
        }
    }

    /// A solid success dot for a saved sign-in, a hollow steel ring without one, a red dot when the switch failed: the state
    /// reads by shape as well as by color.
    fn render(&self, palette: Palette) -> AnyElement {
        let dot = div().flex_none().size(px(7.)).rounded(px(4.));
        let (dot, color) = match self {
            Status::SavedSignIn(_) => (dot.bg(Palette::success()), palette.faint),
            Status::SignedOut => (dot.border_1().border_color(palette.steel()), palette.faint),
            Status::Failed(_) => (dot.bg(palette.destructive), palette.destructive),
        };
        let line = h_flex()
            .min_w(px(0.))
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(color)
            .child(dot)
            .child(div().min_w(px(0.)).truncate().child(self.text()));
        match self {
            Status::Failed(problem) => crate::motion::arrive(
                line,
                ElementId::Name(format!("gateway-switch-problem-{problem}").into()),
                4.,
            )
            .into_any_element(),
            _ => line.into_any_element(),
        }
    }
}

/// A Gateway as a card row: its icon, its host over its sign-in, and its control at the right. `status` is `None`
/// while the keychain answers: a bar holds the line's place.
fn gateway_row(
    url: &str,
    tile: Lucide,
    pill: Option<&'static str>,
    status: Option<Status>,
    end: Option<AnyElement>,
    palette: Palette,
) -> Div {
    let (scheme, host) = host_label(url);
    let detail = match status {
        Some(status) => status.render(palette),
        None => div()
            .my(px(4.))
            .h(px(8.))
            .w(px(120.))
            .rounded(px(4.))
            .bg(palette.hover)
            .into_any_element(),
    };
    ui::card_row()
        .child(ui::icon_tile(tile, palette))
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(2.))
                .child(
                    h_flex()
                        .min_w(px(0.))
                        .gap(px(8.))
                        .child(
                            h_flex()
                                .min_w(px(0.))
                                .text_size(px(13.))
                                .font_family(ui::MONO)
                                .text_color(palette.strong)
                                .children(
                                    scheme.map(|scheme| div().flex_none().text_color(palette.faint).child(scheme)),
                                )
                                .child(div().min_w(px(0.)).truncate().child(host.to_string())),
                        )
                        .children(pill.map(|pill| ui::pill(pill, palette))),
                )
                .child(detail),
        )
        .children(end)
}

/// A search target (`SettingsFields::field`) drawn flush with its neighbours: the field's 2px highlight ring sits
/// outside its content, so the heading and the rows line up with the ones that are not search targets.
fn flush(field: Div) -> Div {
    field.w_auto().mx(px(-2.)).my(px(-2.))
}

/// How a row writes a Gateway's address: its host, with `https://` left out, and a faint `http://` kept, so a Gateway
/// reached without TLS still stands out.
fn host_label(url: &str) -> (Option<&'static str>, &str) {
    if let Some(host) = url.strip_prefix("https://") {
        (None, host)
    } else if let Some(host) = url.strip_prefix("http://") {
        (Some("http://"), host)
    } else {
        (None, url)
    }
}

impl Render for GatewaysPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let locked = locked();
        let current = self.render_current(locked, palette, cx);
        let head = flush(self.ctx.settings_fields.field(
            "gateways",
            ui::group_head(if locked { "Gateway" } else { "Gateways" }, palette),
            None,
            cx,
        ));
        let mut rows = vec![Some(current)];
        if !locked {
            // The Nexus app shows no list while the organization sets the Gateway (`gateway-settings.tsx`).
            if let Load::Ready(known) = &self.known {
                rows.extend(
                    known
                        .iter()
                        .enumerate()
                        .skip(1)
                        .map(|(ix, known)| Some(self.render_saved(ix, known, palette, cx))),
                );
            }
            rows.push(Some(self.render_add(palette, cx)));
        }
        let failed = match &self.known {
            Load::Failed(error) => Some(ui::error_line(error.clone(), palette).mt(px(8.))),
            _ => None,
        };
        let page = v_flex()
            .w_full()
            .child(ui::lead(
                "The Nexus Gateway this app talks to. Each gateway keeps its own sign-in.",
                palette,
            ))
            .child(head)
            .child(ui::group(rows, palette))
            .children(failed)
            .children(self.where_from().map(|(icon, line)| {
                h_flex()
                    .mt(px(8.))
                    .gap(px(6.))
                    .items_start()
                    .child(ui::icon(icon, palette.faint).size(px(13.)).mt(px(2.)))
                    .child(ui::setting_detail(line, palette))
            }));
        if locked {
            return page.into_any_element();
        }
        page.child(ui::footnote(
            "Switching keeps your place on the gateway you leave, so you can switch back.",
            palette,
        ))
        .child(ui::group_head("Related", palette))
        .child(ui::group(
            [Some(
                ui::nav_row(
                    "gateways-agents",
                    Lucide::Route,
                    "Coding agent routing",
                    "Send coding agents through the gateway or your own AI plan.",
                    false,
                    palette,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    let navigate = this.ctx.navigate.clone();
                    navigate(PageId::Agents, window, cx);
                }))
                .into_any_element(),
            )],
            palette,
        ))
        .into_any_element()
    }
}

/// Whether the organization locked the Gateway on this computer (`managed.rs`).
fn locked() -> bool {
    locked_by(crate::managed::policy())
}

fn locked_by(policy: &nexus_desktop_core::policy::Policy) -> bool {
    crate::managed::locked_gateway(policy).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use nexus_desktop_core::settings::{DEFAULT_GATEWAY, KnownGateway};

    #[test]
    fn lists_the_current_gateway_then_the_ones_signed_in_to_then_production() {
        let settings = Settings {
            gateway_url: Some("https://gw.example.com".into()),
            org: Some("acme".into()),
            gateways: vec![KnownGateway {
                url: "http://localhost:8080".into(),
                org: Some("dev".into()),
                kind: Some("cli".into()),
            }],
            ..Settings::default()
        };
        assert_eq!(
            known_gateways(&settings, "https://gw.example.com"),
            vec![
                ("https://gw.example.com".to_string(), Some("acme".to_string())),
                ("http://localhost:8080".to_string(), Some("dev".to_string())),
                (DEFAULT_GATEWAY.to_string(), None),
            ]
        );
    }

    #[test]
    fn a_row_shows_the_host_and_keeps_only_a_plain_http_scheme() {
        assert_eq!(host_label("https://api.nexus.inc"), (None, "api.nexus.inc"));
        assert_eq!(host_label("http://localhost:8080"), (Some("http://"), "localhost:8080"));
        assert_eq!(host_label("gw.test"), (None, "gw.test"));
    }

    #[test]
    fn a_status_says_the_sign_in_or_why_the_switch_failed() {
        let known = |saved_sign_in, org: Option<&str>| Known {
            url: "https://gw.example.com".into(),
            org: org.map(String::from),
            saved_sign_in,
        };
        assert_eq!(Status::of(&known(true, Some("acme"))).text(), "Saved sign-in for acme");
        assert_eq!(Status::of(&known(true, None)).text(), "Saved sign-in");
        assert_eq!(Status::of(&known(false, Some("acme"))).text(), "No saved sign-in");
        assert_eq!(Status::Failed("Could not reach it".into()).text(), "Could not reach it");
    }

    #[test]
    fn a_switch_saves_the_gateway_and_keeps_the_last_ones_organization() {
        let file = crate::desktop_settings::TempFile::new("gateways");
        nexus_desktop_core::settings::save(
            file.path(),
            &Settings {
                gateway_url: Some(DEFAULT_GATEWAY.into()),
                org: Some("acme".into()),
                kind: Some("cli".into()),
                ..Settings::default()
            },
        )
        .expect("save");
        let (saved, url) = file
            .update(|settings| {
                crate::desktop_settings::switch_gateway(settings, DEFAULT_GATEWAY, true, " http://localhost:8080/ ")
            })
            .expect("switched");
        assert_eq!(url, "http://localhost:8080");
        assert_eq!(saved.gateway_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(saved.org, None);
        assert_eq!(
            known_gateways(&nexus_desktop_core::settings::load(file.path()), &url),
            vec![
                (url.clone(), None),
                (DEFAULT_GATEWAY.to_string(), Some("acme".to_string())),
            ]
        );
        let refused = file.update(|settings| crate::desktop_settings::switch_gateway(settings, &url, true, "gw.test"));
        assert!(refused.is_err());
    }

    #[test]
    fn a_locked_gateway_offers_no_switch() {
        use nexus_desktop_core::policy::{Value as Pref, key};
        let locked = crate::managed::tests::policy_of(
            &[(key::GATEWAY_URL, Pref::Text("https://nexus.acme.test".into()))],
            None,
        );
        assert!(locked_by(&locked));
        assert!(!locked_by(&crate::managed::tests::policy_of(&[], None)));
    }
}
