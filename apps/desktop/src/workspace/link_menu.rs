//! Native link context menu. The text renderer resolves the actual link under the pointer.
use super::{View, Workspace};
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::base::TextSelection;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::link_menu::{LinkMenu, Preview, Session};
use std::sync::Arc;

pub(super) type LinkHandler = Arc<dyn Fn(&SharedString, &ClickEvent, &mut Window, &mut App) + Send + Sync>;
pub(super) struct LinkPopup {
    menu: Entity<PopupMenu>,
    position: Point<Pixels>,
    _dismiss: Subscription,
}

#[cfg(test)]
impl LinkPopup {
    /// An empty menu, for the overlay tests.
    pub(super) fn sample(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let menu = PopupMenu::build(window, cx, |menu, _, _| menu);
        let dismiss = cx.subscribe_in(&menu, window, |_, _, _: &DismissEvent, _, _| {});
        LinkPopup {
            menu,
            position: Point::default(),
            _dismiss: dismiss,
        }
    }
}
impl Workspace {
    pub(super) fn context_link_handler(
        &self,
        normal: impl Fn(&SharedString, &ClickEvent, &mut Window, &mut App) + Send + Sync + 'static,
        cx: &Context<Self>,
    ) -> LinkHandler {
        let this = cx.entity().downgrade();
        let normal: LinkHandler = Arc::new(normal);
        Arc::new(move |href, event, window, cx| {
            let right = matches!(event, ClickEvent::Mouse(click) if click.up.button == MouseButton::Right);
            if right {
                let ClickEvent::Mouse(click) = event else { return };
                if click.up.modifiers.shift || !TextSelection::selected_text(window, cx).is_empty() {
                    return;
                }
                let href = href.to_string();
                let position = click.up.position;
                let _ = this.update(cx, |this, cx| this.open_link_menu(&href, position, window, cx));
            } else if !matches!(event, ClickEvent::Touch(click) if click.long_press) {
                normal(href, event, window, cx);
            }
        })
    }
    fn open_link_menu(&mut self, href: &str, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let session = match &self.view {
            // ChatBrowser registers a local preview target for Gateway chats too.
            View::Chat => Some(Session::Local),
            View::Code(view) if !view.is_draft() => Some(if crate::machines::get().machine_of(&view.id).is_some() {
                Session::Remote
            } else {
                Session::Local
            }),
            _ => None,
        };
        let ui = self.gateway.site_url();
        let Some(model) = nexus_workbench::link_menu::for_link(href, &format!("{ui}/app/"), &ui, session) else {
            return;
        };
        window.prevent_default();
        cx.stop_propagation();
        let previous = window.focused(cx);
        let weak = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            if let Some(previous) = previous {
                menu = menu.action_context(previous);
            }
            let copy = match model {
                LinkMenu::Internal { path, copy } => {
                    let weak = weak.clone();
                    menu = menu.item(PopupMenuItem::new("Open").on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| this.open_nexus_path(&path, window, cx));
                    }));
                    copy
                }
                LinkMenu::External { url, preview } => {
                    let (address, note) = match preview {
                        Preview::Url(url) => (Some(url), None),
                        Preview::Unsupported => (None, Some("Use HTTP or HTTPS without a username or password")),
                        Preview::NoSession => (None, Some("Open a Code session to preview")),
                        Preview::Remote => (None, Some("Preview is local to this computer")),
                    };
                    let disabled = address.is_none();
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Open in integrated browser")
                            .disabled(disabled)
                            .on_click(move |_, window, cx| {
                                if let Some(url) = &address {
                                    let _ = weak.update(cx, |this, cx| this.show_in_side_browser(url, window, cx));
                                }
                            }),
                    );
                    if let Some(note) = note {
                        menu = menu.item(PopupMenuItem::label(note));
                    }
                    let system = url.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Open in system browser")
                            .on_click(move |_, _, cx| crate::external_link::open(&system, cx)),
                    );
                    url
                }
            };
            menu.item(
                PopupMenuItem::new("Copy link")
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))),
            )
        });
        let dismiss = cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, _, cx| {
            this.overlay.close_link_popup();
            cx.notify();
        });
        menu.focus_handle(cx).focus(window, cx);
        self.open_overlay(Overlay::LinkPopup(LinkPopup {
            menu,
            position,
            _dismiss: dismiss,
        }));
        cx.notify();
    }
    /// A file chip's menu (`file_chips.rs`): Open in Files, Open in app, Reveal in file manager, Copy path. The app and
    /// the file manager are this computer's, so a server's session has only Files and the path.
    pub(super) fn open_file_menu(
        &mut self,
        reference: nexus_workbench::inline_media::FileReference,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = self.reference_folder();
        let openable = folder
            .as_ref()
            .is_some_and(|folder| nexus_workbench::output_files::openable(folder, &reference.path).is_ok());
        let previous = window.focused(cx);
        let weak = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            if let Some(previous) = previous {
                menu = menu.action_context(previous);
            }
            let (files, app, reveal) = (weak.clone(), weak.clone(), weak.clone());
            let (to_files, to_app, to_reveal) = (reference.clone(), reference.path.clone(), reference.path.clone());
            let app_folder = folder.clone();
            menu = menu.item(PopupMenuItem::new("Open in Files").on_click(move |_, window, cx| {
                let reference = to_files.clone();
                let _ = files.update(cx, |this, cx| {
                    this.open_in_files(reference.path, reference.line, window, cx)
                });
            }));
            menu = menu.item(
                PopupMenuItem::new("Open in app")
                    .disabled(!openable)
                    .on_click(move |_, _, cx| {
                        if let Some(folder) = &app_folder {
                            let _ = app.update(cx, |this, cx| this.open_reference_in_app(folder, &to_app, cx));
                        }
                    }),
            );
            if folder.is_some() {
                menu = menu.item(PopupMenuItem::new("Reveal in file manager").on_click(move |_, _, cx| {
                    let _ = reveal.update(cx, |this, cx| this.reveal_reference(&to_reveal, cx));
                }));
            }
            let path = reference.path.clone();
            menu.item(
                PopupMenuItem::new("Copy path")
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(path.clone()))),
            )
        });
        let dismiss = cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, _, cx| {
            this.overlay.close_link_popup();
            cx.notify();
        });
        menu.focus_handle(cx).focus(window, cx);
        self.open_overlay(Overlay::LinkPopup(LinkPopup {
            menu,
            position,
            _dismiss: dismiss,
        }));
        cx.notify();
    }

    pub(super) fn render_link_menu(&self) -> Option<AnyElement> {
        let popup = self.overlay.link_popup()?;
        Some(
            deferred(
                anchored()
                    .position(popup.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(popup.menu.clone()),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }
}
