//! `--mcp-launch=<name>`: starts one of a run's MCP servers for an agent that takes its servers in its session
//! (Antigravity). The agent stores its servers' environment in plain text, so it gets only this launcher, the app's
//! socket and a launch credential ([`crate::mcp_servers::session_entries`]). The launcher asks the app for
//! the named server's command and environment, values included, and runs it on the launcher's own stdin and stdout.
//! The app answers each server once, and the credential ends with the run, so a copy on disk starts nothing else.
//! Claude Code starts each of its stdio servers through it too, with its whole environment: the launcher removes the
//! credential variables from it first ([`crate::credential_env`]).

use std::ffi::OsString;
use std::process::Command;
use std::time::Duration;

use crate::app_socket::LAUNCH_TOOLS;
use crate::mcp_servers::{APP_ADDRESS_VAR, APP_TOKEN_VAR, InheritedEnv, Launchable};
use serde_json::{Value, json};

pub use crate::mcp_servers::LAUNCH_ARG as ARG;

/// How long the launcher waits for the app to answer.
const WAIT: Duration = Duration::from_secs(10);

/// The server's name in the `--mcp-launch=<name>` argument; None when `args` does not start the launcher.
pub fn name(args: impl IntoIterator<Item = String>) -> Option<String> {
    let prefix = format!("{ARG}=");
    args.into_iter()
        .find_map(|arg| arg.strip_prefix(&prefix).map(String::from))
}

/// The request for the server `name`, with the launch credential.
pub fn request(token: &str, name: &str) -> Value {
    json!({"token": token, "tools": LAUNCH_TOOLS,
           "message": {"jsonrpc": "2.0", "id": 1, "method": "launch", "params": {"name": name}}})
}

/// The server in the app's answer, or the reason it gave none.
pub fn answer(reply: &str) -> Result<Launchable, String> {
    let reply: Value = serde_json::from_str(reply.trim()).map_err(|error| error.to_string())?;
    if let Some(error) = reply["error"]["message"].as_str() {
        return Err(error.to_string());
    }
    serde_json::from_value(reply["result"].clone()).map_err(|error| error.to_string())
}

/// The command for `server`, which would inherit the launcher's environment, `inherited`. The launcher's own credential
/// is never passed on, and the server keeps of the rest what its [`InheritedEnv`] says. The server's own environment is
/// added last, so a value it declares reaches it under any name.
pub fn command(server: &Launchable, inherited: impl IntoIterator<Item = (OsString, OsString)>) -> Command {
    let mut command = crate::process::command(&server.command);
    command.args(&server.args);
    match server.inherited {
        InheritedEnv::AppCredentialOnly => {}
        InheritedEnv::NoCredentials => {
            for (name, _) in inherited {
                if name.to_str().is_some_and(crate::credential_env::stripped) {
                    command.env_remove(&name);
                }
            }
        }
    }
    command
        .env_remove(APP_ADDRESS_VAR)
        .env_remove(APP_TOKEN_VAR)
        .envs(&server.env);
    command
}

/// How many more times the launcher asks after a failed relay. The app's socket answers a limited number of connections
/// at once and closes the rest unanswered, and an agent starts all its servers together.
const RETRIES: u32 = 3;

/// How long the launcher waits before it asks again, so connections that hold a slot can finish.
const RETRY_PAUSE: Duration = Duration::from_secs(1);

/// The first success of `attempt`, or its last error. After a failure it calls `pause` with [`RETRY_PAUSE`] and tries
/// again, up to [`RETRIES`] more times. `attempt` is the relay alone: an answer from the app, an error included, is
/// not a relay failure and is never asked for again, because the app hands each server out once. A reply lost after the
/// app answered (a read that timed out) is asked again, and then says the server has already started.
fn with_retries(
    mut attempt: impl FnMut() -> Result<String, String>,
    mut pause: impl FnMut(Duration),
) -> Result<String, String> {
    let mut result = attempt();
    for _ in 0..RETRIES {
        if result.is_ok() {
            break;
        }
        pause(RETRY_PAUSE);
        result = attempt();
    }
    result
}

/// Runs the named server when started with [`ARG`], and answers its exit code; `None` otherwise.
pub fn serve() -> Option<i32> {
    let name = name(std::env::args())?;
    let fail = |why: String| {
        eprintln!("{ARG} {name}: {why}");
        Some(1)
    };
    let Some((address, token)) = crate::mcp_bridge::app_socket_env() else {
        return fail(format!(
            "start it from a Nexus session ({APP_ADDRESS_VAR} and {APP_TOKEN_VAR})."
        ));
    };
    let request = request(&token, &name);
    let reply = with_retries(
        || crate::app_socket::relay(&address, &request, WAIT),
        std::thread::sleep,
    );
    let server = match reply.and_then(|reply| answer(&reply)) {
        Ok(server) => server,
        Err(error) => return fail(format!("Nexus for desktop did not start it: {error}")),
    };
    match run(command(&server, std::env::vars_os())) {
        Ok(code) => code,
        Err(error) => fail(format!("could not run {}: {error}", server.command)),
    }
}

