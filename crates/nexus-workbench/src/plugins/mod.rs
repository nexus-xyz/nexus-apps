//! Plugins (plugin API v0): a folder with a manifest, `nexus-plugin.json`, and the files of one MCP server.
//!
//! - [`manifest`]: the manifest, read and checked. A manifest that breaks a rule is refused with every reason.
//! - [`server`]: an approved plugin as the [`crate::mcp_servers::McpServer`] the sessions start, and the fingerprint
//!   of what the person approved.
//!
//! Nothing here installs, approves or runs a plugin yet: the app does that behind `NEXUS_DESKTOP_PLUGINS` (off).

pub mod manifest;
pub mod server;

pub use manifest::{Access, Manifest, ManifestError, McpEntry, PathGrant, Permissions};
pub use server::{approval, server};

/// The manifest's file name in a plugin's folder.
pub const MANIFEST_FILE: &str = "nexus-plugin.json";

/// The one manifest version this app reads.
pub const API_VERSION: u32 = 0;
