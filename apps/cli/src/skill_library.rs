//! `nexus skills library`, and the name `nexus skills export` resolves as chat does (`skill-library.ts`). The
//! library is the skills the person's organization and teams approved for them to load (`GET /v1/skill-library`). Neither
//! list carries a SHA-256, so Gateway marks a name that collides with `duplicateName`; the CLI trusts that marker and
//! compares no bytes. A name never shadows: a marked name loads from neither skill.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::cli::Cmd;
use crate::error::{CliError, Error, Result};
use crate::json::{a, s};
use crate::promotion_verbs::{
    Target, parse_target, place, print_json, safe_line, team_slugs, unknown_team, value_flag,
};
use crate::safe_text::safe_text;
use crate::skill_errors::refused;
use crate::skills::{Found, Source};
use crate::style::{col, flex, list, right, rows_of};

/// The library skills the caller may load. A Gateway without the route (404) and a key that may not hold skills (403) have
/// none. Any other answer is an error, because a library nobody could read is not "no library".
pub(crate) fn lenient_library(cmd: &Cmd) -> Result<Vec<Value>> {
    match cmd.core.get("/v1/skill-library") {
        Ok(list) => Ok(a(&list, "skills").to_vec()),
        Err(Error::Core(refusal)) if refusal.status == 404 || refusal.status == 403 => Ok(Vec::new()),
        Err(error) => Err(refused(error)),
    }
}

/// The library, for a name or id that `nexus skills export` was given. An id of one of the person's own skills names that
/// skill whatever the library holds, so the library is not read for it and a failing library route cannot stop that export.
pub(crate) fn library_for(cmd: &Cmd, own: &Value, who: &str) -> Result<Vec<Value>> {
    let wanted = who.trim();
    if a(own, "skills").iter().any(|skill| s(skill, "id") == Some(wanted)) {
        return Ok(Vec::new());
    }
    lenient_library(cmd)
}

fn marked(entry: &Value) -> bool {
    entry.get("duplicateName").and_then(Value::as_bool) == Some(true)
}

fn source_of(entry: &Value) -> Source {
    Source {
        scope: s(entry, "scope").unwrap_or("org").to_string(),
        team_id: s(entry, "teamId").map(String::from),
    }
}

