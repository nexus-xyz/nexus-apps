use super::*;

// ------------------------------------------------------------------ The page

/// Keep the last successful snapshot after a refresh failure. An initial failure remains an error.
pub(super) fn refreshed<T>(
    current: &mut Load<T>,
    answer: Result<T, String>,
    errors: &mut HashMap<&'static str, String>,
    part: &'static str,
) {
    match answer {
        Ok(value) => {
            *current = Load::Ready(value);
            errors.remove(part);
        }
        Err(error) => {
            errors.insert(part, error.clone());
            if !matches!(current, Load::Ready(_)) {
                *current = Load::Failed(error.into());
            }
        }
    }
}

pub(super) struct ConnectorsPage {
    pub(super) refresh_errors: HashMap<&'static str, String>,
    pub(super) ctx: Ctx,
    /// Whose settings this page holds.
    pub(super) side: Side,
    /// What each opened connector lets agents do, read when its row first opens.
    pub(super) abilities: HashMap<String, Load<Vec<Ability>>>,
    /// The connector whose abilities list is unfolded.
    pub(super) abilities_open: Option<String>,
    pub(super) me: Load<Rights>,
    pub(super) catalog: Load<Vec<Connector>>,
    pub(super) connections: Load<Vec<Connection>>,
    /// Owners and admins only: `None` for anyone else.
    pub(super) availability: Option<Load<Vec<Availability>>>,
    pub(super) chat: Option<Load<bool>>,
    /// The open row's connector.
    pub(super) open: Option<String>,
    /// A key form's inputs (`<connector>|<scope>|<field>`) and the teams inputs (`<connector>|teams`).
    pub(super) inputs: HashMap<String, Entity<InputState>>,
    /// The action in flight, by its key.
    pub(super) busy: Option<String>,
    /// The account a first Disconnect click picked: (connector, account, scope).
    pub(super) confirm: Option<(String, String, String)>,
    /// The last action's result, by connector (`""` for chat writes).
    pub(super) notice: Option<(String, Result<String, String>)>,
    /// The filter over the cards: a connector's name or line.
    pub(super) filter: Entity<InputState>,
    pub(super) _filter_changes: Subscription,
    /// The folded groups a person opened, by heading.
    pub(super) unfolded: Vec<&'static str>,
    /// The connector whose key form a person opened under its OAuth button (`Use an API key instead`).
    pub(super) key_form: Option<String>,
    /// After an OAuth start: re-reads the connections until the new account shows, so no Refresh is needed.
    pub(super) _oauth_poll: Task<()>,
}

impl ConnectorsPage {
    pub(super) fn load(&mut self, cx: &mut Context<Self>) {
        let gateway = self.ctx.gateway.clone();
        super::read(
            cx,
            gateway.query("/v1/tenants/me".into()),
            |this, answer, cx| match answer {
                Ok(me) => {
                    let rights = parse_rights(&me);
                    refreshed(&mut this.me, Ok(rights), &mut this.refresh_errors, "access");
                    this.load_rest(rights, cx);
                }
                Err(error) => refreshed(&mut this.me, Err(plain(&error)), &mut this.refresh_errors, "access"),
            },
        );
    }

