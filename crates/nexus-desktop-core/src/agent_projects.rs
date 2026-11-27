//! Errands in projects: the words and the rules of filing an errand in a project, as Nexus
//! UI's `lib/errand-projects.ts` and the kit's `errandProjects`. Nexus GPUI draws them (`desktop-gpui`'s Errands view
//! and sidebar).
//!
//! An errand starts loose under the Assistant; filing it moves it into a project. The Assistant stays the hub: its
//! sidebar rows show the loose errands and "2 in projects", and a Projects group lists each project with its errands.
//! A project is the name its errands share (`project` on the errand), or a Gateway project the person is in, which
//! groups repositories and people. The agent API keeps each person's errands per organization, so a project
//! holds one organization's errands only.
//!
//! A project's page reads `GET /agent/projects/view` ([`project_view_of`]): its repositories, where its files come
//! from, the sessions its errands set up, and what Nexus knows there. With Gateway's projects off, the page is its
//! errands alone.

use crate::agent::{project_id, project_name};
use crate::agent_errands::{Errand, Status, errand_name};
use serde_json::{Value, json};

/// The most errands the sidebar lists under one project; its page has the rest.
pub const PROJECT_ERRANDS: usize = 5;

fn open(errand: &Errand) -> bool {
    matches!(errand.status, Status::NeedsYou | Status::Running)
}

/// The projects the errands are filed in, by name, once each, in name order (the kit's `errandProjects`).
pub fn errand_projects(errands: &[Errand]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in errands.iter().filter_map(|errand| errand.project.as_ref()) {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
    names
}

/// The errands that stay under the Assistant's row: the loose ones.
pub fn loose_errands(errands: &[Errand]) -> Vec<Errand> {
    errands
        .iter()
        .filter(|errand| errand.project.is_none())
        .cloned()
        .collect()
}

/// The row under the loose errands: "2 in projects", for the open errands filed in one; None with none.
pub fn in_projects_text(errands: &[Errand]) -> Option<String> {
    let filed = errands
        .iter()
        .filter(|errand| errand.project.is_some() && open(errand))
        .count();
    (filed > 0).then(|| format!("{filed} in projects"))
}

/// A project's errands: what needs the person first, then what runs, then what closed, each in the API's order, at
/// most `max`.
pub fn project_errands(errands: &[Errand], name: &str, max: usize) -> Vec<Errand> {
    let mine = || errands.iter().filter(|errand| errand.project.as_deref() == Some(name));
    mine()
        .filter(|errand| errand.status == Status::NeedsYou)
        .chain(mine().filter(|errand| errand.status == Status::Running))
        .chain(mine().filter(|errand| !open(errand)))
        .take(max)
        .cloned()
        .collect()
}

/// Where a filed errand is, under its name: "in Fundraise".
pub fn in_project(errand: &Errand) -> Option<String> {
    errand.project.as_ref().map(|project| format!("in {project}"))
}

/// What the view says once an errand moved: into a project, or out of one.
pub fn moved_text(project: Option<&str>, errand: &Errand) -> String {
    let name = errand_name(errand);
    match project {
        Some(project) => format!("{name} is in {project} now."),
        None => format!("{name} is loose again, under the Assistant."),
    }
}

/// "Acme projects only": a project holds one organization's errands.
pub fn scope_note(organization: Option<&str>) -> String {
    match organization.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!("{name} projects only"),
        None => "This organization’s projects only".into(),
    }
}

/// A project page's crumb: "Acme · project".
pub fn project_crumb(organization: Option<&str>) -> String {
    match organization.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!("{name} · project"),
        None => "Project".into(),
    }
}

/// A project page's lede.
pub const PROJECT_LEDE: &str = "No repository. Its errands together, each with what it needs from you.";

/// A project page's composer: "Ask Ada about Fundraise".
pub fn ask_about(assistant: &str, project: &str) -> String {
    format!("Ask {assistant} about {project}")
}

/// Whether Gateway's projects answer (`gateway` in the agent API's projects).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gateway {
    On,
    Off,
    Unavailable,
}

/// A session one of the project's errands set up: the link card its thread showed.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSession {
    pub title: String,
    /// `running`, `done`, `failed`, `stopped` or `ready`.
    pub state: String,
    pub repo: Option<String>,
    /// `https:` or `nexus:`, as the reader checked.
    pub url: String,
}

/// One note of "What Nexus knows here".
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectNote {
    pub id: String,
    pub text: String,
}

