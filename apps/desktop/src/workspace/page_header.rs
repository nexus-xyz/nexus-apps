//! The open page's header in the Workbench's Artifacts tab, as Claude's artifact header: the title opens the page's
//! menu, then Share, Full view and Close on the right. The design's Comments button waits for the desktop to draw page
//! comments (see `render_page_header`).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::h_flex;
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Workspace, bar_button};
use crate::library::Action;
use crate::palette::Palette;

/// Below this Workbench width, Share shows its icon only, so the title keeps room before anything else gives way.
pub(super) const SHARE_LABEL_MIN_WIDTH: f32 = 400.;

/// What Share says: "Share" with a lock while only you can open the page, "Shared" with people once someone else can.
pub(super) fn share_face(shared: bool) -> (Lucide, &'static str) {
    if shared {
        (Lucide::Users, "Shared")
    } else {
        (Lucide::Lock, "Share")
    }
}

impl Workspace {
    /// The header, `height` tall: the title and its menu, Share, Full view and Close. `narrow` drops Share's label.
    pub(super) fn render_page_header(
        &self,
        title: SharedString,
        height: f32,
        narrow: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let full = self.side.page_full();
        h_flex()
            .h(px(height))
            .flex_none()
            .pl(px(crate::side::INSET - 8.))
            .pr(px(8.))
            .gap(px(4.))
            .border_b_1()
            .border_color(palette.border)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .child(self.render_page_title(title, palette, cx)),
            )
            .children(self.render_page_share(narrow, cx))
            // Comments (the design's Comments ⌄): the icon opens the page's comment threads and shows how many are
            // open; the caret's menu shows them on the page, shows resolved ones, or sends them to a session. Hidden
            // until the desktop reads and draws page comments (Gateway's `GET /v1/pages/{id}/comments`). Then add it
            // here, between Share and Full view:
            //
            // .child(
            //     Button::new("page-comments")
            //         .icon(Lucide::MessageSquare)
            //         .ghost()
            //         .dropdown_caret(true)
            //         .when(open > 0, |button| button.label(open.to_string()))
            //         .tooltip("Comments")
            //         .on_click(cx.listener(|this, _, window, cx| this.toggle_page_comments(window, cx)))
            //         .dropdown_menu_with_anchor(Anchor::TopRight, page_comments_menu),
            // )
            .child(
                bar_button(
                    "page-full-view",
                    if full { Lucide::Minimize2 } else { Lucide::Maximize2 },
                    palette,
                )
                .debug_selector(|| "page-full-view".into())
                .tooltip(move |window, cx| {
                    Tooltip::new(if full { "Leave full view (Esc)" } else { "Full view" }).build(window, cx)
                })
                .on_click(cx.listener(|this, _, window, cx| this.toggle_page_full_view(window, cx))),
            )
            .child(
                bar_button("page-close", Lucide::X, palette)
                    .debug_selector(|| "page-close".into())
                    .tooltip(|window, cx| Tooltip::new("Close the Artifact").build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.close_page(window, cx))),
            )
    }

    /// The title, with a caret that opens the page's menu (`page_menu`). A page with no menu (no Library item and no
    /// address) shows its title only. A title too long for the header fades out at its right edge, as a sidebar
    /// session's does (`title_fade.rs`), and hovering it shows the whole title.
    fn render_page_title(&self, title: SharedString, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.page_item().is_none() && self.page_link().is_none() {
            let tip = title.clone();
            return div()
                .id("page-title")
                .px(px(8.))
                .min_w(px(0.))
                .flex()
                .overflow_hidden()
                .text_size(px(14.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .debug_selector(|| "page-title".into())
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(super::title_fade::fitted_fading_title(title))
                .into_any_element();
        }
        let workspace = cx.entity().downgrade();
        Button::new("page-title")
            .child(super::title_fade::fitted_fading_title(title.clone()))
            .dropdown_caret(true)
            .ghost()
            // A button does not shrink by default, so a long title would push past Share and Full view.
            .flex_shrink(1.)
            .min_w(px(0.))
            .max_w_full()
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .tooltip(title.clone())
            .accessibility_label(format!("Actions for {title}"))
            .debug_selector(|| "page-title".into())
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, cx| {
                let Some(workspace) = workspace.upgrade() else {
                    return menu;
                };
                workspace.update(cx, |this, cx| this.page_menu(menu, cx))
            })
            .into_any_element()
    }

    /// Share, for a page of yours the Library lists: it opens the Share dialog. None for anything you cannot share.
    fn render_page_share(&self, narrow: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let admin = self
            .account
            .as_ref()
            .is_some_and(|a| matches!(a.role.as_str(), "owner" | "admin"));
        let item = self.page_item()?;
        if !item.actions(admin).contains(&Action::Share) {
            return None;
        }
        let (icon, label) = share_face(self.page_shared());
        Some(
            Button::new("page-share")
                .icon(icon)
                .ghost()
                .when(!narrow, |button| button.label(label))
                .flex_none()
                .accessibility_label(label)
                .tooltip(if self.page_shared() {
                    "Shared: change who can open this Artifact"
                } else {
                    "Only you can open this Artifact: share it"
                })
                .debug_selector(|| "page-share".into())
                .on_click(cx.listener(|this, _, window, cx| this.share_page(window, cx)))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn share_says_whether_someone_else_can_open_the_page() {
        assert!(matches!(share_face(false), (Lucide::Lock, "Share")));
        assert!(matches!(share_face(true), (Lucide::Users, "Shared")));
    }
}

#[cfg(test)]
#[path = "page_header_tests.rs"]
mod view_tests;
