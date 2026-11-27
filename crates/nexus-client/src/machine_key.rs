//! A cloud machine's own Nexus sign-in, for `nexus mcp` there.
//!
//! On a cloud machine, Gateway writes the host's folder (`~/.nexus/host`, or `NEXUS_HOST_DIR`): `cloud.json` names the
//! machine, and `credentials` holds the machine's own Nexus token, the one nexus-host takes a native session's runner
//! lease with. `nexus mcp` makes its session link with that token, so the link records the machine's key as its parent
//! and Gateway ties the link's connector writes to the lease. A write that lost its fence's environment is then refused
//! (`NEXUS_SESSION_FENCE_REQUIRED`), where the person's own CLI sign-in would tie it to nothing.
//!
//! It is read from files, not from the environment: an agent that filters its MCP servers' environment (Codex) drops
//! the fence, and would drop a credential handed the same way. The token never leaves the machine; the agent already
//! runs as the same user, with the same token as its model route's key. Nothing here is a secret built into a client.
//! Elsewhere (a laptop, a person's own server with no `cloud.json`) there is none.

use std::path::{Path, PathBuf};

/// The host's folder: `NEXUS_HOST_DIR`, else `<home>/.nexus/host` (nexus-host's `Paths::from_env`).
pub fn host_dir(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("NEXUS_HOST_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".nexus").join("host"))
}

/// The machine's own token in `dir`, when `dir` is a cloud machine's host folder signed in to `gateway`: `cloud.json`
/// names a machine and `credentials` holds a token for the same Gateway. `None` otherwise, so the caller keeps the
/// person's own sign-in. A trailing `/` does not count.
pub fn cloud_machine_key(dir: &Path, gateway: &str) -> Option<String> {
    let cloud: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("cloud.json")).ok()?).ok()?;
    cloud["machineId"].as_str().filter(|id| !id.trim().is_empty())?;
    let credentials: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("credentials")).ok()?).ok()?;
    let same = |url: &str| url.trim().trim_end_matches('/').to_string();
    let signed_in = credentials["gateway"].as_str()?;
    if same(signed_in) != same(gateway) {
        return None;
    }
    credentials["token"]
        .as_str()
        .map(str::trim)
        .filter(|token| !token.is_empty() && !token.chars().any(char::is_whitespace))
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATEWAY: &str = "https://api.nexus.inc";

    fn folder(name: &str, cloud: Option<&str>, credentials: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-machine-key-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        if let Some(cloud) = cloud {
            std::fs::write(dir.join("cloud.json"), cloud).expect("cloud.json");
        }
        if let Some(credentials) = credentials {
            std::fs::write(dir.join("credentials"), credentials).expect("credentials");
        }
        dir
    }

    fn credentials(gateway: &str, token: &str) -> String {
        serde_json::json!({ "gateway": gateway, "token": token, "org": "acme", "label": "Cloud machine" }).to_string()
    }

    #[test]
    fn a_cloud_machine_signed_in_to_the_same_gateway_gives_its_token() {
        let cloud = r#"{ "machineId": "cm_1", "gatewayUrl": "https://api.nexus.inc" }"#;
        let dir = folder(
            "same",
            Some(cloud),
            Some(&credentials("https://api.nexus.inc/", "nx_machine")),
        );
        assert_eq!(cloud_machine_key(&dir, GATEWAY).as_deref(), Some("nx_machine"));
        assert_eq!(
            cloud_machine_key(&dir, "https://api.nexus.inc/").as_deref(),
            Some("nx_machine")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn anything_else_keeps_the_persons_own_sign_in() {
        let cloud = r#"{ "machineId": "cm_1" }"#;
        let cases = [
            // A laptop, or a person's own server: no cloud.json.
            ("laptop", None, Some(credentials(GATEWAY, "nx_host"))),
            ("no-credentials", Some(cloud), None),
            (
                "no-machine",
                Some(r#"{ "machineId": " " }"#),
                Some(credentials(GATEWAY, "nx_machine")),
            ),
            ("not-json", Some("{"), Some(credentials(GATEWAY, "nx_machine"))),
            (
                "other-gateway",
                Some(cloud),
                Some(credentials("https://staging.nexus.test", "nx_machine")),
            ),
            ("empty-token", Some(cloud), Some(credentials(GATEWAY, " "))),
            ("odd-token", Some(cloud), Some(credentials(GATEWAY, "nx a"))),
        ];
        for (name, cloud, credentials) in cases {
            let dir = folder(name, cloud, credentials.as_deref());
            assert_eq!(cloud_machine_key(&dir, GATEWAY), None, "{name}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn the_host_folder_is_nexus_host_dir_else_under_home() {
        let home = Path::new("/home/a");
        let unset = |_: &str| None;
        assert_eq!(host_dir(home, &unset), PathBuf::from("/home/a/.nexus/host"));
        let empty = |_: &str| Some(String::new());
        assert_eq!(host_dir(home, &empty), PathBuf::from("/home/a/.nexus/host"));
        let moved = |name: &str| (name == "NEXUS_HOST_DIR").then(|| "/srv/host".to_string());
        assert_eq!(host_dir(home, &moved), PathBuf::from("/srv/host"));
    }
}
