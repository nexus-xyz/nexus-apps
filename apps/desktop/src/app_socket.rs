//! The app's socket for the built-in Nexus server (`mcp.rs`): the pull request tools and `request_secret` of this
//! build's Code sessions.
//!
//! - The endpoint: a TCP port on 127.0.0.1, bound when the runner takes the Code sessions store (`code_run.rs`) and
//!   closed when it lets it go. Only the store's owner serves it, so a session's tools reach the app that runs it.
//! - The credential: one per agent run, made when the agent starts and handed to the built-in server only through its
//!   environment (`NEXUS_MCP_APP_ADDR`, `NEXUS_MCP_APP_TOKEN`). It ends with the run, and works only while this build
//!   is signed in to the same organization as the same person.
//! - The protocol is `nexus_desktop_mcp::app_socket`'s: one JSON line in, `{"token", "tools", "message"}`, and the
//!   JSON-RPC answer out. `tools` says where the call goes: `pull_request` to the pull request watch (`pr_watch.rs`),
//!   `secret` to the run's `request_secret` (`secret_request.rs`), and none to the session's browser: the agent's
//!   `nexus_browser` server (`nexus_desktop_mcp::browser`), which the windows answer (`side/browser/agent.rs`).

use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use nexus_desktop_mcp::app_socket::{self, Checked, Grant, Grants, Launchables};
use nexus_desktop_mcp::mcp::PULL_REQUEST_TOOLS;
use serde_json::{Value, json};

use crate::secret_request::GpuiSecrets;

/// What the app is signed in to now: its scope (`<gateway>|<org>`) and the person's sign-in.
pub type Current = Arc<dyn Fn() -> (String, String) + Send + Sync>;

/// Hands a browser call to the windows; false when none takes it. `side::browser::agent::post`, or a test's fake.
pub type BrowserPost = Arc<dyn Fn(crate::side::browser::agent::Call) -> bool + Send + Sync>;

/// The listening socket and its credentials.
pub struct AppSocket {
    address: SocketAddr,
    grants: Arc<Mutex<Grants>>,
    stop: Arc<AtomicBool>,
    secrets: GpuiSecrets,
}

/// What a call needs besides its request: the credentials, what the app is signed in to, where the work runs.
struct Context {
    /// The local store that owns these run credentials; never supplied by the request.
    data: PathBuf,
    grants: Arc<Mutex<Grants>>,
    current: Current,
    runtime: tokio::runtime::Handle,
    secrets: GpuiSecrets,
    browser: BrowserPost,
}

fn lock(grants: &Mutex<Grants>) -> MutexGuard<'_, Grants> {
    grants.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AppSocket {
    /// Binds a port on 127.0.0.1 and answers on threads of its own. The calls run on `runtime`.
    pub fn bind(
        data: PathBuf,
        runtime: tokio::runtime::Handle,
        secrets: GpuiSecrets,
        current: Current,
        browser: BrowserPost,
    ) -> Result<AppSocket, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let grants = Arc::new(Mutex::new(Grants::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let context = Arc::new(Context {
            data,
            grants: grants.clone(),
            current,
            runtime,
            secrets: secrets.clone(),
            browser,
        });
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("nexus-app-socket".into())
            .spawn(move || {
                app_socket::serve(listener, stopped, move |stream| {
                    let _ = answer(&context, stream);
                });
            })
            .map_err(|error| error.to_string())?;
        Ok(AppSocket {
            address,
            grants,
            stop,
            secrets,
        })
    }

    #[cfg(test)]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// A new credential for the run `run` of the session `thread`, in `scope`, signed in as `sign_in`. None once the
    /// socket is closed.
    pub fn grant(&self, thread: &str, scope: &str, run: &str, sign_in: &str) -> Option<(SocketAddr, String)> {
        if self.stop.load(Ordering::SeqCst) {
            return None;
        }
        let token = lock(&self.grants).add(Grant {
            thread: thread.into(),
            scope: scope.into(),
            run: run.into(),
            sign_in: sign_in.into(),
        });
        Some((self.address, token))
    }

    /// A new launch credential for the run `run`, which starts only `servers` (`nexus_desktop_mcp::launch`). None once
    /// the socket is closed.
    pub fn grant_launch(
        &self,
        thread: &str,
        scope: &str,
        run: &str,
        sign_in: &str,
        servers: Launchables,
    ) -> Option<(SocketAddr, String)> {
        if self.stop.load(Ordering::SeqCst) {
            return None;
        }
        let grant = Grant {
            thread: thread.into(),
            scope: scope.into(),
            run: run.into(),
            sign_in: sign_in.into(),
        };
        Some((self.address, lock(&self.grants).add_launch(grant, servers)))
    }

    pub(crate) fn grant_local_launch(&self, run: &str, servers: Launchables) -> Option<(String, String)> {
        let mut grants = lock(&self.grants);
        if self.stop.load(Ordering::SeqCst) {
            return None;
        }
        let token = grants.add_local_launch(run, servers);
        Some((self.address.to_string(), token))
    }

    /// The run `run` ended: its credentials go, its open secret card closes and its files go.
    pub fn end_run(&self, run: &str) {
        lock(&self.grants).end_run(run);
        self.secrets.end_run(run);
        crate::side::browser::agent::REQUESTS.prune_invalid();
    }

    /// Stops listening: every credential goes, with each run's open card and files. The runner calls it when it lets
    /// the store go.
    pub fn close(&self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        for run in lock(&self.grants).clear() {
            self.secrets.end_run(&run);
        }
        crate::side::browser::agent::REQUESTS.prune_invalid();
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.address);
    }
}

