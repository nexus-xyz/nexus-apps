//! Nexus's contact card (brand v5 A22·d, A24): the header, or ⌘I, opens it as a popover under the header. It is drawn
//! as the board draws it: 380 px, the raised tone with a hairline and a soft shadow, rows of 40 px between hairlines,
//! and a foot with Discover and a Settings pill. Each row opens its one home.
//!
//! With the strip gone from the thread (A24·c), the card is where the views are reached. This app has a home for six
//! of the board's rows: Errands and Projects (the Errands view, native), Accounts (Settings › Connectors), Memory,
//! Approvals and Watches (native views, else Nexus UI's Assistant page). Personality and voice has no source here yet,
//! so the card leaves it out, and it shows no counts it cannot read. Discover opens the Discover view, else sends
//! "See what I can do" (A15). Settings opens Account › Assistant, as the settings sidebar's row does.
//! Esc, a click outside or a row closes it.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::AgentPage;
use crate::pages::PageId;
use crate::palette::Palette;
use nexus_desktop_core::agent_cards as cards;

/// The card's rows, in the board's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Errands,
    Projects,
    Accounts,
    Memory,
    Approvals,
    Watches,
    Discover,
    Settings,
}

impl Row {
    pub fn label(self) -> &'static str {
        match self {
            Row::Errands => "Errands",
            Row::Projects => "Projects",
            Row::Accounts => "Accounts",
            Row::Memory => "Memory",
            Row::Approvals => "Approvals",
            Row::Watches => "Watches",
            Row::Discover => "Discover",
            Row::Settings => "Settings",
        }
    }

    fn icon(self) -> Lucide {
        match self {
            Row::Errands => Lucide::ListChecks,
            Row::Projects => Lucide::FolderOpen,
            Row::Accounts => Lucide::Plug,
            Row::Memory => Lucide::Brain,
            Row::Approvals => Lucide::ShieldCheck,
            Row::Watches => Lucide::Eye,
            Row::Discover | Row::Settings => Lucide::Settings,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Row::Errands => "agent-card-errands",
            Row::Projects => "agent-card-projects",
            Row::Accounts => "agent-card-accounts",
            Row::Memory => "agent-card-memory",
            Row::Approvals => "agent-card-approvals",
            Row::Watches => "agent-card-watches",
            Row::Discover => "agent-card-discover",
            Row::Settings => "agent-card-settings",
        }
    }
}

/// The rows between hairlines; Discover and Settings are the card's foot.
pub const ROWS: [Row; 6] = [
    Row::Errands,
    Row::Projects,
    Row::Accounts,
    Row::Memory,
    Row::Approvals,
    Row::Watches,
];

/// Where Accounts goes: this app's Settings › Connectors.
pub const ACCOUNTS_PATH: &str = "/settings/connectors";
/// Settings opens Account › Assistant, which is Nexus UI's Assistant settings until a native page exists
/// (`settings_registry.rs`, `Dest::Assistant`).
pub const SETTINGS_WEB: &str = "agent?view=memory";

impl AgentPage {
    /// The header, or ⌘I: open the card, or close it.
    pub fn toggle_card(&mut self, cx: &mut Context<Self>) {
        self.card_open = !self.card_open;
        cx.notify();
    }

    /// Whether the contact card shows, for the view tests.
    #[cfg(test)]
    pub fn card_open(&self) -> bool {
        self.card_open
    }

    /// A row of the card: it opens its home, and the card closes.
    pub fn open_row(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        self.card_open = false;
        let view = |this: &Self, id: PageId, window: &mut Window, cx: &mut Context<Self>| {
            this.open_view.clone().map(|open| open(id, None, window, cx)).is_some()
        };
        match row {
            Row::Errands | Row::Projects => {
                view(self, PageId::AgentErrands, window, cx);
            }
            Row::Accounts => (self.ctx.navigate_settings)(ACCOUNTS_PATH.into(), window, cx),
            Row::Memory => {
                if !view(self, PageId::AgentMemory, window, cx) {
                    self.ctx.open_web("agent?view=memory", cx)
                }
            }
            Row::Approvals => {
                if !view(self, PageId::AgentApprovals, window, cx) {
                    self.ctx.open_web("agent?view=approvals", cx)
                }
            }
            Row::Watches => {
                if !view(self, PageId::AgentWatches, window, cx) {
                    self.ctx.open_web("agent?view=watches", cx)
                }
            }
            Row::Discover => {
                if !view(self, PageId::AgentDiscover, window, cx) {
                    self.start_with(cards::DISCOVER_LINE, window, cx)
                }
            }
            Row::Settings => self.ctx.open_web(SETTINGS_WEB, cx),
        }
        cx.notify();
    }

