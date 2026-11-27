//! The app's tools for the agents of a person's own server, as the app answers its background service's
//! (`code_run.rs`, `ServiceSink`). The server's host passes each granted call over the SSH connection
//! (`host:app_call`, `nexus_desktop_mcp::service_relay`), and this build answers it with `host_app_reply`:
//!
//! - `request_secret`: the card opens in the session's window, as for a session here. The value goes back in the answer
//!   (`secret_request::answer_remote`), and the server's host writes the file the agent reads.
//! - The pull request tools: the same watch as a session here (`pr_watch.rs`).
//! - `nexus_browser`: the session's browser in this build, while the person is signed in to the run's organization.
//!
//! The server also gets Settings' MCP servers with their values from the keychain (`host_mcp_servers`), when it
//! connects and after each save. A cloud machine gets none of this: Gateway relays it, and its host refuses both calls
//! from a relayed client. Several apps connected to one server: the first answer wins, as on the background service.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use nexus_desktop_mcp::service_relay::{Pushed, reply};
use nexus_remote::machine::Remote;
use nexus_workbench::event::ThreadSummary;
use serde_json::Value;

use super::{Caller, Machines};

/// What this build answers a server's tool calls with, set by the session runner once it runs (`code_run.rs`).
#[derive(Clone)]
pub struct ServerTools {
    /// The open `request_secret` cards: the same as this computer's sessions', so the window shows them alike.
    pub secrets: crate::secret_request::GpuiSecrets,
    pub runtime: tokio::runtime::Handle,
    /// Hands a browser call to the windows.
    pub browser: crate::app_socket::BrowserPost,
    /// The store's folder, the browser calls' origin.
    pub data: PathBuf,
    /// The organization this build is signed in to (`<gateway>|<org>`): a call of another is refused.
    pub scope: Arc<dyn Fn() -> String + Send + Sync>,
}

/// What the server tools keep: the runner's handles, the runs that ended, and Settings' servers as last handed.
#[derive(Default)]
pub(super) struct Held {
    tools: Option<ServerTools>,
    ended: HashSet<String>,
    mcp: Option<Value>,
}

/// Whether a server's browser call still holds: this build is signed in to the run's organization, and the run goes
/// on. The run's person is the server's own sign-in, never this build's, so it is not compared.
pub(super) fn call_holds(grant_scope: &str, run: &str, scope: &str, ended: &HashSet<String>) -> bool {
    grant_scope == scope && !ended.contains(run)
}

impl Machines {
    /// The session runner's handles for the servers' tool calls: from now on they are answered.
    pub fn set_server_tools(&self, tools: ServerTools) {
        if let Ok(mut held) = self.server_tools.lock() {
            held.tools = Some(tools);
        }
    }

    /// Whether a server said its run `run` ended.
    #[cfg(test)]
    pub(super) fn server_run_ended(&self, run: &str) -> bool {
        self.server_tools.lock().is_ok_and(|held| held.ended.contains(run))
    }