/// `GET /agent/projects/view`, as the page draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectView {
    pub name: String,
    /// The Gateway project's id; None for a name-only project.
    pub id: Option<String>,
    /// Its repositories as `owner/name`, each with its GitHub URL.
    pub repos: Vec<(String, String)>,
    /// A name-only project's offer: the Gateway project of the same name, by id and name.
    pub link: Option<(String, String)>,
    pub gateway: Gateway,
    pub sessions: Vec<ProjectSession>,
    pub notes: Vec<ProjectNote>,
}

fn repo_name(text: &str) -> bool {
    let Some((owner, name)) = text.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && owner.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !name.is_empty()
        && !matches!(name, "." | "..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The page's body, read strictly: a project with a name the API reads, an id of its shape, its files from GitHub
/// only, and each session link `https:` or `nexus:`. A row that is not one is left out; None when the body is none.
pub fn project_view_of(body: &Value) -> Option<ProjectView> {
    let project = body.get("project")?;
    let name = string(project, "name").filter(|name| project_name(name).as_deref() == Some(name.as_str()))?;
    let id = match project.get("id") {
        None => None,
        Some(Value::String(id)) if project_id(id) => Some(id.clone()),
        Some(_) => return None,
    };
    let gateway = match body.get("gateway")?.as_str()? {
        "on" => Gateway::On,
        "off" => Gateway::Off,
        "unavailable" => Gateway::Unavailable,
        _ => return None,
    };
    let files = body.get("files")?;
    if files.get("source")?.as_str()? != "repositories" {
        return None;
    }
    let repos = files
        .get("repos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|repo| {
            let full = string(repo, "fullName").filter(|full| repo_name(full))?;
            let url = string(repo, "url").filter(|url| *url == format!("https://github.com/{full}"))?;
            Some((full, url))
        })
        .collect();
    let link = id.is_none().then(|| project.get("link")).flatten().and_then(|link| {
        let id = string(link, "id").filter(|id| project_id(id))?;
        let name = string(link, "name").filter(|name| project_name(name).is_some())?;
        Some((id, name))
    });
    let sessions = body
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|session| {
            let url = string(session, "url").filter(|url| url.starts_with("https:") || url.starts_with("nexus:"))?;
            let state = string(session, "state")
                .filter(|state| ["running", "done", "failed", "stopped", "ready"].contains(&state.as_str()))?;
            Some(ProjectSession {
                title: string(session, "title")?,
                state,
                repo: string(session, "repo"),
                url,
            })
        })
        .collect();
    let notes = body
        .get("notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|note| {
            Some(ProjectNote {
                id: string(note, "id").filter(|id| crate::agent::note_path(id).is_some())?,
                text: string(note, "text").filter(|text| !text.trim().is_empty())?,
            })
        })
        .collect();
    Some(ProjectView {
        name,
        id,
        repos,
        link,
        gateway,
        sessions,
        notes,
    })
}

/// A new errand's body in a Gateway project, by its id: it then reads the project's repositories.
pub fn goal_body_in(goal: &str, project_id: &str) -> String {
    let goal: String = goal.trim().chars().take(crate::agent::ERRAND_GOAL_CHARS).collect();
    json!({ "goal": goal, "projectId": project_id }).to_string()
}

/// `POST /agent/projects/link`'s body.
pub fn link_body(name: &str, project_id: &str) -> String {
    json!({ "name": name, "projectId": project_id }).to_string()
}

/// A note's body scoped to the project of `view`.
pub fn project_note_body(text: &str, view: &ProjectView) -> String {
    let project = match &view.id {
        Some(id) => json!({ "id": id, "name": view.name }),
        None => json!({ "name": view.name }),
    };
    json!({ "text": text.trim(), "project": project }).to_string()
}

/// What the page says under its name, from the view: a Gateway project's repositories, or a name-only one.
pub fn project_lede(view: &ProjectView) -> String {
    match (&view.id, view.repos.len()) {
        (None, _) if view.gateway == Gateway::On && view.link.is_some() => {
            "A project of your errands. A Gateway project has the same name: link them to use its repositories.".into()
        }
        (None, _) => "A project of your errands. No repository.".into(),
        (Some(_), 0) => {
            "A Gateway project with no repository yet. Sessions, errands and what Nexus knows, together.".into()
        }
        (Some(_), 1) => "A Gateway project with 1 repository. Its files come from it.".into(),
        (Some(_), count) => format!("A Gateway project with {count} repositories. Its files come from them."),
    }
}