/// Replaces the launcher with the server, so the agent's signals and its closing of stdin reach the server itself.
#[cfg(unix)]
fn run(mut command: Command) -> Result<Option<i32>, String> {
    use std::os::unix::process::CommandExt;
    Err(command.exec().to_string())
}

/// Runs the server on the launcher's stdin and stdout, and exits with it.
#[cfg(not(unix))]
fn run(mut command: Command) -> Result<Option<i32>, String> {
    let status = command.status().map_err(|error| error.to_string())?;
    Ok(Some(status.code().unwrap_or(1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// The name rides in the launcher's own argument, so a server named like another mode's flag is never an argument
    /// of its own: the app's other modes match whole arguments (`--mcp`, `--browser-mcp`, the web host's).
    #[test]
    fn the_launcher_reads_its_server_name_and_only_then() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        let arg = crate::mcp_servers::launch_arg("db");
        assert_eq!(name(args(&["/app", &arg])), Some("db".into()));
        let flag_named = crate::mcp_servers::launch_arg("--mcp");
        assert_eq!(name(args(&["/app", &flag_named])), Some("--mcp".into()));
        assert!(
            !args(&["/app", &flag_named])
                .iter()
                .any(|arg| arg == crate::mcp_servers::SERVE_ARG)
        );
        assert_eq!(name(args(&["/app", ARG, "db"])), None);
        assert_eq!(name(args(&["/app", "--mcp"])), None);
    }

    #[test]
    fn the_server_starts_with_its_own_values_and_without_the_launch_credential() {
        let server = Launchable {
            command: "db-mcp".into(),
            args: vec!["--stdio".into()],
            env: BTreeMap::from([("DB_TOKEN".into(), "s1".into())]),
            inherited: InheritedEnv::AppCredentialOnly,
        };
        let command = command(&server, std::iter::empty());
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&(APP_TOKEN_VAR.as_ref(), None)));
        assert!(envs.contains(&(APP_ADDRESS_VAR.as_ref(), None)));
        assert!(envs.contains(&("DB_TOKEN".as_ref(), Some("s1".as_ref()))));
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--stdio"]);
    }

    /// Claude Code's servers inherit its whole environment: every credential variable in it goes, the person's own
    /// variables stay, and the server's own values come last, the built-in server's `NEXUS_MCP_*` ones included.
    #[test]
    fn the_server_starts_without_any_credential_variable() {
        let inherited: Vec<(OsString, OsString)> = [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_CUSTOM_HEADERS",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "NEXUS_API_KEY",
            "NEXUS_APP_MCP_TOKEN",
            "NEXUS_MCP_TOKEN",
            "NEXUS_MCP_0_0",
            "NEXUS_BROWSER_TOKEN",
            "GOOGLE_CLOUD_PROJECT",
            "PATH",
        ]
        .iter()
        .map(|name| (OsString::from(name), OsString::from("inherited")))
        .collect();
        let server = |name: &str, env: &[(&str, &str)], inherited: InheritedEnv| Launchable {
            command: name.into(),
            args: Vec::new(),
            env: env
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            inherited,
        };
        let removed = |command: &Command| -> Vec<String> {
            let mut names: Vec<String> = command
                .get_envs()
                .filter(|(_, value)| value.is_none())
                .map(|(key, _)| key.to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        let value = |command: &Command, name: &str| {
            command
                .get_envs()
                .filter(|(key, _)| *key == name)
                .last()
                .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
        };

        let db = command(
            &server("db-mcp", &[("DB_TOKEN", "s1")], InheritedEnv::NoCredentials),
            inherited.clone(),
        );
        let mut expected: Vec<String> = inherited
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .filter(|name| !["GOOGLE_CLOUD_PROJECT", "PATH"].contains(&name.as_str()))
            .chain([APP_ADDRESS_VAR.to_string(), APP_TOKEN_VAR.to_string()])
            .collect();
        expected.sort();
        assert_eq!(removed(&db), expected);
        assert_eq!(value(&db, "DB_TOKEN").as_deref(), Some("s1"));

        // The built-in server's own token and socket credential reach it, after the inherited ones went.
        let nexus = command(
            &server(
                "/app",
                &[("NEXUS_MCP_TOKEN", "own"), (APP_TOKEN_VAR, "tools_tok")],
                InheritedEnv::NoCredentials,
            ),
            inherited.clone(),
        );
        assert_eq!(value(&nexus, "NEXUS_MCP_TOKEN").as_deref(), Some("own"));
        assert_eq!(value(&nexus, APP_TOKEN_VAR).as_deref(), Some("tools_tok"));
        assert_eq!(value(&nexus, "ANTHROPIC_AUTH_TOKEN"), None);

        // agy's servers keep today's two removals.
        let agy = command(&server("db-mcp", &[], InheritedEnv::AppCredentialOnly), inherited);
        assert_eq!(removed(&agy), [APP_ADDRESS_VAR, APP_TOKEN_VAR]);
    }

    /// The answer crosses the app socket: the field keeps its name, each kind its own, and an answer without the field
    /// (an Antigravity server's from an app before this field) keeps only the launcher's credential out.
    #[test]
    fn the_inherited_environment_crosses_the_socket_by_name() {
        let server = |inherited| Launchable {
            command: "db-mcp".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            inherited,
        };
        let sent = serde_json::to_value(server(InheritedEnv::NoCredentials)).unwrap_or_default();
        assert_eq!(sent["strip_credentials"], "no_credentials");
        let sent = serde_json::to_value(server(InheritedEnv::AppCredentialOnly)).unwrap_or_default();
        assert_eq!(sent["strip_credentials"], "app_credential_only");
        let reply = json!({"jsonrpc": "2.0", "id": 1, "result": {"command": "db-mcp"}});
        assert_eq!(
            answer(&reply.to_string()).map(|server| server.inherited),
            Ok(InheritedEnv::AppCredentialOnly)
        );
        let reply =
            json!({"jsonrpc": "2.0", "id": 1, "result": {"command": "db-mcp", "strip_credentials": "no_credentials"}});
        assert_eq!(
            answer(&reply.to_string()).map(|server| server.inherited),
            Ok(InheritedEnv::NoCredentials)
        );
    }

    /// The built-in server's own socket credential is its environment's, set after the launcher's is removed.
    #[test]
    fn a_server_may_carry_its_own_socket_credential() {
        let server = Launchable {
            command: "/app".into(),
            args: vec!["--mcp".into()],
            env: BTreeMap::from([(APP_TOKEN_VAR.into(), "tools_tok".into())]),
            inherited: InheritedEnv::AppCredentialOnly,
        };
        let command = command(&server, std::iter::empty());
        let last = command.get_envs().filter(|(key, _)| *key == APP_TOKEN_VAR).last();
        assert_eq!(last, Some((APP_TOKEN_VAR.as_ref(), Some("tools_tok".as_ref()))));
    }

    /// Runs `with_retries` over `results`, one per attempt, and answers its result with the pauses it took.
    fn retried(results: &[Result<&str, &str>]) -> (Result<String, String>, Vec<Duration>, usize) {
        let mut next = results.iter();
        let mut attempts = 0;
        let mut pauses = Vec::new();
        let result = with_retries(
            || {
                attempts += 1;
                next.next()
                    .expect("no more attempts than results")
                    .map(String::from)
                    .map_err(String::from)
            },
            |pause| pauses.push(pause),
        );
        (result, pauses, attempts)
    }

    #[test]
    fn a_relay_that_works_first_time_does_not_pause() {
        let (result, pauses, attempts) = retried(&[Ok("reply")]);
        assert_eq!(result, Ok("reply".into()));
        assert!(pauses.is_empty());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn a_failed_relay_is_asked_again_after_a_pause() {
        let (result, pauses, attempts) = retried(&[Err("closed"), Err("refused"), Ok("reply")]);
        assert_eq!(result, Ok("reply".into()));
        assert_eq!(pauses, [RETRY_PAUSE; 2]);
        assert_eq!(attempts, 3);
    }

    #[test]
    fn a_relay_that_keeps_failing_gives_its_last_error_after_three_pauses() {
        let (result, pauses, attempts) = retried(&[Err("one"), Err("two"), Err("three"), Err("four")]);
        assert_eq!(result, Err("four".into()));
        assert_eq!(pauses, [RETRY_PAUSE; 3]);
        assert_eq!(attempts, 4);
    }

    #[test]
    fn the_answer_is_the_server_or_the_apps_reason() {
        let ok = json!({"jsonrpc": "2.0", "id": 1, "result": {"command": "db-mcp", "args": [], "env": {}}});
        assert_eq!(
            answer(&ok.to_string()).map(|server| server.command),
            Ok("db-mcp".into())
        );
        let refused = json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32001, "message": "expired"}});
        assert_eq!(answer(&refused.to_string()), Err("expired".into()));
        assert_eq!(request("t", "db")["tools"], LAUNCH_TOOLS);
    }
}
