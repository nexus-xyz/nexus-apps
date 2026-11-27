//! A Nexus sign-in that blocks an action (board P4 of "Desktop · Setup that blocks an action"): a setup line with Sign
//! in, and the blocked action goes on by itself after the sign-in. A Code session that could not start starts again;
//! the Assistant asks the message that was not sent again (`pages/agent/sign_in.rs`). A prompt link opened while
//! signed out waits in `pending_links` and opens with the next account (`open_prompt_link`).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace};
use crate::setup::{self, Fix, FixKey, SetupLine, SignIn};

/// The Code session start that waits for the sign-in.
pub(crate) const START_KEY: &str = "start:nexus-sign-in";
/// The Assistant's question that waits for the sign-in.
pub(crate) const ASSISTANT_KEY: &str = "assistant:nexus-sign-in";

/// What waits on a Nexus sign-in: each resumes once the account of the new sign-in is read.
const KEYS: [&str; 2] = [START_KEY, ASSISTANT_KEY];

/// The line's first words, and its second sentence.
pub(crate) const ENDED: &str = "Your Nexus sign-in ended.";
pub(crate) const WAITS: &str = "Your message waits here.";

/// A start refused because this app has no Nexus sign-in (`code_run.rs`, `nexus-remote`'s local host).
pub(crate) fn start_needs_sign_in(error: &str) -> bool {
    error.starts_with("Sign in to Nexus to start ") || error == "Sign in to Nexus to run Code sessions."
}

/// The setup line for a Nexus sign-in, with Sign in as the view's one white button.
pub(crate) fn line(key: &'static str, why: Option<&'static str>) -> SetupLine {
    let line = SetupLine::new(key, Lucide::LogIn, ENDED)
        .fix("Sign in", Fix::SignIn(SignIn::Nexus))
        .main();
    match why {
        Some(why) => line.why(why),
        None => line,
    }
}

impl Workspace {
    /// The failed start's line, when a Nexus sign-in is what it lacks: Sign in, then Edit to change the message.
    pub(super) fn start_sign_in_line(&self, error: &str, cx: &mut Context<Self>) -> Option<SetupLine> {
        if !start_needs_sign_in(error) {
            return None;
        }
        let this = cx.entity().downgrade();
        let edit = Fix::act(move |window, cx| {
            if let Err(error) = this.update(cx, |workspace, cx| workspace.edit_pending(window, cx)) {
                eprintln!("nexus-desktop-gpui: edit the start: {error}");
            }
        });
        Some(line(START_KEY, Some(WAITS)).fix("Edit", edit))
    }

    /// The start of `draft` failed for want of a Nexus sign-in: it starts again after the sign-in, if its view is
    /// still open and still failed.
    pub(super) fn start_waits_for_sign_in(&mut self, draft: &str, error: &str, cx: &mut Context<Self>) {
        let key = FixKey::from(START_KEY);
        if !start_needs_sign_in(error) {
            return;
        }
        // One start waits at a time: the latest one.
        setup::forget(&key, cx);
        let this = cx.entity().downgrade();
        let draft = draft.to_string();
        setup::on_done(key, cx, move |window, cx| {
            let resumed = this.update(cx, |workspace, cx| {
                let failed = match &workspace.view {
                    View::Code(view) if view.id == draft => view.pending.as_ref().and_then(|p| p.failed.clone()),
                    _ => None,
                };
                if failed.is_some_and(|error| start_needs_sign_in(&error)) {
                    workspace.retry_pending(window, cx);
                }
            });
            if let Err(error) = resumed {
                eprintln!("nexus-desktop-gpui: start after sign-in: {error}");
            }
        });
    }

    /// An account was read: after a new sign-in (`Event::SignedIn`), what waited on it goes on.
    pub(super) fn account_after_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // After this update: what resumes updates the workspace itself.
        if std::mem::take(&mut self.setup.signed_in) {
            window.defer(cx, resume_after_sign_in);
        }
    }
}

/// Run what waits on a Nexus sign-in, once.
fn resume_after_sign_in(window: &mut Window, cx: &mut App) {
    for key in KEYS {
        setup::done(&FixKey::from(key), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn only_a_missing_nexus_sign_in_reads_as_one() {
        assert!(start_needs_sign_in("Sign in to Nexus to start Claude Code."));
        assert!(start_needs_sign_in("Sign in to Nexus to run Code sessions."));
        assert!(!start_needs_sign_in(
            "Start this local session while signed out of Nexus."
        ));
        assert!(!start_needs_sign_in("Sign in to Nexus to use your cloud machine."));
        assert!(!start_needs_sign_in("Claude Code is not installed."));
    }

    #[gpui_kit::test]
    fn a_sign_in_resumes_the_start_and_the_question_once(cx: &mut TestAppContext) {
        use std::cell::Cell;
        use std::rc::Rc;
        let window = cx.add_empty_window();
        let resumed = Rc::new(Cell::new(0));
        window.update(|_, cx| {
            for key in KEYS {
                let count = resumed.clone();
                setup::on_done(key, cx, move |_, _| count.set(count.get() + 1));
            }
        });
        window.update(resume_after_sign_in);
        assert_eq!(resumed.get(), 2);
        window.update(resume_after_sign_in);
        assert_eq!(resumed.get(), 2, "a later sign-in resumes nothing");
    }
}
