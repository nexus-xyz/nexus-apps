//! Code model picker rules, independent of the window and its input events.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row<A> {
    pub agent: A,
    /// None selects the agent's own default model.
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section<A> {
    Favorites,
    Agent(A),
}

pub(crate) struct Model {
    pub id: String,
    pub name: String,
}

pub(crate) struct Catalog<A> {
    pub agent: A,
    pub name: String,
    pub installed: bool,
    pub default_name: String,
    /// The models after the agent's default and aliases of it have been folded into the default row.
    pub models: Vec<Model>,
}

/// Show the resolved model without a suffix. The detail pane explains when the agent chooses it.
pub(crate) fn default_label(runs: Option<&str>) -> String {
    match runs {
        Some(model) => model.into(),
        None => "Agent model".into(),
    }
}

/// The model id of the Auto row: Gateway's router picks the model when the session starts, the person's plan first.
pub(crate) const AUTO: &str = "auto";

/// Whether `agent` takes Auto: Gateway's router picks its model, so it needs a route through Gateway (Claude Code,
/// Codex). An agent that runs only on its plan, and Nexus's own agent, take none.
pub(crate) fn takes_auto(agent: nexus_workbench::event::SessionAgent) -> bool {
    agent
        .coding()
        .is_some_and(|coding| coding.entry().billing.gateway.is_some())
}

/// An agent's rows with Auto first, when `offered`: a new session of an agent that [`takes_auto`]. An open session
/// keeps its model, so it never offers Auto.
pub(crate) fn with_auto(offered: bool, models: Vec<Model>) -> Vec<Model> {
    if !offered {
        return models;
    }
    std::iter::once(Model {
        id: AUTO.into(),
        name: "Auto".into(),
    })
    .chain(models.into_iter().filter(|model| model.id != AUTO))
    .collect()
}

/// An existing local session stays local even when the new-session form remembers a server.
pub(crate) fn target_machine(
    in_session: bool,
    session_machine: Option<String>,
    new_project_machine: Option<&str>,
) -> Option<String> {
    if in_session {
        session_machine
    } else {
        new_project_machine.map(str::to_owned)
    }
}

/// Clicks, keyboard picks and favorite shortcuts use the same current catalog and readiness check.
pub(crate) fn can_pick<A: Copy + Eq>(
    catalogs: &[Catalog<A>],
    ready: bool,
    session_agent: Option<A>,
    row: &Row<A>,
) -> bool {
    ready
        && session_agent.is_none_or(|agent| agent == row.agent)
        && catalogs.iter().any(|catalog| {
            catalog.agent == row.agent
                && catalog.installed
                && row
                    .model
                    .as_ref()
                    .is_none_or(|id| catalog.models.iter().any(|model| model.id == *id))
        })
}

/// Search spans installed agents for a new session, but never changes an existing session's agent.
pub(crate) fn rows<A: Copy + Eq>(
    catalogs: &[Catalog<A>],
    ready: bool,
    session_agent: Option<A>,
    section: Section<A>,
    favorites: &[Row<A>],
    query: &str,
) -> Vec<Row<A>> {
    if !ready {
        return Vec::new();
    }
    let query = query.trim().to_lowercase();
    if session_agent.is_none() && query.is_empty() && matches!(section, Section::Favorites) {
        let mut rows = Vec::new();
        for row in favorites {
            if can_pick(catalogs, ready, session_agent, row) && !rows.contains(row) {
                rows.push(row.clone());
            }
        }
        return rows;
    }
    catalogs
        .iter()
        .filter(|catalog| {
            catalog.installed
                && match session_agent {
                    Some(agent) => catalog.agent == agent,
                    None if query.is_empty() => section == Section::Agent(catalog.agent),
                    None => true,
                }
        })
        .flat_map(|catalog| {
            std::iter::once((None, catalog.default_name.as_str()))
                .chain(
                    catalog
                        .models
                        .iter()
                        .map(|model| (Some(model.id.as_str()), model.name.as_str())),
                )
                .filter(|(id, name)| {
                    query.is_empty()
                        || (id.is_none() && "default".contains(&query))
                        || [*name, id.unwrap_or_default(), catalog.name.as_str()]
                            .iter()
                            .any(|field| field.to_lowercase().contains(&query))
                })
                .map(|(model, _)| Row {
                    agent: catalog.agent,
                    model: model.map(str::to_owned),
                })
        })
        .collect()
}

