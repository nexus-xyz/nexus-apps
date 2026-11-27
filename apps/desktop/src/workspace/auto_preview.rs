//! Check fresh local-server activity, then offer its ready URL to Follow / Manual navigation.
//! Readiness does not select a tool or change execution permissions.

use gpui_kit::*;

use super::{View, Workspace};
use crate::side::browser::auto_preview::{self, AutoPreview, Offer, Signal};

/// One session view's automatic Browser: the rules' state, the running check, and the pane as last seen.
pub(super) struct AutoBrowser {
    tracker: AutoPreview,
    /// The check of the offered server; dropped (and stopped) with a newer offer or the view.
    check: Option<(Task<()>, tokio::task::AbortHandle, Offer)>,
    /// A server that answered while its offer was current, to show at the next draw.
    ready: Option<(Offer, u64)>,
}

impl AutoBrowser {
    /// `fresh`: a session this build just started, whose output before the first read counts.
    pub fn new(fresh: bool) -> AutoBrowser {
        AutoBrowser {
            tracker: AutoPreview::new(fresh),
            check: None,
            ready: None,
        }
    }

    fn cancel(&mut self) {
        if let Some((_, job, _)) = self.check.take() {
            job.abort();
        }
        self.ready = None;
    }

    /// The transcript read again: an offer to check, if it makes one. A newer offer or a new turn stops the check.
    pub fn observe(&mut self, signals: &[Signal], last: i64) -> Option<Offer> {
        let offer = self.tracker.observe(signals, last);
        if self
            .check
            .as_ref()
            .is_some_and(|(_, _, checked)| !self.tracker.current(checked))
        {
            self.cancel();
        }
        offer
    }
}

impl Drop for AutoBrowser {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Workspace {
    /// Check readiness off the UI thread. The navigation setting filters automatic selection, not background evidence.
    pub(super) fn check_preview(&mut self, thread: String, offer: Offer, cx: &mut Context<Self>) {
        let revision = self.navigation(&thread).revision;
        let View::Code(view) = &mut self.view else {
            return;
        };
        if view.id != thread {
            return;
        }
        // A server's session: its dev server is the server's, checked through an SSH forward. A cloud machine's
        // previews come from its Cloud panel only.
        let machines = crate::machines::get();
        let server = match machines.machine_of(&thread) {
            None => None,
            Some(machine) if machines.is_server(&machine) => {
                Some(crate::side::browser::server_preview::ServerPreview::new(&machine).opener)
            }
            Some(_) => {
                view.auto_browser.tracker.release(&offer);
                return;
            }
        };
        let url = offer.url.clone();
        let checked = offer.clone();
        let job = crate::code_run::runtime().spawn(async move {
            let (within, every) = (auto_preview::READY_WITHIN, auto_preview::ASK_EVERY);
            match server {
                Some(opener) => {
                    let probe = |url| {
                        let opener = opener.clone();
                        async move { crate::side::browser::server_preview::probe(opener, url).await.0 }
                    };
                    auto_preview::wait_ready(probe, &url, within, every).await
                }
                None => auto_preview::wait_ready(nexus_browser::preview::probe, &url, within, every).await,
            }
        });
        let abort = job.abort_handle();
        let task = cx.spawn(async move |this, cx| {
            let ready = job.await.unwrap_or(false);
            this.update(cx, |this, cx| {
                let View::Code(view) = &mut this.view else {
                    return;
                };
                if view.id != thread {
                    return;
                }
                // The finished check stays until the next one: dropping it here would cancel this very task.
                let auto = &mut view.auto_browser;
                if ready {
                    auto.ready = Some((offer, revision));
                    cx.notify();
                } else {
                    auto.tracker.release(&offer);
                }
            })
            .ok();
        });
        view.auto_browser.cancel();
        view.auto_browser.check = Some((task, abort, checked));
    }

    /// Offer a ready server without changing Manual or reopening a hidden pane.
    pub(super) fn sync_auto_preview(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else {
            return;
        };
        let draft = view.is_draft();
        let auto = &mut view.auto_browser;
        if draft {
            auto.tracker.dismiss();
            auto.cancel();
            return;
        }
        let Some((offer, revision)) = auto.ready.take() else {
            return;
        };
        if !auto.tracker.accept(&offer) {
            return;
        }
        self.offer_workbench_at(
            super::side_pane::navigation::Target::Preview(offer.url),
            true,
            revision,
            cx,
        );
    }
}
