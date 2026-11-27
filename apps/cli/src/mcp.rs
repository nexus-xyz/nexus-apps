//! `nexus mcp`: the Nexus MCP server ([`nexus_client::mcp`]) on stdio, under the CLI's saved sign-in. The
//! TypeScript CLI's twin is `interfaces/src/mcp.ts`.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nexus_client::mcp::{LocalTool, tool_result};
use reqwest::Method;
use serde_json::{Value, json};

use crate::VERSION;
use crate::cli::Cmd;
use crate::client::{CLI_VERSION_HEADER, ClientOptions, USER_AGENT};
use crate::error::Result;
use crate::local_review::{Options, collect, result, review};

/// The product Nexus Review belongs to.
const PRODUCT: &str = "robustness";
/// How often the tool asks for the run's report.
const POLL: Duration = Duration::from_millis(1500);

/// A client for Gateway at `url`, as `nexus mcp`. `cli-mcp/…` starts with `cli`, so `cli_too_old` applies; its `-mcp`
/// makes the audit log's surface `mcp`.
fn gateway(
    url: &str,
    api_key: Option<String>,
    mut headers: Vec<(String, String)>,
    timeout: u64,
) -> nexus_client::blocking::Client {
    headers.push((CLI_VERSION_HEADER.to_string(), VERSION.to_string()));
    nexus_client::blocking::Client::new(ClientOptions {
        api_key,
        headers,
        client: Some(format!("cli-mcp/{VERSION}")),
        timeout: std::time::Duration::from_secs(timeout),
        user_agent: Some(USER_AGENT.to_string()),
        ..ClientOptions::new(url)
    })
}

/// Serve on stdin and stdout until the agent closes stdin, or until a signal: Codex and Claude Code end their servers
/// with SIGTERM or SIGINT. Either way the session's link is deleted first.
pub fn serve(cmd: &Cmd) -> Result<i32> {
    let key = cmd.config.api_key.clone();
    let url = cmd.config.url.clone();
    let plain = |api_key: Option<String>, method: Method, path: &str, body: Option<Value>| {
        gateway(&url, api_key, Vec::new(), 30).call(method, path, body)
    };
    let host = cmd.ctx.hostname.clone().unwrap_or_else(|| "this machine".into());
    // On a cloud machine, its own Nexus token makes the session's link, so Gateway ties the link's connector
    // writes to the native session's runner lease, also when the fence's environment did not reach this server.
    let env = cmd.ctx.env_fn();
    let host_dir = nexus_client::machine_key::host_dir(&nexus_client::config::home_dir(&env), &env);
    let machine = nexus_client::machine_key::cloud_machine_key(&host_dir, &url);
    let session = Arc::new(nexus_client::mcp::SessionLink::new(key, format!("nexus mcp on {host}")).made_with(machine));
    #[cfg(unix)]
    end_on_signal(session.clone(), url.clone());
    let unfenced = |method: Method, path: &str, headers: Vec<(String, String)>, body: Option<Value>| {
        // The tool's own headers ride on the call, not on the link's own request.
        let with_headers = |api_key: Option<String>, method: Method, path: &str, body: Option<Value>| {
            gateway(&url, api_key, headers.clone(), 120).call(method, path, body)
        };
        session.call(&plain, &with_headers, method, path, body)
    };
    // Inside a native session, the runner that started the agent handed its fence in the environment: each
    // connector change carries it, and none goes once Gateway said the runner lost the session.
    let fence = nexus_client::fence::Fence::from_env(|name| cmd.ctx.var(name));
    let send = nexus_client::mcp::fenced(fence.as_ref(), &unfenced);
    let cwd = cmd.ctx.cwd.clone();
    let memory = nexus_agent_memory::Store::from_env(&cmd.ctx.env_fn())
        .ok()
        .and_then(|store| {
            nexus_agent_memory::tools::Session::with_store(
                store,
                &cwd,
                cmd.ctx.var(nexus_agent_memory::tools::AGENT_ENV),
                cmd.ctx.var(nexus_agent_memory::tools::SESSION_ENV),
            )
        });
    let mut local = vec![review_local(cwd, &send)];
    if let Some(memory) = &memory {
        local.extend(nexus_workbench::memory::local_tools(memory));
    }
    nexus_client::mcp::serve_with(
        std::io::stdin().lock(),
        VERSION,
        &mut |line| cmd.ctx.io.out(line),
        &send,
        &local,
    );
    session.end(&plain);
    Ok(0)
}