/// The model panel's one list: Favorites, then each installed agent's rows without the favorites, so a
/// favorite shows once. A search or an open session lists as `rows` does. Also answers how many rows are favorites.
pub(crate) fn panel_rows<A: Copy + Eq>(
    catalogs: &[Catalog<A>],
    ready: bool,
    session_agent: Option<A>,
    favorites: &[Row<A>],
    query: &str,
) -> (Vec<Row<A>>, usize) {
    if session_agent.is_some() || !query.trim().is_empty() {
        return (
            rows(catalogs, ready, session_agent, Section::Favorites, favorites, query),
            0,
        );
    }
    let mut listed = listed_favorites(catalogs, ready, favorites);
    let starred = listed.len();
    for catalog in catalogs {
        for row in rows(catalogs, ready, None, Section::Agent(catalog.agent), favorites, "") {
            if !listed[..starred].contains(&row) {
                listed.push(row);
            }
        }
    }
    (listed, starred)
}

/// The favorites the panel lists, in order: once each, only those that can be picked now. Their places are the
/// Ctrl+1 to Ctrl+9 shortcuts, for the badges and the keys alike.
pub(crate) fn listed_favorites<A: Copy + Eq>(
    catalogs: &[Catalog<A>],
    ready: bool,
    favorites: &[Row<A>],
) -> Vec<Row<A>> {
    rows(catalogs, ready, None, Section::Favorites, favorites, "")
}

