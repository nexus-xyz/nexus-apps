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
/// its name in any case. Open projects are looked at first, by folder, then repository, then name: an archived one
/// never makes a match ambiguous or hides an open one. Only when no open project matches is an archived one found,
/// and [`in_sight`] then refuses it with its own reason.
pub fn find_project<'a>(projects: &'a [Project], query: &str) -> Result<&'a Project, String> {
    let query = query.trim();
    if let Some(project) = projects.iter().find(|project| project.id == query) {
        return Ok(project);
    }
    let (open, archived): (Vec<&Project>, Vec<&Project>) = projects.iter().partition(|project| !project.archived);
    for candidates in [&open, &archived] {
        if let Some(found) = matching(candidates, query) {
            return found;
        }
    }
    if std::path::Path::new(query).is_absolute() {
        return Err(format!(
            "No project in Nexus holds {query}. Add its folder in the app first."
        ));
    }
    Err(format!(
        "No open project is named {query} or is its repository. Give its folder, GitHub owner/repo or id."
    ))
}

/// The project among `candidates` that `query` names by folder, repository or name; None when none does.
fn matching<'a>(candidates: &[&'a Project], query: &str) -> Option<Result<&'a Project, String>> {
    let path = std::path::Path::new(query);
    if path.is_absolute() {
        return candidates
            .iter()
            .filter(|project| path.starts_with(&project.path))
            .max_by_key(|project| project.path.len())
            .map(|project| Ok(*project));
    }
    let wanted = query.to_lowercase();
    let repository: Vec<&Project> = candidates
        .iter()
        .filter(|project| {
            project
                .remotes
                .iter()
                .any(|remote| github_repo(remote).as_deref() == Some(&wanted))
        })
        .copied()
        .collect();
    if !repository.is_empty() {
        return Some(one(repository, &format!("More than one project is {query}")));
    }
    let named: Vec<&Project> = candidates
        .iter()
        .filter(|project| !query.is_empty() && project.name.to_lowercase() == wanted)
        .copied()
        .collect();
    (!named.is_empty()).then(|| one(named, &format!("More than one project is named {query}")))
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

    fn archived(mut project: Project) -> Project {
        project.archived = true;
        project
    }

    /// An open and an archived clone of one repository, or two projects of one name: the open one, with no ambiguity.
    #[test]
    fn an_open_project_wins_over_an_archived_one() {
        let clones = [
            archived(project(
                "old",
                "site",
                "/home/a/old/site",
                &["git@github.com:acme/site.git"],
            )),
            project("new", "site", "/home/a/new/site", &["https://github.com/acme/site"]),
        ];
        assert_eq!(id(open_project(&clones, "acme/site")), Ok("new".into()));
        assert_eq!(id(open_project(&clones, "site")), Ok("new".into()));
        // An archived project's repository does not hide an open project's name.
        let both = [
            archived(project("a", "tools", "/home/a/tools", &["git@github.com:acme/web.git"])),
            project("b", "acme/web", "/home/a/web", &[]),
        ];
        assert_eq!(id(open_project(&both, "acme/web")), Ok("b".into()));
        // Nor does an archived project deeper in a folder hide the open one that holds it.
        let nested = [
            project("top", "top", "/home/a/top", &[]),
            archived(project("sub", "sub", "/home/a/top/sub", &[])),
        ];
        assert_eq!(id(open_project(&nested, "/home/a/top/sub/x")), Ok("top".into()));
    }

    /// Only when the one match is archived is the project called archived.
    #[test]
    fn archived_is_said_only_when_the_only_match_is_archived() {
        let only = [archived(project(
            "old",
            "site",
            "/home/a/site",
            &["git@github.com:acme/site.git"],
        ))];
        for query in ["acme/site", "site", "/home/a/site/src"] {
            assert_eq!(
                id(open_project(&only, query)),
                Err("The project site is archived in Nexus. Unarchive it in the app first.".into()),
                "{query}"
            );
        }
        assert_eq!(
            id(open_project(&only, "missing")),
            Err(
                "No open project is named missing or is its repository. Give its folder, GitHub owner/repo or id."
                    .into()
            )
        );
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
