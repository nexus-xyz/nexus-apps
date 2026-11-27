use super::*;

pub(super) struct UsagePage {
    pub(super) ctx: Ctx,
    pub(super) marker: Option<String>,
    pub(super) billing_limit: AnyView,
    pub(super) mutation: std::rc::Rc<std::cell::Cell<bool>>,
    pub(super) money_epoch: u64,
    pub(super) account_epoch: u64,
    pub(super) caps_epoch: u64,
    pub(super) tab: Tab,
    pub(super) range: Range,
    /// Bumped when the range changes, so a late answer for the old range is dropped.
    pub(super) generation: u64,
    pub(super) me: Load<Me>,
    pub(super) spend: Load<Spend>,
    pub(super) budgets: Load<Budgets>,
    // Overview.
    pub(super) chart_by: By,
    pub(super) now: Load<Report>,
    pub(super) before: Load<Report>,
    pub(super) now_other: Load<Report>,
    pub(super) before_other: Load<Report>,
    // Breakdown.
    pub(super) by: By,
    pub(super) under: Option<String>,
    pub(super) breakdown: Load<Report>,
    pub(super) whole: Option<Load<Report>>,
    pub(super) open_teams: HashMap<String, Load<Report>>,
    pub(super) everyone: bool,
    // Adoption.
    pub(super) adoption: Load<Adoption>,
    // Calls.
    pub(super) filters: CallFilters,
    pub(super) cursors: Vec<String>,
    pub(super) calls: Load<CallPage>,
    pub(super) names: HashMap<String, String>,
    pub(super) agents: HashMap<String, String>,
    pub(super) model_options: Vec<(String, String)>,
    pub(super) product_options: Vec<(String, String)>,
    pub(super) menu: Option<Menu>,
    pub(super) selected: Option<String>,
    pub(super) detail: Option<Load<Call>>,
    pub(super) pricing: HashMap<String, (f64, f64)>,
    pub(super) team_names: HashMap<String, String>,
    pub(super) all_fields: bool,
    pub(super) copied: bool,
    pub(super) call_id: Entity<InputState>,
    // Limits.
    pub(super) limit_filter: LimitFilter,
    pub(super) people_query: Entity<InputState>,
    pub(super) more: bool,
    pub(super) editing: Option<Edit>,
    pub(super) amount: Entity<InputState>,
    pub(super) saving: bool,
    pub(super) edit_error: Option<SharedString>,
    /// "No default" pressed once, waiting for the second press.
    pub(super) confirm_clear: bool,
    pub(super) manager_busy: Option<String>,
    pub(super) manager_error: Option<SharedString>,
    // Export.
    pub(super) export_menu: bool,
    pub(super) exporting: bool,
    /// How the last export ended: its note, or why it failed.
    pub(super) exported: Option<Result<String, String>>,
    /// The next part of a cut file, while one is offered.
    pub(super) export_next: Option<super::usage_export::Ask>,
    // Overview's team caps.
    pub(super) caps: Load<Vec<super::usage_caps::Read>>,
    pub(super) _subscriptions: Vec<Subscription>,
}

