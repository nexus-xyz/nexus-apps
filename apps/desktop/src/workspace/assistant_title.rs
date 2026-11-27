//! The title bar over the Assistant, as brand v5 A24 draws it on the desktop.
//!
//! - Over the thread (A24·c) the title is empty: the thread's own header names Nexus and the scope. ⋯ sits at the right
//!   end, and holds New conversation and the Assistant's views. The composer has no New conversation link, and no
//!   strip of tabs sits over the thread.
//! - Over an errand opened alone (A24·b) the title is a crumb: the 15 px mark, the Assistant's name, "›" and the
//!   errand's name. The name goes back to the thread.
//!
//! The words and sizes are the board's: 13 px, the name in the second tone, "›" in the third with 6 px each side, and
//! the errand in the ink. The web has the same title bar.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::code_dock::menu_box;
use super::overlay::{Overlay, OverlaySlot as _};
use super::{View, Workspace, bar_button};
use crate::pages::PageId;
use crate::palette::Palette;
use crate::workspace::{MenuTrigger as _, menu_item_hover};

/// The views ⋯ opens over the thread, in the order the strip had them.
pub(super) const MENU_VIEWS: [PageId; 7] = [
    PageId::AgentErrands,
    PageId::AgentDiscover,
    PageId::AgentMemory,
    PageId::AgentApprovals,
    PageId::AgentVault,
    PageId::AgentWatches,
    PageId::AgentPrivacy,
];

impl Workspace {
    /// Whether the title bar over `id` is the Assistant's: the thread, or an errand opened alone.
    pub(super) fn assistant_titled(&self, id: PageId, cx: &App) -> bool {
        if !self.assistant_available() {
            return false;
        }
        match id {
            PageId::Agent => true,
            PageId::AgentErrands => self.assistant_alone(cx),
            _ => false,
        }
    }

    /// The Assistant's name in the crumb: the person's choice, else Nexus.
    fn assistant_name(&self) -> SharedString {
        self.assistant
            .name
            .clone()
            .unwrap_or_else(|| nexus_desktop_core::agent_cards::DEFAULT_ASSISTANT_NAME.into())
    }

    /// The thread's title (empty, with ⋯) or an errand's crumb.
    pub(super) fn render_assistant_title(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let bar = h_flex()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .pl(px(20.))
            .pr(px(8.))
            .gap(px(4.))
            .text_size(px(13.));
        if matches!(self.view, View::Page(PageId::AgentErrands)) {
            return bar.child(self.render_errand_crumb(palette, cx));
        }
        bar.child(div().flex_1()).child(self.render_assistant_more(palette, cx))
    }

    /// "Nexus › Vendor contracts" (A24·b). The name opens the thread; before the errand is read, the name alone.
    fn render_errand_crumb(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("assistant-crumb")
            .min_w(px(0.))
            .child(div().flex_none().mr(px(8.)).child(crate::pages::agent::avatar(
                "assistant-crumb-mark",
                15.,
                false,
                palette,
            )))
            .child(
                div()
                    .id("assistant-crumb-name")
                    .flex_none()
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.strong))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| this.open_assistant(PageId::Agent, None, window, cx)))
                    .child(self.assistant_name()),
            )
            .children(
                self.assistant_crumb()
                    .map(|errand| {
                        [
                            div()
                                .flex_none()
                                .px(px(6.))
                                .text_color(palette.faint)
                                .child("›")
                                .into_any_element(),
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .text_color(palette.foreground)
                                .child(errand.to_string())
                                .into_any_element(),
                        ]
                    })
                    .into_iter()
                    .flatten(),
            )
    }

    /// ⋯ at the title bar's right end over the thread, and its menu.
    fn render_assistant_more(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.overlay.assistant_menu();
        div()
            .relative()
            .flex_none()
            .child(
                bar_button("assistant-more", Lucide::Ellipsis, palette)
                    .when(open, |button| button.bg(palette.hover))
                    .when(!open, |button| {
                        button
                            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("More").build(window, cx))
                    })
                    .menu_trigger(
                        open,
                        cx,
                        |this: &Workspace| this.overlay.assistant_menu(),
                        |this, _, cx| {
                            this.toggle_overlay(Overlay::AssistantMenu);
                            cx.notify();
                        },
                    ),
            )
            .when(open, |more| more.child(self.render_assistant_menu(palette, cx)))
    }

    /// New conversation while there is one to leave, then the Assistant's views (the web's ⋯).
    fn render_assistant_menu(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |id: ElementId, label: &'static str| {
            h_flex()
                .id(id)
                .h(px(30.))
                .px(px(8.))
                .rounded(px(6.))
                .text_color(palette.foreground)
                .cursor_pointer()
                .hover(|style| menu_item_hover(style, palette))
                .child(label)
        };
        let fresh = self
            .assistant_kept_page()
            .is_some_and(|page| page.read(cx).can_start_over());
        deferred(
            menu_box("assistant-menu", 220., palette)
                .absolute()
                .top(px(34.))
                .right_0()
                .rounded(px(10.))
                .text_size(px(13.))
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.overlay.close_assistant_menu();
                    cx.notify();
                }))
                .when(fresh, |menu| {
                    menu.child(
                        item("assistant-new-conversation".into(), "New conversation").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.overlay.close_assistant_menu();
                                if let Some(page) = this.assistant_kept_page() {
                                    page.update(cx, |page, cx| page.start_over(window, cx));
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(div().my(px(4.)).h(px(1.)).bg(palette.border))
                })
                .children(MENU_VIEWS.map(|id| {
                    item(("assistant-menu-view", id as usize).into(), id.label()).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.overlay.close_assistant_menu();
                            this.open_assistant(id, None, window, cx);
                        },
                    ))
                })),
        )
        .with_priority(1)
    }

    /// Whether Errands shows one errand alone (A24·b), also while it loads.
    pub(super) fn assistant_alone(&self, cx: &App) -> bool {
        self.assistant_errands_page().is_some_and(|page| page.read(cx).alone())
    }
}
