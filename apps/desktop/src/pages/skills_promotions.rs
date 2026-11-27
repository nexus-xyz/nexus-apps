//! Existing promotion API adapter, sharing SkillsPage's mutation lock and account/selection epochs.
#[path = "skills_promotions_model.rs"]
pub(super) mod model;
#[path = "skills_promotions_view.rs"]
mod view;
use super::{Load, Scope, SkillsPage, Target, Verb, fetch, routes};
use gpui_kit::component::input::InputState;
use gpui_kit::{prelude::*, *};
use model::{Action, Detail, List, Promotion, Team, Viewer};
use serde_json::{Value, json};

pub(super) struct Request {
    pub skill: String,
    pub version: u64,
    pub team: Option<String>,
    pub note: Entity<InputState>,
}
pub(super) struct Step {
    pub id: String,
    pub action: Action,
    pub reason: Entity<InputState>,
}
pub(super) struct State {
    pub authority: Load<Viewer>,
    pub teams: Load<Vec<Team>>,
    pub list: Load<List>,
    pub mine: bool,
    pub filters_set: bool,
    pub detail_target: Option<String>,
    pub pending: Option<Target>,
    pub filter: &'static str,
    pub next: Option<String>,
    pub selected: Option<(String, Load<Detail>)>,
    pub request: Option<Request>,
    pub step: Option<Step>,
    pub context_epoch: u64,
    pub list_epoch: u64,
    pub detail_epoch: u64,
    pub content: bool,
    pub diff: bool,
    pub existing: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            authority: Load::Loading,
            teams: Load::Loading,
            list: Load::Loading,
            mine: true,
            filters_set: false,
            detail_target: None,
            pending: None,
            filter: "held",
            next: None,
            selected: None,
            request: None,
            step: None,
            context_epoch: 0,
            list_epoch: 0,
            detail_epoch: 0,
            content: false,
            diff: false,
            existing: None,
        }
    }
}
impl SkillsPage {
    /// Returns false rather than replacing an editor, confirmation or in-flight write with a link target.
    pub(crate) fn set_promotion_target(&mut self, target: Target, cx: &mut Context<Self>) -> bool {
        if !self.current() || self.busy || self.confirming || self.editor.is_some() || self.promotion_form_open() {
            return false;
        }
        self.change_scope(Scope::Requests, cx);
        self.promotions.pending = Some(target);
        true
    }
    pub(super) fn choose_promotion_filters(
        &mut self,
        mine: Option<bool>,
        state: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if !self.current()
            || self.busy
            || self.promotions.step.is_some()
            || !self.promotion_viewer().is_some_and(Viewer::requests)
        {
            return;
        }
        if mine == Some(false) && !self.promotion_viewer().is_some_and(Viewer::approver) {
            return;
        }
        self.promotions.pending = None;
        self.promotions.filters_set = true;
        if let Some(mine) = mine {
            self.promotions.mine = mine;
            self.promotions.filter = if mine { "all" } else { "held" };
        }
        if let Some(state) = state {
            self.promotions.filter = state;
        }
        self.promotion_list(None, cx);
    }
    fn initialize_promotion_filters(&mut self, of: Option<&str>, state: Option<&str>) {
        let approver = self.promotion_viewer().is_some_and(Viewer::approver);
        (self.promotions.mine, self.promotions.filter) = routes::filters(approver, of, state);
        self.promotions.filters_set = true;
    }
    /// Called only after current authority resolves. Detail links do not depend on a default list containing them.
    fn apply_promotion_target(&mut self, cx: &mut Context<Self>) -> bool {
        // Authority can shrink without an identity change. A retained reach filter must not suppress the next read.
        // Keep the explicit state while narrowing the list to the viewer's own requests, as requestsFilters does.
        if !self.promotion_viewer().is_some_and(Viewer::approver) {
            self.promotions.mine = true;
        }
        match self.promotions.pending.take() {
            Some(Target::Detail(id)) => {
                if !self.promotions.filters_set {
                    self.initialize_promotion_filters(None, None);
                }
                self.promotions.detail_target = Some(id.clone());
                self.open_promotion(id, cx);
                true
            }
            Some(Target::Requests { of, state }) => {
                self.initialize_promotion_filters(of.as_deref(), state.as_deref());
                false
            }
            None => {
                if !self.promotions.filters_set {
                    self.initialize_promotion_filters(None, None);
                }
                false
            }
        }
    }

