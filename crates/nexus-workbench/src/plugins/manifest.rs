//! The plugin manifest, `nexus-plugin.json`. [`Manifest::parse`] reads it and checks every rule,
//! and answers all the reasons a manifest is refused, so the author fixes them in one pass. Unknown fields are refused:
//! a misspelt permission never passes silently.

use serde::{Deserialize, Serialize};

use super::API_VERSION;

/// A plugin's manifest, as read and checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub api_version: u32,
    /// The plugin's id, and its MCP server's name: tools are `mcp__<id>__<tool>`.
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub mcp: McpEntry,
    /// Variable names (stdio) or header names (HTTP) whose values the person enters; they go in the keychain.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub permissions: Permissions,
}

/// How the plugin's MCP server runs: a program on stdio, or a remote server over streamable HTTP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum McpEntry {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Http {
        url: String,
    },
}

/// What the plugin says it uses. The person approves this list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    /// Host names, or `*` for any host.
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default)]
    pub filesystem: Vec<PathGrant>,
    /// Programs the plugin runs, by name.
    #[serde(default)]
    pub commands: Vec<String>,
}

/// One path the plugin reads or writes; `~` is the home folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathGrant {
    pub path: String,
    pub access: Access,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Read,
    Write,
}

/// Why a manifest is refused: every reason, in the order of the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError(pub Vec<String>);

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.join("; "))
    }
}

impl std::error::Error for ManifestError {}

impl Manifest {
    /// The manifest in `json`, or every reason it is refused.
    pub fn parse(json: &str) -> Result<Manifest, ManifestError> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|error| ManifestError(vec![format!("not JSON: {error}")]))?;
        // The version first: a manifest of another version may have fields this app does not know.
        match value.get("apiVersion").and_then(serde_json::Value::as_u64) {
            Some(version) if version == u64::from(API_VERSION) => {}
            Some(version) => {
                return Err(ManifestError(vec![format!(
                    "apiVersion {version} is not supported: this app reads apiVersion {API_VERSION}"
                )]));
            }
            None => return Err(ManifestError(vec!["apiVersion is required".into()])),
        }
        let manifest: Manifest =
            serde_json::from_value(value).map_err(|error| ManifestError(vec![error.to_string()]))?;
        let problems = manifest.problems();
        if problems.is_empty() {
            Ok(manifest)
        } else {
            Err(ManifestError(problems))
        }
    }

    fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if !valid_id(&self.id) {
            problems.push(format!(
                "id {:?}: 3 to 40 characters, lower-case letters, digits and -, starting with a letter",
                self.id
            ));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 60 {
            problems.push("name: 1 to 60 characters".into());
        }
        if !valid_semver(&self.version) {
            problems.push(format!("version {:?} is not SemVer (1.2.3)", self.version));
        }
        if self.description.chars().count() > 280 {
            problems.push("description: at most 280 characters".into());
        }
        if let Some(homepage) = &self.homepage
            && !url::Url::parse(homepage).is_ok_and(|url| url.scheme() == "https")
        {
            problems.push("homepage: an https URL".into());
        }
        problems.extend(self.mcp_problems());
        for secret in &self.secrets {
            if !valid_name(secret) {
                problems.push(format!("secret {secret:?}: letters, digits, _ and -"));
            } else if crate::credential_env::stripped(secret) {
                problems.push(format!(
                    "secret {secret:?}: a Nexus or provider credential, which a plugin never gets"
                ));
            }
        }
        for host in &self.permissions.network {
            if host != "*" && !valid_host(host) {
                problems.push(format!("network {host:?}: a host name, or * for any host"));
            }
        }
        for grant in &self.permissions.filesystem {
            if grant.path.trim().is_empty() {
                problems.push("filesystem: a path is empty".into());
            }
        }
        for command in &self.permissions.commands {
            if !valid_program(command) {
                problems.push(format!("command {command:?}: a program name, with no path"));
            }
        }
        problems
    }

    fn mcp_problems(&self) -> Vec<String> {
        match &self.mcp {
            McpEntry::Stdio { command, .. } => {
                if command.trim().is_empty() {
                    vec!["mcp.command is empty".into()]
                } else if inside_folder(command) || self.permissions.commands.contains(command) {
                    vec![]
                } else {
                    vec![format!(
                        "mcp.command {command:?}: a path inside the plugin folder, or a program in permissions.commands"
                    )]
                }
            }
            McpEntry::Http { url } => match url::Url::parse(url) {
                Ok(parsed) if parsed.scheme() == "https" => {
                    let host = parsed.host_str().unwrap_or_default();
                    let network = &self.permissions.network;
                    if network.iter().any(|allowed| allowed == "*" || allowed == host) {
                        vec![]
                    } else {
                        vec![format!("mcp.url: its host {host:?} is not in permissions.network")]
                    }
                }
                _ => vec!["mcp.url: an https URL".into()],
            },
        }
    }
}

