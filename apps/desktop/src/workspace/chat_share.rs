//! The chat's Share sheet. Gateway owns the frozen versions and enforces organization policy.

use crate::workspace::overlay::OverlaySlot;
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::sharing::{self, Audience};
use serde_json::{Value, json};

use super::{View, Workspace};
use crate::gateway::Verb;
use crate::pages::fetch;

pub(super) struct ChatShare {
    gateway: crate::workspace::gateway_access::AccountGateway,
    pub(super) session: String,
    personal: bool,
    /// The organization's name, for "Anyone at Nexus".
    org: String,
    /// Turns a blocked audience on, or asks who can (`share_ui::blocked_row_end`).
    fixer: crate::setup::Fixer,
    /// The History page (versions, requests, readers) shows instead of the audience.
    history: bool,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    pub(super) closed: bool,
    busy: bool,
    ready: bool,
    problem: Option<String>,
    detail: Value,
    policy: Value,
    role: String,
    kind: Audience,
    picks: Vec<String>,
    teams: Result<Vec<(String, String)>, String>,
    people: Result<Vec<(String, String)>, String>,
    outside: Entity<InputState>,
    expiry: Entity<InputState>,
    allow_continue: bool,
    copied: bool,
    access: Option<Result<Value, String>>,
    confirm_revoke: Option<String>,
}

impl Workspace {
    pub(super) fn open_chat_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.view, View::Chat) || self.entries.is_empty() || self.asking.is_some() {
            return;
        }
        let Some(session) = self.current.clone() else {
            return;
        };
        let Some(gateway) = self.account_gateway() else { return };
        let personal = self.account.as_ref().is_some_and(|a| a.personal);
        let org = self
            .account
            .as_ref()
            .map(|a| a.org_name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "your organization".into());
        let fixer = self.fixer(cx);
        let sheet = cx.new(|cx| ChatShare::new(gateway, session, personal, org, fixer, window, cx));
        let focus = sheet.read(cx).focus.clone();
        window.focus(&focus, cx);
        self.chat_share = Some(sheet);
        self.overlay.close_chat_menu();
        cx.notify();
    }

    pub(super) fn render_chat_share(&self, cx: &App) -> Option<AnyElement> {
        let sheet = self.chat_share.as_ref()?;
        let state = sheet.read(cx);
        if state.closed || self.current.as_deref() != Some(&state.session) || !matches!(self.view, View::Chat) {
            return None;
        }
        Some(sheet.clone().into_any_element())
    }
}

fn word(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().into()
}
fn list(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}
fn path(session: &str) -> String {
    format!(
        "/v1/chat/sessions/{}",
        url::form_urlencoded::byte_serialize(session.as_bytes()).collect::<String>()
    )
}

