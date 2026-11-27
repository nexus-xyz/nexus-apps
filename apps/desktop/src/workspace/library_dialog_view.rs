//! The Library's dialogs draw in the Share sheet's shell: a 480 wide sheet on the scrim, its title
//! and a close button on top. Errors stay visible in it.
use super::*;
use crate::pages::ui;
use crate::palette::Palette;
use crate::workspace::glass::FloatingGround;
use crate::workspace::share_ui::{icon_button, ring};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};

const WIDTH: f32 = 480.;

impl Render for LibraryDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::pages::palette(cx);
        let body = if self.action == Action::Share {
            self.render_share(palette, cx)
        } else {
            self.render_other(palette, cx)
        };
        deferred(
            div()
                .id("library-dialog-overlay")
                .absolute()
                .inset_0()
                .size_full()
                .occlude()
                .bg(palette.scrim)
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                )
                .child(
                    div()
                        .id("library-dialog")
                        .track_focus(&self.focus)
                        .key_context("LibraryDialog")
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            let keys = &event.keystroke;
                            match keys.key.as_str() {
                                "escape" => this.close(window, cx),
                                "enter" if keys.modifiers.control || keys.modifiers.platform => {
                                    if this.action == Action::Share {
                                        this.save_share(cx);
                                    }
                                }
                                "tab" if this.action == Action::Share && this.share.kind == Audience::External => {
                                    let input = this.share.outside.clone();
                                    if input.read(cx).focus_handle(cx).is_focused(window) {
                                        window.focus(&this.focus, cx);
                                    } else {
                                        input.update(cx, |state, cx| state.focus(window, cx));
                                    }
                                }
                                _ => return,
                            }
                            cx.stop_propagation();
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(px(WIDTH))
                        .max_w(relative(0.94))
                        .max_h(relative(0.9))
                        .flex()
                        .flex_col()
                        .rounded(px(12.))
                        .floating_ground(12., palette.menu, true, &palette)
                        .border_1()
                        .border_color(ring(palette))
                        .shadow(palette.pop_shadow(palette.border))
                        .text_size(px(13.))
                        .text_color(palette.foreground)
                        // The body scrolls inside the card, so the card's glass stays under it.
                        .child(
                            div()
                                .id("library-dialog-scroll")
                                .min_h(px(0.))
                                .overflow_y_scroll()
                                .child(body),
                        ),
                ),
        )
        .with_priority(3)
    }
}

impl LibraryDialog {
    /// The sheet's title row: the title, and a close button that names its key.
    pub(super) fn header(&self, title: &'static str, palette: Palette, cx: &mut Context<Self>) -> Div {
        h_flex()
            .h(px(44.))
            .pt(px(16.))
            .pr(px(12.))
            .pl(px(20.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(16.))
                    .line_height(px(24.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                icon_button("library-dialog-close", Lucide::X, palette)
                    .aria_label("Close")
                    .tooltip(|window, cx| Tooltip::new("Close  Esc").build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
            )
    }

    /// The error line, in the destructive color.
    pub(super) fn problem_line(&self, palette: Palette) -> Option<Div> {
        self.problem.as_ref().map(|problem| {
            div()
                .px(px(20.))
                .py(px(4.))
                .line_height(px(20.))
                .text_color(palette.destructive)
                .child(problem.clone())
        })
    }

    /// Version history, Stop sharing this link and Delete.
    fn render_other(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let title = match self.action {
            Action::Versions => "Version history",
            Action::Revoke => "Stop sharing this link?",
            _ => "Delete this Artifact?",
        };
        let mut body = v_flex().px(px(20.)).pt(px(4.)).pb(px(8.)).gap(px(12.));
        body = body.child(div().text_color(palette.muted).truncate().child(self.row.title.clone()));
        let mut footer = h_flex()
            .pt(px(8.))
            .px(px(20.))
            .pb(px(20.))
            .gap(px(8.))
            .child(div().flex_1());
        if !self.ready {
            body = body.child(if self.busy {
                "Loading…"
            } else {
                "Could not load this item."
            });
            if !self.busy {
                footer = footer.child(
                    ui::button("library-dialog-retry", "Retry", palette)
                        .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                );
            }
        } else if self.completed {
            body = body.child(match self.action {
                Action::Revoke => "This link was revoked. Other links and the original content stay available.",
                _ => "The Artifact and all its versions were deleted.",
            });
            footer = footer.child(
                ui::button("library-dialog-done", "Done", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
            );
        } else if self.action == Action::Versions {
            body = body.child("Select a version to open it in your browser.");
            body = body.children(self.version_rows(palette, cx));
        } else {
            body = body.child(if self.action == Action::Revoke {
                "Readers will lose access through this link. The original content and other links stay available."
            } else {
                "This permanently deletes the Artifact and every version. This cannot be undone."
            });
            let label = if self.busy {
                "Saving…"
            } else if self.action == Action::Revoke {
                "Stop sharing"
            } else {
                "Delete Artifact"
            };
            footer = footer
                .child(
                    ui::button("library-dialog-cancel", "Cancel", palette)
                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                )
                .child(
                    ui::danger_button("library-confirm-action", label, palette).when(!self.busy, |button| {
                        button.on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                    }),
                );
        }
        v_flex()
            .child(self.header(title, palette, cx))
            .child(body)
            .children(self.problem_line(palette))
            .child(footer)
            .into_any_element()
    }

    fn version_rows(&self, palette: Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (Some(versions), Some(page)) = (self.detail["versions"].as_array(), &self.row.owned_page) else {
            return Vec::new();
        };
        versions
            .iter()
            .filter_map(|version| {
                let number = version["version"].as_u64()?;
                let request_path = format!("/v1/pages/{}/versions/{number}/frame", nexus_client::url::enc(page));
                let detail = format!(
                    "{} · {}",
                    version["title"].as_str().unwrap_or_default(),
                    version["createdAt"].as_str().unwrap_or_default()
                );
                Some(
                    h_flex()
                        .id(SharedString::from(format!("library-version-{number}")))
                        .min_h(px(40.))
                        .mx(px(-10.))
                        .px(px(10.))
                        .gap(px(12.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.menu_hover))
                        .child(div().flex_none().child(format!("Version {number}")))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_color(palette.muted)
                                .child(detail),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let request = this.gateway.request(Verb::Get, request_path.clone(), None);
                            fetch(cx, request, |this, answer, cx| {
                                if !this.current(cx) {
                                    return;
                                }
                                match answer {
                                    Ok(answer) => {
                                        if let Some(url) = answer["url"]
                                            .as_str()
                                            .and_then(|value| library::reader_url(&this.gateway.site_url(), value))
                                        {
                                            crate::external_link::open(&url, cx);
                                        }
                                    }
                                    Err(error) => this.problem = Some(error),
                                }
                            });
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }
}
