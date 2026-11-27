//! Recovery actions for Code store and transcript reads.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace};
use crate::palette::Palette;

/// A failed local store must not hide independent server or cloud snapshots.
pub(super) fn with_remote<T>(local: Result<Vec<T>, String>, remote: Vec<T>) -> (Vec<T>, Option<String>) {
    let (mut projects, error) = match local {
        Ok(projects) => (projects, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    projects.extend(remote);
    (projects, error)
}

impl Workspace {
    pub(super) fn retry_code_store(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(scope) = self.code_scope.clone() {
            self.open_code_scope(scope.into(), window, cx);
        } else if let Some(org) = self.account.as_ref().map(|account| account.org.clone()) {
            self.open_code_store(&org, window, cx);
        }
        cx.notify();
    }

    pub(super) fn retry_code_log(&mut self, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        view.error = None;
        view.loaded = false;
        self.read_code_transcript(cx);
        cx.notify();
    }

    pub(super) fn render_code_store_error(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        v_flex().size_full().justify_center().items_center().p(px(24.)).child(
            v_flex()
                .w_full()
                .max_w(px(560.))
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(18.))
                        .text_color(palette.foreground)
                        .child("Code could not start"),
                )
                .child(
                    div().text_size(px(13.)).text_color(palette.muted).child(
                        self.code_error
                            .clone()
                            .unwrap_or_else(|| "The app's core did not answer.".into()),
                    ),
                )
                .child(
                    Button::new("code-store-retry")
                        .ghost()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, window, cx| this.retry_code_store(window, cx))),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_local_store_keeps_cloud_projects_and_the_retry_error() {
        let (projects, error) = super::with_remote(Err("database unavailable".into()), vec!["cloud project"]);
        assert_eq!(projects, ["cloud project"]);
        assert_eq!(error.as_deref(), Some("database unavailable"));
        let (projects, error) = super::with_remote(Ok(vec!["local project"]), vec!["cloud project"]);
        assert_eq!(projects, ["local project", "cloud project"]);
        assert_eq!(error, None);
    }
}
