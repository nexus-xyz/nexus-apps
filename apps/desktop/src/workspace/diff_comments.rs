//! Diff comments waiting in a Code session's composer, as the Nexus app's comment chips.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::diff_comment::DiffComment;

use super::{View, Workspace};
use crate::palette::Palette;

pub(super) const NEW_CODE_DRAFT: &str = "::new-code";

impl Workspace {
    fn diff_comment_scope(&self) -> Option<String> {
        if self.editing_local_code() {
            Some(nexus_workbench::session_scope::SessionScope::local_text().into())
        } else {
            serde_json::to_string(&(self.code_scope.as_ref()?, &self.account.as_ref()?.email)).ok()
        }
    }

    pub(super) fn load_diff_comments(&mut self) {
        self.diff_comments = self
            .diff_comment_scope()
            .map(|scope| crate::paths::diff_comment_drafts(&scope))
            .unwrap_or_default();
    }

    pub(super) fn save_diff_comments(&self) {
        if let Some(scope) = self.diff_comment_scope() {
            crate::paths::save_diff_comment_drafts(&scope, &self.diff_comments);
        }
    }

    fn diff_comment_key(&self) -> Option<&str> {
        match &self.view {
            View::Code(view) => Some(&view.id),
            View::NewCode => Some(NEW_CODE_DRAFT),
            View::Chat => Some("::chat"),
            _ => None,
        }
    }

    pub(super) fn pending_diff_comments(&self) -> &[DiffComment] {
        self.diff_comment_key()
            .and_then(|key| self.diff_comments.get(key))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub(super) fn replace_diff_comments(&mut self, comments: Vec<DiffComment>) {
        let Some(key) = self.diff_comment_key().map(str::to_string) else {
            return;
        };
        if comments.is_empty() {
            self.diff_comments.remove(&key);
        } else {
            self.diff_comments.insert(key, comments);
        }
        self.save_diff_comments();
    }

    pub(super) fn render_diff_comments(&self, palette: Palette, cx: &Context<Self>) -> Option<AnyElement> {
        let thread = self.diff_comment_key()?.to_string();
        let comments = self.pending_diff_comments();
        if comments.is_empty() {
            return None;
        }
        let chips = comments.iter().map(|comment| {
            let id = comment.id.clone();
            let thread = thread.clone();
            let hint = comment.text.clone();
            h_flex()
                .gap(px(6.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(palette.border)
                .text_size(px(12.))
                .text_color(palette.muted)
                .child(Icon::new(Lucide::MessageSquare).size(px(12.)))
                .child(format!("{} · {}", comment.path, comment.range))
                .child(
                    div()
                        .id(SharedString::from(format!("remove-diff-comment-{}", comment.id)))
                        .cursor_pointer()
                        .child(Icon::new(Lucide::X).size(px(12.)))
                        .tooltip(|window, cx| Tooltip::new("Remove comment").build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(comments) = this.diff_comments.get_mut(&thread) {
                                comments.retain(|comment| comment.id != id);
                                if comments.is_empty() {
                                    this.diff_comments.remove(&thread);
                                }
                            }
                            this.save_diff_comments();
                            cx.notify();
                        })),
                )
                .id(SharedString::from(format!("diff-comment-{}", comment.id)))
                .tooltip(move |window, cx| Tooltip::new(hint.clone()).build(window, cx))
        });
        Some(
            h_flex()
                .flex_wrap()
                .gap(px(6.))
                .px(px(16.))
                .pt(px(10.))
                .children(chips)
                .into_any_element(),
        )
    }
}