/// 3 to 40 characters: lower-case letters, digits and `-`, starting with a letter, not ending with `-`.
fn valid_id(id: &str) -> bool {
    (3..=40).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && !id.ends_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .split('.')
            .all(|label| !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// A program name with no path, such as `git` or `node`.
fn valid_program(program: &str) -> bool {
    valid_name(program) || (program.contains('.') && !program.contains(['/', '\\']) && !program.starts_with('.'))
}

/// A relative path that stays inside the plugin folder: no root, no drive and no `..`.
fn inside_folder(path: &str) -> bool {
    let path = std::path::Path::new(path);
    path.is_relative()
        && !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_) | std::path::Component::CurDir))
        && path
            .components()
            .any(|part| matches!(part, std::path::Component::Normal(_)))
}

/// `MAJOR.MINOR.PATCH`, numbers without leading zeros, with an optional `-pre` and `+build`.
fn valid_semver(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let rest = &version[core.len()..];
    let numbers: Vec<&str> = core.split('.').collect();
    let number = |part: &&str| {
        !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()) && (part.len() == 1 || !part.starts_with('0'))
    };
    let extra = |part: &str| !part.is_empty() && part.split('.').all(|piece| !piece.is_empty() && valid_name(piece));
    let suffix_ok = match rest.split_once('+') {
        Some((pre, build)) => extra(build) && (pre.is_empty() || pre.strip_prefix('-').is_some_and(extra)),
        None => rest.is_empty() || rest.strip_prefix('-').is_some_and(extra),
    };
    numbers.len() == 3 && numbers.iter().all(number) && suffix_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn base() -> Value {
        json!({
            "apiVersion": 0,
            "id": "acme-notes",
            "name": "Acme Notes",
            "version": "1.2.0",
            "mcp": { "command": "bin/acme-notes", "args": ["--stdio"] },
        })
    }

    fn with(changes: Value) -> String {
        let mut manifest = base();
        for (key, value) in changes.as_object().expect("object") {
            manifest[key] = value.clone();
        }
        manifest.to_string()
    }

    fn refused(json: &str) -> Vec<String> {
        Manifest::parse(json).expect_err("refused").0
    }

    #[test]
    fn a_minimal_manifest_reads() {
        let manifest = Manifest::parse(&base().to_string()).expect("reads");
        assert_eq!(manifest.id, "acme-notes");
        assert_eq!(
            manifest.mcp,
            McpEntry::Stdio {
                command: "bin/acme-notes".into(),
                args: vec!["--stdio".into()]
            }
        );
        assert_eq!(manifest.permissions, Permissions::default());
    }

    #[test]
    fn the_design_example_reads() {
        let manifest = Manifest::parse(&with(json!({
            "description": "Search and add notes in Acme.",
            "author": "Acme",
            "homepage": "https://example.com/acme-notes",
            "secrets": ["ACME_TOKEN"],
            "permissions": {
                "network": ["api.example.com"],
                "filesystem": [{ "path": "~/Documents/Acme", "access": "read" }],
                "commands": ["git"],
            },
        })))
        .expect("reads");
        assert_eq!(manifest.permissions.filesystem[0].access, Access::Read);
        assert_eq!(manifest.secrets, ["ACME_TOKEN"]);
    }

    #[test]
    fn another_api_version_is_refused_before_its_fields_are_read() {
        let problems = refused(&json!({ "apiVersion": 1, "somethingNew": true }).to_string());
        assert_eq!(problems, ["apiVersion 1 is not supported: this app reads apiVersion 0"]);
        assert_eq!(refused(&json!({ "id": "x" }).to_string()), ["apiVersion is required"]);
        assert!(refused("{not json")[0].starts_with("not JSON"));
    }

    #[test]
    fn an_unknown_field_is_refused() {
        assert!(refused(&with(json!({ "permisions": {} })))[0].contains("permisions"));
        let typo = with(json!({ "permissions": { "netwrok": ["a.example"] } }));
        assert!(refused(&typo)[0].contains("netwrok"));
    }

    #[test]
    fn the_id_is_an_mcp_server_name() {
        for id in ["acme-notes", "a1b", "x".repeat(40).as_str()] {
            assert!(Manifest::parse(&with(json!({ "id": id }))).is_ok(), "{id}");
        }
        for id in [
            "ab",
            "Acme",
            "1acme",
            "acme_notes",
            "acme-",
            "a b c",
            "x".repeat(41).as_str(),
        ] {
            assert!(refused(&with(json!({ "id": id })))[0].starts_with("id "), "{id}");
        }
    }

    #[test]
    fn name_version_description_and_homepage_are_checked() {
        assert_eq!(refused(&with(json!({ "name": " " }))), ["name: 1 to 60 characters"]);
        for version in ["1.2.0", "0.1.0-beta.1", "1.0.0+build.5", "1.0.0-rc.1+sha.abc"] {
            assert!(
                Manifest::parse(&with(json!({ "version": version }))).is_ok(),
                "{version}"
            );
        }
        for version in ["1.2", "01.2.0", "1.2.0-", "v1.2.0", "1.2.0+"] {
            assert!(
                refused(&with(json!({ "version": version })))[0].starts_with("version"),
                "{version}"
            );
        }
        let long = "x".repeat(281);
        assert_eq!(
            refused(&with(json!({ "description": long }))),
            ["description: at most 280 characters"]
        );
        assert_eq!(
            refused(&with(json!({ "homepage": "http://example.com" }))),
            ["homepage: an https URL"]
        );
    }

    #[test]
    fn a_stdio_command_is_inside_the_folder_or_declared() {
        for command in ["bin/server", "./server.js", "server"] {
            let ok = with(json!({ "mcp": { "command": command } }));
            assert!(Manifest::parse(&ok).is_ok(), "{command}");
        }
        for command in ["/usr/bin/node", "../other/server", "bin/../../escape"] {
            let outside = with(json!({ "mcp": { "command": command } }));
            assert!(refused(&outside)[0].starts_with("mcp.command"), "{command}");
        }
        let declared = with(json!({
            "mcp": { "command": "node", "args": ["server.js"] },
            "permissions": { "commands": ["node"] },
        }));
        assert!(Manifest::parse(&declared).is_ok());
    }

    #[test]
    fn a_remote_server_is_https_on_a_declared_host() {
        let url = json!({ "mcp": { "url": "https://mcp.example.com/mcp" } });
        assert_eq!(
            refused(&with(url.clone())),
            ["mcp.url: its host \"mcp.example.com\" is not in permissions.network"]
        );
        let mut declared = url.clone();
        declared["permissions"] = json!({ "network": ["mcp.example.com"] });
        assert!(Manifest::parse(&with(declared)).is_ok());
        let mut any = url;
        any["permissions"] = json!({ "network": ["*"] });
        assert!(Manifest::parse(&with(any)).is_ok());
        let plain = with(json!({ "mcp": { "url": "http://mcp.example.com" }, "permissions": { "network": ["*"] } }));
        assert_eq!(refused(&plain), ["mcp.url: an https URL"]);
    }

    #[test]
    fn a_plugin_never_asks_for_a_nexus_or_provider_credential() {
        for secret in ["ANTHROPIC_API_KEY", "NEXUS_MCP_TOKEN"] {
            let problems = refused(&with(json!({ "secrets": [secret] })));
            assert!(
                problems[0].contains("a Nexus or provider credential"),
                "{secret}: {problems:?}"
            );
        }
        assert!(Manifest::parse(&with(json!({ "secrets": ["ACME_TOKEN", "x-api-key"] }))).is_ok());
        assert!(refused(&with(json!({ "secrets": ["BAD NAME"] })))[0].starts_with("secret"));
    }

    #[test]
    fn permissions_are_checked_one_by_one() {
        let problems = refused(&with(json!({
            "permissions": {
                "network": ["api.example.com", "bad host", "*"],
                "filesystem": [{ "path": "", "access": "write" }],
                "commands": ["git", "/bin/sh"],
            },
        })));
        assert_eq!(
            problems,
            [
                "network \"bad host\": a host name, or * for any host",
                "filesystem: a path is empty",
                "command \"/bin/sh\": a program name, with no path",
            ]
        );
        let bad_access = with(json!({ "permissions": { "filesystem": [{ "path": "~", "access": "exec" }] } }));
        assert!(refused(&bad_access)[0].contains("exec"));
    }

    #[test]
    fn every_reason_is_given_at_once() {
        let problems = refused(&with(json!({ "id": "X", "name": "", "version": "1" })));
        assert_eq!(problems.len(), 3, "{problems:?}");
    }
}
