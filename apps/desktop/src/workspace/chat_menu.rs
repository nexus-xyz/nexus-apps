//! The open chat's title in the top bar and its "More actions" menu, as Nexus UI has them (`chat/session-view.tsx`):
//! Copy as Markdown and Download .md (both Gateway's export, so they are the same bytes), Rename in place, Archive or
//! Restore from the archive, and Delete after "Delete it and its messages?".

use crate::motion;
use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::json;

use super::code_dock::menu_box;
use super::{View, Workspace, glyph};
use crate::gateway::Verb;
use crate::pages::fetch;
use crate::palette::Palette;

/// Which page of the menu shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum ChatMenu {
    Main,
    /// "Copied" in place of "Copy as Markdown", for a moment.
    Copied,
    Delete,
}

impl Workspace {
    /// The open chat's title button (or the field renaming it), then the Share button.
    /// The title's chevron opens the chat menu; a double click renames. Find is Ctrl F, the menu and the palette; the
    /// Browser is a tab of the right panel (Ctrl Shift B), so the bar keeps only Share and the panel toggle.
    pub(super) fn render_chat_title(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let saved = matches!(self.view, View::Chat) && self.current.is_some();
        let open = self.overlay.chat_menu().is_some();
        let title = match &self.chat_rename {
            Some((input, _)) if saved => div()
                .w(px(320.))
                .h(px(28.))
                .flex()
                .items_center()
                .ml(px(-6.))
                .rounded(px(6.))
                .bg(palette.raised)
                .border_1()
                .border_color(palette.border)
                .text_color(palette.foreground)
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.chat_rename = None;
                    cx.notify();
                }))
                .child(Input::new(input).appearance(false))
                .into_any_element(),
            _ if saved => div()
                .relative()
                .min_w(px(0.))
                .max_w(relative(0.5))
                .child(
                    h_flex()
                        .id("chat-title")
                        .h(px(28.))
                        .min_w(px(0.))
                        .ml(px(-6.))
                        .pl(px(6.))
                        .pr(px(6.))
                        .gap(px(4.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(open, |button| button.bg(palette.selected))
                        .hover(|style| style.bg(palette.selected))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, event: &MouseDownEvent, window, cx| {
                                // The title bar's double click zooms the window: here it renames.
                                if event.click_count == 2 {
                                    cx.stop_propagation();
                                    cx.defer_in(window, |this, window, cx| this.start_chat_rename(window, cx));
                                }
                            }),
                        )
                        .child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette.foreground)
                                .child(self.title.clone()),
                        )
                        .child(glyph(Lucide::ChevronDown, palette.faint).size(px(14.)).flex_none())
                        .menu_trigger(
                            open,
                            cx,
                            |this| this.overlay.chat_menu().is_some(),
                            |this, _, cx| {
                                this.toggle_overlay(Overlay::ChatMenu(ChatMenu::Main));
                                cx.notify();
                            },
                        ),
                )
                .children(
                    motion::presence_of("chat-menu", self.overlay.chat_menu(), cx)
                        .map(|(phase, page)| self.render_chat_menu(page, phase, palette, cx)),
                )
                .into_any_element(),
            _ => div()
                .min_w(px(0.))
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.foreground)
                .child(self.title.clone())
                .into_any_element(),
        };
        // Share shows on a saved chat with messages; while an answer streams it stays in place, dimmed, so nothing
        // jumps when the answer ends.
        let streaming = self.asking.is_some();
        h_flex()
            .flex_1()
            .min_w(px(0.))
            .pl(px(16.))
            .pr(px(4.))
            .gap(px(4.))
            .text_size(px(13.))
            .child(title)
            .child(div().flex_1())
            .when(saved && !self.entries.is_empty(), |row| {
                row.child(
                    div()
                        .id("chat-share-button")
                        .flex_none()
                        .h(px(28.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .bg(palette.hover)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.foreground)
                        .child("Share")
                        .when(streaming, |button| {
                            button.opacity(0.4).tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Share when this answer finishes")
                                    .build(window, cx)
                            })
                        })
                        .when(!streaming, |button| {
                            button
                                .cursor_pointer()
                                .hover(|style| style.bg(palette.selected))
                                .tooltip(|window, cx| {
                                    crate::shortcut_label::tooltip("Share", SHARE_KEYS).build(window, cx)
                                })
                                .on_click(cx.listener(|this, _, window, cx| this.open_chat_share(window, cx)))
                        }),
                )
            })
    }

    fn render_chat_menu(
        &self,
        page: ChatMenu,
        phase: motion::Phase,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let item = |id: &'static str, icon: Lucide, label: &'static str, danger: bool| {
            let tone = if danger {
                palette.destructive
            } else {
                palette.foreground
            };
            h_flex()
                .id(id)
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(6.))
                .text_color(tone)
                .cursor_pointer()
                .hover(|style| menu_item_hover(style, palette))
                .child(glyph(icon, if danger { palette.destructive } else { palette.icon }).size(px(16.)))
                .child(div().flex_1().child(label))
        };
        let has_messages = !self.entries.is_empty();
        let mut body = menu_box("chat-menu", 240., palette)
            .rounded(px(10.))
            .text_size(px(13.))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.overlay.close_chat_menu();
                cx.notify();
            }));
        body = match page {
            // The Delete page: the menu turns into it in place, same box and anchor.
            ChatMenu::Delete => body
                .child(
                    div()
                        .px(px(8.))
                        .pt(px(6.))
                        .pb(px(2.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Delete this chat and its messages?"),
                )
                .child(
                    div()
                        .px(px(8.))
                        .pt(px(4.))
                        .pb(px(6.))
                        .text_size(px(12.))
                        .text_color(palette.muted)
                        .child("Its pages stay in Artifacts."),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap(px(6.))
                        .pt(px(6.))
                        .px(px(4.))
                        .pb(px(4.))
                        .child(
                            menu_button("chat-keep", "Keep", palette.menu_hover, palette.foreground)
                                .hover(|style| style.bg(palette.selected))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.overlay.close_chat_menu();
                                    cx.notify();
                                })),
                        )
                        .child(
                            menu_button("chat-delete-it", "Delete", palette.destructive, gpui_kit::white())
                                .font_weight(FontWeight::MEDIUM)
                                .hover(|style| style.opacity(0.9))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(id) = this.current.clone() {
                                        this.delete_chat(id, window, cx);
                                    }
                                })),
                        ),
                ),
            // Rename, Copy as Markdown, Download .md, Find in this chat, Archive, then Delete…
            // Share lives on the top bar's button, Ctrl Shift S and the palette.
            _ => body
                .child(
                    item("chat-rename", Lucide::Pencil, "Rename", false)
                        .on_click(cx.listener(|this, _, window, cx| this.start_chat_rename(window, cx))),
                )
                .when(has_messages, |body| {
                    let copied = page == ChatMenu::Copied;
                    body.child(
                        item(
                            "chat-copy",
                            if copied { Lucide::Check } else { Lucide::Copy },
                            if copied { "Copied" } else { "Copy as Markdown" },
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.export_chat(false, cx))),
                    )
                    .child(
                        item("chat-download", Lucide::Download, "Download .md", false)
                            .on_click(cx.listener(|this, _, _, cx| this.export_chat(true, cx))),
                    )
                    .child(
                        item("chat-find", Lucide::Search, "Find in this chat", false)
                            .children(
                                crate::key_bindings::keystroke("find-in-session").map(|keys| key_text(&keys, palette)),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.overlay.close_chat_menu();
                                this.open_find(window, cx);
                            })),
                    )
                })
                .child(
                    item(
                        "chat-archive",
                        if self.chat_archived {
                            Lucide::ArchiveRestore
                        } else {
                            Lucide::Archive
                        },
                        if self.chat_archived {
                            "Restore from the archive"
                        } else {
                            "Archive"
                        },
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(id) = this.current.clone() {
                            this.set_chat_archived(id, !this.chat_archived, cx);
                        }
                    })),
                )
                .child(div().h(px(1.)).mx(px(8.)).my(px(4.)).bg(palette.border.opacity(0.7)))
                .child(
                    item("chat-delete", Lucide::Trash, "Delete…", true).on_click(cx.listener(|this, _, _, cx| {
                        this.open_overlay(Overlay::ChatMenu(ChatMenu::Delete));
                        cx.notify();
                    })),
                ),
        };
        // 4 below the title button, left edges aligned.
        div().absolute().left(px(-6.)).top(px(32.)).child(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(phase.menu("chat-menu", -4., body)),
            )
            .with_priority(1),
        )
    }

    fn chat_path(&self) -> Option<String> {
        self.current.as_deref().map(chat_path)
    }

    /// Gateway's Markdown export, onto the clipboard or into a file the person picks.
    pub(super) fn export_chat(&mut self, download: bool, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(path) = self.chat_path() else { return };
        if download {
            self.overlay.close_chat_menu();
        }
        let name = markdown_filename(&self.title);
        let request = account_gateway.text(format!("{path}/export.md"));
        cx.spawn(async move |this, cx| {
            let answer = request.await;
            let markdown = match answer {
                Ok(markdown) => markdown,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.notice = Some(format!("Couldn't export the chat: {error}").into());
                        cx.notify();
                    });
                    return;
                }
            };
            if !download {
                let _ = this.update(cx, |this, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(markdown));
                    if this.overlay.chat_menu().is_some() {
                        this.open_overlay(Overlay::ChatMenu(ChatMenu::Copied));
                    }
                    cx.notify();
                });
                return;
            }
            let folder = downloads_folder();
            let picked = cx.update(|cx| cx.prompt_for_new_path(&folder, Some(&name)));
            if let Ok(Ok(Some(file))) = picked.await
                && let Err(error) = std::fs::write(&file, markdown)
            {
                let _ = this.update(cx, |this, cx| {
                    this.notice = Some(format!("Couldn't save {}: {error}", file.display()).into());
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    pub(super) fn start_chat_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.close_chat_menu();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(self.title.to_string(), window, cx);
            state
        });
        let subscription = cx.subscribe_in(&input, window, |this, state, event: &InputEvent, _, cx| match event {
            InputEvent::PressEnter { .. } => {
                let title = state.read(cx).value().trim().to_string();
                this.chat_rename = None;
                if !title.is_empty() && title != this.title.as_ref() {
                    if let Some(id) = this.current.clone() {
                        this.rename_chat(id, title, cx);
                    }
                }
                cx.notify();
            }
            InputEvent::Blur => {
                this.chat_rename = None;
                cx.notify();
            }
            _ => {}
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        self.chat_rename = Some((input, subscription));
        cx.notify();
    }

    /// Rename a chat (`PATCH /v1/chat/sessions/{id}`); the top bar shows the new title at once when it is open.
    pub(super) fn rename_chat(&mut self, id: String, title: String, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let before = (self.current.as_deref() == Some(id.as_str()))
            .then(|| std::mem::replace(&mut self.title, title.clone().into()));
        if let Some(row) = self.sessions.iter_mut().find(|session| session.id == id) {
            row.title = title.clone();
        }
        let request = account_gateway.request(Verb::Patch, chat_path(&id), Some(json!({ "title": title })));
        fetch(cx, request, move |this, answer, cx| {
            if let Err(error) = answer {
                if let Some(before) = before {
                    this.title = before;
                }
                this.notice = Some(format!("Couldn't rename it: {error}").into());
            }
            this.refresh_chat_lists(cx);
        });
        cx.notify();
    }

    /// Archive a chat, or bring it back from the archive. It leaves the Sessions list at once.
    pub(super) fn set_chat_archived(&mut self, id: String, archived: bool, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        self.overlay.close_chat_menu();
        self.overlay.close_row_menu();
        let open = self.current.as_deref() == Some(id.as_str());
        if open {
            self.chat_archived = archived;
        }
        if archived {
            self.sessions.retain(|session| session.id != id);
        }
        let request = account_gateway.request(Verb::Patch, chat_path(&id), Some(json!({ "archived": archived })));
        fetch(cx, request, move |this, answer, cx| {
            if let Err(error) = answer {
                if open {
                    this.chat_archived = !archived;
                }
                this.notice = Some(format!("Couldn't change it: {error}").into());
            }
            this.refresh_chat_lists(cx);
        });
        cx.notify();
    }

    /// Delete a chat and its messages; a new chat takes its place when it was open.
    pub(super) fn delete_chat(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        self.cancel_chat(&id);
        self.overlay.close_chat_menu();
        self.overlay.close_row_menu();
        self.sessions.retain(|session| session.id != id);
        let request = account_gateway.request(Verb::Delete, chat_path(&id), None);
        fetch(cx, request, |this, answer, cx| {
            if let Err(error) = answer {
                this.notice = Some(format!("Couldn't delete it: {error}").into());
            }
            this.refresh_chat_lists(cx);
        });
        if self.current.as_deref() == Some(id.as_str()) {
            self.new_chat(window, cx);
        }
        cx.notify();
    }

    /// Read the Sessions list again, and the archived chats when their group is open.
    pub(super) fn refresh_chat_lists(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        account_gateway.list_sessions();
        if self.archived_open {
            self.load_archived(cx);
        }
    }
}

/// Share's key, as `Workspace::on_key_down` reads it (Cmd Shift S on a Mac, Ctrl Shift S elsewhere).
pub(super) const SHARE_KEYS: &str = "secondary-shift-s";

/// A 28 button on a menu page: "Keep" on `control`, "Delete" on `danger`.
fn menu_button(id: &'static str, label: &'static str, fill: Hsla, ink: Hsla) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .px(px(12.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .bg(fill)
        .text_color(ink)
        .cursor_pointer()
        .child(label)
}

/// A shortcut as plain faint text, words split by a space ("Ctrl F"), never one box per key.
pub(super) fn key_text(keystroke: &str, palette: Palette) -> impl IntoElement + use<> {
    div()
        .flex_none()
        .text_size(px(12.))
        .text_color(palette.faint)
        .child(crate::shortcut_label::keys(keystroke).join(" "))
}

pub(super) fn chat_path(id: &str) -> String {
    format!("/v1/chat/sessions/{}", urlencoding(id))
}

fn urlencoding(id: &str) -> String {
    url::form_urlencoded::byte_serialize(id.as_bytes()).collect()
}

/// Where the save dialog opens: the Downloads folder, else home.
fn downloads_folder() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let downloads = home.join("Downloads");
    if downloads.is_dir() { downloads } else { home }
}

/// A file name from a chat's title, as Nexus UI names its downloads (`markdownFilename`): lower-case words joined by
/// hyphens, accents dropped, at most 60 characters, "nexus-chat" when nothing is left.
pub(super) fn markdown_filename(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars().flat_map(char::to_lowercase).map(fold_accent) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.chars().take(60).collect();
    let slug = slug.trim_end_matches('-');
    format!("{}.md", if slug.is_empty() { "nexus-chat" } else { slug })
}

/// The plain letter under a common Latin accent; other characters as they are. Nexus UI strips accents after Unicode
/// decomposition; this covers the Latin letters without adding a Unicode table to the app.
fn fold_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::markdown_filename;

    #[test]
    fn names_downloads_as_nexus_ui() {
        assert_eq!(markdown_filename("How do we deploy?"), "how-do-we-deploy.md");
        assert_eq!(markdown_filename("Café — résumé"), "cafe-resume.md");
        assert_eq!(markdown_filename("???"), "nexus-chat.md");
        assert_eq!(markdown_filename(&"x".repeat(80)).len(), 63);
    }
}
