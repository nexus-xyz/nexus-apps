//! The prompt stash: Mod+S parks the
//! composer's draft so the person can write something else first. With the composer empty, Mod+S brings the one
//! stashed draft back, or opens the menu when there are several. Once something is stashed, a button in the
//! composer's bar shows the count and opens the menu: each draft, newest first, to bring back (click or Enter) or
//! delete (its ✕, or Delete). A draft already in the composer takes the restored one's place in the stash.
//!
//! The rule is `nexus_workbench::prompt_stash`'s. One stash for every window and session, at most 20. It is kept in
//! `prompt-stash.json`, beside `settings.json`. Only the text lasts across a restart; attachments stay in memory.

use crate::workspace::overlay::{Overlay, OverlaySlot};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::prompt_stash::{self, Key, Stash, StashDraft};

use super::code_dock::{menu_box, menu_label};
use super::{View, Workspace, menu_item_hover};
use crate::attachments::Attachment;
use crate::palette::Palette;

actions!(workspace, [StashPrompt]);

/// Mod+S, as the Nexus app's `stash-prompt`.
pub const STASH_KEY: &str = if cfg!(target_os = "macos") { "cmd-s" } else { "ctrl-s" };
/// The width of a menu row's snippet, in characters (`snippet`'s default).
const SNIPPET: usize = 80;

/// A stashed draft: the composer's text and what was attached to it.
#[derive(Clone, Default)]
pub(super) struct StashedDraft {
    pub text: String,
    pub images: Vec<Attachment>,
    pub files: Vec<Attachment>,
    pub comments: Vec<nexus_workbench::diff_comment::DiffComment>,
}

impl StashDraft for StashedDraft {
    fn text(&self) -> &str {
        &self.text
    }
    fn from_text(text: String) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }
    fn images(&self) -> usize {
        self.images.len()
    }
    fn files(&self) -> usize {
        self.files.len()
    }
    fn comments(&self) -> usize {
        self.comments.len()
    }
}

/// The stash every window shares, and the file it is kept in.
pub(super) struct PromptStash {
    pub stash: Stash<StashedDraft>,
    file: Option<PathBuf>,
}

impl Global for PromptStash {}

impl PromptStash {
    fn save(&self) {
        if let Some(file) = &self.file
            && let Err(error) = save_to(file, &self.stash)
        {
            // The stash still lasts for this run.
            log::warn!("could not keep the prompt stash: {error}");
        }
    }
}

/// The stash kept in `file`; empty when there is none or it cannot be read.
fn load_from(file: &Path) -> Stash<StashedDraft> {
    std::fs::read_to_string(file)
        .map(|text| Stash::from_json(&text))
        .unwrap_or_default()
}

fn save_to(file: &Path, stash: &Stash<StashedDraft>) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, stash.to_json())
}

