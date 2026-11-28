//! Finds the project a person or an agent names: by its id, a folder inside it, its GitHub `owner/repo` or its name.
//! The outside start (`nexus sessions start --project`) and `start_session`'s `project` both use it, so one word finds
//! the same project in both.

use crate::event::Project;

/// `owner/repo` from a GitHub remote URL (`git@github.com:o/r.git`, `https://github.com/o/r`), lowercased.
pub fn github_repo(remote: &str) -> Option<String> {
    let rest = remote
        .strip_prefix("git@github.com:")
        .or_else(|| remote.split_once("github.com/").map(|(_, rest)| rest))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && !repo.is_empty()).then(|| format!("{owner}/{repo}").to_lowercase())
}

/// `project`, unless the person archived it. The app hides an archived project's sessions, so a turn started there
/// would run out of sight.
pub fn in_sight(project: &Project) -> Result<&Project, String> {
    if project.archived {
        return Err(format!(
            "The project {} is archived in Nexus. Unarchive it in the app first.",
            project.name
        ));
    }
    Ok(project)
}

/// The project `query` names: its id, an absolute path inside it (the deepest project wins), GitHub `owner/repo`, or
/// its name in any case. An archived project is found too: [`in_sight`] refuses it with its own reason.
pub fn find_project<'a>(projects: &'a [Project], query: &str) -> Result<&'a Project, String> {
    let query = query.trim();
    if let Some(project) = projects.iter().find(|project| project.id == query) {
        return Ok(project);
    }
    let path = std::path::Path::new(query);
    if path.is_absolute() {
        return projects
            .iter()
            .filter(|project| path.starts_with(&project.path))
            .max_by_key(|project| project.path.len())
            .ok_or_else(|| format!("No project in Nexus holds {query}. Add its folder in the app first."));
    }
    let wanted = query.to_lowercase();
    let repository: Vec<&Project> = projects
        .iter()
        .filter(|project| {
            project
                .remotes
                .iter()
                .any(|remote| github_repo(remote).as_deref() == Some(&wanted))
        })
        .collect();
    if !repository.is_empty() {
        return one(repository, &format!("More than one project is {query}"));
    }
    let named: Vec<&Project> = projects
        .iter()
        .filter(|project| !query.is_empty() && project.name.to_lowercase() == wanted)
        .collect();
    if named.is_empty() {
        return Err(format!(
            "No open project is named {query} or is its repository. Give its folder, GitHub owner/repo or id."
        ));
    }
    one(named, &format!("More than one project is named {query}"))
}

/// The only project in `found`, else `many` with their folders.
fn one<'a>(found: Vec<&'a Project>, many: &str) -> Result<&'a Project, String> {
    match found.as_slice() {
        [one] => Ok(one),
        _ => Err(format!(
            "{many}: {}. Name one by its folder.",
            found
                .iter()
                .map(|project| project.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// [`find_project`], then [`in_sight`]: a project open and in sight in Nexus.
pub fn open_project<'a>(projects: &'a [Project], query: &str) -> Result<&'a Project, String> {
    in_sight(find_project(projects, query)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str, name: &str, path: &str, remotes: &[&str]) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            branch: Some("main".into()),
            remotes: remotes.iter().map(|remote| remote.to_string()).collect(),
            added_at: 0,
            orchestration: false,
            pinned: false,
            archived: false,
            host: Default::default(),
        }
    }

    fn projects() -> [Project; 4] {
        [
            project("p1", "nexus", "/home/a/nexus", &["git@github.com:acme/nexus.git"]),
            project("p2", "sub", "/home/a/nexus/eng/sub", &["https://github.com/acme/sub"]),
            project("p3", "Other", "/home/a/other", &[]),
            project("p4", "other", "/home/b/other", &[]),
        ]
    }

    fn id(found: Result<&Project, String>) -> Result<String, String> {
        found.map(|project| project.id.clone())
    }

    #[test]
    fn finds_a_project_by_id() {
        assert_eq!(id(find_project(&projects(), "p3")), Ok("p3".into()));
        assert_eq!(id(find_project(&projects(), " p2 ")), Ok("p2".into()));
    }

    #[test]
    fn finds_the_deepest_project_holding_a_folder() {
        let all = projects();
        assert_eq!(id(find_project(&all, "/home/a/nexus/src")), Ok("p1".into()));
        assert_eq!(id(find_project(&all, "/home/a/nexus/eng/sub/x")), Ok("p2".into()));
        let missing = find_project(&all, "/home/a/nexusish").expect_err("no project holds it");
        assert!(
            missing.contains("No project in Nexus holds /home/a/nexusish"),
            "{missing}"
        );
    }

    #[test]
    fn finds_a_project_by_its_github_repository_in_any_case() {
        assert_eq!(id(find_project(&projects(), "Acme/nexus")), Ok("p1".into()));
        assert_eq!(id(find_project(&projects(), "acme/sub")), Ok("p2".into()));
        let missing = find_project(&projects(), "acme/missing").expect_err("no such repository");
        assert!(
            missing.starts_with("No open project is named acme/missing"),
            "{missing}"
        );
    }

    #[test]
    fn finds_a_project_by_its_name_in_any_case() {
        assert_eq!(id(find_project(&projects(), "NEXUS")), Ok("p1".into()));
        assert_eq!(id(find_project(&projects(), "Sub")), Ok("p2".into()));
    }

    #[test]
    fn two_projects_with_one_name_are_named_by_their_folders() {
        let refused = find_project(&projects(), "other").expect_err("two projects are named other");
        assert_eq!(
            refused,
            "More than one project is named other: /home/a/other, /home/b/other. Name one by its folder."
        );
    }

    #[test]
    fn no_match_names_every_accepted_form() {
        let refused = find_project(&projects(), "mytest").expect_err("no project is named mytest");
        assert_eq!(
            refused,
            "No open project is named mytest or is its repository. Give its folder, GitHub owner/repo or id."
        );
        assert!(find_project(&projects(), "").is_err());
    }

    #[test]
    fn an_archived_project_is_found_but_out_of_sight() {
        let mut all = projects();
        all[0].archived = true;
        assert_eq!(id(find_project(&all, "nexus")), Ok("p1".into()));
        assert_eq!(
            id(open_project(&all, "nexus")),
            Err("The project nexus is archived in Nexus. Unarchive it in the app first.".into())
        );
        assert_eq!(id(open_project(&all, "sub")), Ok("p2".into()));
    }
}
