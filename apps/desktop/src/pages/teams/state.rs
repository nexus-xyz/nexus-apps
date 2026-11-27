use super::*;

pub(super) struct TeamsPage {
    pub(super) ctx: Ctx,
    pub(super) me: Load<Me>,
    pub(super) tree: Load<Vec<TreeRow>>,
    /// The tree as bands and cards, made from `tree` and `folded` whenever either changes.
    pub(super) blocks: Vec<Block>,
    /// The teams folded shut in the tree, by slug.
    pub(super) folded: HashSet<String>,
    /// Owners and admins only.
    pub(super) pending: Option<Load<Vec<PendingEntry>>>,
    pub(super) outcome: Option<Outcome>,
    pub(super) creating: bool,
    pub(super) create: CreateForm,
    pub(super) create_outcome: Option<Outcome>,
    /// The team open, by slug; `None` shows the tree.
    pub(super) open: Option<String>,
    pub(super) team: Load<Detail>,
    pub(super) team_outcome: Option<Outcome>,
    /// The organization's members (user id, email), to add one by email.
    pub(super) org_members: Option<Vec<(String, String)>>,
    pub(super) add_email: Entity<InputState>,
    pub(super) add_lead: bool,
    pub(super) add_home: bool,
    pub(super) add_outcome: Option<Outcome>,
    pub(super) edit: EditForm,
    pub(super) edit_outcome: Option<Outcome>,
    pub(super) confirm: Option<Confirm>,
    /// A change is on its way: buttons wait for it.
    pub(super) working: bool,
    /// The open team's spend: None while unread or when the person may not read it (a 403).
    pub(super) spend: Option<TeamSpend>,
    /// The open team's Models section, and the team it was made for.
    pub(super) policy: Option<(String, Entity<super::policy::PolicyPage>)>,
    pub(super) _subscriptions: Vec<Subscription>,
}

