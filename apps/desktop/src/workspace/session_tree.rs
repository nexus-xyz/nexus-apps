//! Linked sessions in the sidebar: a session a lead started sits under that lead, on an indented line, as the Nexus
//! app's `SessionList` draws `session-tree.ts`'s rows. The order rule is `nexus_workbench::session_tree`; this file
//! only applies it to a project's listed sessions and nests the drawn rows.

use std::collections::HashMap;
use std::iter::Peekable;

use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::session_tree::{Link, session_tree};

use crate::code::CodeSession;
use crate::palette::Palette;

/// `sessions` with each linked one after its parent, and its depth. A parent left out of `sessions` (a search, a
/// snooze) leaves its children at the top level.
pub(super) fn tree<'a>(sessions: &[&'a CodeSession]) -> Vec<(&'a CodeSession, usize)> {
    let links: HashMap<&str, Link> = sessions
        .iter()
        .map(|session| {
            let link = Link {
                parent: session.parent.as_deref(),
                project: &session.project_id,
                archived: session.archived,
            };
            (session.id.as_str(), link)
        })
        .collect();
    let by_id: HashMap<&str, &CodeSession> = sessions.iter().map(|session| (session.id.as_str(), *session)).collect();
    let ids: Vec<&str> = sessions.iter().map(|session| session.id.as_str()).collect();
    session_tree(&ids, &links)
        .into_iter()
        .filter_map(|row| by_id.get(row.id).map(|session| (*session, row.depth)))
        .collect()
}

/// For each row of [`tree`], whether it is in a crew: a linked session, or a lead whose children follow it. A crew's
/// rows name their agent, so a person sees which ones are Claude Code and which Codex.
pub(super) fn in_crew(rows: &[(&CodeSession, usize)]) -> Vec<bool> {
    rows.iter()
        .enumerate()
        .map(|(at, (_, depth))| *depth > 0 || rows.get(at + 1).is_some_and(|(_, next)| next > depth))
        .collect()
}

pub(super) const HANDOFF_CAPTION_HEIGHT: f32 = 16.;

pub(super) fn row_height(session: &CodeSession) -> f32 {
    if session.handoff_from.is_some() {
        super::sidebar::ROW + HANDOFF_CAPTION_HEIGHT
    } else {
        super::sidebar::ROW
    }
}

/// The line under a session that another one handed its work to: "from a Claude session".
pub(super) fn handoff_line(session: &CodeSession) -> Option<String> {
    let from = session.handoff_from.as_ref()?;
    Some(format!("from a {} session", crate::agents::ui(from.agent).short_name()))
}

/// For each row of `rows` (ids in tree order, with their depths), its family: the top-level session it hangs under
/// (itself at the top level) and the family's last row. A session dropped on a family lands before its top or after
/// its last row, never inside it (`session_drag.rs`).
pub(super) fn families<'a>(rows: &[(&'a str, usize)]) -> Vec<(&'a str, &'a str)> {
    let mut families = Vec::with_capacity(rows.len());
    let mut start = 0;
    while start < rows.len() {
        let end = rows[start + 1..]
            .iter()
            .position(|(_, depth)| *depth == 0)
            .map_or(rows.len(), |next| start + 1 + next);
        let (top, last) = (rows[start].0, rows[end - 1].0);
        families.extend(std::iter::repeat_n((top, last), end - start));
        start = end;
    }
    families
}

/// Rows drawn in tree order, each with its depth, as nested lists: a row's children hang under it behind a thin line
/// (`.wb-linked-threads`).
pub(super) fn nest(rows: Vec<(AnyElement, usize)>, palette: Palette) -> Vec<AnyElement> {
    level(&mut rows.into_iter().peekable(), 0, palette)
}

fn level(
    rows: &mut Peekable<impl Iterator<Item = (AnyElement, usize)>>,
    depth: usize,
    palette: Palette,
) -> Vec<AnyElement> {
    let mut drawn = Vec::new();
    while let Some((row, _)) = rows.next_if(|(_, at)| *at >= depth) {
        let next = rows.peek().map(|(_, at)| *at).filter(|at| *at > depth);
        drawn.push(match next {
            Some(next) => {
                let children = level(rows, next, palette);
                v_flex().child(row).child(linked(children, palette)).into_any_element()
            }
            None => row,
        });
    }
    drawn
}

