//! What a window holds for one organization's sign-in, and the one step that lets go of it.
//!
//! A window leaves its organization four ways: the switcher (`switch_org`), a new sign-in over its account
//! (`Event::SignedIn` after another window's switch), an account read that names another organization, and sign-out or a
//! Gateway switch (`clear_session`). Each runs `leave_org`, so a field added here is forgotten on every way out. The next
//! organization's lists come back with `SignedIn` and `Event::Account` (`org_entered`).
//!
//! Late answers of the old sign-in are dropped where they land: Gateway's reads go through `Event::ReadSnapshot`, and a
//! Code access read names its credential (`CodeEvent::Access`).

use super::{View, Workspace, inbox_view};
use gpui_kit::Context;

impl Workspace {
    /// Forget everything this window read for the organization it shows. Its chats' answers stop; what streamed stays on
    /// screen. Navigation and the open view stay: each way out decides where the window goes.
    pub(super) fn leave_org(&mut self) {
        self.cancel_live_chats();
        self.dismiss_delete_notice();
        self.chat_share = None;
        self.forget_org_pays();
        // Auto's preview and its pending answer were the old sign-in's.
        self.auto_route = Default::default();
        self.free_start_max = None;
        self.account = None;
        // No organization's picks until the next one is entered: a pick made meanwhile is not saved under this one.
        crate::pages::subscriptions::leave_org();
        self.settings.clear_account();
        self.onboarding.forget_plans();
        self.sessions.clear();
        self.kept_chats.clear();
        // The runner reads the next organization's when its scope changes (`CodeRunner::set_scope`).
        crate::live_fields::forget();
        self.sessions_loaded = false;
        self.sessions_error = None;
        self.projects.clear();
        self.archived_code.clear();
        self.archived_chats = None;
        self.inbox = 0;
        self.inbox_state = inbox_view::InboxState::new();
        self.team_presence = Default::default();
        self.chat_skills = Default::default();
        self.library.lists = None;
        // An artifact or a chat still opening answers for the old organization, and is dropped: nothing waits on it.
        self.library_opening = None;
        self.page_opening = None;
        self.page_opening_auto = false;
        self.models_page.providers = None;
        // A search still on its way answers for the old organization.
        self.palette_seq += 1;
        self.palette_hits.clear();
        self.palette_searching = false;
    }

    /// The window's account is `org`'s now (its first account, or a switch): new sessions start on the agent accounts
    /// `org` chose, and an agent it chose none for is asked for on New session. A personal organization never takes
    /// over a work login.
    pub(super) fn enter_org_accounts(&mut self, org: &str, cx: &mut Context<Self>) {
        // Tests never write the settings file, and `off_thread` is an OS thread the test scheduler cannot follow.
        if cfg!(test) {
            return;
        }
        let entering = crate::pages::subscriptions::enter_org(org.to_string());
        cx.spawn(async move |this, cx| {
            let saved = entering.await;
            let _ = this.update(cx, |this, cx| {
                match saved {
                    Ok(settings) => {
                        this.settings_saved(&settings);
                        this.plan_usage = None;
                        this.probe_plans(cx);
                        this.recheck_readiness(cx);
                    }
                    Err(error) => eprintln!("nexus-desktop-gpui: could not apply the organization's accounts: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The window shows another organization than before, after a switch or a failed one: read again what `leave_org`
    /// forgot and `SignedIn` does not read (the lists of the view on screen, and who may pay for a new session).
    pub(super) fn org_entered(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if let Some(runner) = &self.runner {
            runner.load_access();
        }
        self.refresh_code(cx);
        if self.archived_open {
            self.load_archived(cx);
        }
        match self.view {
            View::Inbox => self.reload_inbox(),
            View::Models => account_gateway.list_providers(),
            // Library reads again on every account (its audience labels name the organization).
            _ => {}
        }
    }
}
