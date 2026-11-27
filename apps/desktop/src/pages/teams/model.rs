use super::*;

/// A team's spend over 30 days (`GET /v1/analytics/usage?team=…&groupBy=person`), with the teams under it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct TeamSpend {
    pub(super) spent: Option<f64>,
    pub(super) calls: u64,
    /// Dollars by user id.
    pub(super) people: Vec<(String, f64)>,
}

pub(super) fn parse_spend(value: &Value) -> TeamSpend {
    let dollars = |row: &Value| super::usage::amount(row, "spentUsd");
    let totals = value.get("totals");
    TeamSpend {
        spent: totals.and_then(dollars),
        calls: totals.and_then(|t| t.get("calls")).and_then(Value::as_u64).unwrap_or(0),
        people: value
            .get("groups")
            .and_then(Value::as_array)
            .map(|groups| {
                groups
                    .iter()
                    .filter_map(|group| Some((group.get("key")?.as_str()?.to_string(), dollars(group)?)))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Money as the members page writes it: `<$0.01` for a spend under a cent.
pub(super) fn spent_text(dollars: f64) -> String {
    if dollars > 0. && dollars < 0.005 {
        "<$0.01".into()
    } else {
        super::usage::cents(dollars)
    }
}

/// Who is looking: `GET /v1/tenants/me`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Me {
    pub(super) user_id: String,
    pub(super) manage: bool,
    pub(super) personal: bool,
}

pub(super) fn parse_me(value: &Value) -> Me {
    Me {
        user_id: value
            .pointer("/user/id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        manage: value
            .get("permissions")
            .and_then(Value::as_array)
            .is_some_and(|list| list.iter().any(|each| each.as_str() == Some("teams.manage"))),
        personal: value.pointer("/tenant/kind").and_then(Value::as_str) == Some("personal"),
    }
}

/// A team in the tree (`GET /v1/teams?tree=1`), flattened: parents before children, by name.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TreeRow {
    pub(super) id: String,
    pub(super) slug: String,
    pub(super) name: String,
    pub(super) level: String,
    pub(super) depth: usize,
    pub(super) leads: Vec<String>,
    pub(super) headcount: u64,
    pub(super) pending: u64,
    pub(super) my_role: String,
    pub(super) is_default: bool,
}

pub(super) fn parse_tree(value: &Value) -> Vec<TreeRow> {
    pub(super) fn walk(nodes: &[Value], depth: usize, out: &mut Vec<TreeRow>) {
        let mut nodes: Vec<&Value> = nodes.iter().collect();
        nodes.sort_by_key(|node| ui::text(node, "name").to_lowercase());
        for node in nodes {
            let slug = ui::text(node, "slug");
            if slug.is_empty() {
                continue;
            }
            let leads = node
                .get("leads")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|lead| {
                            let who = [
                                ui::text(lead, "name"),
                                ui::text(lead, "email"),
                                ui::text(lead, "userId"),
                            ]
                            .into_iter()
                            .find(|each| !each.is_empty())
                            .unwrap_or("Someone");
                            if lead.get("pending").and_then(Value::as_bool) == Some(true) {
                                format!("{who} · pending")
                            } else {
                                who.to_string()
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push(TreeRow {
                id: ui::text(node, "id").to_string(),
                slug: slug.to_string(),
                name: name_or_slug(node),
                level: ui::text(node, "level").to_string(),
                depth: node
                    .get("depth")
                    .and_then(Value::as_u64)
                    .map(|depth| depth as usize)
                    .unwrap_or(depth),
                leads,
                headcount: node.get("headcount").and_then(Value::as_u64).unwrap_or(0),
                pending: node.get("pending").and_then(Value::as_u64).unwrap_or(0),
                my_role: ui::text(node, "myRole").to_string(),
                is_default: node.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
            });
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                walk(children, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(roots) = value.get("tree").and_then(Value::as_array) {
        walk(roots, 1, &mut out);
    }
    out
}

/// The slugs of a team and every team under it, from the flattened tree: where it cannot move.
pub(super) fn subtree(rows: &[TreeRow], slug: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(at) = rows.iter().position(|row| row.slug == slug) else {
        out.insert(slug.to_string());
        return out;
    };
    let depth = rows[at].depth;
    out.insert(slug.to_string());
    for row in &rows[at + 1..] {
        if row.depth <= depth {
            break;
        }
        out.insert(row.slug.clone());
    }
    out
}

/// Someone added to a team by an email not in the organization yet (`GET /v1/teams/pending`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PendingEntry {
    pub(super) id: String,
    pub(super) team: String,
    pub(super) email: String,
    pub(super) lead: bool,
    pub(super) home: bool,
    pub(super) created_at: String,
}

pub(super) fn parse_pending_entry(row: &Value) -> Option<PendingEntry> {
    let id = ui::text(row, "id");
    let email = ui::text(row, "email");
    (!id.is_empty() && !email.is_empty()).then(|| PendingEntry {
        id: id.to_string(),
        team: ui::text(row, "team").to_string(),
        email: email.to_string(),
        lead: ui::text(row, "role") == "lead",
        home: row.get("home").and_then(Value::as_bool).unwrap_or(false),
        created_at: ui::text(row, "createdAt").to_string(),
    })
}

pub(super) fn parse_pending(value: &Value) -> Vec<PendingEntry> {
    value
        .get("pending")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(parse_pending_entry).collect())
        .unwrap_or_default()
}

/// The pending entries one per person, by email, with all their teams.
pub(super) fn pending_people(entries: &[PendingEntry]) -> Vec<(String, Vec<PendingEntry>)> {
    let mut people: Vec<(String, Vec<PendingEntry>)> = Vec::new();
    for entry in entries {
        match people.iter_mut().find(|(email, _)| *email == entry.email) {
            Some((_, list)) => list.push(entry.clone()),
            None => people.push((entry.email.clone(), vec![entry.clone()])),
        }
    }
    people.sort_by(|a, b| a.0.cmp(&b.0));
    people
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TeamMember {
    pub(super) user_id: String,
    pub(super) email: String,
    pub(super) name: String,
    pub(super) lead: bool,
    pub(super) is_default: bool,
}

impl TeamMember {
    pub(super) fn who(&self) -> String {
        [&self.name, &self.email, &self.user_id]
            .into_iter()
            .find(|each| !each.is_empty())
            .cloned()
            .unwrap_or_else(|| "Someone".into())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Child {
    pub(super) slug: String,
    pub(super) name: String,
    pub(super) level: String,
    pub(super) members: u64,
    pub(super) pending: u64,
    pub(super) child_count: u64,
}

/// One team: `GET /v1/teams/{slug}`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Detail {
    /// Gateway's id: a control on the team names it.
    pub(super) id: String,
    pub(super) slug: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) level: String,
    /// The parent's slug; empty at the top level.
    pub(super) parent: String,
    pub(super) created_at: String,
    pub(super) members: u64,
    pub(super) pending: u64,
    pub(super) my_role: String,
    pub(super) is_default: bool,
    pub(super) member_list: Vec<TeamMember>,
    pub(super) pending_list: Vec<PendingEntry>,
    /// Its ancestors, top level first: slug and name.
    pub(super) path: Vec<(String, String)>,
    pub(super) children: Vec<Child>,
}

pub(super) fn parse_detail(value: &Value) -> Detail {
    let list = |key: &str| value.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let count = |row: &Value, key: &str| row.get(key).and_then(Value::as_u64).unwrap_or(0);
    let mut member_list: Vec<TeamMember> = list("memberList")
        .iter()
        .filter_map(|row| {
            let user_id = ui::text(row, "userId");
            (!user_id.is_empty()).then(|| TeamMember {
                user_id: user_id.to_string(),
                email: ui::text(row, "email").to_string(),
                name: ui::text(row, "name").to_string(),
                lead: ui::text(row, "role") == "lead",
                is_default: row.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect();
    // Leads first, then by name, as the tree lists them.
    member_list.sort_by(|a, b| {
        b.lead
            .cmp(&a.lead)
            .then_with(|| a.who().to_lowercase().cmp(&b.who().to_lowercase()))
    });
    Detail {
        id: ui::text(value, "id").to_string(),
        slug: ui::text(value, "slug").to_string(),
        name: name_or_slug(value),
        description: ui::text(value, "description").to_string(),
        level: ui::text(value, "level").to_string(),
        parent: ui::text(value, "parent").to_string(),
        created_at: ui::text(value, "createdAt").to_string(),
        members: count(value, "members"),
        pending: count(value, "pending"),
        my_role: ui::text(value, "myRole").to_string(),
        is_default: value.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
        member_list,
        pending_list: list("pendingList").iter().filter_map(parse_pending_entry).collect(),
        path: list("path")
            .iter()
            .map(|step| (ui::text(step, "slug").to_string(), name_or_slug(step)))
            .filter(|(slug, _)| !slug.is_empty())
            .collect(),
        children: list("children")
            .iter()
            .filter(|row| !ui::text(row, "slug").is_empty())
            .map(|row| Child {
                slug: ui::text(row, "slug").to_string(),
                name: name_or_slug(row),
                level: ui::text(row, "level").to_string(),
                members: count(row, "members"),
                pending: count(row, "pending"),
                child_count: count(row, "childCount"),
            })
            .collect(),
    }
}

pub(super) fn name_or_slug(value: &Value) -> String {
    let name = ui::text(value, "name");
    if name.is_empty() {
        ui::text(value, "slug").to_string()
    } else {
        name.to_string()
    }
}

/// A member of the organization, to add by email: `GET /v1/orgs/members`.
pub(super) fn parse_org_members(value: &Value) -> Vec<(String, String)> {
    value
        .get("members")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|row| {
                    let user_id = ui::text(row, "userId");
                    let email = ui::text(row, "email");
                    (!user_id.is_empty() && !email.is_empty()).then(|| (user_id.to_string(), email.to_lowercase()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The slug a name suggests: `Platform Team` is `platform-team`.
pub(super) fn slug_from(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(127).collect()
}

/// Lower-case letters, digits and dashes, starting with a letter or digit, up to `max` long.
pub(super) fn valid_label(text: &str, max: usize) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    text.chars().count() <= max
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// What `POST /v1/teams/{slug}/members` did.
pub(super) fn added_text(value: &Value, team: &str) -> String {
    if value.get("pending").and_then(Value::as_bool) == Some(true) {
        let email = value.pointer("/entry/email").and_then(Value::as_str).unwrap_or("They");
        return format!("{email} is not in your organization yet: they join {team} when they sign up with that email.");
    }
    let member = value.get("member").cloned().unwrap_or(Value::Null);
    let who = [ui::text(&member, "email"), ui::text(&member, "userId")]
        .into_iter()
        .find(|each| !each.is_empty())
        .unwrap_or("them");
    let default = member.get("isDefault").and_then(Value::as_bool) == Some(true);
    format!(
        "Added {who} to {team}{}.",
        if default { "; it is now their default team" } else { "" }
    )
}

/// Gateway's refusal said plainly: a 403 names who may do it; anything else is Gateway's own words.
pub(super) fn refusal(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.starts_with("only the org's owner or admins") {
        "Only organization admins can change this.".into()
    } else if lower.starts_with("only the org's owner,") {
        "Only organization admins and team leads can change this.".into()
    } else if lower.starts_with("only the org's owner") {
        "Only the organization's owner can do this.".into()
    } else {
        format!("{}.", message.trim().trim_end_matches('.'))
    }
}

pub(super) fn plural(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// A line under a section: what a change did, or why it did not happen.
#[derive(Debug, Clone)]
pub(super) struct Outcome {
    pub(super) text: SharedString,
    pub(super) error: bool,
}

impl Outcome {
    pub(super) fn done(text: impl Into<SharedString>) -> Option<Outcome> {
        Some(Outcome {
            text: text.into(),
            error: false,
        })
    }

    pub(super) fn invalid(text: impl Into<SharedString>) -> Option<Outcome> {
        Some(Outcome {
            text: text.into(),
            error: true,
        })
    }

    pub(super) fn failed(title: &str, message: &str) -> Option<Outcome> {
        Outcome::invalid(format!("{title}: {}", refusal(message)))
    }
}

/// A button pressed once that waits for the second press.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Confirm {
    /// Cancel a person pending in every team (by email).
    CancelPerson(String),
    /// Cancel one pending entry (by id).
    CancelEntry(String),
    RemoveMember(String),
    DeleteTeam,
}

/// Await a Gateway call and apply its answer with the window at hand (to fill or clear a field), then redraw.
pub(super) fn fetch_in<V: 'static>(
    window: &mut Window,
    cx: &mut Context<V>,
    request: impl std::future::Future<Output = Result<Value, String>> + 'static,
    apply: impl FnOnce(&mut V, Result<Value, String>, &mut Window, &mut Context<V>) + 'static,
) {
    cx.spawn_in(window, async move |this, cx| {
        let answer = request.await;
        this.update_in(cx, |view, window, cx| {
            apply(view, answer, window, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

/// The fields of the Create a team form.
pub(super) struct CreateForm {
    pub(super) name: Entity<InputState>,
    pub(super) slug: Entity<InputState>,
    pub(super) level: Entity<InputState>,
    pub(super) description: Entity<InputState>,
    pub(super) parent: Option<String>,
}

/// The fields of a team's Settings: filled from the team when it loads.
pub(super) struct EditForm {
    pub(super) name: Entity<InputState>,
    pub(super) slug: Entity<InputState>,
    pub(super) level: Entity<InputState>,
    pub(super) description: Entity<InputState>,
    pub(super) parent: Option<String>,
}

impl EditForm {
    pub(super) fn values(&self, cx: &App) -> (Vec<String>, Option<String>) {
        (
            [&self.name, &self.slug, &self.level, &self.description]
                .iter()
                .map(|field| field.read(cx).value().to_string())
                .collect(),
            self.parent.clone(),
        )
    }
}

pub(super) fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A path segment or query value, percent-encoded.
pub(super) fn encode(segment: &str) -> String {
    let mut out = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The slug a name would get: lower case, words joined by `-`.
pub(super) fn slug_of(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}