/// A level of linked sessions: the Nexus app's 13px in, 8px padding and its edge line, less the 4px this sidebar's
/// rows sit further left. Each child joins the line with a short tick at its row's middle, so the eye follows who
/// started whom.
fn linked(children: Vec<AnyElement>, palette: Palette) -> impl IntoElement {
    v_flex()
        .ml(px(9.))
        .pl(px(8.))
        .border_l_1()
        .border_color(palette.border)
        .children(children.into_iter().map(|child| {
            div()
                .relative()
                .child(
                    div()
                        .absolute()
                        .left(px(-8.))
                        .top(px(super::sidebar::ROW / 2.))
                        .w(px(6.))
                        .h(px(1.))
                        .bg(palette.border),
                )
                .child(child)
        }))
}

#[cfg(test)]
mod tests {
    use super::{families, tree};
    use crate::code::CodeSession;

    #[test]
    fn each_row_knows_the_top_and_the_last_row_of_its_family() {
        let rows = [("other", 0), ("lead", 0), ("child", 1), ("grandchild", 2), ("last", 0)];
        assert_eq!(
            families(&rows),
            [
                ("other", "other"),
                ("lead", "grandchild"),
                ("lead", "grandchild"),
                ("lead", "grandchild"),
                ("last", "last"),
            ]
        );
        assert!(families(&[]).is_empty());
    }

    fn session(id: &str, parent: Option<&str>) -> CodeSession {
        let mut thread: nexus_workbench::event::ThreadSummary = serde_json::from_value(serde_json::json!({
            "id": id, "projectId": "p", "title": id, "agent": "claude", "model": null, "permissionMode": "default",
            "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [], "contextTokens": null,
            "contextWindow": null, "createdAt": 0, "updatedAt": 1, "archived": false,
        }))
        .expect("thread");
        thread.parent_thread_id = parent.map(str::to_string);
        CodeSession::of(thread)
    }

    #[test]
    fn a_linked_session_follows_its_lead_one_level_in() {
        let sessions = [
            session("child", Some("lead")),
            session("other", None),
            session("lead", None),
            session("grandchild", Some("child")),
        ];
        let listed: Vec<&CodeSession> = sessions.iter().collect();
        let rows: Vec<(&str, usize)> = tree(&listed)
            .into_iter()
            .map(|(session, depth)| (session.id.as_str(), depth))
            .collect();
        assert_eq!(rows, [("other", 0), ("lead", 0), ("child", 1), ("grandchild", 2)]);
    }

    #[test]
    fn a_lead_with_children_and_its_children_are_a_crew_and_a_lone_session_is_not() {
        let sessions = [
            session("lead", None),
            session("child", Some("lead")),
            session("alone", None),
        ];
        let listed: Vec<&CodeSession> = sessions.iter().collect();
        let rows = tree(&listed);
        let crew: Vec<(&str, bool)> = rows
            .iter()
            .zip(super::in_crew(&rows))
            .map(|((session, _), crew)| (session.id.as_str(), crew))
            .collect();
        assert_eq!(crew, [("lead", true), ("child", true), ("alone", false)]);
    }

    #[test]
    fn a_handed_off_session_says_which_agent_it_came_from() {
        let mut next = session("next", None);
        assert_eq!(super::handoff_line(&next), None);
        next.handoff_from = Some(nexus_workbench::event::HandoffFrom {
            thread_id: "old".into(),
            title: "Fix login".into(),
            agent: nexus_workbench::event::SessionAgent::Claude,
        });
        assert_eq!(super::handoff_line(&next).as_deref(), Some("from a Claude session"));
    }

    #[test]
    fn a_child_without_its_listed_lead_stays_at_the_top() {
        let sessions = [session("child", Some("lead"))];
        let listed: Vec<&CodeSession> = sessions.iter().collect();
        assert_eq!(
            tree(&listed).into_iter().map(|(_, depth)| depth).collect::<Vec<_>>(),
            [0]
        );
    }
}
