//! A crew member's route: the hue it wears in every view. Shape tells the state,
//! hue tells the route. The Agents tab, the pill and the mini map read the same number.
use nexus_workbench::crew_fold::{AgentKind, Crew};

/// How many route hues there are (Ice, Apricot, Mint, Iris, Orchid). A sixth route starts again at 1.
pub(crate) const ROUTES: usize = 5;

/// A sub-agent's route, 1 to [`ROUTES`], by the order the session's sub-agents started, never by vendor or model.
/// 0 for the lead, a shell, or an id not in the tree: they wear the lead's hue.
pub(crate) fn route_of(crew: &Crew, id: &str) -> usize {
    let Some(target) = crew.resolve(id) else {
        return 0;
    };
    let mut n = 0;
    for node in &crew.nodes {
        if matches!(node.kind, AgentKind::Lead | AgentKind::Shell) {
            continue;
        }
        if node.id == target.id {
            return n % ROUTES + 1;
        }
        n += 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::route_of;
    use nexus_workbench::crew_fold::fold_crew;
    use nexus_workbench::event::{SessionAgent, StoredEvent, WbEvent};
    use serde_json::json;

    fn tool(seq: i64, id: &str, name: &str, input: serde_json::Value) -> StoredEvent {
        StoredEvent {
            thread_id: "t".into(),
            seq,
            at: seq * 1000,
            event: WbEvent::Tool {
                id: id.into(),
                name: name.into(),
                input,
                parent: None,
            },
        }
    }

    #[test]
    fn routes_follow_start_order_and_skip_the_lead_and_shells() {
        let mut events = vec![];
        for ix in 1..=7 {
            events.push(tool(
                ix,
                &format!("a{ix}"),
                "Agent",
                json!({"description": format!("a{ix}")}),
            ));
        }
        events.push(tool(
            8,
            "sh",
            "Bash",
            json!({"command": "pnpm dev", "run_in_background": true}),
        ));
        let crew = fold_crew(&events, SessionAgent::Claude);
        assert_eq!(route_of(&crew, "lead"), 0);
        assert_eq!(route_of(&crew, "a1"), 1);
        assert_eq!(route_of(&crew, "a5"), 5);
        // A sixth sub-agent starts the hues again.
        assert_eq!(route_of(&crew, "a6"), 1);
        assert_eq!(route_of(&crew, "a7"), 2);
        assert_eq!(route_of(&crew, "missing"), 0);
        if crew.resolve("sh").is_some() {
            assert_eq!(route_of(&crew, "sh"), 0);
        }
    }
}
