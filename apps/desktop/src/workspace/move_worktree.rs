//! Confirmed moves use the shared core's validated operation on the session's own host.

use super::Workspace;
use crate::workspace::overlay::OverlaySlot;
use gpui_kit::*;

impl Workspace {
    pub(super) fn move_session_worktree(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(runs) = self.runs(&id) else { return };
        self.overlay.close_session_menu();
        let scope = self.code_scope.clone();
        let answer = runs.call("wb_move_to_worktree", serde_json::json!({ "threadId": id }));
        cx.spawn(async move |this, cx| {
            let answer = answer
                .await
                .unwrap_or_else(|_| Err("The move's answer was lost. Read the session before retrying.".into()));
            let _ = this.update(cx, |this, cx| {
                if this.code_scope != scope {
                    return;
                }
                match answer {
                    Ok(_) => {
                        this.refresh_code(cx);
                        this.composer_note =
                            Some("Moved to a new worktree. Your next message resumes the agent there.".into());
                    }
                    Err(error) => this.composer_note = Some(format!("Couldn't move the session: {error}").into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
