//! What the host's tests share: a host in this process on a socket in a fresh folder, its agents the stand-in
//! `claude` (nexus-workbench's src/bin/fake-claude.rs).
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use nexus_host::agent_files::AgentFiles;
use nexus_host::server::{HostInfo, Hub, PROTOCOL, Server, serve_unix};
use nexus_workbench::api::{AgentInfo, Api, Machine};
use nexus_workbench::event::{SessionAgent, Usage};
use nexus_workbench::process::Spawn;
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd, Workbench};
use tokio::sync::watch;

/// The stand-in `claude`, its events pushed to the hub as the real host's are.
pub struct TestHost {
    hub: Hub,
    memory: PathBuf,
    /// The host's folder: its `agent.env` and `mcp.json` reach the stand-in as they reach the real agents.
    files: AgentFiles,
}

impl Host for TestHost {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch, String>> {
        let mut env: HashMap<String, String> = [
            ("FAKE_CLAUDE_DIR".to_string(), self.memory.to_string_lossy().to_string()),
            // Background work that ends while the client is away.
            ("FAKE_CLAUDE_BACKGROUND_MS".to_string(), "300".to_string()),
        ]
        .into_iter()
        .collect();
        let mut args = request.args.clone();
        self.files.apply(
            request.agent,
            request.via,
            request.account_dir.is_none(),
            &mut args,
            &mut env,
        );
        Box::pin(async move {
            Ok(Launch {
                session_mcp: Default::default(),
                attribution: None,
                spawn: Spawn {
                    program: PathBuf::from(env!("CARGO_BIN_EXE_fake-claude")),
                    args,
                    env,
                    cwd: request.cwd,
                },
                nexus_session: Some("ns_1".into()),
                local_run: None,
                local_provider: None,
                model: None,
            })
        })
    }

    fn ended(&self, _nexus_session: String, _end: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }

    fn price(&self, _model: &str, _usage: &Usage) -> Option<f64> {
        None
    }

    /// A catalog with nothing to pay: a linked session's spend can be counted, and never trips its cap.
    fn top_price(&self, _usage: &Usage) -> Option<f64> {
        Some(0.0)
    }

    fn emit(&self, emit: Emit) {
        self.hub.publish(&emit);
    }
}

pub struct TestMachine {
    home: PathBuf,
}

impl Machine for TestMachine {
    fn tool_path(&self) -> String {
        std::env::var("PATH").unwrap_or_default()
    }

    fn home(&self) -> Result<PathBuf, String> {
        Ok(self.home.clone())
    }

    fn agent(&self, agent: SessionAgent) -> AgentInfo {
        AgentInfo {
            agent,
            path: None,
            version: None,
            models: None,
            problem: None,
        }
    }

    fn subscriptions_allowed(&self) -> BoxFuture<'static, bool> {
        Box::pin(async { false })
    }
}

pub const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

pub struct Running {
    pub root: PathBuf,
    pub socket: PathBuf,
    pub repo: PathBuf,
    pub api: Api,
    pub stop: watch::Sender<bool>,
    /// The same server the socket serves, for a connection a test opens itself.
    pub server: Arc<Server>,
    pub hub: Hub,
}

pub fn root(name: &str) -> PathBuf {
    // Short: a Unix socket's path has a length limit.
    let root = std::env::temp_dir().join(format!("nh-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    root
}

/// A host on a socket in a fresh folder, in this process.
pub fn start(name: &str) -> Running {
    start_with(name, false)
}

/// [`start`], with its sessions' shared terminal on (`terminal`), as a person's own server has it.
pub fn start_with(name: &str, terminal: bool) -> Running {
    let root = root(name);
    let repo = root.join("repo");
    let memory = root.join("memory");
    let _ = std::fs::create_dir_all(&repo);
    let _ = std::fs::create_dir_all(&memory);
    let hub = Hub::new();
    let host = Arc::new(TestHost {
        hub: hub.clone(),
        memory,
        files: AgentFiles::new(root.clone()),
    });
    let store = Store::open(&root.join("workbench.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        // Linked sessions' worktrees (tests/orchestra.rs).
        worktrees: Some(root.join("worktrees")),
        shell: nexus_workbench::process::Shell::sh(),
        terminal,
        ..Config::default()
    };
    let workbench = Workbench::new(store, host, config).unwrap_or_else(|e| panic!("{e}"));
    let api = Api::new(
        workbench,
        Arc::new(TestMachine {
            home: root.join("home"),
        }),
    );
    let server = Arc::new(Server {
        api: api.clone(),
        hub: hub.clone(),
        token: TOKEN.into(),
        info: HostInfo {
            version: "9.9.9".into(),
            protocol: PROTOCOL,
            machine: "box".into(),
            org: Some("org".into()),
            features: api
                .workbench()
                .features()
                .into_iter()
                .chain([nexus_workbench::shell_pty::FEATURE.to_string()])
                .collect(),
        },
        shells: Some(nexus_workbench::shell_pty::Shells::new(
            nexus_workbench::process::Shell::sh(),
            Arc::new(String::new),
            nexus_workbench::shell_pty::Limits::default(),
        )),
        local: None,
        provision: Some(nexus_host::provision::Places::with_vars(
            root.clone(),
            root.join("home"),
            |_| None,
        )),
        runners: None,
        tools_host: None,
    });
    let socket = root.join("host.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap_or_else(|e| panic!("{e}"));
    let (stop, stopped) = watch::channel(false);
    tokio::spawn(serve_unix(listener, server.clone(), stopped));
    Running {
        root,
        socket,
        repo,
        api,
        stop,
        server,
        hub,
    }
}
