//! The committed `commands.json` is generated from the Rust CLI's default catalog, with SEMVER alone.
//! Docs read it through `@nexus/cli/commands.json`; commands, flags, examples and their order must stay current.

use nexus_cli::catalog::command_catalog;
use nexus_cli::json::pretty;

/// The CLI's copy of `services/frontend/core/interfaces/commands.json`, so this builds outside the monorepo.
/// `npm run commands -w @nexus/cli` writes both; `cross_package_check.py` keeps them the same bytes.
const COMMANDS_JSON: &str = include_str!("fixtures/interfaces/commands.json");

#[test]
fn the_catalog_is_commands_json() {
    let catalog = with_staff(without_plans(command_catalog(nexus_cli::SEMVER)));
    let generated = format!("{}\n", pretty(&catalog));
    if generated != COMMANDS_JSON {
        let expected: serde_json::Value = serde_json::from_str(COMMANDS_JSON).expect("commands.json");
        assert_eq!(
            catalog, expected,
            "the catalog differs; run `npm run commands -w @nexus/cli` from eng/factory"
        );
        panic!("same content, different bytes; run `npm run commands -w @nexus/cli` from eng/factory");
    }
}

/// `commands.json` is the default CLI's: a build with plans (`--features plans`) lists the dormant plan commands too, and
/// the rest must still match.
#[cfg(feature = "plans")]
fn without_plans(mut catalog: serde_json::Value) -> serde_json::Value {
    let billing = catalog["commands"]
        .as_array_mut()
        .and_then(|commands| commands.iter_mut().find(|command| command["name"] == "billing"))
        .expect("billing");
    let subcommands = billing["subcommands"].as_array_mut().expect("subcommands");
    let before = subcommands.len();
    subcommands.retain(|sub| !matches!(sub["name"].as_str(), Some("billing plan" | "billing plans")));
    assert_eq!(before - subcommands.len(), 2, "the plan commands are in the catalog");
    catalog
}

#[cfg(not(feature = "plans"))]
fn without_plans(catalog: serde_json::Value) -> serde_json::Value {
    catalog
}

/// `commands.json` is the shipped CLI's, with the staff-only commands (`staff`, a default feature). The open-source
/// build (`--no-default-features`) lists none of them, and the rest must still match.
#[cfg(feature = "staff")]
fn with_staff(catalog: serde_json::Value) -> serde_json::Value {
    catalog
}

#[cfg(not(feature = "staff"))]
fn with_staff(catalog: serde_json::Value) -> serde_json::Value {
    let mut expected: serde_json::Value = serde_json::from_str(COMMANDS_JSON).expect("commands.json");
    let commands = expected["commands"].as_array_mut().expect("commands");
    let before = commands.len();
    commands.retain(|command| command["name"] != "triage");
    assert_eq!(before - commands.len(), 1, "commands.json lists triage");
    assert_eq!(
        catalog, expected,
        "the open build's catalog is commands.json without triage"
    );
    // The comparison above is done: hand back commands.json so the caller's byte check passes too.
    serde_json::from_str(COMMANDS_JSON).expect("commands.json")
}

#[test]
fn the_catalog_uses_the_factory_semver() {
    let expected: serde_json::Value = serde_json::from_str(COMMANDS_JSON).expect("commands.json");
    assert_eq!(expected["version"], nexus_cli::SEMVER);
    assert!(nexus_cli::VERSION.starts_with(nexus_cli::SEMVER));
}

/// Session filters list the Gateway catalog. Launch and setup use the local launcher list.
#[test]
fn help_separates_session_agents_from_launchers() {
    let agents = nexus_client::agent_entries::Agent::ALL;
    let ids: Vec<&str> = agents.iter().map(|agent| agent.as_str()).collect();
    let commands: Vec<&str> = agents.iter().map(|agent| agent.command()).collect();
    let launchers: Vec<&str> = agents
        .iter()
        .filter(|agent| agent.entry().session.cli_launcher)
        .map(|agent| agent.command())
        .collect();
    for listed in [format!("<{}>", launchers.join("|")), format!("({})", ids.join(" or "))] {
        assert!(
            COMMANDS_JSON.contains(&listed),
            "commands.json lists the agents as {listed}"
        );
    }
    let catalog = command_catalog(nexus_cli::SEMVER);
    let names: Vec<&str> = catalog["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .filter_map(|command| command["name"].as_str())
        .filter(|name| commands.contains(name))
        .collect();
    assert_eq!(names, launchers);
}