impl Drop for AppSocket {
    fn drop(&mut self) {
        self.close();
    }
}

/// One connection: its request, checked against the credentials, and the answer.
fn answer(context: &Context, mut stream: TcpStream) -> Result<(), String> {
    let input = app_socket::read_request(&stream)?;
    let (scope, sign_in) = (context.current)();
    let token = input["token"].as_str().unwrap_or_default().to_string();
    // A launch credential's call is answered by the check itself, from the run's servers.
    let checked = lock(&context.grants).check(&input, &scope, &sign_in);
    let reply = match checked {
        Checked::Answered(reply) => reply,
        Checked::Call(grant) => answer_call(context, &grant, &token, &input),
    };
    writeln!(stream, "{reply}").map_err(|error| error.to_string())
}

/// A granted call on this build's socket, by the tools it names: the session's browser when it names none.
fn answer_call(context: &Context, grant: &Grant, token: &str, input: &Value) -> Value {
    match input["tools"].as_str() {
        None => browser_call(context, grant, token, &input["message"]),
        Some(_) => call(grant, input, &context.runtime, &context.secrets),
    }
}

/// A granted call of the built-in server's tools: on this build's socket, or passed on by the background service
/// (`code_run.rs`). Runs on a thread of its own, never on the runtime, so it may wait on it.
pub(crate) fn call(grant: &Grant, input: &Value, runtime: &tokio::runtime::Handle, secrets: &GpuiSecrets) -> Value {
    let message = &input["message"];
    match input["tools"].as_str() {
        Some(PULL_REQUEST_TOOLS) => runtime.block_on(nexus_pull_requests::watch::answer(
            &crate::pr_watch::GpuiShell,
            &grant.thread,
            message,
        )),
        Some(nexus_desktop_mcp::secret_request::TOOLS) => {
            runtime.block_on(secrets.answer(&grant.thread, &grant.run, message))
        }
        // The browser's calls go to [`browser`]: on this socket by `answer_call`, from the service by `code_run.rs`.
        None => json!({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32000,
            "message": "The session browser is unavailable."}}),
        Some(_) => {
            json!({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "Unknown tools."}})
        }
    }
}

/// A call of the run's `nexus_browser` server: the windows answer it, waiting up to the person's ten minutes for
/// access. Its credential is checked again while it runs: the run's end or another sign-in ends it.
fn browser_call(context: &Context, grant: &Grant, token: &str, message: &Value) -> Value {
    let valid: Arc<dyn Fn() -> bool + Send + Sync> = {
        let (grants, current, token) = (context.grants.clone(), context.current.clone(), token.to_string());
        Arc::new(move || {
            let (scope, sign_in) = current();
            lock(&grants).authorize(&token, &scope, &sign_in).is_some()
        })
    };
    browser(&context.browser, &context.data, None, grant, message, valid)
}

