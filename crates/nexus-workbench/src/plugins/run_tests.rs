//! A plugin in a session: under Claude Code, Codex and Antigravity it starts only through the
//! launcher, in its own folder, with its declared values and no credential variable.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use serde_json::json;

use super::{Manifest, server};
use crate::app_socket::Launchables;
use crate::event::SessionAgent;
use crate::mcp_launch;
use crate::mcp_servers::{
    self, APP_TOKEN_VAR, InheritedEnv, LAUNCH_ADDRESS_VAR, LAUNCH_TOKEN_VAR, Launchable, McpServer, Values,
};

const PROGRAM: &str = "/opt/Nexus/nexus-desktop";
const FOLDER: &str = "/config/plugins/acme-notes";

/// The approved plugin, with one secret.
fn plugin() -> McpServer {
    let manifest = Manifest::parse(
        &json!({
            "apiVersion": 0,
            "id": "acme-notes",
            "name": "Acme Notes",
            "version": "1.0.0",
            "mcp": { "command": "bin/acme-notes", "args": ["--stdio"] },
            "secrets": ["ACME_TOKEN"],
        })
        .to_string(),
    )
    .expect("valid");
    server(&manifest, Path::new(FOLDER))
}

/// A Settings server, which keeps today's path.
fn settings_server() -> McpServer {
    McpServer {
        name: "db".into(),
        enabled: true,
        command: Some("srv".into()),
        args: Vec::new(),
        url: None,
        secrets: vec!["DB_TOKEN".into()],
        tool_timeout_sec: None,
        plugin: None,
    }
}

fn values() -> Values {
    Values::from([
        (
            "acme-notes".into(),
            BTreeMap::from([("ACME_TOKEN".into(), "acme-secret".into())]),
        ),
        ("db".into(), BTreeMap::from([("DB_TOKEN".into(), "db-secret".into())])),
    ])
}

/// The credential variables an agent's environment can hold when the launcher starts: the Nexus token for Codex, the
/// app socket's credential, the launch credential and the built-in server's token. `HOME` is the person's own.
const INHERITED: [&str; 5] = [
    nexus_local_agents::agents::CODEX_KEY_ENV,
    APP_TOKEN_VAR,
    LAUNCH_TOKEN_VAR,
    mcp_servers::TOKEN_VAR,
    "HOME",
];

/// What the plugin's process gets when the launcher starts it in an environment with every [`INHERITED`] variable.
fn started(launchable: &Launchable) -> std::process::Command {
    let inherited = INHERITED
        .iter()
        .map(|name| (OsString::from(name), OsString::from("sentinel-credential")));
    mcp_launch::command(launchable, inherited)
}

/// The plugin runs in its folder, with its own value, and every credential variable removed.
fn assert_runs_clean(launchable: &Launchable) {
    assert_eq!(launchable.inherited, InheritedEnv::NoCredentials);
    assert_eq!(launchable.cwd.as_deref(), Some(FOLDER));
    assert_eq!(launchable.command, format!("{FOLDER}/bin/acme-notes"));
    let command = started(launchable);
    assert_eq!(command.get_current_dir(), Some(Path::new(FOLDER)));
    let envs: BTreeMap<_, _> = command
        .get_envs()
        .map(|(name, value)| (name.to_string_lossy().into_owned(), value.map(|v| v.to_owned())))
        .collect();
    for name in INHERITED.iter().filter(|name| **name != "HOME") {
        assert_eq!(envs.get(*name), Some(&None), "{name} reaches the plugin");
    }
    assert!(!envs.contains_key("HOME"), "the person's own variables stay");
    assert_eq!(envs["ACME_TOKEN"], Some(OsString::from("acme-secret")));
    assert!(!envs.contains_key("DB_TOKEN"), "another server's value");
}

/// Runs `through_launcher_or_builtin` and keeps what the app was asked to grant.
fn through_launcher(agent: SessionAgent, granted: bool) -> (Option<mcp_servers::LauncherLaunch>, Launchables) {
    let mut asked = Launchables::new();
    let servers = [settings_server(), plugin()];
    let launched = mcp_servers::through_launcher_or_builtin(agent, PROGRAM, &servers, &values(), |launchable| {
        asked = launchable;
        granted.then(|| ("127.0.0.1:4100".to_string(), "launch-credential".to_string()))
    });
    (launched, asked)
}

