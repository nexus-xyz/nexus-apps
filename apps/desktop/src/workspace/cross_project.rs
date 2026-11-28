//! What the app says about a subsession in another project than its parent's (`ThreadSummary::cross_project`). The
//! sidebar lists it in its own project, at the top level, so its hover card names its parent and the parent's
//! project. The parent's Agents tab and crew map list it with its subtree, and their detail text names its project.
//! No badge and no label on a row: only text where a row or node already shows some.

use nexus_workbench::crew_fold::AgentNode;

use super::Workspace;
use crate::code::CodeSession;

/// The most characters the hover card's line takes: it fits the card's three lines (`session_peek::PARENT_LINES`).
pub(super) const LINE_BUDGET: usize = 90;
/// A parent title keeps at least this many characters, however long the project's name.
const TITLE_FLOOR: usize = 24;

/// The hover card's line: "Subsession of “Fix the login” in nexus-web". The project comes last, so a long parent
/// title is cut to fit [`LINE_BUDGET`], ending in "…", and the project always shows. A parent the app no longer lists
/// has no title or project to name.
pub(super) fn subsession_line(parent_title: Option<&str>, parent_project: Option<&str>) -> String {
    let project = parent_project
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("another project");
    let fixed = "Subsession of “” in ".chars().count() + project.chars().count();
    let budget = LINE_BUDGET.saturating_sub(fixed).max(TITLE_FLOOR);
    let parent = parent_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map_or_else(
            || "a session".to_string(),
            |title| format!("“{}”", shorten(title, budget)),
        );
    format!("Subsession of {parent} in {project}")
}

/// `text` in at most `budget` characters: cut, it ends in "…".
fn shorten(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_string();
    }
    let kept: String = text.chars().take(budget.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
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
    fn a_long_parent_title_is_cut_and_the_project_always_shows() {
        // 80 characters, the longest title `start_session` takes, and a longer one a person typed.
        let longest = "Move the gateway's invite codes to the audited routes and check every deploy now";
        assert_eq!(longest.chars().count(), 80);
        let typed = "Move the gateway's invite codes to the audited routes ".repeat(4);
        for title in [longest, typed.as_str()] {
            for project in ["nexus-web", "a-rather-long-project-name-for-the-gateway"] {
                let line = subsession_line(Some(title), Some(project));
                assert!(line.ends_with(&format!("…” in {project}")), "{line}");
                assert!(line.chars().count() <= LINE_BUDGET, "{line}");
            }
        }
        let short = subsession_line(Some("Fix the login"), Some("nexus-web"));
        assert!(!short.contains('…'), "a short title stays whole: {short}");
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
