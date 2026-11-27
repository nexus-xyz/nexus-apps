//! Entry to and exit from the account-free workspace.

use std::collections::HashMap;

use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::session_scope::SessionScope;

use super::first_connection;
use super::overlay::OverlaySlot;
use super::sign_in_parts::{Button, button};
use super::{Auth, Workspace};
use crate::code_run::ServiceHandoff;
use crate::palette::Palette;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry {
    Chosen,
    SignedOut,
    Launch,
}

pub(super) struct PendingProviderStart {
    pub(super) project: String,
    pub(super) submission: super::drafts::Submission,
}

#[derive(Default)]
pub(super) struct LocalMode {
    #[cfg(test)]
    pub(super) isolated: bool,
    pub(super) opt_in: nexus_workbench::provider::LocalProvidersOptIn,
    pub(super) remembered: bool,
    pub(super) connection: Option<first_connection::Page>,
    // Keep starts in flight across navigation. Saved-provider starts also keep the submission for recovery.
    pub(super) starts: HashMap<String, Option<PendingProviderStart>>,
    // The first form was replaced while its start still owns the saved preselection.
    pub(super) pending_first_provider: Option<String>,
    pub(super) failed_provider_drafts: HashMap<String, String>,
    /// Keep the open place while the browser approves sign-in.
    pub(super) signing_in: bool,
    pub(super) code_blocker: Option<(&'static str, bool)>,
}

impl Workspace {
    pub(super) fn byo_entry(&self) -> bool {
        !self.panel && self.local_mode.opt_in.enabled()
    }

    pub(super) fn is_local(&self) -> bool {
        matches!(self.auth, Auth::Local)
    }

    pub(super) fn workspace_open(&self) -> bool {
        matches!(self.auth, Auth::Local | Auth::SignedIn)
    }

    pub(super) fn editing_local_code(&self) -> bool {
        self.is_local()
            || matches!(&self.view,
            super::View::Code(view) if self.code.as_ref().is_some_and(|store| store.is_local_session(&view.id)))
    }

    pub(super) fn refresh_local_choice(&mut self) {
        #[cfg(test)]
        if self.local_mode.isolated {
            return;
        }
        self.local_mode.remembered |= self.byo_entry() && crate::paths::local_workspace_remembered();
    }

    pub(super) fn enter_local(&mut self, reason: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if !self.byo_entry() || self.gateway.marker().is_some() || self.is_local() {
            return;
        }
        if reason == Entry::Chosen {
            #[cfg(test)]
            let isolated = self.local_mode.isolated;
            #[cfg(not(test))]
            let isolated = false;
            if !isolated {
                crate::paths::remember_local_workspace();
            }
            self.local_mode.remembered = true;
        } else if !self.local_mode.remembered {
            return;
        }
        self.auth = Auth::Local;
        self.sync_local_servers();
        self.sign_in_collapse = None;
        self.onboarding.leave();
        if std::mem::take(&mut self.local_mode.signing_in) {
            self.load_local_provider_choices(cx);
            self.update_code_blocker();
            self.focus_composer(window, cx);
        } else {
            self.code_form = Default::default();
            self.code_form.local_subscription = true;
            self.cloud_ui.on = false;
            self.cloud_ui.reset_repos();
            self.groups = Default::default();
            self.readiness = Default::default();
            self.clear_navigation();
            self.history = vec![super::Place::NewCode];
            self.open_code_scope(SessionScope::Local, window, cx);
            self.open_first_connection(window, cx);
            self.open_new_code_form(window, cx);
            self.load_local_provider_choices(cx);
            self.tray_ready(window, cx);
        }
        for url in std::mem::take(&mut self.pending_links) {
            self.open_prompt_link(&url, window, cx);
        }
        cx.notify();
    }

    /// Leave Local through an explicit sign-in or when another window signs in.
    pub(super) fn leave_local(&mut self, next: Auth, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_local() {
            if !self.local_mode.signing_in {
                self.local_mode.connection = None;
                self.release_first_provider(cx);
            }
            self.overlay.close_person_menu();
            self.cancel_navigation_restore();
        }
        self.auth = next;
        if matches!(self.auth, Auth::SignedIn) {
            self.local_mode.signing_in = false;
            self.local_mode.connection = None;
            self.release_first_provider(cx);
            self.code_form.begin_first_provider(None);
            self.code_form.local_subscription = false;
            self.code_form.clear_local_provider_choices();
            self.readiness = Default::default();
        }
        if self.workspace_open() {
            self.focus_composer(window, cx);
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    pub(super) fn sign_in_from_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_local() {
            self.local_mode.signing_in = true;
            self.leave_local(Auth::Checking, window, cx);
            self.gateway.begin_device_sign_in();
        }
    }

    pub(super) fn finish_sign_in_attempt(
        &mut self,
        error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.local_mode.signing_in {
            self.enter_local(Entry::SignedOut, window, cx);
            if self.is_local()
                && let Some(error) = error
            {
                self.show_notice(error, None, std::time::Duration::from_secs(8), cx);
            }
        } else {
            self.auth = Auth::SignedOut {
                error: error.map(Into::into),
            };
            cx.notify();
        }
    }

    pub(super) fn local_code_blocker(&self) -> Option<(&'static str, bool)> {
        use nexus_remote::machine::Status;

        if self.gateway.marker().is_some() {
            return Some(("Signing in to Nexus…", false));
        }
        let Some(runner) = self.runner.as_ref() else {
            return Some(("Code is waiting for access to its local sessions store.", true));
        };
        if let Some(blocker) = self.code_handoff_blocker() {
            return Some(blocker);
        }
        match runner.service_status() {
            None | Some(Status::Connected { .. }) => None,
            Some(Status::Unreachable { .. } | Status::NeedsUpdate { .. }) => {
                Some(("Could not connect to the background service.", true))
            }
            Some(_) => Some(("Connecting to the background service…", false)),
        }
    }

    pub(super) fn code_handoff_blocker(&self) -> Option<(&'static str, bool)> {
        match self.runner.as_ref()?.service_handoff() {
            ServiceHandoff::Stopping => Some(("Waiting for the background service to stop…", false)),
            ServiceHandoff::Failed(_) => Some(("The background service could not stop.", true)),
            ServiceHandoff::Ready | ServiceHandoff::Released => None,
        }
    }

    pub(super) fn update_code_blocker(&mut self) {
        self.local_mode.code_blocker = if self.is_local() {
            self.local_code_blocker()
        } else {
            self.code_handoff_blocker()
        };
    }

    pub(super) fn render_code_handoff(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (message, retry) = self.local_mode.code_blocker?;
        Some(self.render_service_blocker(message, retry, palette, cx))
    }

    fn render_service_blocker(
        &self,
        message: &'static str,
        retry: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        v_flex()
            .size_full()
            .justify_center()
            .items_center()
            .gap(px(12.))
            .child(div().text_color(palette.foreground).child(message))
            .when(retry, |body| {
                body.child(
                    button("code-service-retry", Button::Ghost, "Retry", None, palette)
                        .on_click(cx.listener(|this, _, window, cx| this.retry_code_service(window, cx))),
                )
            })
            .into_any_element()
    }

    fn retry_code_service(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(runner) = &self.runner {
            runner.stop_signed_in_service();
        } else {
            self.try_run_code(window, cx);
        }
        self.update_code_blocker();
        cx.notify();
    }
}
