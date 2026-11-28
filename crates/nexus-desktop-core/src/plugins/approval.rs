//! What the person approved for each plugin, kept in `settings.json` under `plugins` by id. An
//! approval keeps the grant itself, not only its fingerprint, so the update card can say what a new version changes.

use std::collections::BTreeMap;

use nexus_workbench::plugins::{Manifest, McpEntry, PathGrant, Permissions, approval as fingerprint};
use serde::{Deserialize, Serialize};

/// What the person approved: how the plugin runs, its secrets and its permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub mcp: McpEntry,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub permissions: Permissions,
}

impl Grant {
    pub fn of(manifest: &Manifest) -> Grant {
        Grant {
            mcp: manifest.mcp.clone(),
            secrets: manifest.secrets.clone(),
            permissions: manifest.permissions.clone(),
        }
    }

    /// The fingerprint of this grant, as `nexus_workbench::plugins::approval` gives it for a manifest.
    fn fingerprint(&self, manifest: &Manifest) -> String {
        let mut approved = manifest.clone();
        approved.mcp = self.mcp.clone();
        approved.secrets = self.secrets.clone();
        approved.permissions = self.permissions.clone();
        fingerprint(&approved)
    }
}

/// One plugin's approval in `settings.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    /// The version the person approved, or the latest one that asked for nothing new.
    pub version: String,
    pub grant: Grant,
    /// The person's own switch, as Settings' MCP servers have.
    #[serde(default = "on")]
    pub enabled: bool,
}

fn on() -> bool {
    true
}

/// Where an installed plugin stands for the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Never approved: the approval card lists every permission.
    New,
    /// Approved for this grant; `enabled` is the person's switch.
    Approved { enabled: bool },
    /// Approved for another grant: the update card lists what changed. The plugin does not run until approved again.
    Changed { changes: Vec<String> },
}

/// Where `manifest` stands against the approvals.
pub fn state(manifest: &Manifest, approvals: &BTreeMap<String, Approval>) -> State {
    match approvals.get(&manifest.id) {
        None => State::New,
        Some(approval) if approval.grant.fingerprint(manifest) == fingerprint(manifest) => State::Approved {
            enabled: approval.enabled,
        },
        Some(approval) => State::Changed {
            changes: changes(&approval.grant, &Grant::of(manifest)),
        },
    }
}

/// Records the person's approval of `manifest`, enabled.
pub fn approve(manifest: &Manifest, approvals: &mut BTreeMap<String, Approval>) {
    approvals.insert(
        manifest.id.clone(),
        Approval {
            version: manifest.version.clone(),
            grant: Grant::of(manifest),
            enabled: true,
        },
    );
}

/// A new version whose grant is the approved one keeps its approval, under its own version number.
pub fn keep_version(manifest: &Manifest, approvals: &mut BTreeMap<String, Approval>) {
    if matches!(state(manifest, approvals), State::Approved { .. })
        && let Some(approval) = approvals.get_mut(&manifest.id)
    {
        approval.version = manifest.version.clone();
    }
}

/// What `new` changes from `old`, one line each, for the update card.
pub fn changes(old: &Grant, new: &Grant) -> Vec<String> {
    let mut lines = Vec::new();
    if old.mcp != new.mcp {
        lines.push(format!("runs {} instead of {}", describe(&new.mcp), describe(&old.mcp)));
    }
    lines.extend(listed("secret", &old.secrets, &new.secrets));
    lines.extend(listed("network", &old.permissions.network, &new.permissions.network));
    let paths = |grants: &[PathGrant]| -> Vec<String> {
        grants
            .iter()
            .map(|grant| format!("{} ({})", grant.path, access(grant)))
            .collect()
    };
    lines.extend(listed(
        "filesystem",
        &paths(&old.permissions.filesystem),
        &paths(&new.permissions.filesystem),
    ));
    lines.extend(listed("command", &old.permissions.commands, &new.permissions.commands));
    lines
}

fn describe(mcp: &McpEntry) -> String {
    match mcp {
        McpEntry::Stdio { command, args } if args.is_empty() => command.clone(),
        McpEntry::Stdio { command, args } => format!("{command} {}", args.join(" ")),
        McpEntry::Http { url } => url.clone(),
    }
}