fn found_library(entry: &Value) -> Found {
    Found {
        id: s(entry, "id").unwrap_or("").to_string(),
        name: s(entry, "name").unwrap_or("").to_string(),
        latest: entry.get("version").and_then(Value::as_i64).unwrap_or(0),
        library: Some(source_of(entry)),
    }
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

/// The skill `who` names among the person's own skills and the library, as chat names one (`skills::chat::named`). An id
/// is one skill. A name that a library skill marks `duplicateName` is shared, and none of the skills loads: it is an error
/// that names where each is. A name that one unmarked library skill has is that library skill (a skill of the person's
/// with the same name has its bytes at its latest version). Otherwise it is the person's own skill.
///
/// A library skill numbers its versions on its own. So a version number that the person typed (`explicit_version`) cannot
/// name a version of the library skill when they have a skill of that name: it names a version of theirs. The library
/// skill then has its id, as `nexus skills library` lists it.
pub(crate) fn resolve(cmd: &Cmd, own: &Value, library: &[Value], who: &str, explicit_version: bool) -> Result<Found> {
    let wanted = who.trim();
    let name = wanted.to_lowercase();
    let own_found = |skill: &Value| Found {
        id: s(skill, "id").unwrap_or("").to_string(),
        name: s(skill, "name").unwrap_or("").to_string(),
        latest: skill.get("latestVersion").and_then(Value::as_i64).unwrap_or(0),
        library: None,
    };
    if let Some(skill) = a(own, "skills").iter().find(|skill| s(skill, "id") == Some(wanted)) {
        return Ok(own_found(skill));
    }
    if let Some(entry) = library.iter().find(|entry| s(entry, "id") == Some(wanted)) {
        return Ok(found_library(entry));
    }
    let mine = a(own, "skills")
        .iter()
        .find(|skill| s(skill, "name") == Some(name.as_str()));
    let shared: Vec<&Value> = library
        .iter()
        .filter(|entry| s(entry, "name") == Some(name.as_str()))
        .collect();
    if shared.iter().any(|entry| marked(entry)) || shared.len() > 1 {
        return Err(collision(cmd, &name, mine, &shared));
    }
    if let Some(entry) = shared.first() {
        return Ok(match mine {
            Some(skill) if explicit_version => own_found(skill),
            _ => found_library(entry),
        });
    }
    match mine {
        Some(skill) => Ok(own_found(skill)),
        None => Err(refused(Error::Core(Box::new(crate::error::CoreError {
            status: 404,
            message: format!("you have no skill named {who}"),
            code: Some("skill_not_found".into()),
            interval: None,
            details: None,
        })))),
    }
}

/// A shared name: `duplicate_name`, with where each skill is and how to name one of them.
fn collision(cmd: &Cmd, name: &str, mine: Option<&Value>, shared: &[&Value]) -> Error {
    let items: Vec<Value> = shared.iter().map(|entry| (*entry).clone()).collect();
    let slugs = team_slugs(cmd, &items);
    let mut places: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    if let Some(skill) = mine {
        places.push("your own skills".to_string());
        ids.push(safe_text(s(skill, "id").unwrap_or("")));
    }
    for entry in shared {
        places.push(match s(entry, "scope") {
            Some("team") => format!("{}'s library", place(entry, &slugs, true)),
            _ => "the organization's library".to_string(),
        });
        ids.push(safe_text(s(entry, "id").unwrap_or("")));
    }
    let fix = if mine.is_some() {
        "rename or delete your own skill"
    } else {
        "ask an approver to withdraw all but one"
    };
    Error::Cli(CliError::hint(
        format!(
            "{} skills are named {}: {} (duplicate_name)",
            places.len(),
            safe_text(name),
            and_list(&places)
        ),
        format!(
            "None of them loads while the name is shared. Export one by its id ({}), or {fix}.",
            ids.join(", ")
        ),
    ))
}

/// What `nexus skills export` says after it wrote a library skill: which skill it was, and the approved version it wrote.
pub(crate) fn exported_from(cmd: &Cmd, found: &Found, source: &Source, version: i64) -> String {
    let entry = json!({ "scope": source.scope, "teamId": source.team_id });
    let items = if source.team_id.is_some() {
        vec![entry.clone()]
    } else {
        Vec::new()
    };
    let slugs = team_slugs(cmd, &items);
    let whose = match source.scope.as_str() {
        "team" => format!("{}'s library", place(&entry, &slugs, true)),
        _ => "the organization's library".to_string(),
    };
    format!(
        "Exported {whose} skill {} ({}), approved version {}.",
        safe_text(&found.name),
        safe_text(&found.id),
        version
    )
}

/// `nexus skills library [--to TARGET]`.
pub fn library(cmd: &Cmd) -> Result<i32> {
    let to = match value_flag(cmd, "to", "a target: org or team/<slug>")? {
        Some(to) => Some(parse_target(to)?),
        None => None,
    };
    let page = cmd.core.get("/v1/skill-library").map_err(refused)?;
    let mut entries: Vec<Value> = a(&page, "skills").to_vec();
    let mut slugs: HashMap<String, String> = HashMap::new();
    match &to {
        Some(Target::Org) => entries.retain(|entry| s(entry, "scope") == Some("org")),
        Some(Target::Team(slug)) => {
            let teams = cmd.core.get("/v1/teams").map_err(refused)?;
            let Some(id) = a(&teams, "teams")
                .iter()
                .find(|team| s(team, "slug") == Some(slug.as_str()) || s(team, "id") == Some(slug.as_str()))
                .and_then(|team| s(team, "id"))
            else {
                return Err(unknown_team(slug));
            };
            entries.retain(|entry| s(entry, "scope") == Some("team") && s(entry, "teamId") == Some(id));
            slugs.insert(id.to_string(), slug.clone());
        }
        None => {}
    }
    if cmd.json {
        let shown = if to.is_some() {
            json!({ "skills": entries })
        } else {
            page
        };
        return Ok(print_json(cmd, &shown));
    }
    if entries.is_empty() {
        match &to {
            Some(_) => cmd.out("No library skills for that target."),
            None => cmd.out(format!(
                "The library is empty. Promote a skill: {}",
                cmd.style.cyan("nexus skills promote <name> --to org")
            )),
        }
        return Ok(0);
    }
    if slugs.is_empty() {
        slugs = team_slugs(cmd, &entries);
    }
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            let name = safe_text(s(entry, "name").unwrap_or(""));
            vec![
                if marked(entry) {
                    format!("{name} (duplicate_name)")
                } else {
                    name
                },
                place(entry, &slugs, false),
                format!("v{}", entry.get("version").and_then(Value::as_i64).unwrap_or(0)),
                safe_line(s(entry, "description").unwrap_or("")),
            ]
        })
        .collect();
    cmd.lines(list(
        cmd.style,
        &[col("Skill"), col("Scope"), right("Version"), flex("Description")],
        rows_of(rows),
        None,
    ));
    let mut shared: Vec<String> = Vec::new();
    for entry in entries.iter().filter(|entry| marked(entry)) {
        let name = safe_text(s(entry, "name").unwrap_or(""));
        if !shared.contains(&name) {
            shared.push(name);
        }
    }
    if !shared.is_empty() {
        cmd.out("");
        for name in &shared {
            cmd.out(format!(
                "{} {name} (duplicate_name): another skill has this name, so none of them loads.",
                cmd.style.warn
            ));
        }
        cmd.out(format!(
            "  {} Rename or delete your own skill, or ask an approver to withdraw all but one library skill of the name.",
            cmd.style.arrow
        ));
    }
    Ok(0)
}
