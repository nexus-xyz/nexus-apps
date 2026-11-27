//! Page sharing, history, and destructive confirmations use the public Gateway API.

use super::*;
use crate::gateway::Verb;
use crate::pages::fetch;
use gpui_kit::component::input::InputState;
use nexus_desktop_core::sharing::Audience;
use serde_json::{Value, json};

pub(in crate::workspace) struct LibraryDialog {
    gateway: crate::workspace::gateway_access::AccountGateway,
    marker: Option<String>,
    org_name: Option<String>,
    workspace: WeakEntity<Workspace>,
    /// Turns a blocked audience on, or asks who can (`share_ui::blocked_row_end`).
    fixer: crate::setup::Fixer,
    row: Row,
    action: Action,
    pub(in crate::workspace) focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    pub(in crate::workspace) closed: bool,
    busy: bool,
    ready: bool,
    problem: Option<String>,
    detail: Value,
    completed: bool,
    share: share::ShareState,
}

impl LibraryDialog {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::workspace) fn new(
        gateway: crate::workspace::gateway_access::AccountGateway,
        row: Row,
        action: Action,
        org_name: Option<String>,
        workspace: WeakEntity<Workspace>,
        fixer: crate::setup::Fixer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let share = share::ShareState::new(window, cx);
        // Typing addresses re-checks the primary button (disabled while empty).
        cx.observe(&share.outside, |_, _, cx| cx.notify()).detach();
        let mut this = Self {
            marker: gateway.marker(),
            org_name,
            workspace,
            fixer,
            gateway,
            row,
            action,
            focus: cx.focus_handle(),
            return_focus: window.focused(cx),
            closed: false,
            busy: false,
            ready: false,
            problem: None,
            detail: Value::Null,
            completed: false,
            share,
        };
        this.load(window, cx);
        this
    }

    pub(in crate::workspace) fn matches(&self, gateway: &crate::workspace::gateway_access::AccountGateway) -> bool {
        self.marker == gateway.marker() && self.gateway.url() == gateway.url()
    }

    fn current(&self, cx: &App) -> bool {
        self.workspace.upgrade().is_some_and(|workspace| {
            workspace
                .read(cx)
                .account_gateway()
                .is_some_and(|gateway| self.matches(&gateway))
        })
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.action == Action::Share {
            return self.load_share(window, cx);
        }
        self.busy = true;
        self.problem = None;
        let gateway = self.gateway.clone();
        let action = self.action;
        let page = self.row.owned_page.clone();
        fetch(
            cx,
            async move {
                if matches!(action, Action::Revoke | Action::Delete) {
                    return Ok(json!({}));
                }
                let id = page.ok_or("This page is not owned by the current account.")?;
                gateway
                    .request(Verb::Get, format!("/v1/pages/{}", nexus_client::url::enc(&id)), None)
                    .await
            },
            |this, answer, cx| {
                this.busy = false;
                if !this.current(cx) {
                    this.problem = Some("The signed-in account changed. Close this dialog and try again.".into());
                    return;
                }
                match answer {
                    Ok(answer) => {
                        this.ready = true;
                        this.detail = answer;
                    }
                    Err(error) => this.problem = Some(error),
                }
            },
        );
    }

    /// Revoke or Delete, after the person confirms.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.ready || self.completed {
            return;
        }
        if !self.current(cx) {
            self.problem = Some("The signed-in account changed. Close this dialog and try again.".into());
            cx.notify();
            return;
        }
        let (verb, path) = match self.action {
            Action::Delete => {
                let Some(id) = &self.row.owned_page else { return };
                (Verb::Delete, format!("/v1/pages/{}", nexus_client::url::enc(id)))
            }
            Action::Revoke => {
                let Some(id) = self.row.artifact_id() else { return };
                (Verb::Delete, format!("/v1/artifacts/{}", nexus_client::url::enc(id)))
            }
            _ => return,
        };
        self.busy = true;
        self.problem = None;
        let request = self.gateway.request(verb, path, None);
        fetch(cx, request, |this, answer, cx| {
            this.busy = false;
            if !this.current(cx) {
                this.ready = false;
                this.problem = Some("The signed-in account changed. Close this dialog and try again.".into());
                return;
            }
            match answer {
                Ok(_) => {
                    this.completed = true;
                    this.gateway.load_library(this.org_name.clone());
                }
                Err(error) => this.problem = Some(error),
            }
        });
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy && self.ready {
            return;
        }
        self.closed = true;
        if let Some(focus) = &self.return_focus {
            window.focus(focus, cx);
        }
        cx.notify();
    }
}

#[path = "library_share.rs"]
mod share;
#[path = "library_share_view.rs"]
mod share_view;
#[path = "library_dialog_view.rs"]
mod view;
