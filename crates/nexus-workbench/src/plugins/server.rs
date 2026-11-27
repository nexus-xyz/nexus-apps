//! An approved plugin as the MCP server the sessions start, and the fingerprint of what the person
//! approved: a new version keeps its approval while it asks for nothing more.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::manifest::{Manifest, McpEntry};
use crate::mcp_servers::McpServer;

/// The plugin installed in `folder` as one MCP server: named by its id, its program resolved inside the folder (or the
/// declared program by name), and its declared secrets. It starts enabled: the app adds only approved plugins.
pub fn server(manifest: &Manifest, folder: &Path) -> McpServer {
    let (command, args, url) = match &manifest.mcp {
        McpEntry::Stdio { command, args } => {
            let program = if manifest.permissions.commands.contains(command) {
                command.clone()
            } else {
                folder.join(command).to_string_lossy().into_owned()
            };
            (Some(program), args.clone(), None)
        }
        McpEntry::Http { url } => (None, Vec::new(), Some(url.clone())),
    };
    McpServer {
        name: manifest.id.clone(),
        enabled: true,
        command,
        args,
        url,
        secrets: manifest.secrets.clone(),
        tool_timeout_sec: None,
    }
}

/// The fingerprint of what the person approves: how the plugin runs, its secrets and its permissions, each list
/// sorted, so the order of a manifest's lines does not matter. Its name, version and description are not part of it.
pub fn approval(manifest: &Manifest) -> String {
    let sorted = |items: &[String]| {
        let mut items = items.to_vec();
        items.sort();
        items.dedup();
        items
    };
    let mut filesystem: Vec<_> = manifest
        .permissions
        .filesystem
        .iter()
        .map(|grant| (grant.path.clone(), grant.access))
        .collect();
    filesystem.sort();
    filesystem.dedup();
    let approved = serde_json::json!({
        "mcp": manifest.mcp,
        "secrets": sorted(&manifest.secrets),
        "network": sorted(&manifest.permissions.network),
        "filesystem": filesystem,
        "commands": sorted(&manifest.permissions.commands),
    });
    let digest = Sha256::digest(approved.to_string().as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(changes: serde_json::Value) -> Manifest {
        let mut value = json!({
            "apiVersion": 0,
            "id": "acme-notes",
            "name": "Acme Notes",
            "version": "1.2.0",
            "mcp": { "command": "bin/acme-notes", "args": ["--stdio"] },
            "secrets": ["ACME_TOKEN"],
            "permissions": { "network": ["api.example.com", "cdn.example.com"], "commands": ["git"] },
        });
        for (key, change) in changes.as_object().expect("object") {
            value[key] = change.clone();
        }
        Manifest::parse(&value.to_string()).expect("valid")
    }

    #[test]
    fn a_stdio_plugin_runs_its_program_from_its_folder() {
        let folder = Path::new("/home/ada/.config/nexus/plugins/acme-notes");
        let server = server(&manifest(json!({})), folder);
        assert_eq!(server.name, "acme-notes");
        assert!(server.enabled);
        assert_eq!(
            server.command.as_deref(),
            Some(folder.join("bin/acme-notes").to_str().expect("utf-8"))
        );
        assert_eq!(server.args, ["--stdio"]);
        assert_eq!(server.url, None);
        assert_eq!(server.secrets, ["ACME_TOKEN"]);
    }

    #[test]
    fn a_declared_program_runs_by_name_and_a_remote_plugin_by_url() {
        let folder = Path::new("/plugins/acme-notes");
        let node = manifest(json!({
            "mcp": { "command": "node", "args": ["server.js"] },
            "permissions": { "commands": ["node"] },
        }));
        assert_eq!(server(&node, folder).command.as_deref(), Some("node"));
        let remote = manifest(json!({
            "mcp": { "url": "https://mcp.example.com/mcp" },
            "permissions": { "network": ["mcp.example.com"] },
        }));
        let remote = server(&remote, folder);
        assert_eq!(
            (remote.command, remote.url.as_deref()),
            (None, Some("https://mcp.example.com/mcp"))
        );
    }

    #[test]
    fn the_approval_ignores_order_and_wording_but_not_a_new_permission() {
        let approved = approval(&manifest(json!({})));
        let reordered = manifest(json!({
            "version": "1.3.0",
            "description": "Now faster.",
            "permissions": { "network": ["cdn.example.com", "api.example.com"], "commands": ["git"] },
        }));
        assert_eq!(
            approval(&reordered),
            approved,
            "a new version that asks for nothing more keeps its approval"
        );
        for more in [
            json!({ "permissions": { "network": ["api.example.com", "cdn.example.com", "evil.example"], "commands": ["git"] } }),
            json!({ "permissions": { "network": ["api.example.com", "cdn.example.com"], "commands": ["git", "curl"] } }),
            json!({ "secrets": ["ACME_TOKEN", "ACME_OTHER"] }),
            json!({ "mcp": { "command": "bin/other" } }),
        ] {
            assert_ne!(approval(&manifest(more.clone())), approved, "{more}");
        }
        let write = manifest(json!({
            "permissions": {
                "network": ["api.example.com", "cdn.example.com"],
                "commands": ["git"],
                "filesystem": [{ "path": "~/Acme", "access": "write" }],
            },
        }));
        assert_ne!(approval(&write), approved);
        assert_eq!(approved.len(), 64);
    }
}