    /// The card over the thread, centered under the header, with a backdrop that closes it on a click outside.
    pub(super) fn contact_card_view(&self, top: f32, palette: Palette, cx: &mut Context<Self>) -> Div {
        let needs_you = self.row().needs_you;
        let line = |row: Row| {
            h_flex()
                .id(row.id())
                .min_h(px(40.))
                .gap(px(12.))
                .border_t_1()
                .border_color(palette.border)
                .text_size(px(13.5))
                .cursor_pointer()
                .hover(|style| style.text_color(palette.strong))
                .on_click(cx.listener(move |this, _, window, cx| this.open_row(row, window, cx)))
                .child(Icon::new(row.icon()).size(px(15.)).text_color(palette.muted))
                .child(div().flex_1().text_color(palette.foreground).child(row.label()))
        };
        let rows = ROWS.map(|row| {
            let waiting = self.errands.len();
            let trailing = match row {
                Row::Approvals if needs_you => Some("Needs you".to_string()),
                Row::Errands if waiting > 0 => Some(format!("{waiting} needs you")),
                _ => None,
            };
            line(row)
                .children(trailing.map(|text| div().text_color(palette.muted).child(text)))
                .child(Icon::new(Lucide::ChevronRight).size(px(13.)).text_color(palette.muted))
        });
        let foot = h_flex()
            .gap(px(8.))
            .pt(px(12.))
            .border_t_1()
            .border_color(palette.border)
            .child(
                div()
                    .id(Row::Discover.id())
                    .flex_1()
                    .text_size(px(12.5))
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.strong))
                    .on_click(cx.listener(|this, _, window, cx| this.open_row(Row::Discover, window, cx)))
                    .child(Row::Discover.label()),
            )
            .child(
                div()
                    .id(Row::Settings.id())
                    .flex_none()
                    .h(px(28.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .rounded(px(14.))
                    .border_1()
                    .border_color(palette.border)
                    .text_size(px(12.5))
                    .text_color(palette.foreground)
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .on_click(cx.listener(|this, _, window, cx| this.open_row(Row::Settings, window, cx)))
                    .child(Row::Settings.label()),
            );
        let card = v_flex()
            .id("agent-contact-card")
            .occlude()
            .w(px(380.))
            .pt(px(18.))
            .px(px(20.))
            .pb(px(14.))
            .rounded(px(14.))
            .bg(palette.pop)
            .border_1()
            .border_color(palette.border)
            .shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., 0.25),
                offset: point(px(0.), px(24.)),
                blur_radius: px(60.),
                spread_radius: px(-24.),
                inset: false,
            }])
            .children(rows)
            .child(foot);
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("agent-card-backdrop")
                    .occlude()
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.card_open = false;
                            cx.notify();
                        }),
                    ),
            )
            .child(
                h_flex()
                    .absolute()
                    .top(px(top))
                    .left_0()
                    .right_0()
                    .justify_center()
                    .child(crate::motion::fade_in(card, "agent-contact-card-in")),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{ROWS, Row};

    #[test]
    fn the_card_has_the_boards_rows_this_app_has_a_home_for() {
        let labels: Vec<_> = ROWS.iter().map(|row| row.label()).collect();
        assert_eq!(
            labels,
            ["Errands", "Projects", "Accounts", "Memory", "Approvals", "Watches"]
        );
        assert_eq!((Row::Discover.label(), Row::Settings.label()), ("Discover", "Settings"));
    }
}