/// Artifacts and files' line under its repositories, or in their place.
pub fn files_note(view: &ProjectView, assistant: &str) -> String {
    match (&view.id, view.repos.is_empty()) {
        (_, false) => format!("With GitHub connected, {assistant} reads these repositories' files as you when an errand here needs one."),
        (Some(_), true) => "No repository yet. Files come from the Gateway project's repositories: add one to it to bring its files here.".into(),
        (None, true) => "Files come from a Gateway project's repositories. This project has none.".into(),
    }
}

/// The link offer: "Fundraise is a Gateway project you are in. …".
pub fn link_offer(view: &ProjectView, assistant: &str) -> Option<String> {
    let (_, name) = view.link.as_ref()?;
    Some(format!(
        "{name} is a Gateway project you are in. Link {} to it: its errands and notes move there, and {assistant} reads its repositories.",
        view.name
    ))
}

/// A session row's line: its repository and its state.
pub fn session_line(session: &ProjectSession) -> String {
    let state = match session.state.as_str() {
        "running" => "Running",
        "done" => "Done",
        "failed" => "Failed",
        "stopped" => "Stopped",
        _ => "Ready to send",
    };
    match &session.repo {
        Some(repo) => format!("{repo} · {state}"),
        None => state.to_string(),
    }
}