/// A browser call of the run in `grant`, on this build's socket, passed on by the background service (`code_run.rs`),
/// or by server `machine`'s host (`machines/server_tools.rs`): the windows answer it while `valid` holds. Waits, so
/// never on the runtime.
pub(crate) fn browser(
    post: &BrowserPost,
    data: &Path,
    machine: Option<&str>,
    grant: &Grant,
    message: &Value,
    valid: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Value {
    let unavailable = || json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32000,"message":"The session browser is unavailable."}});
    let (reply, answer) = std::sync::mpsc::channel();
    let posted = post(crate::side::browser::agent::Call {
        origin: crate::side::browser::agent::LocalOrigin {
            data: data.to_path_buf(),
            scope: grant.scope.clone(),
            machine: machine.map(str::to_string),
        },
        thread: grant.thread.clone(),
        run: grant.run.clone(),
        message: message.clone(),
        valid,
        reply,
    });
    if !posted {
        return unavailable();
    }
    answer
        .recv_timeout(nexus_desktop_mcp::browser::WAIT + std::time::Duration::from_secs(20))
        .unwrap_or_else(|_| unavailable())
}

#[cfg(test)]
mod tests {
    use super::{BrowserPost, browser};
    use nexus_desktop_mcp::app_socket::Grant;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn browser_producer_keeps_store_and_grant_scope_separate_from_request_and_live_validity() {
        // Both the bound socket and local service use this constructor. Identical run/session IDs
        // from different local stores must keep distinct origins, without changing the live gate.
        for (data, scope) in [("/local/store-a", "gw|org-a"), ("/local/store-b", "gw|org-b")] {
            let data = PathBuf::from(data);
            let grant = Grant {
                thread: "same-session".into(),
                run: "same-run".into(),
                scope: scope.into(),
                sign_in: "person".into(),
            };
            let live = Arc::new(AtomicBool::new(true));
            let valid: Arc<dyn Fn() -> bool + Send + Sync> = {
                let live = live.clone();
                Arc::new(move || live.load(Ordering::SeqCst))
            };
            let post: BrowserPost = {
                let (data, scope, valid, live) = (data.clone(), scope.to_string(), valid.clone(), live.clone());
                Arc::new(move |call| {
                    assert_eq!(call.origin.data, data);
                    assert_eq!(call.origin.scope, scope);
                    assert_eq!(call.thread, "same-session");
                    assert_eq!(call.run, "same-run");
                    assert!(Arc::ptr_eq(&call.valid, &valid), "preserve the original run guard");
                    assert!((call.valid)());
                    live.store(false, Ordering::SeqCst);
                    assert!(!(call.valid)(), "origin must not replace live run validity");
                    call.reply
                        .send(json!({"result": "received"}))
                        .expect("waiting producer");
                    true
                })
            };
            let message = json!({"id": 1, "origin": {"data": "/forged", "scope": "forged"}});
            let reply: Value = browser(&post, &data, None, &grant, &message, valid);
            assert_eq!(reply, json!({"result": "received"}));
        }
    }

    #[test]
    fn a_servers_browser_call_names_its_server_in_the_origin_and_a_local_call_names_none() {
        let grant = Grant {
            thread: "server-session".into(),
            run: "run".into(),
            scope: "gw|org-a".into(),
            sign_in: "person".into(),
        };
        for machine in [Some("m_server"), None] {
            let post: BrowserPost = Arc::new(move |call| {
                assert_eq!(call.origin.machine.as_deref(), machine);
                assert_eq!(call.origin.scope, "gw|org-a");
                call.reply
                    .send(json!({"result": "received"}))
                    .expect("waiting producer");
                true
            });
            let reply = browser(
                &post,
                &PathBuf::from("/store"),
                machine,
                &grant,
                &json!({"id": 1}),
                Arc::new(|| true),
            );
            assert_eq!(reply, json!({"result": "received"}));
        }
    }
}