#[test]
fn under_claude_code_a_plugin_starts_through_the_launcher_with_no_credential() {
    let (launched, asked) = through_launcher(SessionAgent::Claude, true);
    let launched = launched.expect("Claude Code has a launcher");
    let config = launched.args.join(" ");
    assert!(
        config.contains(&format!(
            r#""acme-notes":{{"type":"stdio","command":"{PROGRAM}","args":["--mcp-launch=acme-notes"],"env":{{}}}}"#
        )),
        "{config}"
    );
    assert!(!config.contains("bin/acme-notes"), "{config}");
    assert!(
        !launched.env.iter().any(|(_, value)| value == "acme-secret"),
        "the plugin's value stays in the app"
    );
    assert_runs_clean(&asked["acme-notes"]);
}

#[test]
fn under_codex_a_plugin_starts_through_the_launcher_and_settings_servers_as_before() {
    let (launched, asked) = through_launcher(SessionAgent::Codex, true);
    let launched = launched.expect("a plugin goes through the launcher");
    assert!(launched.left_out.is_empty());
    let args = launched.args.join(" ");
    assert!(
        args.contains(&format!(
            r#"mcp_servers.acme-notes={{command="{PROGRAM}",args=["--mcp-launch=acme-notes"],env_vars=["{LAUNCH_ADDRESS_VAR}","{LAUNCH_TOKEN_VAR}"]}}"#
        )),
        "{args}"
    );
    assert!(
        args.contains(r#"mcp_servers.db={command="srv",args=[],env_vars=["DB_TOKEN"]}"#),
        "{args}"
    );
    let env: BTreeMap<_, _> = launched.env.iter().cloned().collect();
    assert_eq!(env[LAUNCH_TOKEN_VAR], "launch-credential");
    assert_eq!(env[LAUNCH_ADDRESS_VAR], "127.0.0.1:4100");
    assert_eq!(env["DB_TOKEN"], "db-secret");
    assert!(!env.contains_key("ACME_TOKEN"), "the plugin's value stays in the app");
    assert_eq!(
        asked.keys().collect::<Vec<_>>(),
        ["acme-notes"],
        "only plugins are granted"
    );
    assert_runs_clean(&asked["acme-notes"]);
}

#[test]
fn under_codex_without_a_launch_credential_no_plugin_starts() {
    let (launched, _) = through_launcher(SessionAgent::Codex, false);
    let launched = launched.expect("a plugin goes through the launcher");
    assert_eq!(launched.left_out, ["acme-notes"]);
    let args = launched.args.join(" ");
    assert!(!args.contains("acme-notes"), "{args}");
    assert!(args.contains("mcp_servers.db="), "{args}");
}

#[test]
fn under_codex_without_a_plugin_nothing_changes() {
    let servers = [settings_server()];
    let launched = mcp_servers::through_launcher_or_builtin(SessionAgent::Codex, PROGRAM, &servers, &values(), |_| {
        panic!("nothing to grant")
    });
    assert!(launched.is_none());
}

#[test]
fn under_antigravity_a_plugin_keeps_no_credential_either() {
    let servers = [settings_server(), plugin()];
    let session = mcp_servers::session(SessionAgent::Antigravity, &servers, &values()).expect("in session");
    assert_runs_clean(&session.launchable["acme-notes"]);
    assert_eq!(
        session.launchable["db"].inherited,
        InheritedEnv::AppCredentialOnly,
        "a Settings server keeps today's environment"
    );
}

#[test]
fn no_agent_starts_a_plugin_directly() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let (args, env) = mcp_servers::launch(agent, &[settings_server(), plugin()], &values());
        let args = args.join(" ");
        assert!(!args.contains("acme-notes"), "{agent:?}: {args}");
        assert!(args.contains("db"), "{agent:?}: {args}");
        assert!(!env.iter().any(|(name, _)| name == "ACME_TOKEN"), "{agent:?}");
    }
}

#[test]
fn a_plugin_reaches_the_service_marked_as_one() {
    let handed = mcp_servers::handed(&[plugin()], &values());
    let text = serde_json::to_value(&handed).expect("json");
    assert_eq!(text["servers"][0]["plugin"], FOLDER);
    let read: mcp_servers::Handed = serde_json::from_value(text).expect("reads");
    assert!(read.servers[0].launcher_only());
}
