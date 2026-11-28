//! What the app says about a subsession in another project than its parent's (`ThreadSummary::cross_project`). The
//! sidebar lists it in its own project, at the top level, so its hover card names its parent and the parent's
//! project. The parent's Agents tab and crew map list it with its subtree, and their detail text names its project.
//! No badge and no label on a row: only text where a row or node already shows some.

use nexus_workbench::crew_fold::AgentNode;

use super::Workspace;
use crate::code::CodeSession;

/// The hover card's line: "Subsession of “Fix the login” in nexus-web". A parent the app no longer lists has no
/// title or project to name.
pub(super) fn subsession_line(parent_title: Option<&str>, parent_project: Option<&str>) -> String {
    let parent = parent_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map_or_else(|| "a session".to_string(), |title| format!("“{title}”"));
    let project = parent_project
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("another project");
    format!("Subsession of {parent} in {project}")
}

/// A crew node's project note, "in nexus-web", for a linked session outside the lead's project. None for every other
/// node.
pub(super) fn node_note(node: &AgentNode, name_of: impl Fn(&str) -> Option<String>) -> Option<String> {
    let id = node.project.as_deref()?;
    Some(format!(
        "in {}",
        name_of(id).unwrap_or_else(|| "another project".into())
    ))
}

impl Workspace {
    /// `session`'s hover card line when it is a cross-project subsession; None for any other session.
    pub(super) fn cross_project_line(&self, session: &CodeSession) -> Option<String> {
        session.linked.as_ref().filter(|summary| summary.cross_project)?;
        let parent = session.parent.as_deref().and_then(|id| self.session_by_id(id));
        let project = parent.map(|parent| self.project_name(&parent.project_id));
        Some(subsession_line(
            parent.map(|parent| parent.title.as_str()),
            project.as_deref(),
        ))
    }

    /// [`node_note`] with the app's project names.
    pub(super) fn crew_project_note(&self, node: &AgentNode) -> Option<String> {
        node_note(node, |id| {
            Some(self.project_name(id).to_string()).filter(|name| !name.is_empty())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_workbench::crew_fold::fold_crew;
    use nexus_workbench::event::SessionAgent;

    #[test]
    fn the_line_names_the_parent_and_its_project() {
        assert_eq!(
            subsession_line(Some("Fix the login"), Some("nexus-web")),
            "Subsession of “Fix the login” in nexus-web"
        );
    }

    #[test]
    fn a_parent_the_app_no_longer_lists_still_reads() {
        assert_eq!(
            subsession_line(None, None),
            "Subsession of a session in another project"
        );
        assert_eq!(
            subsession_line(Some("  "), Some("api")),
            "Subsession of a session in api"
        );
        assert_eq!(
            subsession_line(Some("Fix the login"), Some("")),
            "Subsession of “Fix the login” in another project"
        );
    }

    #[test]
    fn only_a_node_outside_the_leads_project_gets_a_note() {
        let mut node = fold_crew(&[], SessionAgent::Claude).lead().clone();
        let names = |id: &str| (id == "q").then(|| "nexus-web".to_string());
        assert_eq!(node_note(&node, names), None);
        node.project = Some("q".into());
        assert_eq!(node_note(&node, names).as_deref(), Some("in nexus-web"));
        node.project = Some("gone".into());
        assert_eq!(node_note(&node, names).as_deref(), Some("in another project"));
    }
}
