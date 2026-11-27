//! Share a page: the chat's Share sheet with a page's choices, as Nexus UI's `pages/page-share.tsx`. One audience stays
//! open: Save shares again, then revokes the page's earlier links. Readers in the organization may edit. A view-only
//! share follows the latest version, or pins the one it was saved with.

use super::*;

/// What the Share sheet holds between loads.
pub(super) struct ShareState {
    /// The page's active shares, newest first.
    pub(super) shares: Vec<Value>,
    pub(super) policy: Value,
    pub(super) role: String,
    pub(super) personal: bool,
    pub(super) latest_version: u64,
    pub(super) kind: Audience,
    pub(super) picks: Vec<String>,
    pub(super) teams: Result<Vec<(String, String)>, String>,
    pub(super) people: Result<Vec<(String, String)>, String>,
    pub(super) outside: Entity<InputState>,
    /// `""` (never), a duration such as `7d`, or `current`: the open link's own expiry.
    pub(super) expiry: String,
    pub(super) edit: bool,
    pub(super) follow_latest: bool,
    pub(super) copied: bool,
}

impl ShareState {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            shares: Vec::new(),
            policy: Value::Null,
            role: String::new(),
            personal: false,
            latest_version: 1,
            kind: Audience::Org,
            picks: Vec::new(),
            teams: Ok(Vec::new()),
            people: Ok(Vec::new()),
            outside: cx.new(|cx| InputState::new(window, cx).placeholder("name@partner.com, another@client.com")),
            expiry: String::new(),
            edit: false,
            follow_latest: true,
            copied: false,
        }
    }

    pub(super) fn newest(&self) -> Option<&Value> {
        self.shares.first()
    }

    /// The open link's audience, or Only you.
    pub(super) fn shared_kind(&self) -> Audience {
        self.newest().map_or(Audience::Private, |share| {
            Audience::from_key(word(&share["audience"], "kind"))
        })
    }

    /// Why an audience is off: the organization's policy, or public page links turned off.
    pub(super) fn blocked(&self, kind: Audience) -> Option<&'static str> {
        kind.blocked(&self.policy, &self.role).or((kind == Audience::Public
            && self.policy["artifacts.pages.public"] == false)
            .then_some("Your organization has turned off public links for pages."))
    }

    fn typed(&self, cx: &App) -> Vec<String> {
        split(&self.outside.read(cx).value())
    }

    /// The teams, people or addresses the sheet would share with now.
    pub(super) fn chosen(&self, cx: &App) -> Vec<String> {
        if self.kind == Audience::External {
            self.typed(cx)
        } else {
            self.picks.clone()
        }
    }

    /// Whether Save would change anything: the audience, who, the role, the version, or the expiry.
    pub(super) fn changed(&self, cx: &App) -> bool {
        let Some(share) = self.newest() else {
            return self.kind != Audience::Private;
        };
        if self.shares.len() > 1 || self.kind != self.shared_kind() {
            return true;
        }
        let list = |key: &str| sorted(strings(&share["audience"][key]));
        let who = match self.kind {
            Audience::Teams => list("teams") != sorted(self.picks.clone()),
            Audience::People | Audience::External => list("people") != sorted(self.chosen(cx)),
            _ => false,
        };
        who || self.edit != (share["pageRole"] == "edit")
            || self.follow_latest != share["pageVersion"].is_null()
            || (self.kind == Audience::Public && self.expiry != "current")
    }
}