    pub(super) fn load_rest(&mut self, rights: Rights, cx: &mut Context<Self>) {
        let gateway = self.ctx.gateway.clone();
        super::read(cx, gateway.query("/v1/connectors".into()), |this, answer, _| {
            refreshed(
                &mut this.catalog,
                answer
                    .map(|answer| parse_connectors(&answer))
                    .map_err(|error| plain(&error)),
                &mut this.refresh_errors,
                "catalog",
            );
        });
        if rights.own {
            self.load_connections(cx);
        } else {
            self.connections = Load::Ready(Vec::new());
        }
        // Only the organization's page shows who can use each connector and chat's switch.
        if rights.manage && self.side == Side::Org {
            self.availability.get_or_insert(Load::Loading);
            self.chat.get_or_insert(Load::Loading);
            super::read(
                cx,
                gateway.query("/v1/policies/connectors".into()),
                |this, answer, _| {
                    refreshed(
                        this.availability.get_or_insert(Load::Loading),
                        answer
                            .map(|answer| parse_availability(&answer))
                            .map_err(|error| plain(&error)),
                        &mut this.refresh_errors,
                        "availability",
                    );
                },
            );
            super::read(
                cx,
                gateway.query("/v1/policies/chat-writes".into()),
                |this, answer, _| {
                    refreshed(
                        this.chat.get_or_insert(Load::Loading),
                        answer.map_err(|error| plain(&error)).and_then(|answer| {
                            parse_chat(&answer).ok_or_else(|| "The chat-write setting did not read.".into())
                        }),
                        &mut this.refresh_errors,
                        "chat writes",
                    );
                },
            );
        } else {
            self.availability = None;
            self.chat = None;
        }
    }

    pub(super) fn load_connections(&mut self, cx: &mut Context<Self>) {
        let gateway = self.ctx.gateway.clone();
        super::read(cx, gateway.query("/v1/connections".into()), |this, answer, _| {
            refreshed(
                &mut this.connections,
                answer
                    .map(|answer| parse_connections(&answer))
                    .map_err(|error| plain(&error)),
                &mut this.refresh_errors,
                "connections",
            );
        });
    }