    /// Every server's sessions, as last read: `request_secret` finds its session's title here.
    pub fn server_threads(&self) -> Vec<ThreadSummary> {
        self.state
            .lock()
            .map(|state| {
                state
                    .servers
                    .iter()
                    .filter(|server| matches!(server.reach, super::Reach::Ssh(_)))
                    .flat_map(|server| server.threads.values().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Session `thread` as server `machine` last said, when `machine` is a server reached over SSH: a browser call of
    /// its agent opens that session's browser. Never a cloud machine's session.
    pub fn server_session(&self, machine: &str, thread: &str) -> Option<ThreadSummary> {
        let state = self.state.lock().ok()?;
        let server = state.servers.iter().find(|server| server.setting.id == machine)?;
        match server.reach {
            super::Reach::Ssh(_) => server.threads.get(thread).cloned(),
            super::Reach::Cloud => None,
        }
    }

    /// A server reached over SSH that never connects, holding session `summary`: a window test's server session. Its
    /// failed connect tells no one, so no window wakes off the test's thread.
    #[cfg(test)]
    pub(crate) fn fixture_server_session(&self, machine: &str, summary: ThreadSummary) {
        struct Offline;
        impl nexus_remote::machine::Link for Offline {
            fn open(&self) -> futures::future::BoxFuture<'_, Result<nexus_remote::machine::Opened, String>> {
                Box::pin(async { Err("offline fixture".to_string()) })
            }
            fn update(&self) -> futures::future::BoxFuture<'_, Result<(), String>> {
                Box::pin(async { Ok(()) })
            }
        }
        impl nexus_remote::machine::Sink for Offline {
            fn push(&self, _: &nexus_remote::client::Pushed) {}
            fn status(&self, _: &nexus_remote::machine::Status) {}
        }
        let setting = nexus_desktop_core::settings::MachineSetting {
            id: machine.into(),
            name: machine.into(),
            kind: "ssh".into(),
            ssh: "fixture@invalid".into(),
            cloud: None,
        };
        let reach = {
            let _inside = self.runtime.enter();
            super::Reach::Ssh(Remote::start_manual(Arc::new(Offline), Arc::new(Offline), self.backoff))
        };
        let mut threads = std::collections::HashMap::new();
        threads.insert(summary.id.clone(), summary);
        if let Ok(mut state) = self.state.lock() {
            state.servers.push(super::Server {
                setting,
                reach,
                read: true,
                projects: Vec::new(),
                threads,
                followed: Default::default(),
                since_read: None,
            });
        }
    }

    /// Server `machine`'s push for the app's tools. True when it was one.
    pub(super) fn app_tool(&self, machine: &str, push: &nexus_remote::client::Pushed) -> bool {
        let Some(pushed) = nexus_desktop_mcp::service_relay::read(&push.event, &push.payload) else {
            return false;
        };
        let Ok(Caller::Remote(remote)) = self.caller(machine) else {
            return true;
        };
        match pushed {
            Pushed::Call(call) => {
                let Some(tools) = self.server_tools.lock().ok().and_then(|held| held.tools.clone()) else {
                    return true;
                };
                let (me, machine) = (self.me.clone(), machine.to_string());
                std::thread::spawn(move || {
                    let args = reply(&call, &(tools.scope)(), |grant, input| {
                        answer(&tools, &me, &machine, grant, input)
                    });
                    let answered = tools
                        .runtime
                        .block_on(remote.call(nexus_desktop_mcp::app_socket::REPLY_CALL, args));
                    if let Err(error) = answered {
                        eprintln!("nexus-desktop-gpui: could not answer a server's tool call: {error}");
                    }
                });
            }
            Pushed::RunEnded(run) => {
                if let Ok(mut held) = self.server_tools.lock() {
                    if let Some(tools) = &held.tools {
                        tools.secrets.end_run(&run);
                    }
                    held.ended.insert(run);
                }
                crate::side::browser::agent::REQUESTS.prune_invalid();
            }
        }
        true
    }

    /// Settings' MCP servers with their values, as `crate::mcp::for_service` makes them: every connected server
    /// gets them now, and each that connects later.
    pub fn hand_mcp_servers(&self, handed: Value) {
        if let Ok(mut held) = self.server_tools.lock() {
            held.mcp = Some(handed.clone());
        }
        for (machine, remote) in self.ssh_remotes() {
            self.send_mcp_servers(&machine, remote, handed.clone());
        }
    }

    /// Server `machine` connected: it gets Settings' MCP servers as last handed.
    pub(super) fn hand_mcp_servers_to(&self, machine: &str) {
        let handed = self.server_tools.lock().ok().and_then(|held| held.mcp.clone());
        if let (Some(handed), Ok(Caller::Remote(remote))) = (handed, self.caller(machine)) {
            self.send_mcp_servers(machine, remote, handed);
        }
    }

    fn send_mcp_servers(&self, machine: &str, remote: Remote, handed: Value) {
        let machine = machine.to_string();
        self.runtime.spawn(async move {
            // An older host takes none: its agents go on without them, as before.
            if let Err(error) = remote.call(nexus_desktop_mcp::mcp::SERVICE_CALL, handed).await {
                eprintln!("nexus-desktop-gpui: {machine} took no MCP servers: {error}");
            }
        });
    }

    /// The servers reached over SSH, with their relays: never a cloud machine.
    fn ssh_remotes(&self) -> Vec<(String, Remote)> {
        self.state
            .lock()
            .map(|state| {
                state
                    .servers
                    .iter()
                    .filter_map(|server| match &server.reach {
                        super::Reach::Ssh(remote) => Some((server.setting.id.clone(), remote.clone())),
                        super::Reach::Cloud => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// One granted call of server `machine`'s agent, answered as the app's own socket answers it. A browser call names
/// the server in its origin: a window admits it for that server's session only. Runs on a thread of its own.
fn answer(
    tools: &ServerTools,
    me: &std::sync::Weak<Machines>,
    machine: &str,
    grant: &nexus_desktop_mcp::app_socket::Grant,
    input: &Value,
) -> Value {
    let message = &input["message"];
    match input["tools"].as_str() {
        Some(nexus_desktop_mcp::secret_request::TOOLS) => tools.runtime.block_on(
            nexus_desktop_mcp::secret_request::answer_remote(&tools.secrets, &grant.thread, &grant.run, message),
        ),
        Some(_) => crate::app_socket::call(grant, input, &tools.runtime, &tools.secrets),
        None => {
            let valid: Arc<dyn Fn() -> bool + Send + Sync> = {
                let (me, scope, grant) = (me.clone(), tools.scope.clone(), grant.clone());
                Arc::new(move || {
                    let Some(machines) = me.upgrade() else {
                        return false;
                    };
                    let Ok(held) = machines.server_tools.lock() else {
                        return false;
                    };
                    call_holds(&grant.scope, &grant.run, &scope(), &held.ended)
                })
            };
            crate::app_socket::browser(&tools.browser, &tools.data, Some(machine), grant, message, valid)
        }
    }
}

/// Keeps the [`Held`] type private to the module while `Machines` holds it.
pub(super) type ServerToolsCell = Mutex<Held>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_servers_call_holds_in_its_organization_while_its_run_goes_on() {
        let mut ended = HashSet::new();
        assert!(call_holds("gw|acme", "ns_1", "gw|acme", &ended));
        assert!(!call_holds("gw|acme", "ns_1", "gw|other", &ended));
        ended.insert("ns_1".to_string());
        assert!(!call_holds("gw|acme", "ns_1", "gw|acme", &ended));
    }
}