/// On SIGTERM, SIGHUP or SIGINT, deletes the session's link, then exits as the signal asks. A SIGKILL still leaves the
/// link, and Windows has no handler; Gateway ends the oldest such link when the person reaches the live-link cap,
/// and it expires after `nexus_client::mcp::SESSION_LINK_HOURS`.
#[cfg(unix)]
fn end_on_signal(session: Arc<nexus_client::mcp::SessionLink>, url: String) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;
    let Ok(mut signals) = Signals::new([SIGTERM, SIGHUP, SIGINT]) else {
        return;
    };
    std::thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            session.end(
                &|api_key: Option<String>, method: Method, path: &str, body: Option<Value>| {
                    gateway(&url, api_key, Vec::new(), 5).call(method, path, body)
                },
            );
            std::process::exit(128 + signal);
        }
    });
}

/// The tool's name, description, schema, annotation and refusal, shared with the TypeScript server
/// (`interfaces/src/mcp-review-local.ts`) so the two CLIs list the same tool.
const DEFINITION: &str = include_str!("mcp-review-local.json");

fn definition() -> Value {
    serde_json::from_str(DEFINITION).expect("mcp-review-local.json")
}

/// The refusal of a `path` outside the server's folder: a 400 with its code, as a Gateway refusal reads.
pub const PATH_OUTSIDE: &str = "path_outside";

/// `review_local` (S4.4): `nexus robustness run review --local --json` as a tool. It reads the working tree, so it is
/// local, not a Gateway operation. It is not read-only: it sends the change's files to Nexus Review and spends the
/// caller's model budget, so the agent asks before it calls it. Its `path` stays inside the folder `nexus mcp` runs in.
pub fn review_local<'a>(cwd: PathBuf, send: &'a nexus_client::mcp::Gateway<'a>) -> LocalTool<'a> {
    let definition = definition();
    LocalTool {
        name: definition["name"].as_str().unwrap_or_default().into(),
        description: definition["description"].as_str().unwrap_or_default().into(),
        input_schema: definition["inputSchema"].clone(),
        read_only: false,
        call: Box::new(move |args: &Value| {
            let text = |name: &str| args[name].as_str().filter(|value| !value.is_empty()).map(String::from);
            let options = Options {
                repo: text("repo"),
                model: text("model"),
                exclude: args["exclude"]
                    .as_array()
                    .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
                    .unwrap_or_default(),
            };
            let gateway = |method: Method, path: &str, body: Option<Value>| {
                send(method, path, vec![], body).map_err(|error| error.to_string())
            };
            let Some(dir) = inside(&cwd, text("path").as_deref()) else {
                let message = definition["refusals"][PATH_OUTSIDE].as_str().unwrap_or_default();
                let refusal = json!({ "error": { "status": 400, "code": PATH_OUTSIDE, "message": message } });
                return tool_result(&refusal, true);
            };
            let answer = collect(&dir, &options).and_then(|collected| {
                review(&gateway, PRODUCT, &collected, POLL).map(|report| result(&report, &collected))
            });
            match answer {
                Ok(answer) => tool_result(&answer, false),
                Err(message) => tool_result(&json!({ "error": { "message": message } }), true),
            }
        }),
    }
}