/// Reads the kept stash once, for every window.
pub(super) fn init(cx: &mut App) {
    if cx.has_global::<PromptStash>() {
        return;
    }
    let file = crate::paths::prompt_stash_file();
    let stash = file.as_deref().map(load_from).unwrap_or_default();
    cx.set_global(PromptStash { stash, file });
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// Changes the stash and keeps it.
fn update(cx: &mut App, change: impl FnOnce(&mut Stash<StashedDraft>)) {
    init(cx);
    cx.update_global::<PromptStash, _>(|global, _| {
        change(&mut global.stash);
        global.save();
    });
}

fn entries(cx: &App) -> &[prompt_stash::Entry<StashedDraft>] {
    cx.try_global::<PromptStash>()
        .map_or(&[], |global| global.stash.entries())
}

/// The menu's state in a window; whether it is open is `Overlay::StashMenu`.
#[derive(Debug, Default)]
pub(super) struct StashMenu {
    /// The row Up and Down move, and Enter and Delete act on.
    pub active: usize,
}

impl Workspace {
    /// The composer's draft as the stash takes it.
    fn current_draft(&self, cx: &App) -> StashedDraft {
        StashedDraft {
            text: self.composer.read(cx).value().to_string(),
            images: self.draft.images.clone(),
            files: self.draft.files.clone(),
            comments: self.pending_diff_comments().to_vec(),
        }
    }

    /// Puts `draft` in the composer, in place of what was there.
    fn show_draft(&mut self, draft: StashedDraft, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |state, cx| state.set_value(draft.text.clone(), window, cx));
        self.composer_empty = draft.text.trim().is_empty();
        self.draft.images = draft.images;
        self.draft.files = draft.files;
        self.replace_diff_comments(draft.comments);
        self.composer_note = None;
    }

    /// The stash waits while attachments are read or sent (the Nexus app's `disabled || preparingImages`).
    fn stash_disabled(&self) -> bool {
        !self.composer_shown() || self.attaching > 0 || self.draft.upload.is_some()
    }

    /// Mod+S (`onStashKey`).
    pub(super) fn on_stash_prompt(&mut self, _: &StashPrompt, window: &mut Window, cx: &mut Context<Self>) {
        if self.stash_disabled() {
            return;
        }
        let draft = self.current_draft(cx);
        match prompt_stash::on_stash_key(&draft, entries(cx)) {
            Key::Stash => {
                update(cx, |stash| {
                    stash.stash(draft, now_ms());
                });
                self.show_draft(StashedDraft::default(), window, cx);
            }
            Key::Restore(id) => self.bring_back(&id, window, cx),
            Key::Menu => {
                self.stash_menu = StashMenu { active: 0 };
                self.open_overlay(Overlay::StashMenu);
                self.focus_composer(window, cx);
            }
            Key::None => {}
        }
        cx.notify();
    }

    /// Brings `id` back into the composer; what was there takes its place in the stash.
    fn bring_back(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.current_draft(cx);
        let mut back = None;
        update(cx, |stash| back = stash.restore(id, current, now_ms()));
        if let Some(draft) = back {
            self.show_draft(draft, window, cx);
            self.overlay.close_stash_menu();
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    fn drop_stashed(&mut self, id: &str, cx: &mut Context<Self>) {
        update(cx, |stash| stash.drop_entry(id));
        let count = entries(cx).len();
        if count == 0 {
            self.overlay.close_stash_menu();
        }
        self.stash_menu.active = self.stash_menu.active.min(count.saturating_sub(1));
        cx.notify();
    }

    /// A prompt link fills New session's composer: a different draft already there goes to the stash first, so
    /// nothing typed is lost. Returns whether it did.
    pub(super) fn stash_for_link(&mut self, prompt: &str, cx: &mut Context<Self>) -> bool {
        let here = self.current_draft(cx);
        let kept = !here.is_empty() && here.text != prompt;
        if kept {
            update(cx, |stash| {
                stash.stash(here, now_ms());
            });
            self.draft.images.clear();
            self.draft.files.clear();
            self.replace_diff_comments(Vec::new());
        }
        kept
    }

    /// Up and Down in the open menu. Returns whether the menu took the key.
    pub(super) fn stash_move(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        if !self.overlay.stash_menu() {
            return false;
        }
        let count = entries(cx).len();
        if count > 0 {
            let active = self.stash_menu.active;
            self.stash_menu.active = if down {
                (active + 1) % count
            } else {
                (active + count - 1) % count
            };
        }
        cx.notify();
        true
    }

    /// Enter in the open menu brings the active draft back.
    pub(super) fn stash_pick_active(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.overlay.stash_menu() {
            return false;
        }
        if let Some(entry) = entries(cx).get(self.stash_menu.active) {
            let id = entry.id.clone();
            self.bring_back(&id, window, cx);
        }
        true
    }

    /// Delete in the open menu deletes the active draft.
    pub(super) fn stash_delete_active(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.overlay.stash_menu() {
            return false;
        }
        if let Some(entry) = entries(cx).get(self.stash_menu.active) {
            let id = entry.id.clone();
            self.drop_stashed(&id, cx);
        }
        true
    }

    pub(super) fn stash_dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.overlay.stash_menu() {
            return false;
        }
        self.overlay.close_stash_menu();
        cx.notify();
        true
    }

    /// The stash button (`.wb-stash-button`, the attach button's look with the count beside the icon), shown once
    /// something is stashed, and its menu over it (below New session's large composer).
    pub(super) fn render_stash(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if entries(cx).is_empty() {
            return None;
        }
        let disabled = self.stash_disabled();
        let open = self.overlay.stash_menu() && !disabled;
        let phase = crate::motion::presence("stash-menu", open, cx);
        let list = entries(cx);
        let tooltip = crate::shortcut_label::with_hint("Stashed drafts", STASH_KEY);
        let button = h_flex()
            .id("stash")
            .h(px(28.))
            .flex_none()
            .px(px(6.))
            .gap(px(3.))
            .rounded_full()
            .items_center()
            .border_1()
            .border_color(palette.border)
            .text_color(palette.icon)
            .when(disabled, |button| button.opacity(0.5))
            .when(!disabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.stash_menu = StashMenu { active: 0 };
                        this.toggle_overlay(Overlay::StashMenu);
                        this.focus_composer(window, cx);
                        cx.notify();
                    }))
            })
            .when(!open, |button| {
                button.tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            })
            .child(Icon::new(Lucide::Bookmark).size(px(15.)))
            .child(div().text_size(px(12.)).child(list.len().to_string()));
        let mut frame = div().relative().child(button);
        if let Some(phase) = phase {
            let below = matches!(self.view, View::NewCode);
            let now = now_ms();
            let mut menu = menu_box("stash-menu", 320., palette)
                .max_h(px(320.))
                .text_size(px(13.))
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.overlay.close_stash_menu();
                    cx.notify();
                }))
                .child(menu_label("Stashed drafts", palette));
            for (ix, entry) in list.iter().enumerate() {
                let on = ix == self.stash_menu.active;
                let id = entry.id.clone();
                let delete_id = entry.id.clone();
                let images = entry.draft.images.len();
                let full: SharedString = entry.draft.text.clone().into();
                menu = menu.child(
                    h_flex()
                        .id(("stash-entry", ix))
                        .group("stash-entry")
                        .h(px(30.))
                        .flex_none()
                        .px(px(8.))
                        .gap(px(8.))
                        .rounded(px(8.))
                        .when(on, |row| row.bg(palette.menu_hover))
                        .cursor_pointer()
                        .hover(|style| menu_item_hover(style, palette))
                        .when(!full.trim().is_empty(), |row| {
                            row.tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
                        })
                        .on_click(cx.listener(move |this, _, window, cx| this.bring_back(&id, window, cx)))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .child(prompt_stash::snippet(&entry.draft, SNIPPET)),
                        )
                        .when(images > 0 && !entry.draft.text.trim().is_empty(), |row| {
                            row.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(palette.muted)
                                    .child(format!("+{images} img")),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.muted)
                                .child(prompt_stash::ago(entry.at, now)),
                        )
                        .child(
                            div()
                                .id(("stash-delete", ix))
                                .p(px(2.))
                                .rounded(px(4.))
                                .text_color(palette.muted)
                                // Shown on hover or on the active row, as the Nexus app's.
                                .when(!on, |delete| {
                                    delete.invisible().group_hover("stash-entry", |s| s.visible())
                                })
                                .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                                .tooltip(|window, cx| Tooltip::new("Delete this stashed draft").build(window, cx))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.drop_stashed(&delete_id, cx);
                                }))
                                .child(Icon::new(Lucide::X).size(px(12.))),
                        ),
                );
            }
            let (anchor, offset) = if below {
                (Anchor::TopLeft, point(px(0.), px(36.)))
            } else {
                (Anchor::BottomLeft, point(px(0.), px(-8.)))
            };
            // Under New session's composer it drops from the button; over a session's it rises from it.
            let menu = phase.menu("stash-menu", if below { -4. } else { 4. }, menu);
            frame = frame.child(
                div().absolute().top_0().left_0().child(
                    deferred(
                        anchored()
                            .anchor(anchor)
                            .offset(offset)
                            .snap_to_window_with_margin(px(12.))
                            .child(menu),
                    )
                    .with_priority(1),
                ),
            );
        }
        Some(frame.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::{STASH_KEY, StashedDraft, load_from, save_to};
    use nexus_workbench::prompt_stash::{self, Key, Stash, StashDraft};

    #[test]
    fn a_comment_only_draft_survives_stash_and_restore() {
        let comment = nexus_workbench::diff_comment::DiffComment {
            id: "comment".into(),
            path: "src/a.rs".into(),
            range: "line 1".into(),
            quote: "+x".into(),
            text: "Why?".into(),
        };
        let draft = StashedDraft {
            comments: vec![comment.clone()],
            ..Default::default()
        };
        assert_eq!(prompt_stash::on_stash_key(&draft, &[]), Key::Stash);
        let mut stash = Stash::default();
        stash.stash(draft, 1);
        let id = stash.entries()[0].id.clone();
        let back = stash.restore(&id, StashedDraft::default(), 2).expect("restored draft");
        assert_eq!(back.comments, vec![comment]);
    }

    #[test]
    fn the_stash_file_keeps_the_text_across_a_restart() {
        let dir = std::env::temp_dir().join(format!("nexus-gpui-stash-{}", std::process::id()));
        let file = dir.join("nested").join("prompt-stash.json");
        let mut stash = Stash::default();
        stash.stash(StashedDraft::from_text("first".into()), 1);
        stash.stash(StashedDraft::from_text("second".into()), 2);
        save_to(&file, &stash).unwrap();
        let back = load_from(&file);
        let texts: Vec<_> = back.entries().iter().map(|entry| entry.draft.text.clone()).collect();
        assert_eq!(texts, ["second", "first"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_or_broken_file_is_an_empty_stash() {
        let dir = std::env::temp_dir().join(format!("nexus-gpui-stash-broken-{}", std::process::id()));
        assert!(load_from(&dir.join("none.json")).is_empty());
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("prompt-stash.json");
        std::fs::write(&file, "not json").unwrap();
        assert!(load_from(&file).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mod_s_is_the_nexus_apps_key() {
        assert_eq!(
            crate::shortcut_label::label(STASH_KEY),
            if cfg!(target_os = "macos") { "⌘S" } else { "Ctrl+S" }
        );
    }
}
