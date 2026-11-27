use gpui_kit::component::input::{InputEvent, InputState};
use serde_json::json;

use super::super::{Ctx, Load, Verb, fetch};

use super::*;

pub(in crate::pages) struct PolicyPage {
    pub(super) shared_settings: Option<AnyView>,
    pub(super) ctx: Ctx,
    /// A team's Models section, else the organization's page.
    pub(super) team: Option<TeamScope>,
    /// The teams the person leads, for a team's view.
    pub(super) led: Vec<String>,
    pub(super) permissions: Load<Option<Vec<String>>>,
    pub(super) catalog: Load<Vec<Model>>,
    pub(super) cards: Vec<Card>,
    /// "Edit as patterns": every card shows its allow and block patterns.
    pub(super) patterns: bool,

    pub(super) controls: Load<Vec<Control>>,
    pub(super) confirm_end: Option<String>,
    pub(super) ending: bool,
    pub(super) control_problem: Option<SharedString>,
    pub(super) control_form: Option<ControlForm>,
    /// The "Add control" menu, while open.
    pub(super) control_menu: bool,
    pub(super) reason: Entity<InputState>,
    pub(super) key_subject: Entity<InputState>,
    /// "At a time…": the end, typed as `END_FORMAT` in the computer's time zone.
    pub(super) end_at: Entity<InputState>,
    pub(super) members: Load<Vec<Choice>>,
    pub(super) teams: Load<Vec<Choice>>,

    pub(super) test_as: Option<Choice>,
    pub(super) test_model: String,
    pub(super) test_agent: Kind,
    pub(super) test_open: Option<&'static str>,
    pub(super) test_run: u64,
    pub(super) test: Load<Decision>,

    pub(super) _subscriptions: Vec<Subscription>,
}