    pub(super) fn promotion_form_open(&self) -> bool {
        self.promotions.request.is_some() || self.promotions.step.is_some()
    }
    pub(super) fn promotion_viewer(&self) -> Option<&Viewer> {
        if !self.current() {
            return None;
        }
        match &self.promotions.authority {
            Load::Ready(viewer) => Some(viewer),
            _ => None,
        }
    }
    pub(super) fn promotion_allowed(&self, action: Action) -> bool {
        let Some(viewer) = self.promotion_viewer() else {
            return false;
        };
        let teams = match &self.promotions.teams {
            Load::Ready(teams) => Some(teams.as_slice()),
            _ => None,
        };
        matches!(&self.promotions.selected,Some((_,Load::Ready(detail))) if detail.promotion.allows(action,viewer,teams))
    }
    pub(super) fn promotion_context(&mut self, cx: &mut Context<Self>) {
        if !self.current() || self.busy {
            return;
        }
        if self.promotions.pending.is_none() {
            self.promotions.pending = self.promotions.detail_target.clone().map(Target::Detail);
        }
        self.promotions.context_epoch += 1;
        self.promotions.list_epoch += 1;
        self.promotions.detail_epoch += 1;
        self.promotions.selected = None;
        self.promotions.step = None;
        self.promotions.list = Load::Loading;
        self.promotions.next = None;
        let epoch = self.promotions.context_epoch;
        self.promotions.authority = Load::Loading;
        self.promotions.teams = Load::Loading;
        let call = self.ctx.gateway.request_typed(Verb::Get, "/v1/tenants/me".into(), None);
        fetch(cx, call, move |this, result, cx| {
            if !this.current() || this.promotions.context_epoch != epoch {
                return;
            }
            this.promotions.authority = match result.map_err(problem).and_then(Viewer::parse) {
                Ok(viewer) => Load::Ready(viewer),
                Err(error) => Load::Failed(error.into()),
            };
            if this.promotion_viewer().is_some_and(Viewer::requests) {
                if this.scope == Scope::Requests && !this.apply_promotion_target(cx) {
                    this.promotion_list(None, cx);
                }
            } else {
                // Pending route intent is pure data: a retry still rechecks authority before fetching its text.
                this.promotions.selected = None;
                this.promotions.step = None;
                this.promotions.list = Load::Loading;
            }
        });
        let call = self.ctx.gateway.request_typed(Verb::Get, "/v1/teams".into(), None);
        fetch(cx, call, move |this, result, _| {
            if !this.current() || this.promotions.context_epoch != epoch {
                return;
            }
            this.promotions.teams = match result.map_err(problem).and_then(|v| super::model::parse::<Teams>(v)) {
                Ok(teams) => Load::Ready(teams.teams),
                Err(error) => Load::Failed(error.into()),
            };
        });
        cx.notify();
    }
    pub(super) fn change_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if !self.current() || self.busy || self.editor.is_some() || self.promotion_form_open() {
            return;
        }
        self.promotions.pending = None;
        self.promotions.detail_target = None;
        self.scope = scope;
        self.selection = None;
        self.confirming = false;
        self.epoch += 1;
        self.problem = None;
        self.notice = None;
        self.promotions.selected = None;
        self.promotions.detail_epoch += 1;
        self.promotions.context_epoch += 1;
        self.promotions.list_epoch += 1;
        if scope == Scope::Requests {
            self.promotion_context(cx);
        }
        cx.notify();
    }
    pub(super) fn promotion_list(&mut self, cursor: Option<String>, cx: &mut Context<Self>) {
        if !self.current()
            || self.busy
            || self.scope != Scope::Requests
            || !self.promotion_viewer().is_some_and(Viewer::requests)
        {
            return;
        }
        if !self.promotions.mine && !self.promotion_viewer().is_some_and(Viewer::approver) {
            return;
        }
        self.promotions.list_epoch += 1;
        let epoch = self.promotions.list_epoch;
        self.promotions.list = Load::Loading;
        self.promotions.next = None;
        let call = self.ctx.gateway.request_typed(
            Verb::Get,
            model::query(self.promotions.mine, self.promotions.filter, cursor.as_deref()),
            None,
        );
        fetch(cx, call, move |this, result, _| {
            if !this.current() || this.promotions.list_epoch != epoch {
                return;
            }
            let denied = matches!(&result, Err(nexus_client::Error::Core(error)) if matches!(error.status, 401|403));
            if denied {
                this.promotions.detail_epoch += 1;
                this.promotions.selected = None;
                this.promotions.step = None;
                this.promotions.authority =
                    Load::Failed("Promotion access changed. Reload account permissions.".into());
            }
            this.promotions.list = match result.map_err(problem).and_then(super::model::parse::<List>) {
                Ok(list) if list.promotions.iter().all(Promotion::valid) => {
                    this.promotions.next = list.next_cursor.clone();
                    Load::Ready(list)
                }
                Ok(_) => Load::Failed("Nexus returned an invalid promotion list.".into()),
                Err(error) => Load::Failed(error.into()),
            };
        });
        cx.notify();
    }
    pub(super) fn open_promotion(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.current()
            || self.busy
            || self.editor.is_some()
            || !self.promotion_viewer().is_some_and(Viewer::requests)
        {
            return;
        }
        self.promotions.request = None;
        self.promotions.step = None;
        self.promotions.existing = None;
        self.promotions.content = false;
        self.promotions.diff = false;
        self.promotions.detail_epoch += 1;
        let epoch = self.promotions.detail_epoch;
        self.promotions.selected = Some((id.clone(), Load::Loading));
        self.scope = Scope::Requests;
        self.selection = None;
        self.epoch += 1;
        let call = self.ctx.gateway.request_typed(Verb::Get, model::path(&id), None);
        fetch(cx, call, move |this, result, _| {
            if !this.current() || this.promotions.detail_epoch != epoch {
                return;
            }
            let denied = matches!(&result,Err(nexus_client::Error::Core(error)) if matches!(error.status,401|403));
            let detail = match result.map_err(problem).and_then(super::model::parse::<Detail>) {
                Ok(detail) if detail.promotion.id == id && detail.promotion.valid() => Load::Ready(detail),
                Ok(_) => Load::Failed("Nexus returned a different promotion. Reload the request.".into()),
                Err(error) => Load::Failed(error.into()),
            };
            if denied {
                this.promotions.authority =
                    Load::Failed("Promotion access changed. Reload account permissions.".into());
            }
            this.promotions.selected = Some((id.clone(), detail));
        });
        cx.notify();
    }
    pub(super) fn begin_promotion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.current() || self.busy || self.editor.is_some() || self.promotion_form_open() {
            return;
        }
        let Some(selection) = &self.selection else {
            return;
        };
        let Load::Ready(file) = &selection.file else {
            return;
        };
        let Load::Ready(detail) = &selection.detail else {
            return;
        };
        if !self.owns(&selection.id)
            || file.skill_id != selection.id
            || selection.version != Some(file.version)
            || !detail.versions.iter().any(|v| v.version == file.version)
        {
            return;
        }
        self.promotions.request = Some(Request {
            skill: selection.id.clone(),
            version: file.version,
            team: None,
            note: cx.new(|cx| InputState::new(window, cx).placeholder("Optional note (500 characters)")),
        });
        self.confirming = false;
        self.problem = None;
        self.notice = None;
        self.promotion_context(cx);
        cx.notify();
    }
    pub(super) fn request_promotion(&mut self, cx: &mut Context<Self>) {
        if !self.current() || self.busy || !self.promotion_viewer().is_some_and(Viewer::requests) {
            return;
        }
        let Some(draft) = &self.promotions.request else {
            return;
        };
        if !self.owns(&draft.skill)
            || !matches!(&self.selection,Some(s) if s.id==draft.skill && s.version==Some(draft.version)
            && matches!(&s.file,Load::Ready(file) if file.skill_id==draft.skill && file.version==draft.version))
        {
            return;
        }
        let mut body = json!({"scope":"org"});
        if let Some(team) = &draft.team {
            let Some(team) = self
                .promotion_viewer()
                .and_then(|viewer| viewer.teams.iter().find(|entry| &entry.id == team))
            else {
                return;
            };
            body = json!({"scope":"team","team":team.slug});
        }
        match model::text(&draft.note.read(cx).value(), false) {
            Ok(Some(note)) => body["note"] = json!(note),
            Ok(None) => {}
            Err(error) => {
                self.problem = Some(error);
                cx.notify();
                return;
            }
        }
        let requester = self.promotion_viewer().and_then(|viewer| viewer.user.clone());
        let skill = draft.skill.clone();
        let version = draft.version;
        let team = draft.team.clone();
        let path = format!("{}/versions/{version}/promotions", super::model::path(&skill));
        self.busy = true;
        self.epoch += 1;
        let epoch = self.epoch;
        self.problem = None;
        self.promotions.existing = None;
        self.promotions.list_epoch += 1;
        self.promotions.detail_epoch += 1;
        let call = self.ctx.gateway.request_typed(Verb::Post, path, Some(body));
        fetch(cx, call, move |this, result, cx| {
            if !this.accepts(epoch) {
                return;
            }
            this.busy = false;
            if matches!(&result, Err(nexus_client::Error::Core(error)) if matches!(error.status,401|403)) {
                this.promotions.authority =
                    Load::Failed("Promotion access changed. Reload account permissions.".into());
            }
            if let Err(error) = &result {
                this.promotions.existing = existing(error);
            }
            match result.map_err(problem).and_then(super::model::parse::<Promotion>) {
                Ok(p)
                    if p.valid()
                        && p.state == "held"
                        && p.source_skill_id == skill
                        && Some(&p.requested_by) == requester.as_ref()
                        && p.source_version == version
                        && p.team_id == team
                        && p.scope == if team.is_some() { "team" } else { "org" } =>
                {
                    this.promotions.request = None;
                    this.notice = Some("Promotion requested. Another authorized person must decide it.".into());
                    this.scope = Scope::Requests;
                    this.promotion_list(None, cx);
                    this.open_promotion(p.id, cx);
                }
                Ok(_) => {
                    this.problem =
                        Some("Nexus returned a different promotion request. Reload requests before retrying.".into())
                }
                Err(error) => this.problem = Some(error),
            }
        });
        cx.notify();
    }
    pub(super) fn begin_decision(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.promotion_form_open() || !self.promotion_allowed(action) {
            return;
        }
        let Some((id, Load::Ready(_))) = &self.promotions.selected else {
            return;
        };
        let reason = cx.new(|cx| InputState::new(window, cx).placeholder("Reason (500 characters)"));
        // The step opens ready for its reason.
        reason.update(cx, |state, cx| state.focus(window, cx));
        self.promotions.step = Some(Step {
            id: id.clone(),
            action,
            reason,
        });
        self.problem = None;
        cx.notify();
    }
    pub(super) fn decide_promotion(&mut self, cx: &mut Context<Self>) {
        if !self.current() || self.busy {
            return;
        }
        let Some(step) = &self.promotions.step else {
            return;
        };
        if !self.promotion_allowed(step.action)
            || !matches!(&self.promotions.selected,Some((id,Load::Ready(_))) if id==&step.id)
        {
            return;
        }
        let body = match model::decision(step.action, &step.reason.read(cx).value()) {
            Ok(body) => body,
            Err(error) => {
                self.problem = Some(error);
                cx.notify();
                return;
            }
        };
        let id = step.id.clone();
        let Some((_, Load::Ready(detail))) = &self.promotions.selected else {
            return;
        };
        let original = detail.promotion.clone();
        let action = step.action;
        let expected = match action {
            Action::Approve => "approved",
            Action::Refuse => "refused",
            _ => "withdrawn",
        };
        let suffix = if matches!(action, Action::Approve | Action::Refuse) {
            "decision"
        } else {
            "withdraw"
        };
        self.busy = true;
        self.epoch += 1;
        let epoch = self.epoch;
        self.problem = None;
        self.promotions.detail_epoch += 1;
        self.promotions.list_epoch += 1;
        self.lists_epoch += 1;
        let call = self
            .ctx
            .gateway
            .request_typed(Verb::Post, format!("{}/{suffix}", model::path(&id)), Some(body));
        fetch(cx, call, move |this, result, cx| {
            if !this.accepts(epoch) {
                return;
            }
            this.busy = false;
            let conflict = matches!(&result,Err(nexus_client::Error::Core(error)) if error.status==409);
            let denied = matches!(&result,Err(nexus_client::Error::Core(error)) if matches!(error.status,401|403));
            match result.map_err(problem).and_then(super::model::parse::<Promotion>) {
                Ok(p)
                    if p.id == id
                        && p.valid()
                        && p.state == expected
                        && p.requested_by == original.requested_by
                        && p.source_skill_id == original.source_skill_id
                        && p.source_version == original.source_version
                        && p.scope == original.scope
                        && p.team_id == original.team_id
                        && p.content_sha256 == original.content_sha256 =>
                {
                    this.promotions.step = None;
                    this.notice = Some(format!("Promotion {expected}."));
                    this.load(cx);
                    this.promotion_list(None, cx);
                    this.open_promotion(id.clone(), cx);
                }
                Ok(_) => this.problem = Some("Nexus did not confirm the requested change. Reload the request.".into()),
                Err(error) => this.problem = Some(error),
            }
            if conflict {
                this.promotions.step = None;
                this.promotion_list(None, cx);
                this.open_promotion(id, cx);
            }
            if denied {
                this.promotions.step = None;
                this.promotions.selected = None;
                this.promotions.authority =
                    Load::Failed("Promotion access changed. Reload account permissions.".into());
            }
        });
        cx.notify();
    }
}
#[derive(serde::Deserialize)]
struct Teams {
    teams: Vec<Team>,
}
fn existing(error: &nexus_client::Error) -> Option<String> {
    match error {
        nexus_client::Error::Core(error) if error.code.as_deref() == Some("promotion_exists") => error
            .details
            .as_ref()
            .and_then(|details| details.get("promotionId"))
            .and_then(Value::as_str)
            .map(String::from),
        _ => None,
    }
}
fn problem(error: nexus_client::Error) -> String {
    let hint = match &error {
        nexus_client::Error::Core(error) => match error.code.as_deref() {
            Some("promotion_decided") => " Reload the request to see the current answer.",
            Some("promotion_exists") => " Open the existing request, or reload requests before trying again.",
            Some("person_session_required") => " Sign in with your account; an API key cannot decide requests.",
            Some("self_approval") => " Ask another authorized person to decide your request.",
            Some("name_taken") => {
                " Refuse this request and ask its author to rename it, or withdraw the other skill first."
            }
            Some("requester_left") => " The requester left. Withdraw the request instead.",
            Some("personal_org") => " Open a company organization before requesting promotion.",
            Some("not_in_team" | "unknown_team") => " Pick the organization or a team in your membership list.",
            Some("library_limit") => " The library is full. Withdrawing versions does not free a place.",
            Some("promotion_not_found") => " Access may have changed, or this request may be gone.",
            _ => "",
        },
        _ => "",
    };
    let code = match &error {
        nexus_client::Error::Core(error) => error.code.clone(),
        _ => None,
    };
    let code = code.map(|code| format!(" ({code})")).unwrap_or_default();
    format!("{}{code}{hint}", super::model::problem(error))
}