/// Effort belongs to a model. Keep it only when the effective model stays the same.
pub(crate) fn effort_after_pick<E>(current: Option<&str>, picked: Option<&str>, effort: Option<E>) -> Option<E> {
    if current == picked { effort } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Agent {
        Claude,
        Codex,
        Missing,
    }
    use Agent::*;

    fn row(agent: Agent, model: Option<&str>) -> Row<Agent> {
        Row {
            agent,
            model: model.map(str::to_owned),
        }
    }

    fn catalogs() -> Vec<Catalog<Agent>> {
        [
            (Claude, "Claude Code", "Opus", "sonnet", "Sonnet"),
            (Codex, "Codex", "GPT", "codex-mini", "Mini"),
            (Missing, "Missing", "Missing default", "missing-model", "Missing model"),
        ]
        .into_iter()
        .map(|(agent, name, default_name, id, model_name)| Catalog {
            agent,
            name: name.into(),
            installed: agent != Missing,
            default_name: default_name.into(),
            models: vec![Model {
                id: id.into(),
                name: model_name.into(),
            }],
        })
        .collect()
    }

    #[test]
    fn a_session_uses_its_own_machine_instead_of_the_new_session_project() {
        assert_eq!(target_machine(true, None, Some("unrelated-server")), None);
        assert_eq!(
            target_machine(true, Some("session-server".into()), Some("unrelated-server")),
            Some("session-server".into())
        );
        assert_eq!(
            target_machine(false, None, Some("project-server")),
            Some("project-server".into())
        );
        assert_eq!(target_machine(false, None, None), None);
    }

    #[test]
    fn a_new_session_lists_auto_first_and_an_open_session_never_does() {
        let models = || {
            vec![Model {
                id: "sonnet".into(),
                name: "Sonnet".into(),
            }]
        };
        let offered: Vec<String> = with_auto(true, models()).into_iter().map(|model| model.id).collect();
        assert_eq!(offered, [AUTO, "sonnet"]);
        let kept: Vec<String> = with_auto(false, models()).into_iter().map(|model| model.id).collect();
        assert_eq!(kept, ["sonnet"]);
        let mut catalog = catalogs();
        catalog[0].models = with_auto(true, std::mem::take(&mut catalog[0].models));
        assert!(can_pick(&catalog, true, None, &row(Claude, Some(AUTO))));
        assert_eq!(
            rows(&catalog, true, None, Section::Agent(Claude), &[], ""),
            [row(Claude, None), row(Claude, Some(AUTO)), row(Claude, Some("sonnet"))]
        );
        assert_eq!(
            rows(&catalog, true, None, Section::Favorites, &[], "auto"),
            [row(Claude, Some(AUTO))]
        );
    }

    #[test]
    fn the_agent_model_has_no_redundant_default_suffix() {
        assert_eq!(default_label(Some("Opus 5.5")), "Opus 5.5");
        assert_eq!(default_label(Some("GPT-6.2-Sol")), "GPT-6.2-Sol");
        assert_eq!(default_label(None), "Agent model");
    }

    #[test]
    fn searching_default_finds_the_default_row() {
        let mut catalog = catalogs();
        catalog[0].default_name = default_label(Some("Opus"));
        assert_eq!(
            rows(&catalog, true, Some(Claude), Section::Favorites, &[], "default"),
            [row(Claude, None)]
        );
    }

    #[test]
    fn browsing_lists_one_agent_with_a_single_default() {
        assert_eq!(
            rows(&catalogs(), true, None, Section::Agent(Claude), &[], ""),
            [row(Claude, None), row(Claude, Some("sonnet"))]
        );
        assert!(rows(&catalogs(), true, None, Section::Agent(Missing), &[], "").is_empty());
    }

    #[test]
    fn searching_matches_name_id_and_agent_across_installed_agents() {
        let catalog = catalogs();
        for (query, expected) in [
            ("  sOnNeT  ", vec![row(Claude, Some("sonnet"))]),
            ("codex-mini", vec![row(Codex, Some("codex-mini"))]),
            ("CODEX", vec![row(Codex, None), row(Codex, Some("codex-mini"))]),
            ("Opus", vec![row(Claude, None)]),
            ("missing", vec![]),
            ("no results", vec![]),
        ] {
            assert_eq!(
                rows(&catalog, true, None, Section::Favorites, &[], query),
                expected,
                "{query}"
            );
        }
    }

    #[test]
    fn clearing_search_returns_to_the_remembered_section() {
        let catalog = catalogs();
        let favorites = [row(Codex, Some("codex-mini"))];
        for query in ["", "   "] {
            assert_eq!(
                rows(&catalog, true, None, Section::Favorites, &favorites, query),
                favorites
            );
            assert_eq!(
                rows(&catalog, true, None, Section::Agent(Claude), &favorites, query),
                [row(Claude, None), row(Claude, Some("sonnet"))]
            );
        }
    }

    #[test]
    fn existing_session_never_lists_or_accepts_another_agent() {
        let catalog = catalogs();
        for section in [Section::Favorites, Section::Agent(Codex)] {
            assert_eq!(
                rows(&catalog, true, Some(Claude), section, &[], ""),
                [row(Claude, None), row(Claude, Some("sonnet"))]
            );
            assert!(rows(&catalog, true, Some(Claude), section, &[], "codex").is_empty());
            assert_eq!(
                rows(&catalog, true, Some(Claude), section, &[], "sonnet"),
                [row(Claude, Some("sonnet"))]
            );
        }
        assert!(!can_pick(&catalog, true, Some(Claude), &row(Codex, None)));
    }

    #[test]
    fn favorites_keep_order_but_drop_duplicates_missing_agents_and_removed_models() {
        let favorites = [
            row(Codex, Some("codex-mini")),
            row(Missing, None),
            row(Claude, Some("removed")),
            row(Codex, Some("codex-mini")),
            row(Claude, None),
        ];
        assert_eq!(
            rows(&catalogs(), true, None, Section::Favorites, &favorites, ""),
            [row(Codex, Some("codex-mini")), row(Claude, None)]
        );
        assert!(rows(&catalogs(), true, None, Section::Favorites, &[], "").is_empty());
    }

    #[test]
    fn shortcut_places_count_the_listed_favorites_not_the_stored_ones() {
        // Stored: a removed model before a kept one. The panel lists only the kept one, first, so it is Ctrl+1.
        let favorites = [row(Claude, Some("removed")), row(Codex, None)];
        let listed = listed_favorites(&catalogs(), true, &favorites);
        assert_eq!(listed, [row(Codex, None)]);
        let (panel, starred) = panel_rows(&catalogs(), true, None, &favorites, "");
        assert_eq!(panel[..starred], listed[..]);
    }

    #[test]
    fn loading_or_error_hides_rows_and_blocks_every_selection_path() {
        let catalog = catalogs();
        let favorites = [row(Claude, None)];
        for session in [None, Some(Claude)] {
            for section in [Section::Favorites, Section::Agent(Claude)] {
                for query in ["", "sonnet"] {
                    assert!(rows(&catalog, false, session, section, &favorites, query).is_empty());
                }
            }
            assert!(!can_pick(&catalog, false, session, &row(Claude, None)));
        }
        assert!(!can_pick(&catalog, true, None, &row(Missing, None)));
        assert!(!can_pick(&catalog, true, None, &row(Claude, Some("removed"))));
        assert!(can_pick(&catalog, true, None, &row(Claude, None)));
        assert!(can_pick(&catalog, true, None, &row(Claude, Some("sonnet"))));
    }

    #[test]
    fn a_catalog_refresh_invalidates_stale_rows_and_recovers_with_new_data() {
        let mut catalog = catalogs();
        let favorite = row(Claude, Some("sonnet"));
        assert!(can_pick(&catalog, true, None, &favorite));
        catalog[0].models.clear();
        assert!(!can_pick(&catalog, true, None, &favorite));
        assert!(rows(&catalog, true, None, Section::Favorites, &[favorite], "").is_empty());
        catalog[0].models.push(Model {
            id: "new-model".into(),
            name: "New model".into(),
        });
        assert_eq!(
            rows(&catalog, true, Some(Claude), Section::Agent(Claude), &[], "new"),
            [row(Claude, Some("new-model"))]
        );
    }

    #[test]
    fn the_panel_lists_favorites_once_then_every_installed_agent() {
        let favorites = [row(Codex, Some("codex-mini")), row(Missing, None)];
        let (rows, starred) = panel_rows(&catalogs(), true, None, &favorites, "");
        assert_eq!(starred, 1);
        assert_eq!(
            rows,
            [
                row(Codex, Some("codex-mini")),
                row(Claude, None),
                row(Claude, Some("sonnet")),
                row(Codex, None),
            ]
        );
        // A search and an open session list as before, with no favorites on top.
        assert_eq!(
            panel_rows(&catalogs(), true, None, &favorites, "sonnet"),
            (vec![row(Claude, Some("sonnet"))], 0)
        );
        assert_eq!(
            panel_rows(&catalogs(), true, Some(Claude), &favorites, "").0,
            [row(Claude, None), row(Claude, Some("sonnet"))]
        );
        assert_eq!(panel_rows(&catalogs(), false, None, &favorites, ""), (Vec::new(), 0));
    }

    #[test]
    fn effort_survives_only_reselecting_the_same_effective_model() {
        for (current, picked, expected) in [
            (None, None, Some("high")),
            (Some("sonnet"), Some("sonnet"), Some("high")),
            (Some("sonnet"), None, None),
            (None, Some("sonnet"), None),
            (Some("sonnet"), Some("opus"), None),
        ] {
            assert_eq!(effort_after_pick(current, picked, Some("high")), expected);
        }
        assert_eq!(effort_after_pick::<&str>(Some("sonnet"), Some("sonnet"), None), None);
    }
}
