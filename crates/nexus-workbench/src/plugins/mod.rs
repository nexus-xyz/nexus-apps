//! Plugins (plugin API v0): a folder with a manifest, `nexus-plugin.json`, and the files of one MCP server.
//!
//! - [`manifest`]: the manifest, read and checked. A manifest that breaks a rule is refused with every reason.
//! - [`server`]: an approved plugin as the [`crate::mcp_servers::McpServer`] the sessions start, and the fingerprint
//!   of what the person approved.
//!
//! A local plugin runs only through the launcher, with no credential variable, under every agent
//! ([`crate::mcp_servers::McpServer::launcher_only`]). The app installs, approves and adds plugins behind
//! `NEXUS_DESKTOP_PLUGINS` (off).

pub mod manifest;
#[cfg(test)]
mod run_tests;
pub mod server;

pub use manifest::{Access, Manifest, ManifestError, McpEntry, PathGrant, Permissions};
pub use server::{approval, server};

/// The manifest's file name in a plugin's folder.
pub const MANIFEST_FILE: &str = "nexus-plugin.json";

/// The one manifest version this app reads.
pub const API_VERSION: u32 = 0;
