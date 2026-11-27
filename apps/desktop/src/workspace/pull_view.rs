//! Opens a Code session's pull request page (`pages/pull_request.rs`): from the topbar's pull request link and from a
//! pull request notification. Back and Esc return to the session.

use std::rc::Rc;

use gpui_kit::*;

use super::{Place, View, Workspace};
use crate::pages::pull_request::{Locate, OpenOther, PullRequestPage};

impl Workspace {
    pub(super) fn open_pull(&mut self, thread_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_pull_at(thread_id, None, window, cx);
    }

    /// The page for `url`, one of the session's pull requests (the top bar's card picks it); None: the one it shows.
    pub(super) fn open_pull_at(
        &mut self,
        thread_id: &str,
        url: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if url.is_none() && matches!(&self.view, View::Pull(page) if page.read(cx).thread_id() == thread_id) {
            return;
        }
        self.visit(Place::Pull(thread_id.to_string()));
        self.reset_chat();
        let this = cx.entity().downgrade();
        let locate: Locate = Rc::new({
            let this = this.clone();
            let id = thread_id.to_string();
            move |cx| {
                let Some(workspace) = this.upgrade() else {
                    return Err(nexus_pull_requests::pull_page::PullError::new(
                        nexus_pull_requests::pull_page::ErrorKind::Session,
                        "The window closed.",
                    ));
                };
                crate::pull_page::locate(workspace.read(cx).code.as_ref(), &id, url.as_deref())
            }
        });
        let on_back: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new({
            let id = thread_id.to_string();
            move |window, cx| {
                this.update(cx, |workspace, cx| workspace.open_code(&id, window, cx))
                    .ok();
            }
        });
        let open_other: OpenOther = Rc::new({
            let this = cx.entity().downgrade();
            let id = thread_id.to_string();
            move |url, window, cx| {
                this.update(cx, |workspace, cx| workspace.open_pull_at(&id, Some(url), window, cx))
                    .ok();
            }
        });
        let id = thread_id.to_string();
        let fixer = self.fixer(cx);
        let page = cx.new(|cx| PullRequestPage::new(id, locate, on_back, open_other, fixer, cx));
        self.title = self
            .session_by_id(thread_id)
            .map(|session| session.title.clone().into())
            .unwrap_or_else(|| "Pull request".into());
        self.view = View::Pull(page);
        cx.notify();
    }
}