/// The folder `path` names inside `cwd`, or `None`: an absolute path, a `..` or a symbolic link out of `cwd` is
/// refused, so an agent cannot point the tool at another repository on the machine.
fn inside(cwd: &Path, path: Option<&str>) -> Option<PathBuf> {
    let Some(path) = path else {
        return Some(cwd.to_path_buf());
    };
    let relative = Path::new(path);
    if !relative
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let (root, real) = (cwd.canonicalize().ok()?, cwd.join(relative).canonicalize().ok()?);
    real.starts_with(&root).then_some(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_stays_inside_the_folder_the_server_runs_in() {
        let root = std::env::temp_dir().join(format!("nexus-mcp-inside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("app/src")).expect("dir");
        std::fs::create_dir_all(root.join("other")).expect("dir");
        let cwd = root.join("app");
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("other"), cwd.join("out")).expect("symlink");
        let real = cwd.canonicalize().expect("real");
        assert_eq!(inside(&cwd, None), Some(cwd.clone()));
        assert_eq!(inside(&cwd, Some("src")), Some(real.join("src")));
        assert_eq!(inside(&cwd, Some("./src")), Some(real.join("src")));
        let outside = root.join("other").display().to_string();
        let refused = ["..", "../other", "src/../../other", outside.as_str(), "/"];
        #[cfg(unix)]
        let refused = [refused.as_slice(), &["out"]].concat();
        for path in refused {
            assert!(inside(&cwd, Some(path)).is_none(), "{path}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn review_local_is_not_read_only() {
        let send = |_: Method, _: &str, _: Vec<(String, String)>, _: Option<Value>| -> nexus_client::Result<Value> {
            unreachable!("not called")
        };
        assert!(!review_local(PathBuf::from("."), &send).read_only);
    }
}

pub(crate) const MEMORY_SERVER: &str = "nexus_memory";

pub(crate) fn memory_launch_args(
    agent: nexus_client::agent_entries::Agent,
    program: &Path,
    store: &Path,
    cwd: &Path,
    session: &str,
) -> Vec<String> {
    use nexus_client::agent_entries::Agent;
    let args = json!(["__memory_mcp", store, cwd, agent.as_str(), session]);
    let program = json!(program);
    match agent {
        Agent::Claude => vec![
            "--mcp-config".into(),
            json!({ "mcpServers": { (MEMORY_SERVER): { "command": program, "args": args } } }).to_string(),
        ],
        Agent::Codex => vec![
            "-c".into(),
            format!("mcp_servers.{MEMORY_SERVER}={{command={program},args={args}}}"),
        ],
        _ => Vec::new(),
    }
}

pub(crate) fn serve_memory(args: &[String], ctx: &crate::Context) -> i32 {
    let [root, cwd, agent, id] = args else { return 2 };
    let memory = nexus_agent_memory::tools::Session::with_store(
        nexus_agent_memory::Store::new(PathBuf::from(root)),
        Path::new(cwd),
        Some(agent.clone()),
        Some(id.clone()),
    );
    nexus_workbench::mcp_bridge::serve_lines(
        std::io::stdin().lock(),
        &mut |line| ctx.io.out(line),
        &|request| memory_response(memory.as_ref(), request),
        None,
    );
    0
}

fn memory_response(memory: Option<&nexus_agent_memory::tools::Session>, request: &Value) -> Option<Value> {
    let id = request.get("id")?;
    let response = match request["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": nexus_client::mcp::negotiate_protocol(&request["params"]["protocolVersion"]),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": MEMORY_SERVER, "version": VERSION}
        }),
        Some("ping") => json!({}),
        Some("tools/list") => json!({ "tools": memory
            .map(|_| nexus_agent_memory::tools::definitions().to_vec()).unwrap_or_default() }),
        Some("tools/call") => {
            let result = memory
                .ok_or_else(|| "Shared memory is off. Turn it on to use these entries.".to_string())
                .and_then(|memory| {
                    memory.call(
                        request["params"]["name"].as_str().unwrap_or_default(),
                        &request["params"]["arguments"],
                    )
                });
            match result {
                Ok(value) => tool_result(&value, false),
                Err(message) => tool_result(&json!({"message": message}), true),
            }
        }
        _ => return Some(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":response}))
}

#[cfg(test)]
mod memory_tests {
    use super::*;
    use nexus_client::agent_entries::Agent;

    #[test]
    fn enabled_launch_configs_attach_only_the_memory_server() {
        for agent in [Agent::Claude, Agent::Codex] {
            let args = memory_launch_args(agent, Path::new("nexus"), Path::new("store"), Path::new("repo"), "id");
            assert_eq!(args.len(), 2);
            assert_eq!(args[0], if agent == Agent::Claude { "--mcp-config" } else { "-c" });
            assert!(args[1].contains("nexus_memory") && args[1].contains("__memory_mcp"));
        }
        let request = json!({"id": 1, "method": "tools/call", "params": {"name": "ask"}});
        assert_eq!(
            memory_response(None, &request).expect("tool response")["result"]["isError"],
            true
        );
        let list = memory_response(None, &json!({"id": 2, "method":"tools/list"})).expect("tool list");
        assert_eq!(list["result"]["tools"], json!([]));
    }
}