fn access(grant: &PathGrant) -> &'static str {
    match grant.access {
        nexus_workbench::plugins::Access::Read => "read",
        nexus_workbench::plugins::Access::Write => "write",
    }
}

fn listed(kind: &str, old: &[String], new: &[String]) -> Vec<String> {
    let added = new
        .iter()
        .filter(|item| !old.contains(item))
        .map(|item| format!("adds {kind} {item}"));
    let removed = old
        .iter()
        .filter(|item| !new.contains(item))
        .map(|item| format!("drops {kind} {item}"));
    added.chain(removed).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(version: &str, network: &[&str]) -> Manifest {
        Manifest::parse(
            &json!({
                "apiVersion": 0,
                "id": "acme-notes",
                "name": "Acme Notes",
                "version": version,
                "mcp": { "command": "bin/acme-notes" },
                "secrets": ["ACME_TOKEN"],
                "permissions": { "network": network },
            })
            .to_string(),
        )
        .expect("valid")
    }

    #[test]
    fn a_plugin_is_new_until_approved_and_then_runs_enabled() {
        let mut approvals = BTreeMap::new();
        let first = manifest("1.0.0", &["api.example.com"]);
        assert_eq!(state(&first, &approvals), State::New);
        approve(&first, &mut approvals);
        assert_eq!(state(&first, &approvals), State::Approved { enabled: true });
        approvals.get_mut("acme-notes").expect("approved").enabled = false;
        assert_eq!(state(&first, &approvals), State::Approved { enabled: false });
    }

    #[test]
    fn a_new_version_that_asks_for_nothing_new_keeps_its_approval() {
        let mut approvals = BTreeMap::new();
        approve(&manifest("1.0.0", &["api.example.com"]), &mut approvals);
        let next = manifest("1.1.0", &["api.example.com"]);
        assert_eq!(state(&next, &approvals), State::Approved { enabled: true });
        keep_version(&next, &mut approvals);
        assert_eq!(approvals["acme-notes"].version, "1.1.0");
    }

    #[test]
    fn a_new_version_that_changes_a_permission_waits_and_says_what_changed() {
        let mut approvals = BTreeMap::new();
        approve(&manifest("1.0.0", &["api.example.com"]), &mut approvals);
        let more = manifest("2.0.0", &["api.example.com", "cdn.example.com"]);
        assert_eq!(
            state(&more, &approvals),
            State::Changed {
                changes: vec!["adds network cdn.example.com".into()]
            }
        );
        keep_version(&more, &mut approvals);
        assert_eq!(
            approvals["acme-notes"].version, "1.0.0",
            "a changed grant keeps the old version"
        );
        let fewer = manifest("2.0.0", &[]);
        assert_eq!(
            state(&fewer, &approvals),
            State::Changed {
                changes: vec!["drops network api.example.com".into()]
            }
        );
    }

    #[test]
    fn the_changes_name_each_part_of_the_grant() {
        let old = Grant::of(&manifest("1.0.0", &[]));
        let new = Grant {
            mcp: McpEntry::Stdio {
                command: "bin/other".into(),
                args: vec!["--fast".into()],
            },
            secrets: vec![],
            permissions: Permissions {
                network: vec![],
                filesystem: vec![PathGrant {
                    path: "~/Acme".into(),
                    access: nexus_workbench::plugins::Access::Write,
                }],
                commands: vec!["git".into()],
            },
        };
        assert_eq!(
            changes(&old, &new),
            [
                "runs bin/other --fast instead of bin/acme-notes",
                "drops secret ACME_TOKEN",
                "adds filesystem ~/Acme (write)",
                "adds command git",
            ]
        );
    }

    #[test]
    fn an_approval_round_trips_through_settings_json() {
        let mut approvals = BTreeMap::new();
        approve(&manifest("1.0.0", &["api.example.com"]), &mut approvals);
        let text = serde_json::to_string(&approvals).expect("json");
        let read: BTreeMap<String, Approval> = serde_json::from_str(&text).expect("reads");
        assert_eq!(read, approvals);
        let without_switch: Approval = serde_json::from_value(json!({
            "version": "1.0.0",
            "grant": { "mcp": { "command": "bin/acme-notes" } },
        }))
        .expect("reads");
        assert!(without_switch.enabled);
    }
}