impl ChatShare {
    #[allow(clippy::too_many_arguments)]
    fn new(
        gateway: crate::workspace::gateway_access::AccountGateway,
        session: String,
        personal: bool,
        org: String,
        fixer: crate::setup::Fixer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut sheet = Self {
            gateway,
            session,
            personal,
            org,
            fixer,
            history: false,
            focus: cx.focus_handle(),
            return_focus: window.focused(cx),
            closed: false,
            busy: false,
            ready: false,
            problem: None,
            detail: Value::Null,
            policy: Value::Null,
            role: String::new(),
            kind: Audience::Private,
            picks: Vec::new(),
            teams: Ok(Vec::new()),
            people: Ok(Vec::new()),
            outside: cx.new(|cx| InputState::new(window, cx).placeholder("Add email")),
            expiry: cx.new(|cx| InputState::new(window, cx).placeholder("Never, 7d, 12h")),
            allow_continue: true,
            copied: false,
            access: None,
            confirm_revoke: None,
        };
        // Typing addresses re-checks the primary button (Share, Save, disabled while empty).
        cx.observe(&sheet.outside, |_, _, cx| cx.notify()).detach();
        sheet.load(window, cx);
        sheet
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true;
        self.problem = None;
        let session = self.gateway.request(Verb::Get, path(&self.session), None);
        let policy = self.gateway.request(Verb::Get, "/v1/orgs/settings".into(), None);
        let me = self.gateway.request(Verb::Get, "/v1/tenants/me".into(), None);
        let teams = self.gateway.request(Verb::Get, "/v1/teams".into(), None);
        let people = self.gateway.request(Verb::Get, "/v1/orgs/members".into(), None);
        cx.spawn_in(window, async move |this, cx| {
            let (session, policy, me, teams, people) = futures::join!(session, policy, me, teams, people);
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match (session, policy, me) {
                    (Ok(detail), Ok(policy), Ok(me)) => {
                        this.detail = detail;
                        this.policy = policy;
                        this.role = word(&me, "role");
                        this.teams = teams.map(|answer| choices(&answer, "teams", "slug"));
                        this.people = people.map(|answer| choices(&answer, "members", "userId"));
                        let current = &this.detail["sharing"];
                        this.kind = Audience::from_key(&word(&current["audience"], "kind"));
                        this.picks = list(
                            &current["audience"],
                            if this.kind == Audience::Teams {
                                "teams"
                            } else {
                                "people"
                            },
                        );
                        this.allow_continue = current["allowContinue"].as_bool().unwrap_or(true);
                        let expiry = word(current, "expiresAt");
                        let expiry = if expiry.is_empty() {
                            this.policy["artifacts.external.expiry"]["maxDays"]
                                .as_u64()
                                .map(|days| format!("{days}d"))
                                .unwrap_or_default()
                        } else {
                            expiry
                        };
                        this.expiry.update(cx, |state, cx| state.set_value(expiry, window, cx));
                        this.outside
                            .update(cx, |state, cx| state.set_value(this.picks.join(", "), window, cx));
                        this.ready = true;
                        this.load_access(cx);
                    }
                    (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => this.problem = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn pick(&mut self, kind: Audience, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || kind == self.kind || kind.blocked(&self.policy, &self.role).is_some() {
            return;
        }
        self.kind = kind;
        self.picks.clear();
        self.problem = None;
        self.copied = false;
        if kind == Audience::External {
            self.outside.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    fn change(&mut self, verb: Verb, endpoint: String, body: Option<Value>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.problem = None;
        self.copied = false;
        self.confirm_revoke = None;
        let request = self.gateway.request(verb, endpoint, body);
        let gateway = self.gateway.clone();
        let session_path = path(&self.session);
        fetch(
            cx,
            async move {
                request.await?;
                Ok(match gateway.request(Verb::Get, session_path, None).await {
                    Ok(detail) => json!({"detail": detail}),
                    Err(error) => json!({"refreshError": error}),
                })
            },
            |this, answer, cx| {
                this.busy = false;
                match answer {
                    Ok(answer) => {
                        this.gateway.load_library(None);
                        if let Some(error) = answer["refreshError"].as_str() {
                            this.ready = false;
                            this.problem = Some(format!(
                                "The change was saved, but the chat could not reload: {error}. Retry reloads it."
                            ));
                        } else {
                            this.detail = answer["detail"].clone();
                            this.load_access(cx);
                        }
                    }
                    Err(error) => this.problem = Some(error),
                }
            },
        );
        cx.notify();
    }

    fn share(&mut self, latest: bool, cx: &mut Context<Self>) {
        let current = &self.detail["sharing"];
        let kind = if latest {
            Audience::from_key(&word(&current["audience"], "kind"))
        } else {
            self.kind
        };
        if let Some(reason) = kind.blocked(&self.policy, &self.role) {
            self.problem = Some(reason.into());
            cx.notify();
            return;
        }
        if kind == Audience::Private {
            if !latest {
                self.change(Verb::Delete, format!("{}/share", path(&self.session)), None, cx);
            }
            return;
        }
        let picks = if latest {
            list(
                &current["audience"],
                if kind == Audience::Teams { "teams" } else { "people" },
            )
        } else if kind == Audience::External {
            self.outside
                .read(cx)
                .value()
                .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                .filter(|part| !part.is_empty())
                .map(String::from)
                .collect()
        } else {
            self.picks.clone()
        };
        if matches!(kind, Audience::Teams | Audience::People | Audience::External) && picks.is_empty() {
            self.problem = Some("Choose at least one team or person.".into());
            cx.notify();
            return;
        }
        let expiry = self.expiry.read(cx).value().to_string();
        if kind == Audience::Public
            && expiry.trim().is_empty()
            && self.policy["artifacts.external.expiry"]["required"] == true
        {
            self.problem = Some("Your organization requires an expiry. Enter a duration such as 7d.".into());
            cx.notify();
            return;
        }
        let allow_continue = self.allow_continue
            && (!kind.outside() || self.policy["artifacts.external.continue"].as_bool().unwrap_or(true));
        let body = sharing::body(kind, &picks, &expiry, allow_continue, !latest);
        self.change(Verb::Post, format!("{}/share", path(&self.session)), Some(body), cx);
    }

    /// Whether the audience picked differs from the one the chat is shared with now (the primary reads "Save").
    fn audience_changed(&self, cx: &App) -> bool {
        let current = &self.detail["sharing"];
        let shared = Audience::from_key(&word(&current["audience"], "kind"));
        if current.is_null() || shared != self.kind {
            return true;
        }
        match self.kind {
            Audience::Teams => list(&current["audience"], "teams") != self.picks,
            Audience::People => list(&current["audience"], "people") != self.picks,
            Audience::External => {
                let typed: Vec<String> = self
                    .outside
                    .read(cx)
                    .value()
                    .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                    .filter(|part| !part.is_empty())
                    .map(String::from)
                    .collect();
                list(&current["audience"], "people") != typed
            }
            _ => false,
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy && self.ready {
            return;
        }
        self.closed = true;
        if let Some(focus) = &self.return_focus {
            window.focus(focus, cx);
        } else {
            window.blur(cx);
        }
        cx.notify();
    }
}

fn choices(answer: &Value, key: &str, id_key: &str) -> Vec<(String, String)> {
    answer[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let id = row[id_key].as_str()?;
            let label = row["name"].as_str().or_else(|| row["email"].as_str()).unwrap_or(id);
            Some((id.into(), label.into()))
        })
        .collect()
}

mod access;
mod view;