impl LibraryDialog {
    pub(super) fn load_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true;
        self.problem = None;
        let Some(page) = self.row.owned_page.clone() else {
            self.busy = false;
            self.problem = Some("This page is not owned by the current account.".into());
            return;
        };
        let get = |path: String| self.gateway.request(Verb::Get, path, None);
        let detail = get(format!("/v1/pages/{}", nexus_client::url::enc(&page)));
        let shares = get(shares_path(&page));
        let policy = get("/v1/orgs/settings".into());
        let me = get("/v1/tenants/me".into());
        let teams = get("/v1/teams".into());
        let people = get("/v1/orgs/members".into());
        cx.spawn_in(window, async move |this, cx| {
            let (detail, shares, policy, me, teams, people) = futures::join!(detail, shares, policy, me, teams, people);
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                if !this.current(cx) {
                    this.problem = Some("The signed-in account changed. Close this dialog and try again.".into());
                } else {
                    match (detail, shares, policy, me) {
                        (Ok(detail), Ok(shares), Ok(policy), Ok(me)) => {
                            let state = &mut this.share;
                            state.latest_version = detail["latestVersion"].as_u64().unwrap_or(1);
                            state.shares = active(&shares);
                            state.policy = policy;
                            state.role = word(&me, "role").into();
                            state.personal = me["tenant"]["kind"] == "personal";
                            state.teams = teams.map(|answer| choices(&answer, "teams", "slug"));
                            state.people = people.map(|answer| choices(&answer, "members", "email"));
                            this.detail = detail;
                            this.reset_share(window, cx);
                            this.ready = true;
                        }
                        (Err(error), ..) | (_, Err(error), ..) | (.., Err(error), _) | (.., Err(error)) => {
                            this.problem = Some(error)
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Start the choices from the open link: its audience, who, role, version and expiry.
    fn reset_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.share;
        let newest = state.newest().cloned().unwrap_or(Value::Null);
        state.kind = match state.shared_kind() {
            // Nothing is shared yet: offer the organization first, or a named person in a personal one.
            Audience::Private if state.personal => Audience::People,
            Audience::Private => Audience::Org,
            kind => kind,
        };
        let audience = &newest["audience"];
        state.picks = strings(
            &audience[if state.kind == Audience::Teams {
                "teams"
            } else {
                "people"
            }],
        );
        state.edit = newest["pageRole"] == "edit";
        state.follow_latest = newest["pageVersion"].is_null();
        state.expiry = if newest["expiresAt"].is_string() {
            "current".into()
        } else {
            expiry_choices(&state.policy)
                .first()
                .map(|(value, _)| value.clone())
                .unwrap_or_default()
        };
        let typed = if state.kind == Audience::External {
            state.picks.join(", ")
        } else {
            String::new()
        };
        state.outside.update(cx, |input, cx| input.set_value(typed, window, cx));
    }

    pub(super) fn pick_audience(&mut self, kind: Audience, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || kind == self.share.kind || self.share.blocked(kind).is_some() {
            return;
        }
        let state = &mut self.share;
        state.kind = kind;
        state.picks.clear();
        state.copied = false;
        if !editable(kind) {
            state.edit = false;
        }
        self.problem = None;
        if kind == Audience::External {
            self.share.outside.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    /// Share with the picked audience, then revoke the page's earlier links, and copy the new link.
    pub(super) fn save_share(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.ready {
            return;
        }
        if self.share.kind == Audience::Private {
            return self.stop_sharing(cx);
        }
        let state = &self.share;
        let current = state
            .newest()
            .and_then(|share| share["expiresAt"].as_str())
            .map(String::from);
        let pin = (!state.follow_latest)
            .then(|| state.newest().and_then(|share| share["pageVersion"].as_u64()))
            .map(|pinned| pinned.unwrap_or(state.latest_version));
        let body = match share_body(
            state.kind,
            &state.policy,
            &state.role,
            &state.chosen(cx),
            &state.expiry,
            current.as_deref(),
            state.edit,
            pin,
        ) {
            Ok(body) => body,
            Err(error) => {
                self.problem = Some(error);
                cx.notify();
                return;
            }
        };
        let Some(page) = self.row.owned_page.clone() else {
            return;
        };
        let old: Vec<String> = state.shares.iter().map(|share| word(share, "id").into()).collect();
        self.change_shares(page, Some(body), old, cx);
    }

    /// Revoke every open link: only you can open the page.
    pub(super) fn stop_sharing(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.row.owned_page.clone() else {
            return;
        };
        let old = self.share.shares.iter().map(|share| word(share, "id").into()).collect();
        self.change_shares(page, None, old, cx);
    }

    fn change_shares(&mut self, page: String, body: Option<Value>, old: Vec<String>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if !self.current(cx) {
            self.problem = Some("The signed-in account changed. Close this dialog and try again.".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.problem = None;
        self.share.copied = false;
        let gateway = self.gateway.clone();
        let site = gateway.site_url();
        fetch(
            cx,
            async move {
                let mut made = Value::Null;
                if let Some(body) = body {
                    let path = format!("/v1/pages/{}/share", nexus_client::url::enc(&page));
                    made = gateway.request(Verb::Post, path, Some(body)).await?;
                }
                let mut failed = None;
                for id in old.iter().filter(|id| made["id"] != id.as_str()) {
                    let path = format!("/v1/artifacts/{}", nexus_client::url::enc(id));
                    if let Err(error) = gateway.request(Verb::Delete, path, None).await {
                        failed = Some(error);
                    }
                }
                let shares = gateway.request(Verb::Get, shares_path(&page), None).await?;
                Ok::<_, String>(json!({"made": made, "shares": shares, "failed": failed}))
            },
            move |this, answer, cx| {
                this.busy = false;
                this.gateway.load_library(this.org_name.clone());
                match answer {
                    Ok(answer) => {
                        this.share.shares = active(&answer["shares"]);
                        if let Some(error) = answer["failed"].as_str() {
                            this.problem = Some(format!("An earlier link could not be revoked: {error}"));
                        }
                        if this.share.shares.is_empty() {
                            this.share.kind = Audience::Private;
                        }
                        if let Some(url) = link(&answer["made"], &site) {
                            cx.write_to_clipboard(ClipboardItem::new_string(url));
                            this.share.copied = true;
                        }
                    }
                    Err(error) => this.problem = Some(error),
                }
            },
        );
        cx.notify();
    }
}

fn shares_path(page: &str) -> String {
    format!(
        "/v1/artifacts?scope=shared_by_me&session={}&status=active&limit=50",
        nexus_client::url::enc(page)
    )
}

/// The active shares in a list answer, newest first.
fn active(answer: &Value) -> Vec<Value> {
    let mut shares: Vec<Value> = answer["artifacts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|share| share["status"] == "active")
        .cloned()
        .collect();
    shares.sort_by(|a, b| word(b, "createdAt").cmp(word(a, "createdAt")));
    shares
}

/// A share's link to copy: the public link when there is one, made absolute on the site.
pub(super) fn link(share: &Value, site: &str) -> Option<String> {
    ["publicUrl", "viewerUrl", "url"]
        .into_iter()
        .find_map(|key| share[key].as_str())
        .and_then(|value| library::reader_url(site, value))
}

/// Whether an audience may edit a page: people in the organization, never outside or a public link.
pub(super) fn editable(kind: Audience) -> bool {
    matches!(kind, Audience::Org | Audience::Teams | Audience::People)
}

/// The choices for a public link's expiry, within the organization's maximum, as Nexus UI's `expiryChoices`.
pub(super) fn expiry_choices(policy: &Value) -> Vec<(String, String)> {
    let rule = &policy["artifacts.external.expiry"];
    let max = rule["maxDays"].as_u64();
    let mut choices: Vec<(String, String)> = [
        (1, "1 day"),
        (7, "7 days"),
        (30, "30 days"),
        (90, "90 days"),
        (365, "1 year"),
    ]
    .into_iter()
    .filter(|(days, _)| max.is_none_or(|max| *days <= max))
    .map(|(days, label)| (format!("{days}d"), label.to_string()))
    .collect();
    if let Some(max) = max
        && !choices.iter().any(|(value, _)| *value == format!("{max}d"))
    {
        choices.push((format!("{max}d"), format!("{max} days")));
    }
    if rule["required"] != true && max.is_none() {
        choices.insert(0, (String::new(), "Never".into()));
    }
    choices
}

/// `POST /v1/pages/{id}/share`'s body, as `pageShareRequest`: only the list the audience reads, `edit` only for an
/// audience that may edit (on the latest version, so no pin), and the expiry only for a public link.
#[allow(clippy::too_many_arguments)]
fn share_body(
    kind: Audience,
    policy: &Value,
    role: &str,
    picks: &[String],
    expiry: &str,
    current_expiry: Option<&str>,
    edit: bool,
    pin: Option<u64>,
) -> Result<Value, String> {
    if kind == Audience::Private {
        return Err("Choose who can open the page.".into());
    }
    if let Some(reason) = kind.blocked(policy, role) {
        return Err(reason.into());
    }
    if kind == Audience::Public && policy["artifacts.pages.public"] == false {
        return Err("Your organization has turned off public links for pages.".into());
    }
    let edit = edit && editable(kind);
    let mut body = json!({"audience": kind.key(), "role": if edit { "edit" } else { "view" }});
    match kind {
        Audience::Teams | Audience::People if picks.is_empty() => {
            return Err("Choose at least one team or person.".into());
        }
        Audience::External if picks.is_empty() => return Err("Enter at least one email address.".into()),
        Audience::Teams => body["teams"] = json!(picks),
        Audience::People | Audience::External => body["people"] = json!(picks),
        _ => {}
    }
    if let Some(pin) = pin.filter(|_| !edit) {
        body["version"] = json!(pin);
    }
    if kind == Audience::Public {
        match (expiry, current_expiry) {
            ("current", Some(at)) => body["expiresAt"] = json!(at),
            ("" | "current", _) if policy["artifacts.external.expiry"]["required"] == true => {
                return Err("Your organization requires a link expiry.".into());
            }
            ("" | "current", _) => {}
            (duration, _) => body["expiresIn"] = json!(duration),
        }
    }
    Ok(body)
}

fn word<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}

fn sorted(mut list: Vec<String>) -> Vec<String> {
    list.sort();
    list
}

fn split(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter(|part| !part.is_empty())
        .map(String::from)
        .collect()
}

/// The teams or members to tick: an id the Gateway reads (a team's slug, a member's email) and a label.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn body(
        kind: Audience,
        policy: Value,
        picks: &[&str],
        expiry: &str,
        edit: bool,
        pin: Option<u64>,
    ) -> Result<Value, String> {
        let picks: Vec<String> = picks.iter().map(|pick| pick.to_string()).collect();
        share_body(kind, &policy, "member", &picks, expiry, None, edit, pin)
    }

    #[::core::prelude::v1::test]
    fn refuses_what_the_policy_or_the_audience_does_not_allow() {
        assert!(body(Audience::Private, json!({}), &[], "", false, None).is_err());
        assert!(body(Audience::People, json!({}), &[], "", false, None).is_err());
        assert!(body(Audience::External, json!({}), &[], "", false, None).is_err());
        assert!(
            body(
                Audience::Public,
                json!({"artifacts.pages.public": false}),
                &[],
                "",
                false,
                None
            )
            .is_err()
        );
        assert!(
            body(
                Audience::External,
                json!({"artifacts.external": "off"}),
                &["x@y.com"],
                "",
                false,
                None
            )
            .is_err()
        );
        let required = json!({"artifacts.external.expiry": {"required": true}});
        assert!(body(Audience::Public, required, &[], "", false, None).is_err());
    }

    #[::core::prelude::v1::test]
    fn sends_the_list_the_audience_reads_and_pins_only_view_shares() {
        assert_eq!(
            body(Audience::Teams, json!({}), &["platform", "core"], "", false, Some(3)).unwrap(),
            json!({"audience": "teams", "teams": ["platform", "core"], "version": 3, "role": "view"})
        );
        assert_eq!(
            body(Audience::Org, json!({}), &[], "", true, Some(3)).unwrap(),
            json!({"audience": "org", "role": "edit"})
        );
        // People outside and a public link never edit.
        assert_eq!(
            body(Audience::External, json!({}), &["a@b.co"], "", true, None).unwrap()["role"],
            "view"
        );
        assert_eq!(
            body(Audience::Public, json!({}), &[], "7d", false, None).unwrap()["expiresIn"],
            "7d"
        );
        let kept = share_body(
            Audience::Public,
            &json!({}),
            "owner",
            &[],
            "current",
            Some("2027-01-01T00:00:00Z"),
            false,
            None,
        );
        assert_eq!(kept.unwrap()["expiresAt"], "2027-01-01T00:00:00Z");
    }

    #[::core::prelude::v1::test]
    fn expiry_choices_follow_the_organizations_rule() {
        let values = |policy: Value| {
            expiry_choices(&policy)
                .into_iter()
                .map(|(value, _)| value)
                .collect::<Vec<_>>()
        };
        assert_eq!(values(json!({})), ["", "1d", "7d", "30d", "90d", "365d"]);
        assert_eq!(
            values(json!({"artifacts.external.expiry": {"maxDays": 14}})),
            ["1d", "7d", "14d"]
        );
        assert_eq!(
            values(json!({"artifacts.external.expiry": {"required": true}})),
            ["1d", "7d", "30d", "90d", "365d"]
        );
    }

    #[::core::prelude::v1::test]
    fn the_newest_active_share_comes_first() {
        let list = json!({"artifacts": [
            {"id": "old", "status": "active", "createdAt": "2026-11-01T00:00:00Z"},
            {"id": "gone", "status": "revoked", "createdAt": "2026-11-09T00:00:00Z"},
            {"id": "new", "status": "active", "createdAt": "2026-11-05T00:00:00Z"}
        ]});
        let ids: Vec<_> = active(&list)
            .iter()
            .map(|share| word(share, "id").to_string())
            .collect();
        assert_eq!(ids, ["new", "old"]);
    }
}