    /// Open or close a row. Opening makes the inputs its forms need.
    pub(super) fn toggle(&mut self, card: &Card, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.as_deref() == Some(card.connector.name.as_str()) {
            self.open = None;
            cx.notify();
            return;
        }
        self.open = Some(card.connector.name.clone());
        self.confirm = None;
        let name = &card.connector.name;
        if !card.connector.operations.is_empty() && !self.abilities.contains_key(name) {
            self.load_abilities(name.clone(), cx);
        }
        if let Some(availability) = &card.availability {
            let key = format!("{name}|teams");
            if !self.inputs.contains_key(&key) {
                let teams = availability.teams.join(", ");
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("Teams, comma-separated (empty: everyone)")
                        .default_value(teams)
                });
                self.inputs.insert(key, input);
            }
        }
        for scope in ["user", "org"] {
            for field in &card.connector.fields {
                let key = format!("{name}|{scope}|{}", field.name);
                if self.inputs.contains_key(&key) {
                    continue;
                }
                let placeholder = field.placeholder.clone().unwrap_or_default();
                let secret = field.secret;
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).masked(secret));
                self.inputs.insert(key, input);
            }
        }
        cx.notify();
    }

    /// Read what a connector lets agents do: the list's operations carry no effect, so Read and Change need its detail.
    fn load_abilities(&mut self, name: String, cx: &mut Context<Self>) {
        self.abilities.insert(name.clone(), Load::Loading);
        let path = format!("/v1/connectors/{}", segment(&name));
        super::read(cx, self.ctx.gateway.query(path), move |this, answer, _| {
            match answer {
                Ok(answer) => {
                    this.abilities
                        .insert(name.clone(), Load::Ready(parse_abilities(&answer)));
                }
                // A saved read stays when its refresh fails.
                Err(error) => {
                    if !matches!(this.abilities.get(&name), Some(Load::Ready(_))) {
                        this.abilities.insert(name.clone(), Load::Failed(plain(&error).into()));
                    }
                }
            }
        });
    }

    /// Fold or unfold a connector's list of abilities.
    pub(super) fn toggle_abilities(&mut self, name: &str, cx: &mut Context<Self>) {
        self.abilities_open = match self.abilities_open.as_deref() == Some(name) {
            true => None,
            false => Some(name.to_string()),
        };
        cx.notify();
    }

    pub(super) fn input_text(&self, key: &str, cx: &App) -> String {
        self.inputs
            .get(key)
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default()
    }

    pub(super) fn done(&mut self, connector: &str, result: Result<String, String>) {
        self.busy = None;
        self.notice = Some((connector.to_string(), result.map_err(|error| plain(&error))));
    }

    /// Owners and admins: who may use a connector, and for which teams.
    pub(super) fn set_availability(&mut self, connector: String, level: &'static str, cx: &mut Context<Self>) {
        if self.busy.is_some() || !self.refresh_errors.is_empty() {
            return;
        }
        let teams = team_list(&self.input_text(&format!("{connector}|teams"), cx));
        let mut body = json!({ "availability": level });
        if !teams.is_empty() {
            body["teams"] = json!(teams);
        }
        self.busy = Some(format!("{connector}|availability"));
        let path = format!("/v1/policies/connectors/{}", segment(&connector));
        let request = self.ctx.gateway.request(Verb::Put, path, Some(body));
        fetch(cx, request, move |this, answer, cx| {
            let saved = answer.and_then(|answer| {
                parse_one_availability(&answer).ok_or_else(|| "Gateway's answer did not read.".to_string())
            });
            match saved {
                Ok(saved) => {
                    let line = format!("{}: {}.", connector_name(&connector), availability_status(&saved).label);
                    if let Some(Load::Ready(list)) = &mut this.availability {
                        match list.iter_mut().find(|item| item.name == saved.name) {
                            Some(item) => *item = saved,
                            None => list.push(saved),
                        }
                    }
                    this.done(&connector, Ok(line));
                    // Policy changes also change the catalog's per-person `usable` result.
                    this.load(cx);
                }
                Err(error) => this.done(&connector, Err(error)),
            }
        });
        cx.notify();
    }

    /// Owners and admins: chat's master switch. The allowlist is left as it is.
    pub(super) fn set_chat(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.busy.is_some() || !self.refresh_errors.is_empty() {
            return;
        }
        self.busy = Some("chat".into());
        let request = self.ctx.gateway.request(
            Verb::Put,
            "/v1/policies/chat-writes".into(),
            Some(json!({ "enabled": enabled })),
        );
        fetch(cx, request, move |this, answer, _| match answer {
            Ok(answer) => {
                let now = parse_chat(&answer).unwrap_or(enabled);
                this.chat = Some(Load::Ready(now));
                let line = if now {
                    "Chat writes are on, for the operations each connector allows."
                } else {
                    "Chat writes are off."
                };
                this.done("", Ok(line.into()));
            }
            Err(error) => this.done("", Err(error)),
        });
        cx.notify();
    }

    /// Connect an account with a key form.
    pub(super) fn connect(
        &mut self,
        connector: &Connector,
        scope: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() || !self.refresh_errors.is_empty() {
            return;
        }
        let mut body = Map::new();
        for field in &connector.fields {
            let key = format!("{}|{scope}|{}", connector.name, field.name);
            let value = self.input_text(&key, cx);
            let value = if field.secret { value } else { value.trim().to_string() };
            if value.is_empty() {
                if field.required {
                    self.notice = Some((connector.name.clone(), Err(format!("Fill in {}.", field.label))));
                    cx.notify();
                    return;
                }
                continue;
            }
            body.insert(field.name.clone(), Value::String(value));
        }
        body.insert("scope".into(), json!(scope));
        // The typed secrets leave the page as soon as they are sent.
        for field in connector.fields.iter().filter(|field| field.secret) {
            if let Some(input) = self.inputs.get(&format!("{}|{scope}|{}", connector.name, field.name)) {
                input.update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        let name = connector.name.clone();
        self.busy = Some(format!("{name}|{scope}|connect"));
        let path = format!("/v1/connections/{}", segment(&name));
        let request = self.ctx.gateway.request(Verb::Post, path, Some(Value::Object(body)));
        fetch(cx, request, move |this, answer, cx| match answer {
            Ok(answer) => {
                let accounts: Vec<String> = parse_connections(&answer)
                    .iter()
                    .map(|item| match &item.identity {
                        Some(identity) => format!("{} as {identity}", item.name),
                        None => item.name.clone(),
                    })
                    .collect();
                this.done(
                    &name,
                    Ok(format!("Connected {}: {}.", connector_name(&name), accounts.join(", "))),
                );
                this.load_connections(cx);
            }
            Err(error) => this.done(&name, Err(error)),
        });
        cx.notify();
    }

    /// Connect with OAuth: the vendor's consent page opens in the browser, and Nexus UI finishes it.
    pub(super) fn start_oauth(&mut self, connector: String, scope: &'static str, cx: &mut Context<Self>) {
        if self.busy.is_some() || !self.refresh_errors.is_empty() {
            return;
        }
        self.busy = Some(format!("{connector}|{scope}|connect"));
        let request = super::oauth_request(&self.ctx.gateway, &connector, scope);
        fetch(cx, request, move |this, answer, cx| {
            match super::authorize_url(answer) {
                Ok(url) => {
                    crate::external_link::open(&url, cx);
                    let name = connector_name(&connector);
                    this.done(
                        &connector,
                        Ok(format!(
                            "Approve it at {name} in your browser. The account shows here when you're done."
                        )),
                    );
                    this.watch_oauth(connector.clone(), cx);
                }
                Err(error) => this.done(&connector, Err(error)),
            }
        });
        cx.notify();
    }

    /// How many accounts the last read of the connections lists for a connector.
    fn accounts_of(&self, connector: &str) -> usize {
        match &self.connections {
            Load::Ready(items) => items.iter().filter(|item| item.connector == connector).count(),
            _ => 0,
        }
    }

    /// Re-read the connections every few seconds, for a few minutes, until the connector has one more account than it
    /// had when the browser opened. Nexus UI finishes the OAuth handshake, so the app only sees its result.
    fn watch_oauth(&mut self, connector: String, cx: &mut Context<Self>) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(3);
        const TRIES: usize = 100;
        let before = self.accounts_of(&connector);
        self._oauth_poll = cx.spawn(async move |this, cx| {
            for _ in 0..TRIES {
                cx.background_executor().timer(EVERY).await;
                let landed = this.update(cx, |this, cx| {
                    let landed = this.accounts_of(&connector) > before;
                    if !landed {
                        this.load_connections(cx);
                    }
                    landed
                });
                if !matches!(landed, Ok(false)) {
                    break;
                }
            }
        });
    }

    /// Disconnect an account: the first click asks, the second does it.
    pub(super) fn disconnect(&mut self, item: &Connection, cx: &mut Context<Self>) {
        if self.busy.is_some() || !self.refresh_errors.is_empty() {
            return;
        }
        let picked = (item.connector.clone(), item.account.clone(), item.scope.clone());
        if self.confirm.as_ref() != Some(&picked) {
            self.confirm = Some(picked);
            cx.notify();
            return;
        }
        self.confirm = None;
        let connector = item.connector.clone();
        let account_name = item.name.clone();
        self.busy = Some(format!("{connector}|{}|disconnect", item.account));
        let mut path = format!("/v1/connections/{}/{}", segment(&connector), segment(&item.account));
        if item.scope == "org" {
            path.push_str("?scope=org");
        }
        let request = self.ctx.gateway.request(Verb::Delete, path, None);
        fetch(cx, request, move |this, answer, cx| match answer {
            Ok(answer) => {
                let gone = answer
                    .get("disconnected")
                    .and_then(|item| string(item, "name"))
                    .unwrap_or(account_name);
                this.done(
                    &connector,
                    Ok(format!("Disconnected {} {gone}.", connector_name(&connector))),
                );
                this.load_connections(cx);
            }
            Err(error) => this.done(&connector, Err(error)),
        });
        cx.notify();
    }
}
