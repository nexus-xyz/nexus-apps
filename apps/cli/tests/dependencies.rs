//! The CLI is a client of the Nexus API: its dependency tree holds no Gateway server crate. An open-source export of
//! the CLI must build without the server's billing, connectors, data access and Postgres store.

use std::process::Command;

/// The workspace crates the CLI may link: the client core and what it builds on, and the desktop's workbench. A new one
/// goes here only if it is client code that the open-source export can publish: not the model catalog (`nexus-catalog`),
/// which holds model prices and stays closed.
const ALLOWED: &[&str] = &[
    "nexus-agents",
    "nexus-cli",
    "nexus-client",
    "nexus-local-agents",
    "nexus-providers",
    "nexus-agent-memory",
    "nexus-workbench",
];

#[test]
fn the_cli_links_no_server_crate() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args([
            "tree",
            "--locked",
            "-p",
            "nexus-cli",
            "-e",
            "normal,build,dev",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8(output.stdout).expect("utf-8");
    let mut found: Vec<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| name.starts_with("nexus-") && !ALLOWED.contains(name))
        .collect();
    found.sort_unstable();
    found.dedup();
    assert!(
        found.is_empty(),
        "nexus-cli depends on server or closed crates: {found:?} (`cargo tree -p nexus-cli -i <crate>`)"
    );
}
