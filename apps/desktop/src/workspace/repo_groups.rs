//! One project per repository. The Code store keeps a project per folder, and each cloud session has a
//! machine and a project of its own, so one repository can be several Code projects: a folder on this computer, a
//! checkout on a server, and one per cloud session. The sidebar and the new session's project menu show them as one
//! project, named for its repository. A project with no remote stays a project of its own.

use crate::cloud::CloudRepo;
use crate::code::{CodeProject, CodeSession};

/// The repository a project's remotes name, as `host/owner/name` in lower case: `origin` first, as the store lists them.
pub(super) fn repo_key(remotes: &[String]) -> Option<String> {
    remotes.iter().find_map(|remote| {
        let repo = crate::links::repo_ref(remote)?;
        Some(format!("{}/{}", repo.host, repo.path).to_lowercase())
    })
}

/// The key of a repository picked from GitHub, to match it with the projects.
pub(super) fn cloud_repo_key(repo: &CloudRepo) -> Option<String> {
    repo_key(std::slice::from_ref(&repo.url))
}

/// The GitHub repository a project works on, for a cloud session on it. None for a folder with no GitHub remote.
pub(super) fn cloud_repo_of(project: &CodeProject) -> Option<CloudRepo> {
    project.remotes.iter().find_map(|remote| {
        let repo = crate::links::repo_ref(remote)?;
        (repo.host == "github.com").then(|| CloudRepo {
            name: repo.path.clone(),
            url: format!("https://github.com/{}", repo.path),
            private: false,
            pushed_at: None,
        })
    })
}

/// One project in the list: the Code projects of one repository. `lead` names it and takes its "+" and its fold: the
/// one on this computer when there is one, else the first.
pub(super) struct RepoGroup<'a, T> {
    pub lead: &'a CodeProject,
    pub members: Vec<&'a CodeProject>,
    pub sessions: Vec<T>,
}

impl<'a, T> RepoGroup<'a, T> {
    pub fn ids(&self) -> Vec<String> {
        self.members.iter().map(|project| project.id.clone()).collect()
    }

    /// Where its members run, the lead's first.
    pub fn machines(&self) -> Vec<Option<&'a str>> {
        self.members.iter().map(|project| project.machine.as_deref()).collect()
    }
}

/// `listed`'s projects, one per repository, in the order each repository first shows (`code::project_order`). A group
/// of several takes all their sessions in `code::place_order`: what their agents do never moves one.
pub(super) fn group<'a>(listed: Vec<(&'a CodeProject, Vec<&'a CodeSession>)>) -> Vec<RepoGroup<'a, &'a CodeSession>> {
    let mut groups: Vec<RepoGroup<'a, &'a CodeSession>> = Vec::new();
    let mut keys: Vec<Option<String>> = Vec::new();
    for (project, sessions) in listed {
        let key = repo_key(&project.remotes);
        let found = key
            .as_ref()
            .and_then(|key| keys.iter().position(|other| other.as_ref() == Some(key)));
        match found {
            Some(ix) => {
                let group = &mut groups[ix];
                if group.lead.machine.is_some() && project.machine.is_none() {
                    group.lead = project;
                    group.members.insert(0, project);
                } else {
                    group.members.push(project);
                }
                group.sessions.extend(sessions);
            }
            None => {
                keys.push(key);
                groups.push(RepoGroup {
                    lead: project,
                    members: vec![project],
                    sessions,
                });
            }
        }
    }
    for group in &mut groups {
        if group.members.len() > 1 {
            group.sessions.sort_by(|a, b| crate::code::place_order(a, b));
        }
    }
    groups
}

/// All the projects, one per repository, with no sessions: the new session's project menu.
pub(super) fn group_projects(projects: &[CodeProject]) -> Vec<RepoGroup<'_, ()>> {
    group(projects.iter().map(|project| (project, Vec::new())).collect())
        .into_iter()
        .map(|group| RepoGroup {
            lead: group.lead,
            members: group.members,
            sessions: Vec::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cloud_session_s_repository_keys_as_its_project_does() {
        // `start_cloud` saves the picked repository's URL; the projects read from the machine name it by its remote.
        let here = project("here", Some("git@github.com:acme/nexus.git"), None);
        let picked = "https://github.com/acme/nexus".to_string();
        assert_eq!(repo_key(std::slice::from_ref(&picked)), repo_key(&here.remotes));
    }

    fn project(id: &str, remote: Option<&str>, machine: Option<&str>) -> CodeProject {
        CodeProject {
            id: id.into(),
            name: id.into(),
            branch: Some("main".into()),
            pinned: false,
            remotes: remote.map(String::from).into_iter().collect(),
            sessions: Vec::new(),
            machine: machine.map(String::from),
            added_at: 0,
        }
    }

    #[test]
    fn a_repository_s_sessions_keep_their_place_while_agents_work() {
        let here = project("here", Some("https://github.com/acme/nexus"), None);
        let cloud = project("cloud", Some("https://github.com/acme/nexus"), Some("m1"));
        let session = |id: &str, acted_at: i64, updated_at: i64| {
            let mut session = crate::workspace::session_keys::tests::session(id);
            (session.acted_at, session.updated_at) = (acted_at, updated_at);
            session
        };
        // "worked" finished a turn just now; "wrote" is where the person last sent a message.
        let worked = session("worked", 10, 900);
        let wrote = session("wrote", 20, 30);
        let groups = group(vec![(&here, vec![&worked]), (&cloud, vec![&wrote])]);
        let order: Vec<_> = groups[0].sessions.iter().map(|session| session.id.as_str()).collect();
        assert_eq!(order, ["wrote", "worked"]);
    }

    #[test]
    fn one_repository_is_one_project_wherever_it_runs() {
        let cloud = project("cloud-1", Some("https://github.com/acme/nexus"), Some("m1"));
        let here = project("here", Some("git@github.com:acme/nexus.git"), None);
        let other = project("other", Some("https://github.com/acme/site"), None);
        let notes = project("notes", None, None);
        let more_notes = project("notes-2", None, None);
        let projects = [cloud, here, other, notes, more_notes];
        let groups = group_projects(&projects);
        assert_eq!(groups.len(), 4);
        // This computer's copy leads, where the repository first showed.
        assert_eq!(groups[0].lead.id, "here");
        assert_eq!(groups[0].ids(), vec!["here", "cloud-1"]);
        assert_eq!(groups[0].machines(), vec![None, Some("m1")]);
        // Folders with no remote never merge.
        assert_eq!(groups[2].lead.id, "notes");
        assert_eq!(groups[3].lead.id, "notes-2");
    }

    #[test]
    fn a_github_project_gives_its_repository_for_the_cloud() {
        let here = project("here", Some("git@github.com:acme/nexus.git"), None);
        let repo = cloud_repo_of(&here).unwrap();
        assert_eq!(repo.name, "acme/nexus");
        assert_eq!(cloud_repo_key(&repo), repo_key(&here.remotes));
        assert!(cloud_repo_of(&project("gl", Some("https://gitlab.com/a/b"), None)).is_none());
    }
}