/// "What Nexus knows here" with nothing in it yet.
pub fn knows_empty(view: &ProjectView) -> String {
    format!("Nothing yet. A note here is read only by errands in {}.", view.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_errands::errand_of;

    fn view_body() -> Value {
        json!({
            "project": { "name": "Fundraise", "id": "prj_fund", "repos": ["acme/data-room"], "errands": 1 },
            "gateway": "on",
            "errands": [],
            "history": [],
            "sessions": [
                { "title": "Metrics", "state": "ready", "where": "desktop", "repo": "acme/data-room", "url": "nexus://new?repo=acme/data-room", "at": "t", "errand": "i3" },
                { "title": "Bad", "state": "ready", "where": "desktop", "url": "javascript:alert(1)", "at": "t", "errand": "i3" },
            ],
            "files": { "source": "repositories", "repos": [
                { "fullName": "acme/data-room", "url": "https://github.com/acme/data-room" },
                { "fullName": "acme/x", "url": "https://evil.test/acme/x" },
            ] },
            "notes": [{ "id": "n1", "text": "Lead by 15 Dec.", "createdAt": "t", "audience": "dm" }],
        })
    }

    #[test]
    fn a_project_page_is_read_strictly() {
        let view = project_view_of(&view_body()).expect("a view");
        assert_eq!(view.id.as_deref(), Some("prj_fund"));
        assert_eq!(
            view.repos,
            [(
                "acme/data-room".to_string(),
                "https://github.com/acme/data-room".to_string()
            )]
        );
        assert_eq!(
            view.sessions.len(),
            1,
            "a session link that is not https: or nexus: is left out"
        );
        assert_eq!(session_line(&view.sessions[0]), "acme/data-room · Ready to send");
        assert_eq!(
            view.notes,
            [ProjectNote {
                id: "n1".into(),
                text: "Lead by 15 Dec.".into()
            }]
        );
        assert_eq!(view.gateway, Gateway::On);
        assert_eq!(view.link, None);
        assert_eq!(
            project_lede(&view),
            "A Gateway project with 1 repository. Its files come from it."
        );
        let mut bad = view_body();
        bad["files"]["source"] = json!("drive");
        assert_eq!(project_view_of(&bad), None);
        let mut bad = view_body();
        bad["project"]["id"] = json!("bad id");
        assert_eq!(project_view_of(&bad), None);
        let mut bad = view_body();
        bad["gateway"] = json!("maybe");
        assert_eq!(project_view_of(&bad), None);
    }

    #[test]
    fn a_name_only_project_offers_its_gateway_project() {
        let mut body = view_body();
        body["project"] = json!({ "name": "fundraise", "repos": [], "errands": 1, "link": { "id": "prj_fund", "name": "Fundraise" } });
        body["files"]["repos"] = json!([]);
        let view = project_view_of(&body).expect("a view");
        assert_eq!(view.link, Some(("prj_fund".into(), "Fundraise".into())));
        assert!(
            link_offer(&view, "Ada")
                .expect("a link offer")
                .starts_with("Fundraise is a Gateway project you are in. Link fundraise")
        );
        assert!(project_lede(&view).contains("link them"));
        assert_eq!(
            files_note(&view, "Ada"),
            "Files come from a Gateway project's repositories. This project has none."
        );
        assert_eq!(
            link_body("fundraise", "prj_fund"),
            r#"{"name":"fundraise","projectId":"prj_fund"}"#
        );
        assert!(crate::agent::project_link_body(Some(&link_body(
            "fundraise",
            "prj_fund"
        ))));
        assert!(!crate::agent::project_link_body(Some(
            r#"{"name":"x","projectId":"bad id"}"#
        )));
        assert_eq!(
            knows_empty(&view),
            "Nothing yet. A note here is read only by errands in fundraise."
        );
    }

    #[test]
    fn a_note_and_an_errand_go_to_the_project_the_api_reads() {
        let view = project_view_of(&view_body()).expect("a view");
        let body = project_note_body(" Lead by 15 Dec. ", &view);
        assert_eq!(
            body,
            r#"{"text":"Lead by 15 Dec.","project":{"id":"prj_fund","name":"Fundraise"}}"#
        );
        assert!(crate::agent::project_note_body(Some(&body)));
        assert!(!crate::agent::project_note_body(Some(r#"{"text":"x"}"#)));
        assert!(!crate::agent::project_note_body(Some(
            r#"{"text":"x","project":{"name":"x","id":"bad id"}}"#
        )));
        assert!(!crate::agent::project_note_body(Some(
            r#"{"text":"x","project":{"name":"x","more":1}}"#
        )));
        let goal = goal_body_in(" Book the venue ", "prj_fund");
        assert_eq!(goal, r#"{"goal":"Book the venue","projectId":"prj_fund"}"#);
        assert!(crate::agent::errand_goal(Some(&goal)));
        assert_eq!(
            crate::agent::project_view_path(None, "Northwind  Labs").as_deref(),
            Some("/agent/projects/view?name=Northwind+Labs")
        );
        assert_eq!(
            crate::agent::project_view_path(Some("prj_fund"), "Fundraise").as_deref(),
            Some("/agent/projects/view?id=prj_fund")
        );
        assert_eq!(crate::agent::project_view_path(Some("bad id"), "x"), None);
    }

    fn row(id: &str, status: &str, project: Option<&str>) -> Errand {
        let mut value =
            json!({ "id": id, "goal": format!("Goal {id}"), "status": status, "at": "2026-11-17T08:00:00Z" });
        if let (Value::Object(row), Some(project)) = (&mut value, project) {
            row.insert("project".into(), json!(project));
        }
        errand_of(&value).expect("a row")
    }

    #[test]
    fn projects_are_the_names_their_errands_share() {
        let errands = [
            row("i1", "running", Some("northwind Labs")),
            row("i2", "needs-you", Some("Fundraise")),
            row("i3", "done", Some("Fundraise")),
            row("i4", "running", None),
            row("i5", "running", Some("Fundraise")),
        ];
        assert_eq!(errand_projects(&errands), ["Fundraise", "northwind Labs"]);
        assert_eq!(
            loose_errands(&errands)
                .iter()
                .map(|errand| errand.id.as_str())
                .collect::<Vec<_>>(),
            ["i4"]
        );
        assert_eq!(
            in_projects_text(&errands).as_deref(),
            Some("3 in projects"),
            "the open ones"
        );
        assert_eq!(in_projects_text(&errands[3..4]), None);
        let fundraise: Vec<_> = project_errands(&errands, "Fundraise", PROJECT_ERRANDS)
            .into_iter()
            .map(|errand| errand.id)
            .collect();
        assert_eq!(fundraise, ["i2", "i5", "i3"], "needs you, runs, closed");
        assert_eq!(project_errands(&errands, "Fundraise", 1).len(), 1);
        assert_eq!(in_project(&errands[1]).as_deref(), Some("in Fundraise"));
        assert_eq!(in_project(&errands[3]), None);
        assert_eq!(
            moved_text(Some("Fundraise"), &errands[3]),
            "Goal i4 is in Fundraise now."
        );
        assert_eq!(
            moved_text(None, &errands[1]),
            "Goal i2 is loose again, under the Assistant."
        );
    }

    #[test]
    fn a_project_belongs_to_one_organization() {
        assert_eq!(scope_note(Some("Acme")), "Acme projects only");
        assert_eq!(scope_note(Some(" ")), "This organization’s projects only");
        assert_eq!(project_crumb(Some("Acme")), "Acme · project");
        assert_eq!(project_crumb(None), "Project");
        assert_eq!(ask_about("Ada", "Fundraise"), "Ask Ada about Fundraise");
    }
}