impl PolicyPage {
    pub(super) fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with(ctx, None, window, cx)
    }

    pub(super) fn with(ctx: Ctx, team: Option<TeamScope>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let mut cards = Vec::new();
        // A team has one rule: its allowed models, with the default for Chat.
        let kinds: &[Kind] = if team.is_some() { &[Kind::Chat] } else { &Kind::ALL };
        for &kind in kinds {
            let allow = cx.new(|cx| InputState::new(window, cx).placeholder("anthropic/*, openai/gpt-6-*"));
            let deny = cx.new(|cx| InputState::new(window, cx).placeholder("anthropic/claude-fable-*"));
            for input in [&allow, &deny] {
                subscriptions.push(cx.subscribe_in(input, window, |_, _, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        cx.notify();
                    }
                }));
            }
            cards.push(Card {
                kind,
                view: Load::Loading,
                mode: Mode::All,
                picked: Vec::new(),
                allow,
                deny,
                default_model: String::new(),
                reset: false,
                saving: false,
                outcome: None,
                adding: false,
                picking_default: false,
                reviewed: None,
            });
        }
        let reason = cx.new(|cx| InputState::new(window, cx).placeholder("People who hit it see this"));
        let key_subject = cx.new(|cx| InputState::new(window, cx).placeholder("Key id"));
        let end_at = cx.new(|cx| InputState::new(window, cx).placeholder("2026-12-31 18:00"));
        for input in [&reason, &key_subject, &end_at] {
            subscriptions.push(cx.subscribe_in(input, window, |_, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }));
        }
        let shared_settings = team
            .is_none()
            .then(|| super::super::general::build_policy(ctx.clone(), window, cx));
        let mut page = PolicyPage {
            shared_settings,
            ctx,
            team,
            led: Vec::new(),
            permissions: Load::Loading,
            catalog: Load::Loading,
            cards,
            patterns: false,
            controls: Load::Loading,
            confirm_end: None,
            ending: false,
            control_problem: None,
            control_form: None,
            control_menu: false,
            reason,
            key_subject,
            end_at,
            members: Load::Loading,
            teams: Load::Loading,
            test_as: None,
            test_model: "nexus/auto".into(),
            test_agent: Kind::Chat,
            test_open: None,
            test_run: 0,
            test: Load::Loading,
            _subscriptions: subscriptions,
        };
        page.load(cx);
        page
    }

    pub(super) fn has(&self, permission: &str) -> bool {
        match &self.permissions {
            Load::Ready(Some(list)) => list.iter().any(|entry| entry == permission),
            // Gateway did not say: let it answer, and show its refusal.
            _ => true,
        }
    }

    pub(super) fn can_manage(&self) -> bool {
        match &self.team {
            Some(_) => self.leads_team(),
            None => self.has("policy.manage") || self.has("org.settings"),
        }
    }

    pub(super) fn can_control(&self) -> bool {
        match &self.team {
            Some(_) => self.leads_team(),
            None => self.has("policy.manage"),
        }
    }

    /// `canManageTeam`: an owner or admin, or a lead of the team or of a team above it.
    pub(super) fn leads_team(&self) -> bool {
        let Some(team) = &self.team else {
            return false;
        };
        self.has("policy.manage")
            || self.led.contains(&team.slug)
            || team.path.iter().any(|(slug, _)| self.led.contains(slug))
    }

    /// Where a card's rule is read and saved.
    pub(super) fn card_path(&self, kind: Kind) -> String {
        match &self.team {
            Some(team) => format!("/v1/policies/teams/{}", super::super::usage::encode(&team.slug)),
            None => kind.path().to_string(),
        }
    }

    /// Where the controls are listed, set and ended: a team's own route for whoever manages it.
    pub(super) fn controls_path(&self) -> String {
        match &self.team {
            Some(team) if self.leads_team() => {
                format!(
                    "/v1/policies/teams/{}/controls",
                    super::super::usage::encode(&team.slug)
                )
            }
            _ => "/v1/policies/controls".into(),
        }
    }

    /// The controls shown: in a team's view, someone who does not manage it sees only those on the team itself.
    pub(super) fn visible_controls<'a>(&self, list: &'a [Control]) -> Vec<&'a Control> {
        match &self.team {
            Some(team) if !self.leads_team() => list
                .iter()
                .filter(|control| control.level == "team" && control.subject == team.id)
                .collect(),
            _ => list.iter().collect(),
        }
    }

    /// The control kinds the person may set: an override needs an owner or admin.
    pub(super) fn control_kinds(&self) -> Vec<ControlKind> {
        let mut kinds = vec![ControlKind::Pause, ControlKind::Limit];
        if self.has("policy.manage") {
            kinds.push(ControlKind::Override);
        }
        kinds
    }

    pub(super) fn load(&mut self, cx: &mut Context<Self>) {
        let request = self.ctx.gateway.query("/v1/tenants/me".into());
        super::super::read(cx, request, |this, answer, cx| {
            if let Ok(value) = &answer {
                this.led = led_teams(value);
            }
            this.permissions = Load::Ready(answer.ok().and_then(|value| permissions(&value)));
            if this.can_manage() && this.team.is_none() {
                this.load_people(cx);
            }
            // A lead reads the team's own route: read again now it is known.
            if this.team.is_some() {
                this.load_controls(cx);
            }
        });
        self.load_catalog(cx);
        for ix in 0..self.cards.len() {
            self.load_card(ix, true, cx);
        }
        self.load_controls(cx);
        if let Some(team) = &self.team {
            self.members = Load::Ready(
                team.members
                    .iter()
                    .map(|(id, name)| Choice {
                        id: id.clone(),
                        value: id.clone(),
                        label: name.clone(),
                    })
                    .collect(),
            );
            self.teams = Load::Ready(vec![Choice {
                id: team.id.clone(),
                value: team.id.clone(),
                label: team.name.clone(),
            }]);
        }
        self.run_test(cx);
    }

    pub(super) fn load_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog = Load::Loading;
        let mut drafts = self.cards.iter().map(|card| card.draft(cx)).collect::<Vec<_>>();
        let request = self.ctx.gateway.query("/v1/models?status=supported".into());
        super::super::read(cx, request, move |this, answer, cx| {
            this.catalog = match answer {
                Ok(value) => Load::Ready(parse_catalog(&value)),
                Err(error) => Load::Failed(error.into()),
            };
            // The modes depend on the catalog: a pattern it names fits on a chip.
            for (ix, draft) in drafts.iter_mut().enumerate().take(this.cards.len()) {
                if this.cards[ix].draft(cx) == *draft {
                    this.reset_card(ix);
                }
                *draft = this.cards[ix].draft(cx);
            }
            cx.notify();
        });
    }

    pub(super) fn load_people(&mut self, cx: &mut Context<Self>) {
        let request = self.ctx.gateway.query("/v1/orgs/members".into());
        super::super::read(cx, request, |this, answer, _| {
            this.members = match answer {
                Ok(value) => Load::Ready(parse_members(&value)),
                Err(error) => Load::Failed(error.into()),
            };
        });
        let request = self.ctx.gateway.query("/v1/teams".into());
        super::super::read(cx, request, |this, answer, _| {
            this.teams = match answer {
                Ok(value) => Load::Ready(parse_teams(&value)),
                Err(error) => Load::Failed(error.into()),
            };
        });
    }

    /// Read a card's policy; `fresh` shows the loading state first.
    pub(super) fn load_card(&mut self, ix: usize, fresh: bool, cx: &mut Context<Self>) {
        if fresh {
            self.cards[ix].view = Load::Loading;
        }
        let mut draft = self.cards[ix].draft(cx);
        let path = self.card_path(self.cards[ix].kind);
        let request = self.ctx.gateway.query(path);
        super::super::read(cx, request, move |this, answer, cx| match answer {
            Ok(value) => {
                let unedited = this.cards[ix].draft(cx) == draft;
                let view = parse_view(&value);
                // A rule that did not change keeps what the person is editing on its card.
                let same = matches!(&this.cards[ix].view, Load::Ready(old) if old.rule == view.rule);
                this.cards[ix].view = Load::Ready(view);
                if unedited && (fresh || !same) {
                    this.reset_card(ix);
                }
                draft = this.cards[ix].draft(cx);
            }
            Err(error) => {
                if fresh || !matches!(this.cards[ix].view, Load::Ready(_)) {
                    this.cards[ix].view = Load::Failed(error.into())
                }
            }
        });
    }

    pub(super) fn known(&self) -> impl Fn(&str) -> bool + '_ {
        move |id: &str| match &self.catalog {
            Load::Ready(models) => models.iter().any(|model| model.id == id),
            _ => false,
        }
    }

    /// Put the stored rule back in the form.
    pub(super) fn reset_card(&mut self, ix: usize) {
        let mode = {
            let Load::Ready(view) = &self.cards[ix].view else {
                return;
            };
            let known = self.known();
            mode_of(&view.allow, &view.deny, &known)
        };
        let card = &mut self.cards[ix];
        let Load::Ready(view) = &card.view else {
            return;
        };
        card.mode = mode;
        card.picked = if mode == Mode::Except {
            view.deny.clone()
        } else {
            view.allow.clone()
        };
        card.default_model = view.defaults.get(card.kind.key()).cloned().unwrap_or_default();
        card.reset = true;
        card.adding = false;
        card.picking_default = false;
        card.reviewed = None;
    }

    pub(super) fn supported(&self, kind: Kind) -> Vec<Model> {
        match &self.catalog {
            Load::Ready(models) => models
                .iter()
                .filter(|model| {
                    kind.surface()
                        .is_none_or(|surface| model.surfaces.iter().any(|s| s == surface))
                })
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn model_name(&self, id: &str) -> Option<String> {
        match &self.catalog {
            Load::Ready(models) => models
                .iter()
                .find(|model| model.id == id)
                .map(|model| model.name.clone()),
            _ => None,
        }
    }

    pub(super) fn show_patterns(&self, ix: usize) -> bool {
        self.patterns || self.cards[ix].mode == Mode::Advanced
    }

    pub(super) fn draft(&self, ix: usize, cx: &App) -> Draft {
        let card = &self.cards[ix];
        let (allow, deny, only) = if self.show_patterns(ix) {
            (
                lines(&card.allow.read(cx).value()),
                lines(&card.deny.read(cx).value()),
                false,
            )
        } else {
            match card.mode {
                Mode::Only => (card.picked.clone(), Vec::new(), true),
                Mode::Except => (Vec::new(), card.picked.clone(), false),
                _ => (Vec::new(), Vec::new(), false),
            }
        };
        Draft {
            allow,
            deny,
            default_model: card.default_model.clone(),
            only,
        }
    }

    pub(super) fn save(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.can_manage() {
            return;
        }
        let draft = self.draft(ix, cx);
        if !review_ready(&mut self.cards[ix].reviewed, &draft) {
            cx.notify();
            return;
        }
        let Load::Ready(view) = &self.cards[ix].view else {
            return;
        };
        if self.cards[ix].saving {
            return;
        }
        let kind = self.cards[ix].kind;
        let body = self.draft(ix, cx).rule(view, kind);
        self.cards[ix].saving = true;
        self.cards[ix].outcome = None;
        let request = self.ctx.gateway.request(Verb::Put, self.card_path(kind), Some(body));
        fetch(cx, request, move |this, answer, cx| {
            this.cards[ix].saving = false;
            match answer {
                Ok(value) => {
                    let saved = parse_view(&value);
                    let count = saved.allowed.len();
                    this.cards[ix].outcome = Some(Outcome {
                        text: format!(
                            "Saved. {count} supported model{} allowed here.",
                            if count == 1 { "" } else { "s" }
                        )
                        .into(),
                        error: false,
                        notes: saved.notes.clone(),
                    });
                    this.cards[ix].view = Load::Ready(saved);
                    this.reset_card(ix);
                    // The organization's rule narrows the agents', so every card reads again.
                    for other in 0..this.cards.len() {
                        if other != ix {
                            this.load_card(other, false, cx);
                        }
                    }
                    this.run_test(cx);
                }
                Err(error) => {
                    this.cards[ix].outcome = Some(Outcome {
                        text: format!("The policy wasn't saved. {}", refusal(&error)).into(),
                        error: true,
                        notes: Vec::new(),
                    })
                }
            }
        });
        cx.notify();
    }

    pub(super) fn run_test(&mut self, cx: &mut Context<Self>) {
        self.test_run += 1;
        let run = self.test_run;
        let mut body = json!({ "model": self.test_model });
        // A team's view asks for a call in the team (the web's "Test in team").
        if let Some(team) = &self.team {
            body["team"] = json!(team.slug);
        } else if let Some(person) = &self.test_as {
            body["as"] = json!(person.value);
        }
        if self.test_agent != Kind::Chat {
            body["agent"] = json!(self.test_agent.key());
        }
        let request = self
            .ctx
            .gateway
            .request(Verb::Post, "/v1/policies/test".into(), Some(body));
        fetch(cx, request, move |this, answer, _| {
            // A later question already went out: this answer is stale.
            if this.test_run != run {
                return;
            }
            this.test = match answer {
                Ok(value) => Load::Ready(parse_decision(&value)),
                Err(error) => Load::Failed(refusal(&error).into()),
            };
        });
    }

    pub(super) fn load_controls(&mut self, cx: &mut Context<Self>) {
        let request = self.ctx.gateway.query(self.controls_path());
        super::super::read(cx, request, |this, answer, _| {
            this.controls = match answer {
                Ok(value) => Load::Ready(parse_controls(&value)),
                Err(error) => Load::Failed(error.into()),
            };
        });
    }

    pub(super) fn end_control(&mut self, id: String, cx: &mut Context<Self>) {
        if self.ending {
            return;
        }
        if self.confirm_end.as_deref() != Some(id.as_str()) {
            self.confirm_end = Some(id);
            cx.notify();
            return;
        }
        self.confirm_end = None;
        self.ending = true;
        self.control_problem = None;
        let path = format!("{}/{id}", self.controls_path());
        let request = self.ctx.gateway.request(Verb::Delete, path, None);
        fetch(cx, request, |this, answer, cx| {
            this.ending = false;
            if let Err(error) = answer {
                this.control_problem = Some(refusal(&error).into());
            }
            this.load_controls(cx);
            this.run_test(cx);
        });
        cx.notify();
    }

    pub(super) fn open_control_form(&mut self, kind: ControlKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.control_form.as_ref().is_some_and(|form| form.kind == kind) {
            self.control_form = None;
        } else {
            self.control_form = Some(ControlForm {
                kind,
                // A team's control starts on the team itself.
                level: if self.team.is_some() { "team" } else { "organization" },
                subject: self.team.as_ref().map(|team| team.id.clone()).unwrap_or_default(),
                models: Vec::new(),
                expiry: 86_400,
                busy: false,
                confirm: false,
                problem: None,
            });
            self.reason.update(cx, |state, cx| state.set_value("", window, cx));
            self.key_subject.update(cx, |state, cx| state.set_value("", window, cx));
            self.end_at.update(cx, |state, cx| state.set_value("", window, cx));
        }
        cx.notify();
    }

    pub(super) fn send_control(&mut self, cx: &mut Context<Self>) {
        let reason = self.reason.read(cx).value().trim().to_string();
        let key_subject = self.key_subject.read(cx).value().trim().to_string();
        let typed_end = self.end_at.read(cx).value().to_string();
        let Some(form) = self.control_form.as_mut() else {
            return;
        };
        if form.busy {
            return;
        }
        let subject = if form.level == "key" {
            key_subject
        } else {
            form.subject.clone()
        };
        let now = now_secs();
        let ends = control_end(form.expiry, &typed_end, now);
        let why = if form.level != "organization" && subject.is_empty() {
            Some("Pick who it applies to.".to_string())
        } else if ends.is_none() {
            Some("Pick an end time in the future.".to_string())
        } else if reason.is_empty() {
            Some("Give a reason. People who hit the control see it.".to_string())
        } else if form.kind != ControlKind::Pause && form.models.is_empty() {
            Some(format!(
                "Pick at least one model to {}.",
                if form.kind == ControlKind::Limit {
                    "limit to"
                } else {
                    "override"
                }
            ))
        } else {
            None
        };
        if let Some(why) = why {
            form.problem = Some(why.into());
            form.confirm = false;
            cx.notify();
            return;
        }
        // A pause stops requests at once: ask twice.
        if form.kind == ControlKind::Pause && !form.confirm {
            form.confirm = true;
            cx.notify();
            return;
        }
        let mut body = json!({
            "kind": form.kind.key(),
            "level": form.level,
            "reason": reason,
            "expiresAt": iso_utc(ends.unwrap_or(now)),
        });
        if form.level != "organization" {
            body["subject"] = json!(subject);
        }
        if form.kind != ControlKind::Pause {
            body["models"] = json!(form.models);
        }
        form.busy = true;
        form.confirm = false;
        form.problem = None;
        let request = self.ctx.gateway.request(Verb::Post, self.controls_path(), Some(body));
        fetch(cx, request, |this, answer, cx| match answer {
            Ok(_) => {
                this.control_form = None;
                this.load_controls(cx);
                this.run_test(cx);
            }
            Err(error) => {
                if let Some(form) = this.control_form.as_mut() {
                    form.busy = false;
                    form.problem = Some(refusal(&error).into());
                }
            }
        });
        cx.notify();
    }

    /// A name for a control's subject from the people and teams read, else the id's tail.
    pub(super) fn name_of(&self, id: &str) -> String {
        let lists = [&self.members, &self.teams];
        lists
            .into_iter()
            .filter_map(|list| match list {
                Load::Ready(list) => list.iter().find(|choice| choice.id == id || choice.value == id),
                _ => None,
            })
            .next()
            .map(|choice| choice.label.clone())
            .unwrap_or_else(|| {
                if id.len() > 8 {
                    format!("…{}", &id[id.len() - 4..])
                } else {
                    id.to_string()
                }
            })
    }
}