impl UsagePage {
    pub(super) fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> UsagePage {
        let call_id = cx.new(|cx| InputState::new(window, cx).placeholder("Search call id"));
        let people_query = cx.new(|cx| InputState::new(window, cx).placeholder("Search people"));
        let amount = cx.new(|cx| InputState::new(window, cx).placeholder("Amount in US dollars"));
        let subscriptions = vec![
            cx.subscribe_in(&call_id, window, |this: &mut Self, state, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let id = state.read(cx).value().trim().to_string();
                    if !id.is_empty() {
                        this.open_call(id, cx);
                    }
                }
            }),
            cx.subscribe_in(&people_query, window, |_: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&amount, window, |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save_amount(cx);
                }
            }),
        ];
        let marker = ctx.gateway.marker();
        let mutation = std::rc::Rc::new(std::cell::Cell::new(false));
        let billing_limit = super::billing::build_usage_limit(ctx.clone(), mutation.clone(), window, cx);
        let mut page = UsagePage {
            ctx,
            marker,
            billing_limit,
            mutation,
            money_epoch: 0,
            account_epoch: 0,
            caps_epoch: 0,
            tab: Tab::Overview,
            range: Range::Month,
            generation: 0,
            me: Load::Loading,
            spend: Load::Loading,
            budgets: Load::Loading,
            chart_by: By::Team,
            now: Load::Loading,
            before: Load::Loading,
            now_other: Load::Loading,
            before_other: Load::Loading,
            by: By::Team,
            under: None,
            breakdown: Load::Loading,
            whole: None,
            open_teams: HashMap::new(),
            everyone: false,
            adoption: Load::Loading,
            filters: CallFilters::default(),
            cursors: Vec::new(),
            calls: Load::Loading,
            names: HashMap::new(),
            agents: HashMap::new(),
            model_options: Vec::new(),
            product_options: Vec::new(),
            menu: None,
            selected: None,
            detail: None,
            pricing: HashMap::new(),
            team_names: HashMap::new(),
            all_fields: false,
            copied: false,
            call_id,
            limit_filter: LimitFilter::All,
            people_query,
            more: false,
            editing: None,
            amount,
            saving: false,
            edit_error: None,
            confirm_clear: false,
            manager_busy: None,
            manager_error: None,
            export_menu: false,
            exporting: false,
            exported: None,
            export_next: None,
            caps: Load::Loading,
            _subscriptions: subscriptions,
        };
        page.load_account(cx);
        page.load_tab(cx);
        page
    }

    pub(super) fn span(&self) -> Span {
        self.range.span(today())
    }

    /// How many days the range holds now.
    pub(super) fn days(&self) -> i64 {
        self.span().days()
    }

    pub(super) fn get(
        &self,
        path: String,
        cx: &mut Context<Self>,
        mut apply: impl FnMut(&mut Self, Result<Value, String>, &mut Context<Self>) + 'static,
    ) {
        let generation = self.generation;
        super::read(cx, self.ctx.gateway.query(path), move |this, answer, cx| {
            if this.current() && this.generation == generation {
                apply(this, answer, cx);
            }
        });
    }

    pub(super) fn usage_path(by: By, team_query: &[(&str, &str)], from: &str, to: &str) -> String {
        let mut pairs = vec![("groupBy", by.slug()), ("from", from), ("to", to)];
        pairs.extend_from_slice(team_query);
        format!("/v1/analytics/usage{}", query(&pairs))
    }

    /// Who is signed in, the spend and the budgets: every tab reads them.
    pub(super) fn load_account(&mut self, cx: &mut Context<Self>) {
        self.account_epoch += 1;
        let epoch = self.account_epoch;
        self.me = Load::Loading;
        super::read(
            cx,
            self.ctx.gateway.query("/v1/tenants/me".into()),
            move |this, answer, cx| {
                if !this.current() || this.account_epoch != epoch {
                    return;
                }
                this.me = load_of(answer, Me::parse);
                if this.can("budgets.manage") {
                    this.load_caps(cx);
                }
            },
        );
        self.load_money(cx);
    }

    pub(super) fn load_money(&mut self, cx: &mut Context<Self>) {
        self.money_epoch += 1;
        let epoch = self.money_epoch;
        self.get("/v1/spend".into(), cx, move |this, answer, _| {
            if this.money_epoch == epoch {
                this.spend = load_of(answer, Spend::parse);
            }
        });
        self.get("/v1/budgets".into(), cx, move |this, answer, _| {
            if this.money_epoch == epoch {
                this.budgets = load_of(answer, Budgets::parse);
            }
        });
    }

    pub(super) fn load_tab(&mut self, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Overview => self.load_overview(cx),
            Tab::Breakdown => self.load_breakdown(cx),
            Tab::Adoption => self.load_adoption(cx),
            Tab::Calls => self.load_calls(cx),
            Tab::Limits => self.load_money(cx),
        }
    }

    pub(super) fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.menu = None;
        self.load_tab(cx);
        cx.notify();
    }

    pub(super) fn set_range(&mut self, range: Range, cx: &mut Context<Self>) {
        if self.range == range {
            return;
        }
        self.range = range;
        self.generation += 1;
        self.now = Load::Loading;
        self.before = Load::Loading;
        self.now_other = Load::Loading;
        self.before_other = Load::Loading;
        self.breakdown = Load::Loading;
        self.open_teams.clear();
        self.adoption = Load::Loading;
        self.cursors.clear();
        self.calls = Load::Loading;
        self.load_tab(cx);
        cx.notify();
    }

    pub(super) fn team_query(by: By) -> Vec<(&'static str, &'static str)> {
        if by == By::Team {
            vec![("depth", "1")]
        } else {
            Vec::new()
        }
    }

    pub(super) fn load_overview(&mut self, cx: &mut Context<Self>) {
        let span = self.span();
        for by in [By::Team, By::Model] {
            for previous in [false, true] {
                let (from, to) = if previous {
                    (&span.previous_from, &span.previous_to)
                } else {
                    (&span.from, &span.to)
                };
                let path = Self::usage_path(by, &Self::team_query(by), from, to);
                // The answer goes to the chart's slots or the other grouping's, by what the chart shows when it lands.
                self.get(path, cx, move |this, a, _| {
                    let report = load_of(a, Report::parse);
                    let slot = match (by == this.chart_by, previous) {
                        (true, false) => &mut this.now,
                        (true, true) => &mut this.before,
                        (false, false) => &mut this.now_other,
                        (false, true) => &mut this.before_other,
                    };
                    *slot = report;
                });
            }
        }
        self.load_money(cx);
    }

    pub(super) fn set_chart_by(&mut self, by: By, cx: &mut Context<Self>) {
        if self.chart_by == by {
            return;
        }
        self.chart_by = by;
        // The other grouping's reports are the same reads, swapped.
        std::mem::swap(&mut self.now, &mut self.now_other);
        std::mem::swap(&mut self.before, &mut self.before_other);
        cx.notify();
    }

    /// Overview's report grouped by `by` (team or model), whichever the chart shows: (now, the range before).
    pub(super) fn reports_by(&self, by: By) -> (&Load<Report>, &Load<Report>) {
        if by == self.chart_by {
            (&self.now, &self.before)
        } else {
            (&self.now_other, &self.before_other)
        }
    }

    pub(super) fn load_breakdown(&mut self, cx: &mut Context<Self>) {
        let span = self.span();
        let by = self.by;
        let team_query: Vec<(&str, &str)> = match (by, &self.under) {
            (By::Team, Some(under)) => vec![("under", under.as_str())],
            (By::Team, None) => vec![("depth", "1")],
            _ => Vec::new(),
        };
        let path = Self::usage_path(by, &team_query, &span.from, &span.to);
        self.get(path, cx, |this, a, _| this.breakdown = load_of(a, Report::parse));
        if self.under.is_some() {
            self.whole = Some(Load::Loading);
            self.get(
                Self::usage_path(By::Team, &[("depth", "1")], &span.from, &span.to),
                cx,
                |this, a, _| this.whole = Some(load_of(a, Report::parse)),
            );
        } else {
            self.whole = None;
        }
    }

    pub(super) fn set_by(&mut self, by: By, under: Option<String>, cx: &mut Context<Self>) {
        self.by = by;
        self.under = under;
        self.open_teams.clear();
        self.breakdown = Load::Loading;
        self.load_breakdown(cx);
        cx.notify();
    }

    pub(super) fn toggle_team(&mut self, slug: String, cx: &mut Context<Self>) {
        if self.open_teams.remove(&slug).is_some() {
            cx.notify();
            return;
        }
        self.load_team_people(slug, cx);
        cx.notify();
    }

    pub(super) fn load_team_people(&mut self, slug: String, cx: &mut Context<Self>) {
        let span = self.span();
        self.open_teams.insert(slug.clone(), Load::Loading);
        let path = Self::usage_path(By::Person, &[("team", slug.as_str())], &span.from, &span.to);
        self.get(path, cx, move |this, a, _| {
            if this.open_teams.contains_key(&slug) {
                this.open_teams.insert(slug.clone(), load_of(a, Report::parse));
            }
        });
    }

    pub(super) fn load_adoption(&mut self, cx: &mut Context<Self>) {
        let span = self.span();
        let path = format!(
            "/v1/analytics/agents{}",
            query(&[("from", &span.from), ("to", &span.to)])
        );
        self.get(path, cx, |this, a, _| this.adoption = load_of(a, Adoption::parse));
    }

    /// The calls' from and to: the day filter, else the range.
    pub(super) fn call_range(&self) -> (String, String) {
        match &self.filters.day {
            Some(day) => (day.clone(), day.clone()),
            None => {
                let span = self.span();
                (span.from, span.to)
            }
        }
    }

    pub(super) fn load_calls(&mut self, cx: &mut Context<Self>) {
        let (from, to) = self.call_range();
        let f = &self.filters;
        let cursor = self.cursors.last().cloned().unwrap_or_default();
        let path = format!(
            "/v1/spend/calls{}",
            query(&[
                ("from", &from),
                ("to", &to),
                ("model", f.model.as_deref().unwrap_or("")),
                ("person", f.person.as_deref().unwrap_or("")),
                ("status", f.status.as_deref().unwrap_or("")),
                ("product", f.product.as_deref().unwrap_or("")),
                ("team", f.team.as_deref().unwrap_or("")),
                ("session", f.session.as_deref().unwrap_or("")),
                ("limit", "50"),
                ("cursor", &cursor),
            ])
        );
        let filters = self.filters.clone();
        let cursors = self.cursors.len();
        self.get(path, cx, move |this, a, _| {
            if this.filters == filters && this.cursors.len() == cursors {
                this.calls = load_of(a, CallPage::parse);
            }
        });
        // Names and choices for the filters: read once.
        if self.names.is_empty() {
            self.get("/v1/orgs/members".into(), cx, |this, a, _| {
                if let Ok(value) = a {
                    this.names = members_of(&value);
                }
            });
        }
        if self.agents.is_empty() {
            self.get("/v1/agents/sessions".into(), cx, |this, a, _| {
                if let Ok(value) = a {
                    this.agents = value
                        .get("sessions")
                        .and_then(Value::as_array)
                        .map(|list| {
                            list.iter()
                                .filter_map(|s| Some((text_of(s, "id")?, text_of(s, "agent")?)))
                                .collect()
                        })
                        .unwrap_or_default();
                }
            });
        }
        for by in [By::Model, By::Product] {
            self.get(Self::usage_path(by, &[], &from, &to), cx, move |this, a, _| {
                let options: Vec<(String, String)> = a
                    .ok()
                    .and_then(|value| Report::parse(&value))
                    .map(|report| report.groups.into_iter().map(|g| (g.key, g.label)).collect())
                    .unwrap_or_default();
                if by == By::Model {
                    this.model_options = options;
                } else {
                    this.product_options = options;
                }
            });
        }
    }

    pub(super) fn set_filters(&mut self, change: impl FnOnce(&mut CallFilters), cx: &mut Context<Self>) {
        change(&mut self.filters);
        self.menu = None;
        self.selected = None;
        self.detail = None;
        self.cursors.clear();
        self.calls = Load::Loading;
        self.load_calls(cx);
        cx.notify();
    }

    pub(super) fn page_calls(&mut self, older: Option<String>, cx: &mut Context<Self>) {
        match older {
            Some(cursor) => self.cursors.push(cursor),
            None => {
                self.cursors.pop();
            }
        }
        self.calls = Load::Loading;
        self.load_calls(cx);
        cx.notify();
    }

    /// Show one call: from the page when it is on it, else read it.
    pub(super) fn open_call(&mut self, id: String, cx: &mut Context<Self>) {
        self.tab = Tab::Calls;
        self.selected = Some(id.clone());
        self.all_fields = false;
        self.copied = false;
        let listed = match &self.calls {
            Load::Ready(page) => page.calls.iter().find(|call| call.id == id).cloned(),
            _ => None,
        };
        match listed {
            Some(call) => self.detail = Some(Load::Ready(call)),
            None => {
                self.detail = Some(Load::Loading);
                let wanted = id.clone();
                self.get(format!("/v1/spend/calls/{}", encode(&id)), cx, move |this, a, _| {
                    if this.selected.as_deref() == Some(wanted.as_str()) {
                        this.detail = Some(match a {
                            Err(error)
                                if error.to_lowercase().contains("unknown")
                                    || error.to_lowercase().contains("not found") =>
                            {
                                Load::Failed("No call has this id in this organization.".into())
                            }
                            other => load_of(other, Call::parse),
                        });
                    }
                });
            }
        }
        if self.pricing.is_empty() {
            self.get("/v1/models?status=all".into(), cx, |this, a, _| {
                if let Ok(value) = a {
                    this.pricing = pricing_of(&value);
                }
            });
        }
        if self.team_names.is_empty() {
            self.get("/v1/teams".into(), cx, |this, a, _| {
                if let Ok(value) = a {
                    this.team_names = value
                        .get("teams")
                        .and_then(Value::as_array)
                        .map(|list| {
                            list.iter()
                                .filter_map(|t| Some((text_of(t, "id")?, text_of(t, "name")?)))
                                .collect()
                        })
                        .unwrap_or_default();
                }
            });
        }
        cx.notify();
    }

    /* -------------------------------------------------------------- limits: changes */

    pub(super) fn start_edit(&mut self, edit: Edit, initial: Option<f64>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.edit_allowed(&edit) || self.mutating() {
            return;
        }
        self.editing = Some(edit);
        self.edit_error = None;
        self.confirm_clear = false;
        let text = initial.map(|value| format!("{value:.2}")).unwrap_or_default();
        // The amount opens ready for typing.
        self.amount.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub(super) fn save_amount(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.clone() else { return };
        if self.mutating() || !self.edit_allowed(&edit) {
            return;
        }
        let text = self.amount.read(cx).value().to_string();
        let Some(decimal) = parse_usd(&text) else {
            self.edit_error = Some("A dollar amount, like 8 or 12.50".into());
            cx.notify();
            return;
        };
        let (verb, path, body) = match &edit {
            Edit::Budget => (
                Verb::Post,
                "/v1/orgs/budget".to_string(),
                json!({ "budgetUsdDecimal": decimal }),
            ),
            Edit::Default => (
                Verb::Put,
                "/v1/budgets/person-default".to_string(),
                json!({ "monthlyUsdDecimal": decimal }),
            ),
            Edit::Person(id) => (
                Verb::Put,
                format!("/v1/people/{}/budget", encode(id)),
                json!({ "monthlyUsdDecimal": decimal }),
            ),
        };
        self.send_limit(verb, path, Some(body), cx);
    }

    /// Clear a limit: the default per person (`null`), or a person's own one so the default applies.
    pub(super) fn clear_limit(&mut self, cx: &mut Context<Self>) {
        let path = match &self.editing {
            Some(Edit::Default) => "/v1/budgets/person-default".to_string(),
            Some(Edit::Person(id)) => format!("/v1/people/{}/budget", encode(id)),
            _ => return,
        };
        self.send_limit(Verb::Put, path, Some(json!({ "monthlyUsd": null })), cx);
    }

    pub(super) fn send_limit(&mut self, verb: Verb, path: String, body: Option<Value>, cx: &mut Context<Self>) {
        if self.mutating() || !self.editing.as_ref().is_some_and(|edit| self.edit_allowed(edit)) {
            return;
        }
        self.money_epoch += 1;
        self.mutation.set(true);
        self.saving = true;
        self.edit_error = None;
        fetch(cx, self.ctx.gateway.request(verb, path, body), |this, answer, cx| {
            if !this.current() {
                return;
            }
            this.mutation.set(false);
            this.saving = false;
            this.confirm_clear = false;
            match answer {
                Ok(_) => {
                    this.editing = None;
                    this.load_money(cx);
                    // What a used-up budget held (a send, a refused answer) goes on.
                    crate::limit_stop::raised(cx);
                }
                Err(error) => {
                    this.edit_error = Some(refusal_text(&error).into());
                    // The write invalidated pending spend/budget reads, even when it failed.
                    this.load_money(cx);
                }
            }
        });
        cx.notify();
    }

    pub(super) fn set_manager(&mut self, user_id: String, on: bool, cx: &mut Context<Self>) {
        if self.mutating()
            || !self.current()
            || !matches!(&self.budgets, Load::Ready(b) if b.scope == "org" && b.can_grant && b.people.iter().any(|p| p.user_id == user_id && p.role == "admin"))
        {
            return;
        }
        self.money_epoch += 1;
        self.mutation.set(true);
        self.manager_busy = Some(user_id.clone());
        self.manager_error = None;
        let verb = if on { Verb::Put } else { Verb::Delete };
        let path = format!("/v1/budgets/limit-managers/{}", encode(&user_id));
        fetch(cx, self.ctx.gateway.request(verb, path, None), |this, answer, cx| {
            if !this.current() {
                return;
            }
            this.mutation.set(false);
            this.manager_busy = None;
            match answer {
                Ok(_) => this.load_money(cx),
                Err(error) => {
                    this.manager_error = Some(refusal_text(&error).into());
                    this.load_money(cx);
                }
            }
        });
        cx.notify();
    }

    pub(super) fn current(&self) -> bool {
        self.marker.is_some() && self.ctx.gateway.marker() == self.marker
    }

    pub(super) fn mutating(&self) -> bool {
        self.mutation.get()
    }

    pub(super) fn edit_allowed(&self, edit: &Edit) -> bool {
        if !self.current() {
            return false;
        }
        match edit {
            Edit::Budget => self.can("budgets.manage"),
            Edit::Default => matches!(&self.budgets, Load::Ready(b) if b.scope == "org" && b.can_grant),
            Edit::Person(id) => {
                matches!(&self.budgets, Load::Ready(b) if b.people.iter().any(|p| &p.user_id == id && b.can_edit(p)))
            }
        }
    }

    pub(super) fn can(&self, permission: &str) -> bool {
        self.current() && matches!(&self.me, Load::Ready(me) if me.can(permission))
    }

    /// Every team's cap (`usage_caps::read_all`).
    pub(super) fn load_caps(&mut self, cx: &mut Context<Self>) {
        if !self.can("budgets.manage") {
            return;
        }
        self.caps_epoch += 1;
        let epoch = self.caps_epoch;
        self.caps = Load::Loading;
        let read = super::usage_caps::read_all(self.ctx.gateway.clone());
        cx.spawn(async move |this, cx| {
            let reads = read.await;
            this.update(cx, |this, cx| {
                if !this.current() || this.caps_epoch != epoch {
                    return;
                }
                this.caps = match reads {
                    Ok(reads) => Load::Ready(reads),
                    Err(error) => Load::Failed(error.into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The tab's export in `format`: Overview and Breakdown the usage for the range, Calls the calls with the filters.
    pub(super) fn export_ask(&self, format: super::usage_export::Format) -> super::usage_export::Ask {
        use super::usage_export::{Ask, Family};
        let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
        let (family, filters) = if self.tab == Tab::Calls {
            let (from, to) = self.call_range();
            let f = &self.filters;
            let mut filters = vec![pair("from", &from), pair("to", &to)];
            for (key, value) in [
                ("team", &f.team),
                ("person", &f.person),
                ("model", &f.model),
                ("product", &f.product),
                ("status", &f.status),
                ("session", &f.session),
            ] {
                if let Some(value) = value.as_deref().filter(|value| !value.is_empty()) {
                    filters.push(pair(key, value));
                }
            }
            (Family::SpendCalls, filters)
        } else {
            let span = self.span();
            (Family::Usage, vec![pair("from", &span.from), pair("to", &span.to)])
        };
        Ask {
            family,
            format,
            filters,
            part: 1,
            cursor: None,
        }
    }

    /// Save an export where the person picks, then say what the file holds and offer its next part if it was cut.
    pub(super) fn export(&mut self, ask: super::usage_export::Ask, cx: &mut Context<Self>) {
        use super::download::Saved;
        use super::usage_export::{Held, failure, held, note};
        if self.exporting {
            return;
        }
        self.export_menu = false;
        self.exporting = true;
        self.exported = None;
        self.export_next = None;
        let request = self.ctx.gateway.text(ask.path());
        let name = ask.file_name(&self.ctx.org, &super::download::today_utc());
        super::download::save_text(cx, request, name, move |this: &mut Self, saved, _| {
            this.exporting = false;
            match saved {
                Saved::File { path, text } => {
                    let held = held(&ask, &text);
                    this.exported = Some(Ok(note(&super::download::file_name(&path), &ask, &held)));
                    if let Held::Cut(_, next) = held {
                        this.export_next = Some(next);
                    }
                }
                Saved::Cancelled => {}
                Saved::Failed(error) => this.exported = Some(Err(failure(&error))),
            }
        });
        cx.notify();
    }

    /// The Export control: a button that opens its two formats, then the last export's note.
    pub(super) fn render_export(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        use super::usage_export::Format;
        let button = export_button(self.export_menu, self.exporting, palette, Some(cx));
        h_flex()
            .gap(px(4.))
            .child(button)
            .when(self.export_menu && !self.exporting, |row| {
                row.children(Format::ALL.map(|format| {
                    ui::chip(
                        SharedString::from(format!("usage-export-{}", format.ext())),
                        format.label(),
                        false,
                        palette,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let ask = this.export_ask(format);
                        this.export(ask, cx);
                    }))
                }))
            })
    }

    /// The last export's note under the head, with "Download part N" when the file was cut.
    pub(super) fn render_exported(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let outcome = self.exported.clone()?;
        let line = match outcome {
            Ok(note) => ui::note(note, palette),
            Err(error) => ui::error_line(error, palette),
        };
        Some(
            h_flex()
                .gap(px(8.))
                .child(line)
                .when_some(self.export_next.clone(), |row, next| {
                    row.child(
                        ui::web_link("usage-export-next", format!("Download part {}", next.part), palette)
                            .on_click(cx.listener(move |this, _, _, cx| this.export(next.clone(), cx))),
                    )
                }),
        )
    }
}

pub(super) fn usage_tab(
    tab: Tab,
    label: &'static str,
    selected: bool,
    palette: Palette,
    cx: Option<&mut Context<UsagePage>>,
) -> Stateful<Div> {
    let id = SharedString::from(format!("usage-tab-{label}"));
    match cx {
        Some(cx) => {
            parts::tab(id, label, selected, palette).on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
        }
        None => ui::disabled_button(id, label, palette),
    }
}

pub(super) fn export_button(
    menu_open: bool,
    exporting: bool,
    palette: Palette,
    cx: Option<&mut Context<UsagePage>>,
) -> Stateful<Div> {
    let label = if exporting {
        "Exporting…"
    } else if menu_open {
        "Export ▴"
    } else {
        "Export ▾"
    };
    match cx.filter(|_| !exporting) {
        Some(cx) => ui::ghost_button("usage-export", label, palette).on_click(cx.listener(|page, _, _, cx| {
            page.export_menu = !page.export_menu;
            cx.notify();
        })),
        None => ui::disabled_button("usage-export", label, palette),
    }
}

impl Render for UsagePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        if !self.current() {
            return v_flex()
                .child(ui::note(
                    "The signed-in account changed. Reopen Usage to continue.",
                    palette,
                ))
                .into_any_element();
        }
        // Limits is a tab, not one control: the Budgets target opens it with no outline.
        if self.ctx.settings_fields.take_place("usage-limits") && self.tab != Tab::Limits {
            self.set_tab(Tab::Limits, cx);
        }
        // A setup line's Raise limit opens on a row of Limits: the tab first, then the row draws and takes the arrival.
        if self.tab != Tab::Limits
            && self
                .ctx
                .settings_fields
                .pending()
                .is_some_and(crate::limit_stop::is_limit_row)
        {
            self.set_tab(Tab::Limits, cx);
        }
        let (outer, column) = parts::frame(palette);
        let head = self.render_head(palette, cx);
        let tabs = parts::tab_row(palette).children(
            TABS.iter()
                .map(|(tab, label)| usage_tab(*tab, label, self.tab == *tab, palette, Some(cx))),
        );
        let exported = self.render_exported(palette, cx);
        let body: AnyElement = match self.tab {
            Tab::Overview => self.render_overview(palette, cx).into_any_element(),
            Tab::Breakdown => self.render_breakdown(palette, cx).into_any_element(),
            Tab::Adoption => self.render_adoption(palette).into_any_element(),
            Tab::Calls => self.render_calls(palette, cx).into_any_element(),
            Tab::Limits => self.render_limits(palette, cx).into_any_element(),
        };
        outer
            .track_scroll(&self.ctx.settings_fields.scroll())
            .child(
                column
                    .child(head)
                    .child(tabs)
                    .children(exported.map(|line| line.mb(px(12.))))
                    .child(body),
            )
            .into_any_element()
    }
}
