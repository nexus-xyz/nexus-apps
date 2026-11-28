//! The plugins a session starts: the approved, enabled ones, as MCP servers after Settings' own,
//! and their secrets' values from their own keychain entry. Each local one starts only through the launcher
//! (`nexus_workbench::mcp_servers::McpServer::launcher_only`).

use std::collections::BTreeMap;
use std::path::Path;

use nexus_workbench::mcp_servers::{McpServer, Values};

use super::approval::{self, Approval, State};
use super::store;
use crate::secrets::Vault;

/// The keychain entry with the plugins' secrets, by id: `{"<id>": {"<SECRET>": "<value>"}}`. Settings' own entry
/// (`mcp-servers`) is apart: a Settings save rewrites it with Settings' servers only.
pub const VAULT_ACCOUNT: &str = "plugin-secrets";

/// The plugins in `plugins_dir` that sessions start: approved for what they ask now, and switched on. None unless
/// plugins are on ([`super::enabled`]). A plugin whose id a Settings server took after its install does not start:
/// the two would share tool names and values.
pub fn servers(plugins_dir: &Path, approvals: &BTreeMap<String, Approval>, settings: &[McpServer]) -> Vec<McpServer> {
    if !super::enabled() {
        return Vec::new();
    }
    approved(plugins_dir, approvals, settings)
}

fn approved(plugins_dir: &Path, approvals: &BTreeMap<String, Approval>, settings: &[McpServer]) -> Vec<McpServer> {
    let (installed, _broken) = store::list(plugins_dir);
    installed
        .into_iter()
        .filter(|plugin| approval::state(&plugin.manifest, approvals) == State::Approved { enabled: true })
        .filter(|plugin| !settings.iter().any(|server| server.name == plugin.manifest.id))
        .map(|plugin| nexus_workbench::plugins::server(&plugin.manifest, &plugin.folder))
        .collect()
}

/// The plugins' saved values, by id. Read it only when a plugin has a secret.
pub fn values(vault: &Vault) -> Values {
    vault
        .get(VAULT_ACCOUNT)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-plugin-session-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// Installs `id` asking for `network`, and answers its manifest.
    fn install(root: &Path, id: &str, network: &[&str]) -> nexus_workbench::plugins::Manifest {
        let source = root.join(format!("source-{id}"));
        let _ = std::fs::remove_dir_all(&source);
        std::fs::create_dir_all(source.join("bin")).expect("folder");
        let manifest = json!({
            "apiVersion": 0, "id": id, "name": "Acme", "version": "1.0.0",
            "mcp": { "command": "bin/server" }, "permissions": { "network": network },
        });
        std::fs::write(
            source.join(nexus_workbench::plugins::MANIFEST_FILE),
            manifest.to_string(),
        )
        .expect("manifest");
        std::fs::write(source.join("bin/server"), "").expect("program");
        store::install(&root.join("plugins"), &source, &[])
            .expect("installed")
            .manifest
    }

    fn settings_server(name: &str) -> McpServer {
        McpServer {
            name: name.into(),
            enabled: true,
            command: Some("srv".into()),
            args: Vec::new(),
            url: None,
            secrets: Vec::new(),
            tool_timeout_sec: None,
            plugin: None,
        }
    }

    #[test]
    fn only_approved_enabled_plugins_start_each_in_its_folder() {
        let root = scratch("approved");
        let plugins = root.join("plugins");
        let mut approvals = BTreeMap::new();
        let notes = install(&root, "acme-notes", &[]);
        approval::approve(&notes, &mut approvals);
        let off = install(&root, "acme-off", &[]);
        approval::approve(&off, &mut approvals);
        approvals.get_mut("acme-off").expect("approved").enabled = false;
        install(&root, "acme-new", &[]);
        let changed = install(&root, "acme-changed", &[]);
        approval::approve(&changed, &mut approvals);
        install(&root, "acme-changed", &["api.example.com"]);

        let started = approved(&plugins, &approvals, &[]);
        assert_eq!(
            started.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["acme-notes"]
        );
        let folder = plugins.join("acme-notes");
        assert_eq!(started[0].plugin.as_deref(), Some(folder.to_string_lossy().as_ref()));
        assert!(started[0].launcher_only());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_plugin_named_like_a_settings_server_does_not_start() {
        let root = scratch("taken");
        let mut approvals = BTreeMap::new();
        approval::approve(&install(&root, "acme-notes", &[]), &mut approvals);
        let started = approved(&root.join("plugins"), &approvals, &[settings_server("acme-notes")]);
        assert!(started.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