impl TeamsPage {
    pub(super) fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> TeamsPage {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let create = CreateForm {
            name: input("Platform", window, cx),
            slug: input("Made from the name", window, cx),
            level: input("Optional, e.g. team", window, cx),
            description: input("What the team works on", window, cx),
            parent: None,
        };
        let edit = EditForm {
            name: input("Name", window, cx),
            slug: input("Slug", window, cx),
            level: input("e.g. department", window, cx),
            description: input("What the team works on", window, cx),
            parent: None,
        };
        let add_email = input("name@company.com", window, cx);
        let mut subscriptions = Vec::new();
        for field in [&create.name, &create.slug, &create.level, &create.description] {
            subscriptions.push(
                cx.subscribe_in(field, window, |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.create_team(window, cx);
                    }
                }),
            );
        }
        for field in [&edit.name, &edit.slug, &edit.level, &edit.description] {
            subscriptions.push(
                cx.subscribe_in(field, window, |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.save_team(window, cx);
                    }
                }),
            );
        }
        subscriptions.push(
            cx.subscribe_in(&add_email, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_member(window, cx);
                }
            }),
        );
        let mut page = TeamsPage {
            ctx,
            me: Load::Loading,
            tree: Load::Loading,
            blocks: Vec::new(),
            folded: HashSet::new(),
            pending: None,
            outcome: None,
            creating: false,
            create,
            create_outcome: None,
            open: None,
            team: Load::Loading,
            team_outcome: None,
            org_members: None,
            add_email,
            add_lead: false,
            add_home: false,
            add_outcome: None,
            edit,
            edit_outcome: None,
            confirm: None,
            working: false,
            spend: None,
            policy: None,
            _subscriptions: subscriptions,
        };
        page.load(cx);
        page
    }

    pub(super) fn manage(&self) -> bool {
        matches!(&self.me, Load::Ready(me) if me.manage)
    }

    pub(super) fn load(&mut self, cx: &mut Context<Self>) {
        self.me = Load::Loading;
        let me = self.ctx.gateway.query("/v1/tenants/me".into());
        super::read(cx, me, |this, answer, cx| match answer {
            Ok(value) => {
                let me = parse_me(&value);
                if me.manage && !me.personal {
                    this.load_pending(cx);
                }
                this.me = Load::Ready(me);
            }
            Err(error) => this.me = Load::Failed(error.into()),
        });
        self.load_tree(cx);
    }

    pub(super) fn load_tree(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.tree, Load::Ready(_)) {
            self.tree = Load::Loading;
        }
        let request = self.ctx.gateway.query("/v1/teams?tree=1".into());
        super::read(cx, request, |this, answer, _| {
            this.tree = match answer {
                Ok(value) => Load::Ready(parse_tree(&value)),
                Err(error) => Load::Failed(error.into()),
            };
            this.relayout();
        });
    }

    /// Lay the tree out again, after it loads or a team folds.
    pub(super) fn relayout(&mut self) {
        self.blocks = match &self.tree {
            Load::Ready(rows) => layout(rows, &self.folded),
            _ => Vec::new(),
        };
    }

    /// Fold a team shut, or open it.
    pub(super) fn toggle_fold(&mut self, slug: String, cx: &mut Context<Self>) {
        if !self.folded.remove(&slug) {
            self.folded.insert(slug);
        }
        self.relayout();
        cx.notify();
    }

    pub(super) fn load_pending(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_none() {
            self.pending = Some(Load::Loading);
        }
        let request = self.ctx.gateway.query("/v1/teams/pending".into());
        super::read(cx, request, |this, answer, _| {
            this.pending = Some(match answer {
                Ok(value) => Load::Ready(parse_pending(&value)),
                Err(error) => Load::Failed(error.into()),
            });
        });
    }

    /// The tree and the pending list, after a change.
    pub(super) fn reload_lists(&mut self, cx: &mut Context<Self>) {
        self.load_tree(cx);
        if self.manage() {
            self.load_pending(cx);
        }
    }

    pub(super) fn open_team(&mut self, slug: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open = Some(slug.clone());
        self.team = Load::Loading;
        self.confirm = None;
        self.team_outcome = None;
        self.add_outcome = None;
        self.edit_outcome = None;
        self.spend = None;
        let request = self
            .ctx
            .gateway
            .query(format!("/v1/analytics/usage?team={}&groupBy=person", encode(&slug)));
        let open = slug.clone();
        super::read(cx, request, move |this, answer, _| {
            if this.open.as_deref() == Some(open.as_str()) {
                // A refusal or a failure reads as nothing to show, as on the web.
                this.spend = answer.ok().map(|value| parse_spend(&value));
            }
        });
        self.load_team(slug, true, window, cx);
        if self.manage() && self.org_members.is_none() {
            let request = self.ctx.gateway.query("/v1/orgs/members".into());
            super::read(cx, request, |this, answer, _| {
                if let Ok(value) = answer {
                    this.org_members = Some(parse_org_members(&value));
                }
            });
        }
        cx.notify();
    }

    /// Read the open team; `fill` puts its values in the Settings fields.
    pub(super) fn load_team(&mut self, slug: String, fill: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut fields = self.edit.values(cx);
        let request = self.ctx.gateway.query(format!("/v1/teams/{}", encode(&slug)));
        super::read_in(window, cx, request, move |this, answer, window, cx| {
            if this.open.as_deref() != Some(slug.as_str()) {
                return;
            }
            match answer {
                Ok(value) => {
                    let detail = parse_detail(&value);
                    if fill && this.edit.values(cx) == fields {
                        let values = [
                            (&this.edit.name, detail.name.clone()),
                            (&this.edit.slug, detail.slug.clone()),
                            (&this.edit.level, detail.level.clone()),
                            (&this.edit.description, detail.description.clone()),
                        ];
                        for (field, text) in values {
                            field.update(cx, |state, cx| state.set_value(text, window, cx));
                        }
                        this.edit.parent = (!detail.parent.is_empty()).then(|| detail.parent.clone());
                        fields = this.edit.values(cx);
                    }
                    // The Models section, made once per team opened.
                    if this.policy.as_ref().is_none_or(|(slug, _)| *slug != detail.slug) {
                        let scope = super::policy::TeamScope {
                            slug: detail.slug.clone(),
                            name: detail.name.clone(),
                            id: detail.id.clone(),
                            path: detail.path.clone(),
                            members: detail
                                .member_list
                                .iter()
                                .map(|member| (member.user_id.clone(), member.who()))
                                .collect(),
                        };
                        let view = super::policy::team_view(this.ctx.clone(), scope, window, cx);
                        this.policy = Some((detail.slug.clone(), view));
                    }
                    this.team = Load::Ready(detail);
                }
                Err(error) => this.team = Load::Failed(error.into()),
            }
        });
    }

    pub(super) fn back(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.confirm = None;
        self.reload_lists(cx);
        cx.notify();
    }

    pub(super) fn create_team(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        let name = self.create.name.read(cx).value().trim().to_string();
        let typed_slug = self.create.slug.read(cx).value().trim().to_string();
        let slug = if typed_slug.is_empty() {
            slug_from(&name)
        } else {
            typed_slug
        };
        let level = self.create.level.read(cx).value().trim().to_lowercase();
        let description = self.create.description.read(cx).value().trim().to_string();
        if name.is_empty() && slug.is_empty() {
            self.create_outcome = Outcome::invalid("Name the team.");
            cx.notify();
            return;
        }
        if !valid_label(&slug, 127) {
            self.create_outcome = Outcome::invalid(
                "The slug takes lower-case letters, digits and dashes, starting with a letter or digit.",
            );
            cx.notify();
            return;
        }
        if !level.is_empty() && !valid_label(&level, 32) {
            self.create_outcome =
                Outcome::invalid("A level is a short label: up to 32 lower-case letters, digits and dashes.");
            cx.notify();
            return;
        }
        let mut body = json!({ "slug": slug, "name": if name.is_empty() { slug.clone() } else { name } });
        if !description.is_empty() {
            body["description"] = json!(description);
        }
        if let Some(parent) = &self.create.parent {
            body["parent"] = json!(parent);
        }
        if !level.is_empty() {
            body["level"] = json!(level);
        }
        self.working = true;
        self.create_outcome = None;
        let request = self.ctx.gateway.request(Verb::Post, "/v1/teams".into(), Some(body));
        fetch_in(window, cx, request, |this, answer, window, cx| {
            this.working = false;
            match answer {
                Ok(value) => {
                    let name = name_or_slug(&value);
                    this.outcome = Outcome::done(format!("Created {name}. Add its leads and members from its page."));
                    this.creating = false;
                    let fields = [
                        this.create.name.clone(),
                        this.create.slug.clone(),
                        this.create.level.clone(),
                        this.create.description.clone(),
                    ];
                    for field in fields {
                        field.update(cx, |state, cx| state.set_value("", window, cx));
                    }
                    this.create.parent = None;
                    this.reload_lists(cx);
                }
                Err(error) => this.create_outcome = Outcome::failed("The team wasn't created", &error),
            }
        });
        cx.notify();
    }

    /// Cancel someone pending in the tree's Pending list.
    pub(super) fn cancel_pending(&mut self, query: String, done: String, failed: String, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        self.working = true;
        self.confirm = None;
        let request = self
            .ctx
            .gateway
            .request(Verb::Delete, format!("/v1/teams/pending?{query}"), None);
        super::fetch(cx, request, move |this, answer, cx| {
            this.working = false;
            this.outcome = match answer {
                Ok(_) => {
                    this.reload_lists(cx);
                    Outcome::done(done)
                }
                Err(error) => Outcome::failed(&failed, &error),
            };
        });
        cx.notify();
    }

    /// Run a change on the open team, then read it again.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn change_team(
        &mut self,
        verb: Verb,
        path: String,
        body: Option<Value>,
        done: String,
        failed: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.working {
            return;
        }
        self.working = true;
        self.confirm = None;
        self.team_outcome = None;
        let request = self.ctx.gateway.request(verb, path, body);
        fetch_in(window, cx, request, move |this, answer, window, cx| {
            this.working = false;
            match answer {
                Ok(_) => {
                    this.team_outcome = Outcome::done(done);
                    if let Some(slug) = this.open.clone() {
                        this.load_team(slug, false, window, cx);
                    }
                }
                Err(error) => this.team_outcome = Outcome::failed(&failed, &error),
            }
        });
        cx.notify();
    }

    pub(super) fn add_member(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(team) = &self.team else {
            return;
        };
        if self.working {
            return;
        }
        let typed = self.add_email.read(cx).value().trim().to_lowercase();
        if typed.is_empty() {
            return;
        }
        if !typed.contains('@') {
            self.add_outcome = Outcome::invalid("Enter the person's email.");
            cx.notify();
            return;
        }
        let known = self
            .org_members
            .as_ref()
            .and_then(|list| list.iter().find(|(_, email)| *email == typed))
            .map(|(user_id, _)| user_id.clone());
        let mut body = match known {
            Some(user_id) => json!({ "userId": user_id }),
            None => json!({ "email": typed }),
        };
        body["role"] = json!(if self.add_lead { "lead" } else { "member" });
        if self.add_home {
            body["home"] = json!(true);
        }
        let slug = team.slug.clone();
        let name = team.name.clone();
        self.working = true;
        self.add_outcome = None;
        let request = self
            .ctx
            .gateway
            .request(Verb::Post, format!("/v1/teams/{}/members", encode(&slug)), Some(body));
        fetch_in(window, cx, request, move |this, answer, window, cx| {
            this.working = false;
            match answer {
                Ok(value) => {
                    this.add_outcome = Outcome::done(added_text(&value, &name));
                    this.add_email.update(cx, |state, cx| state.set_value("", window, cx));
                    this.add_home = false;
                    this.load_team(slug, false, window, cx);
                }
                Err(error) => this.add_outcome = Outcome::failed("They weren't added", &error),
            }
        });
        cx.notify();
    }

    pub(super) fn save_team(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(team) = &self.team else {
            return;
        };
        if self.working || !self.manage() {
            return;
        }
        let name = self.edit.name.read(cx).value().trim().to_string();
        let slug = self.edit.slug.read(cx).value().trim().to_string();
        let description = self.edit.description.read(cx).value().trim().to_string();
        let level = self.edit.level.read(cx).value().trim().to_lowercase();
        let parent = self.edit.parent.clone().unwrap_or_default();
        let mut changes = serde_json::Map::new();
        if !name.is_empty() && name != team.name {
            changes.insert("name".into(), json!(name));
        }
        if slug != team.slug {
            if !valid_label(&slug, 127) {
                self.edit_outcome = Outcome::invalid(
                    "The slug takes lower-case letters, digits and dashes, starting with a letter or digit.",
                );
                cx.notify();
                return;
            }
            changes.insert("slug".into(), json!(slug));
        }
        if description != team.description {
            changes.insert(
                "description".into(),
                if description.is_empty() {
                    Value::Null
                } else {
                    json!(description)
                },
            );
        }
        if level != team.level {
            if !level.is_empty() && !valid_label(&level, 32) {
                self.edit_outcome =
                    Outcome::invalid("A level is a short label: up to 32 lower-case letters, digits and dashes.");
                cx.notify();
                return;
            }
            changes.insert(
                "level".into(),
                if level.is_empty() { Value::Null } else { json!(level) },
            );
        }
        if parent != team.parent {
            changes.insert(
                "parent".into(),
                if parent.is_empty() { Value::Null } else { json!(parent) },
            );
        }
        if changes.is_empty() {
            self.edit_outcome = Outcome::done("Nothing to save.");
            cx.notify();
            return;
        }
        self.working = true;
        self.edit_outcome = None;
        let request = self.ctx.gateway.request(
            Verb::Patch,
            format!("/v1/teams/{}", encode(&team.slug)),
            Some(Value::Object(changes)),
        );
        fetch_in(window, cx, request, |this, answer, window, cx| {
            this.working = false;
            match answer {
                Ok(value) => {
                    this.edit_outcome = Outcome::done("Saved.");
                    let slug = ui::text(&value, "slug").to_string();
                    if let Some(slug) = (!slug.is_empty()).then_some(slug).or(this.open.clone()) {
                        this.open = Some(slug.clone());
                        this.load_team(slug, true, window, cx);
                    }
                    this.load_tree(cx);
                }
                Err(error) => this.edit_outcome = Outcome::failed("The team wasn't saved", &error),
            }
        });
        cx.notify();
    }

    pub(super) fn delete_team(&mut self, cx: &mut Context<Self>) {
        let Load::Ready(team) = &self.team else {
            return;
        };
        if self.working {
            return;
        }
        self.working = true;
        self.confirm = None;
        let cascade = if team.children.is_empty() { "" } else { "?cascade=1" };
        let name = team.name.clone();
        let request =
            self.ctx
                .gateway
                .request(Verb::Delete, format!("/v1/teams/{}{cascade}", encode(&team.slug)), None);
        super::fetch(cx, request, move |this, answer, cx| {
            this.working = false;
            match answer {
                Ok(_) => {
                    this.outcome = Outcome::done(format!("Deleted {name}. Past spend keeps its name in Usage."));
                    this.back(cx);
                }
                Err(error) => this.edit_outcome = Outcome::failed("The team wasn't deleted", &error),
            }
        });
        cx.notify();
    }
}
