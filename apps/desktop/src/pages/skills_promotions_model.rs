//! Same authority hints as web skills/promotions.ts. Gateway rechecks current membership inside decisions.
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(in super::super) struct Promotion {
    pub id: String,
    pub state: String,
    pub scope: String,
    pub team_id: Option<String>,
    pub name: String,
    pub description: String,
    pub content_sha256: String,
    pub bytes: usize,
    pub source_skill_id: String,
    pub source_version: u64,
    pub requested_by: String,
    pub requested_at: String,
    pub requested_via: String,
    pub source_written_at: String,
    pub source_written_via: String,
    pub note: Option<String>,
    pub decision_reason: Option<String>,
    pub decided_by: Option<String>,
    pub library_version: Option<u64>,
    pub library_skill_id: Option<String>,
    pub withdrawn_reason: Option<String>,
    pub withdrawn_by: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in super::super) struct List {
    pub promotions: Vec<Promotion>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in super::super) struct Previous {
    pub name: String,
    pub library_version: u64,
    pub content: String,
}
#[derive(Debug, Clone, Deserialize)]
pub(in super::super) struct Detail {
    #[serde(flatten)]
    pub promotion: Promotion,
    pub content: String,
    pub previous: Option<Previous>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in super::super) struct Team {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub parent_id: Option<String>,
}
#[derive(Debug, Clone)]
pub(in super::super) struct Viewer {
    pub user: Option<String>,
    pub permissions: Vec<String>,
    pub teams: Vec<Team>,
    pub leads: Option<Vec<String>>,
    pub personal: bool,
}
impl Viewer {
    pub fn parse(value: Value) -> Result<Self, String> {
        let tenant = value
            .get("tenant")
            .and_then(Value::as_object)
            .ok_or("Nexus returned no account scope.")?;
        let memberships = value.get("teams").and_then(Value::as_array);
        let mut teams = vec![];
        let mut leads = vec![];
        for team in memberships.into_iter().flatten() {
            let parsed: Team = super::super::model::parse(team.clone())?;
            if team["role"] == "lead" {
                leads.push(parsed.id.clone());
            }
            teams.push(parsed);
        }
        Ok(Self {
            user: value["user"]["id"].as_str().map(String::from),
            permissions: value["permissions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect(),
            teams,
            leads: memberships.map(|_| leads),
            personal: tenant.get("kind").and_then(Value::as_str) == Some("personal"),
        })
    }
    pub fn can(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission)
    }
    pub fn approver(&self) -> bool {
        self.can("skills.approve_org") || self.can("skills.approve_team")
    }
    pub fn requests(&self) -> bool {
        self.user.is_some() && self.can("skills.promote") && !self.personal
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in super::super) enum Action {
    Approve,
    Refuse,
    Withdraw,
    WithdrawAll,
}
impl Promotion {
    pub fn valid(&self) -> bool {
        !self.id.is_empty()
            && self.source_version > 0
            && (self.scope != "team" || self.team_id.is_some())
            && matches!(self.state.as_str(), "held" | "approved" | "refused" | "withdrawn")
            && matches!(self.scope.as_str(), "org" | "team")
    }
    pub fn allows(&self, action: Action, viewer: &Viewer, directory: Option<&[Team]>) -> bool {
        if !viewer.requests() {
            return false;
        }
        let own = viewer.user.as_deref() == Some(self.requested_by.as_str());
        let approver = may_approve(self, viewer, directory);
        match action {
            Action::Approve | Action::Refuse => self.state == "held" && approver && !own,
            Action::Withdraw => self.state == "held" && (own || approver) || self.state == "approved" && approver,
            Action::WithdrawAll => self.state == "approved" && approver,
        }
    }
}
fn lineage(teams: &[Team], id: &str) -> Option<Vec<String>> {
    let mut chain = vec![];
    let mut next = Some(id);
    while let Some(id) = next {
        if chain.len() > 32 || chain.iter().any(|seen| seen == id) {
            return None;
        }
        let team = teams.iter().find(|team| team.id == id)?;
        chain.push(id.to_string());
        next = team.parent_id.as_deref();
    }
    Some(chain)
}
fn may_approve(p: &Promotion, viewer: &Viewer, directory: Option<&[Team]>) -> bool {
    if viewer.can("skills.approve_org") {
        return true;
    }
    if p.scope != "team" || !viewer.can("skills.approve_team") {
        return false;
    }
    let chain = p
        .team_id
        .as_deref()
        .and_then(|id| directory.and_then(|teams| lineage(teams, id)));
    match (chain, &viewer.leads) {
        (Some(chain), Some(leads)) => chain.iter().any(|id| leads.contains(id)),
        _ => true,
    }
}
pub(in super::super) fn text(value: &str, required: bool) -> Result<Option<String>, String> {
    let value = value.trim();
    if required && value.is_empty() {
        return Err("Say why, so the author knows what to change.".into());
    }
    if value.chars().count() > 500 {
        return Err("A note or reason is at most 500 characters.".into());
    }
    Ok((!value.is_empty()).then(|| value.to_string()))
}
pub(in super::super) fn decision(action: Action, reason: &str) -> Result<Value, String> {
    let reason = if action == Action::Approve {
        None
    } else {
        text(reason, action == Action::Refuse)?
    };
    let mut body = match action {
        Action::Approve => json!({"decision":"approved"}),
        Action::Refuse => json!({"decision":"refused"}),
        Action::Withdraw => json!({}),
        Action::WithdrawAll => json!({"allVersions":true}),
    };
    // Approval never carries a reason typed for a canceled refusal. Advisory reviewed flags are omitted,
    // as in the CLI: this renderer does not measure whether the ends of content/diff entered the viewport.
    if action != Action::Approve {
        if let Some(reason) = reason {
            body["reason"] = json!(reason);
        }
    }
    Ok(body)
}
pub(in super::super) fn path(id: &str) -> String {
    format!("/v1/skill-promotions/{}", nexus_client::url::enc(id))
}
pub(in super::super) fn query(mine: bool, state: &str, cursor: Option<&str>) -> String {
    let mut q = "/v1/skill-promotions?limit=50".to_string();
    if mine {
        q.push_str("&mine=true");
    }
    if state != "all" {
        q.push_str(&format!("&state={}", nexus_client::url::enc(state)));
    }
    if let Some(cursor) = cursor {
        q.push_str(&format!("&cursor={}", nexus_client::url::enc(cursor)));
    }
    q
}
/// A conservative line diff: common prefix/suffix are omitted; the changed middle is removed then added.
/// This never hides a changed line by guessing its similarity. CR and invisible Unicode remain visible downstream.
pub(in super::super) fn diff(before: &str, after: &str) -> String {
    if before == after {
        return "No changes to the approved text.".into();
    }
    let a: Vec<_> = before.split_inclusive('\n').collect();
    let b: Vec<_> = after.split_inclusive('\n').collect();
    let prefix = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let ending = |text: &str| {
        if text.contains("\r\n") {
            if text.replace("\r\n", "").contains(['\r', '\n']) {
                "mixed"
            } else {
                "CRLF"
            }
        } else if text.contains('\r') {
            "mixed"
        } else {
            "LF"
        }
    };
    let mut result = format!(
        "--- previous approved version ({})\n+++ requested version ({})\n@@ -{},{} +{},{} @@\n",
        ending(before),
        ending(after),
        prefix + 1,
        a.len() - prefix - suffix,
        prefix + 1,
        b.len() - prefix - suffix
    );
    for (mark, lines) in [('-', &a[prefix..a.len() - suffix]), ('+', &b[prefix..b.len() - suffix])] {
        for line in lines {
            result.push(mark);
            result.push_str(line);
            if !line.ends_with('\n') {
                result.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    result
}

/// The web's exact advisory name skeleton (skill-names.ts). It never changes API names or approval authority.
pub(in super::super) fn name_skeleton(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let normalized = name.nfkc().collect::<String>().to_lowercase();
    // ECMAScript \s differs from Rust White_Space: it includes BOM and excludes NEL (U+0085).
    let js_space = |c| {
        matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
    };
    normalized
        .chars()
        .filter(|c| !js_space(*c) && !matches!(c, '-' | '_' | '.'))
        .map(|c| match c {
            '1' | 'i' => 'l',
            '0' => 'o',
            _ => c,
        })
        .collect::<String>()
        .replace("rn", "m")
        .replace("vv", "w")
}
#[derive(Debug, PartialEq)]
pub(in super::super) struct NameMatch {
    pub name: String,
    pub place: String,
    pub same: bool,
}
/// Library first, then own names; exact matches first, each name/place once. Excludes this same library/source skill.
pub(in super::super) fn name_matches(
    p: &Promotion,
    library: &[super::super::model::LibrarySkill],
    own: &[super::super::model::Skill],
    teams: Option<&[Team]>,
) -> Vec<NameMatch> {
    let skeleton = name_skeleton(&p.name);
    if skeleton.is_empty() {
        return vec![];
    }
    let mut candidates = vec![];
    for entry in library {
        if Some(&entry.id) == p.library_skill_id.as_ref()
            || entry.name == p.name
                && entry.scope == p.scope
                && entry.team_id.as_deref().unwrap_or("") == p.team_id.as_deref().unwrap_or("")
        {
            continue;
        }
        let place = if entry.scope == "org" {
            "in the organization's library".to_string()
        } else if let Some(team) = entry
            .team_id
            .as_ref()
            .and_then(|id| teams.and_then(|teams| teams.iter().find(|team| &team.id == id)))
            .filter(|team| !team.name.is_empty())
        {
            format!("in the library of team {}", team.name)
        } else {
            "in a team's library".into()
        };
        candidates.push((entry.name.clone(), place));
    }
    candidates.extend(
        own.iter()
            .filter(|skill| skill.id != p.source_skill_id)
            .map(|skill| (skill.name.clone(), "in your skills".to_string())),
    );
    let mut seen = std::collections::HashSet::new();
    let mut found = vec![];
    for (name, place) in candidates {
        if name_skeleton(&name) == skeleton && seen.insert((name.clone(), place.clone())) {
            found.push(NameMatch {
                same: name == p.name,
                name,
                place,
            });
        }
    }
    found.sort_by_key(|entry| !entry.same);
    found
}
