//! Linked-session summaries join the native agent tree.
use super::*;
use crate::event::{ThreadStatus, ThreadSummary};

impl Crew {
    /// Include descendants from this host. A child joins when it is in its parent's project, or when it is a
    /// cross-project subsession (`ThreadSummary::cross_project`), whose own subtree then follows the same rule. A link
    /// to another project without that flag, a cycle and an unrelated session never enter the tree.
    pub fn with_linked_sessions(&self, root: Option<&ThreadSummary>, summaries: &[ThreadSummary]) -> Self {
        let Some(root) = root else {
            return self.clone();
        };
        let mut crew = self.clone();
        let mut children: HashMap<&str, Vec<&ThreadSummary>> = HashMap::new();
        for summary in summaries {
            if let Some(parent) = summary.parent_thread_id.as_deref() {
                children.entry(parent).or_default().push(summary);
            }
        }
        for children in children.values_mut() {
            children.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        }
        let mut seen = std::collections::HashSet::from([root.id.as_str()]);
        let mut pending = vec![(root.id.as_str(), root.project_id.as_str(), self.lead().id.clone(), 0)];
        let mut at = 0;
        while at < pending.len() {
            let (thread, project, parent_id, depth) = pending[at].clone();
            at += 1;
            for child in children.get(thread).into_iter().flatten() {
                if (child.project_id != project && !child.cross_project) || !seen.insert(child.id.as_str()) {
                    continue;
                }
                let id = format!("session:{}", child.id);
                let mut node = self.lead().clone();
                node.id = id.clone();
                node.kind = AgentKind::Linked;
                node.task_id = None;
                node.call_id = None;
                node.provider = child.agent;
                node.title = child.title.clone();
                node.name = None;
                node.role = Some("Subsession".into());
                node.task_type = None;
                node.model = child.model.clone();
                node.status = match child.status {
                    ThreadStatus::Idle => AgentStatus::Idle,
                    ThreadStatus::Starting | ThreadStatus::Running => AgentStatus::Running,
                    ThreadStatus::Approval => AgentStatus::Needs,
                    ThreadStatus::Interrupted => AgentStatus::Stopped,
                    ThreadStatus::Error => AgentStatus::Failed,
                };
                node.status_source = StatusSource::Reported;
                node.reason = None;
                node.backgrounded = Some(true);
                node.started_at = child.created_at;
                node.ended_at = None;
                node.tool_count = 0;
                node.steps = 0;
                node.tokens = None;
                node.context_tokens = child.context_tokens;
                node.cost_usd = Some(child.cost_usd);
                node.duration_ms = None;
                node.last = None;
                node.recent.clear();
                node.summary = None;
                node.needs.clear();
                node.depth = depth + 1;
                node.parent_id = Some(parent_id.clone());
                node.children.clear();
                node.run_id = None;
                node.project = (child.project_id != root.project_id).then(|| child.project_id.clone());
                if node.status.is_live() {
                    crew.counts.agents_running += 1;
                }
                if node.status == AgentStatus::Needs {
                    crew.counts.needs += 1;
                    crew.needs.push(id.clone());
                }
                if node.status == AgentStatus::Failed {
                    crew.counts.failed += 1;
                }
                if node.status == AgentStatus::Stopped {
                    crew.counts.stopped += 1;
                }
                crew.counts.total += 1;
                if let Some(parent) = crew.names.get(&parent_id).copied() {
                    crew.nodes[parent].children.push(id.clone());
                }
                crew.names.insert(id.clone(), crew.nodes.len());
                crew.nodes.push(node);
                pending.push((child.id.as_str(), child.project_id.as_str(), id, depth + 1));
            }
        }
        crew
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn summary(id: &str, parent: Option<&str>, project: &str, status: &str) -> ThreadSummary {
        serde_json::from_value(serde_json::json!({"id":id,"projectId":project,"title":id,"agent":"claude","model":null,"permissionMode":"default","via":"nexus","status":status,"costUsd":1.0,"nexusSessions":[],"contextTokens":null,"contextWindow":null,"createdAt":1,"updatedAt":2,"archived":false,"parentThreadId":parent})).expect("valid linked session fixture")
    }
    #[test]
    fn linked_children_keep_their_hierarchy_counts_and_project_boundary() {
        let root = summary("root", None, "p", "running");
        let children = [
            summary("child", Some("root"), "p", "running"),
            summary("nested", Some("child"), "p", "approval"),
            summary("other", Some("root"), "other", "running"),
        ];
        let base = fold_crew(&[], SessionAgent::Claude);
        let crew = base.with_linked_sessions(Some(&root), &children);
        assert_eq!(crew.counts.total, 2);
        assert_eq!(crew.counts.agents_running, 2);
        assert_eq!(crew.counts.needs, 1);
        assert_eq!(
            crew.resolve("session:nested")
                .expect("nested linked session")
                .parent_id
                .as_deref(),
            Some("session:child")
        );
        assert!(crew.resolve("session:other").is_none());
        assert_eq!(base.counts.total, 0);
    }
    fn cross(id: &str, parent: &str, project: &str, status: &str) -> ThreadSummary {
        ThreadSummary {
            cross_project: true,
            ..summary(id, Some(parent), project, status)
        }
    }
    #[test]
    fn a_cross_project_child_brings_its_subtree_and_unrelated_projects_stay_out() {
        let root = summary("root", None, "p", "running");
        let sessions = [
            root.clone(),
            cross("away", "root", "q", "running"),
            summary("near", Some("away"), "q", "approval"),
            cross("far", "near", "r", "idle"),
            // A link to another project without the flag, its child, and a session of another project with no link.
            summary("stray", Some("root"), "q", "running"),
            summary("under-stray", Some("stray"), "q", "running"),
            summary("unrelated", None, "q", "running"),
        ];
        let crew = fold_crew(&[], SessionAgent::Claude).with_linked_sessions(Some(&root), &sessions);
        let node = |id: &str| crew.resolve(id).unwrap_or_else(|| panic!("{id} in the crew"));
        assert_eq!(node("session:away").parent_id.as_deref(), Some(LEAD_ID));
        assert_eq!(node("session:near").parent_id.as_deref(), Some("session:away"));
        assert_eq!(node("session:far").parent_id.as_deref(), Some("session:near"));
        assert_eq!(node("session:far").depth, 3);
        let projects = ["session:away", "session:near", "session:far"].map(|id| node(id).project.clone());
        assert_eq!(projects, [Some("q".into()), Some("q".into()), Some("r".into())]);
        for id in ["session:stray", "session:under-stray", "session:unrelated"] {
            assert!(crew.resolve(id).is_none(), "{id} stays out");
        }
        assert_eq!(
            (crew.counts.total, crew.counts.agents_running, crew.counts.needs),
            (3, 2, 1)
        );
    }
    #[test]
    fn a_child_in_the_leads_project_names_no_project() {
        let root = summary("root", None, "p", "running");
        let crew = fold_crew(&[], SessionAgent::Claude)
            .with_linked_sessions(Some(&root), &[summary("child", Some("root"), "p", "idle")]);
        assert_eq!(crew.resolve("session:child").map(|n| n.project.clone()), Some(None));
    }
    #[test]
    fn cycles_do_not_duplicate_the_root_or_a_member() {
        let root = summary("root", Some("child"), "p", "running");
        let crew = fold_crew(&[], SessionAgent::Claude).with_linked_sessions(
            Some(&root),
            &[root.clone(), summary("child", Some("root"), "p", "idle")],
        );
        assert_eq!(crew.counts.total, 1);
        assert_eq!(
            crew.resolve("session:child").expect("linked child session").status,
            AgentStatus::Idle
        );
    }
}
