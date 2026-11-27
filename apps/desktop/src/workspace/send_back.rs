//! Return a fork's result through a validated local or remote command.

use super::Workspace;
use crate::i18n::tr;
use crate::workspace::overlay::OverlaySlot;
use gpui_kit::*;

impl Workspace {
    pub(super) fn send_back_to_source(
        &mut self,
        fork: String,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay.close_session_menu();
        let Some(runs) = self.runs(&fork) else {
            self.refuse_read_only();
            cx.notify();
            return;
        };
        let scope = self.code_scope.clone();
        let answer = runs.call("wb_send_back", serde_json::json!({ "threadId": fork }));
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer
                .await
                .unwrap_or_else(|_| Err(tr("The command's answer was lost.").to_string().into()));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.code_scope != scope {
                    return;
                }
                match answer {
                    Ok(_) => {
                        this.refresh_code(cx);
                        this.open_code(&source, window, cx);
                    }
                    Err(error) => {
                        this.composer_note = Some(
                            tr("Couldn't send back to source: {error}")
                                .replace("{error}", &error.to_string())
                                .into(),
                        )
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
