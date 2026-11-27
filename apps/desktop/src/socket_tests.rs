//! The runner's two sockets, driven as their clients drive them: the built-in server's relay for the app socket
//! (`app_socket.rs`), and a Unix socket client for outside access (`outside.rs`). The store, the files and the sockets
//! are in temporary folders, and the app socket checks a fixed sign-in, so no test reads the keychain.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

use nexus_desktop_mcp::mcp::{PULL_REQUEST_TOOLS, REPLY_WAIT, relay_to_app};
use nexus_desktop_mcp::outside::grants::Scope;

use super::*;

// Nothing listens on port 9: no Gateway call leaves this computer.
const GATEWAY: &str = "http://127.0.0.1:9";
const SESSION: &str = "s1";
const RUN: &str = "run-1";
const PERSON: &str = "person-one";
const VALUE: &str = "sk-live-7f3a9c-very-secret";

/// A short folder of its own: a Unix socket's path has at most 107 bytes.
fn folder(name: &str) -> PathBuf {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let dir = std::env::temp_dir().join(format!("ngs-{name}-{}", &id[..8]));
    std::fs::create_dir_all(&dir).expect("a temporary folder");
    dir
}

/// A store in `data` with one project and one idle session, [`SESSION`].
fn store_with_a_session(data: &Path, scope: &str) {
    let store = Store::open(&data.join("workbench.sqlite3")).expect("the store opens");
    let now = nexus_workbench::now_ms();
    let project = store.add_project(data, now).expect("a project");
    let summary = serde_json::from_value(json!({
        "id": SESSION, "projectId": project.id, "title": "Deploy the docs", "agent": "claude", "model": null,
        "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
        "contextTokens": null, "contextWindow": null, "createdAt": now, "updatedAt": now, "archived": false,
    }))
    .expect("a session");
    let record = nexus_workbench::store::ThreadRecord {
        summary,
        native_id: "native-s1".into(),
        native_started: false,
        scope: scope.to_string(),
        fork_from: None,
        seed: None,
        automatic_title: false,
        delivered_seq: 0,
        halt: None,
    };
    store.insert_thread(&record).expect("saved");
}

/// The loops of a test: long ticks, `settings`, and the sockets' files under `dir`.
fn loops(dir: &Path, settings: Settings) -> Loops {
    Loops {
        local_providers_opt_in: Default::default(),
        client: Arc::new(|| None),
        tick: Duration::from_secs(3600),
        sweep_first: Duration::from_secs(3600),
        sweep_every: Duration::from_secs(3600),
        settings: Arc::new(move || settings.clone()),
        secret_files: dir.join("secret-files"),
        outside_socket: Some(dir.join("run").join("sessions.sock")),
        keep_running: Arc::new(|| false),
        local_cache: true,
        browser: Arc::new(crate::side::browser::agent::post),
        pull_watch: false,
    }
}

/// A runner over a store with [`SESSION`], and its folder.
fn runner(name: &str, settings: Settings) -> (CodeRunner, PathBuf, String) {
    let dir = folder(name);
    let scope = format!("{GATEWAY}|org-{name}");
    store_with_a_session(&dir, &scope);
    let (runner, _events) = CodeRunner::start_in(dir.clone(), GATEWAY, &scope, loops(&dir, settings))
        .expect("the store opens")
        .expect("free, so taken");
    (runner, dir, scope)
}

/// An app socket on the runner's requests, signed in to `scope` as [`PERSON`].
fn app_socket(runner: &CodeRunner, scope: &str) -> AppSocket {
    let scope = scope.to_string();
    AppSocket::bind(
        runner.host.data.clone(),
        runner.runtime.clone(),
        runner.secrets.clone(),
        Arc::new(move || (scope.clone(), PERSON.to_string())),
        Arc::new(|_| false),
    )
    .expect("bound")
}

fn tool_call(id: i64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

fn text(answer: &Value) -> &str {
    answer["result"]["content"][0]["text"].as_str().unwrap_or_default()
}

fn wait_for(done: impl Fn() -> bool) -> bool {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

#[test]
fn the_runner_serves_the_built_in_servers_socket() {
    let (runner, dir, _) = runner("serves", Settings::default());
    let socket = runner.app_socket().expect("bound when the store was taken");
    assert!(socket.address().ip().is_loopback());
    drop(runner);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_pull_request_call_reaches_the_watch_and_a_bad_credential_is_refused() {
    let (runner, dir, scope) = runner("pull", Settings::default());
    let socket = app_socket(&runner, &scope);
    let (address, token) = socket.grant(SESSION, &scope, RUN, PERSON).expect("a credential");
    let call = tool_call(1, "unwatch_pull_request", json!({}));
    let answer = relay_to_app(&address, &token, &call);
    assert_eq!(answer["id"], 1);
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    assert!(text(&answer).contains("not watching"), "{answer}");

    let refused = |token: &str| {
        let request = json!({"token": token, "tools": PULL_REQUEST_TOOLS, "message": call});
        let reply = nexus_desktop_mcp::app_socket::relay(&address, &request, REPLY_WAIT).expect("a reply");
        serde_json::from_str::<Value>(&reply).expect("json")["error"]["code"] == -32001
    };
    assert!(refused("not-a-credential"));
    // Another sign-in's credential.
    let (_, other) = socket.grant(SESSION, &scope, RUN, "person-two").expect("a credential");
    assert!(refused(&other));
    // The run ended: its credential with it.
    socket.end_run(RUN);
    assert!(refused(&token));
    socket.close();
    assert!(socket.grant(SESSION, &scope, RUN, PERSON).is_none(), "closed");
    drop(runner);
    let _ = std::fs::remove_dir_all(dir);
}

/// Asks for a secret over the socket on a thread of its own; the card shows once it waits.
fn ask_secret(runner: &CodeRunner, socket: &AppSocket, scope: &str) -> std::thread::JoinHandle<Value> {
    let (address, token) = socket.grant(SESSION, scope, RUN, PERSON).expect("a credential");
    let call = tool_call(
        3,
        "request_secret",
        json!({"name": "Stripe key", "reason": "To deploy the docs."}),
    );
    let asking = std::thread::spawn(move || relay_to_app(&address, &token, &call));
    assert!(wait_for(|| runner.secret_request(SESSION).is_some()), "the card shows");
    asking
}

#[test]
fn a_provided_secret_reaches_the_agent_as_a_private_file_only() {
    let (runner, dir, scope) = runner("provide", Settings::default());
    let socket = app_socket(&runner, &scope);
    let asking = ask_secret(&runner, &socket, &scope);
    let card = runner.secret_request(SESSION).expect("the card");
    assert_eq!(
        (card.name.as_str(), card.reason.as_str()),
        ("Stripe key", "To deploy the docs.")
    );
    futures::executor::block_on(runner.answer_secret(card.id.clone(), Some(VALUE.into())))
        .expect("answered")
        .expect("written");
    let answer = asking.join().expect("the call");
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    assert!(!answer.to_string().contains(VALUE), "the value never reaches the agent");
    let path = std::fs::read_dir(dir.join("secret-files").join(SESSION))
        .expect("the session's folder")
        .next()
        .expect("one file")
        .expect("entry")
        .path();
    assert!(text(&answer).contains(&path.display().to_string()));
    assert_eq!(std::fs::read_to_string(&path).expect("the file"), VALUE);
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert!(runner.secret_request(SESSION).is_none(), "the card closed");
    // The run ends: the file goes.
    socket.end_run(RUN);
    assert!(!path.exists());
    drop(runner);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_declined_secret_ends_the_request_with_no_file() {
    let (runner, dir, scope) = runner("decline", Settings::default());
    let socket = app_socket(&runner, &scope);
    let asking = ask_secret(&runner, &socket, &scope);
    let card = runner.secret_request(SESSION).expect("the card");
    futures::executor::block_on(runner.answer_secret(card.id.clone(), None))
        .expect("answered")
        .expect("declined");
    let answer = asking.join().expect("the call");
    assert_eq!(answer["result"]["isError"], true);
    assert!(text(&answer).contains("declined"), "{answer}");
    assert!(runner.secret_request(SESSION).is_none());
    assert!(!dir.join("secret-files").join(SESSION).exists(), "no file");
    // A second answer to the same card finds nothing.
    assert!(
        futures::executor::block_on(runner.answer_secret(card.id, None))
            .expect("answered")
            .is_err()
    );
    drop(runner);
    let _ = std::fs::remove_dir_all(dir);
}

/// One request line to the outside access socket, and its reply.
fn send(path: &Path, request: &Value) -> Value {
    let mut stream = UnixStream::connect(path).expect("connect");
    writeln!(stream, "{request}").expect("write");
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).expect("read");
    serde_json::from_str(&line).expect("json")
}

/// A client asks for `scope`; the person answers the prompt with `answer` once it shows.
fn connect(runner: &CodeRunner, path: &Path, client: &str, scope: &str, answer: Option<Scope>) -> Value {
    let (path_owned, request) = (path.to_path_buf(), json!({"client": client, "connect": scope}));
    let asking = std::thread::spawn(move || send(&path_owned, &request));
    let outside = runner.outside();
    assert!(wait_for(|| !outside.pending().is_empty()), "the prompt shows");
    let prompt = outside.pending().remove(0);
    assert_eq!(prompt.client, client);
    outside.answer(&prompt.id, answer);
    asking.join().expect("the client")
}

fn outside_on() -> Settings {
    Settings {
        outside_access: Some(true),
        ..Settings::default()
    }
}

#[test]
fn outside_access_runs_only_what_the_person_allowed() {
    let (runner, dir, _) = runner("outside", outside_on());
    let path = dir.join("run").join("sessions.sock");
    assert!(
        runner.outside().view().listening,
        "{:?}",
        runner.outside().view().problem
    );

    let denied = connect(&runner, &path, "a script", "act", None);
    assert_eq!(denied["error"]["code"], "denied");
    assert!(runner.outside().view().grants.is_empty());

    let granted = connect(&runner, &path, "nexus CLI", "read", Some(Scope::Read));
    assert_eq!(granted["scope"], "read");
    let token = granted["token"].as_str().expect("token");
    let list = send(
        &path,
        &json!({"token": token, "message": tool_call(1, nexus_desktop_mcp::outside::tools::LIST, json!({}))}),
    );
    let sessions = &list["result"]["structuredContent"]["sessions"];
    assert_eq!(sessions[0]["id"], SESSION, "{list}");
    // A read grant may not act.
    let stop = send(
        &path,
        &json!({"token": token, "message": tool_call(2, nexus_desktop_mcp::outside::tools::STOP,
            json!({"session_id": SESSION}))}),
    );
    assert_eq!(stop["result"]["isError"], true);
    assert_eq!(runner.outside().view().grants.len(), 1);

    // The runner lets the store go: the socket closes.
    drop(runner);
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_build_that_does_not_own_the_store_serves_neither_socket() {
    let dir = folder("notowner");
    let scope = format!("{GATEWAY}|org-notowner");
    store_with_a_session(&dir, &scope);
    // The Nexus app holds the store.
    let other = StoreOwner::take(&dir).expect("the other app owns the store");
    let started = CodeRunner::start_in(dir.clone(), GATEWAY, &scope, loops(&dir, outside_on()));
    assert!(matches!(started, Ok(None)), "no runner, so no app socket");
    assert!(!dir.join("run").join("sessions.sock").exists(), "no outside access");
    drop(other);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn outside_access_stays_with_the_app_that_listened_first() {
    let dir = folder("second");
    let path = dir.join("run").join("sessions.sock");
    std::fs::create_dir_all(path.parent().expect("folder")).expect("folder");
    // The other app listens already.
    let _first = std::os::unix::net::UnixListener::bind(&path).expect("the other app's socket");
    let scope = format!("{GATEWAY}|org-second");
    store_with_a_session(&dir, &scope);
    let (runner, _events) = CodeRunner::start_in(dir.clone(), GATEWAY, &scope, loops(&dir, outside_on()))
        .expect("the store opens")
        .expect("free, so taken");
    let view = runner.outside().view();
    assert!(!view.listening);
    assert_eq!(
        view.problem.as_deref(),
        Some("Another Nexus already offers outside access on this computer.")
    );
    drop(runner);
    let _ = std::fs::remove_dir_all(dir);
}

/// A local test page, served once per request: its title and text.
fn test_site() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            // The whole request head first: its client writes it in pieces, and closing with bytes unread would reset
            // the connection.
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") && std::io::Read::read(&mut stream, &mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let body =
                "<!doctype html><html><head><title>Check page</title></head><body><h1>Hello agent</h1></body></html>";
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    address
}

/// A browser for the socket's browser calls: answers with `nexus_desktop_mcp::browser::handle`, its page loaded over
/// HTTP from the test site, as the windows load one in the engine. It records the trusted origin too.
#[derive(Clone)]
struct SeenBrowserCall {
    thread: String,
    run: String,
    origin: crate::side::browser::agent::LocalOrigin,
    valid: crate::side::browser::agent::Guard,
}

fn fake_browser(seen: Arc<Mutex<Vec<SeenBrowserCall>>>) -> crate::app_socket::BrowserPost {
    let opened: Arc<Mutex<Option<String>>> = Arc::default();
    Arc::new(move |call: crate::side::browser::agent::Call| {
        seen.lock().expect("seen").push(SeenBrowserCall {
            thread: call.thread.clone(),
            run: call.run.clone(),
            origin: call.origin.clone(),
            valid: call.valid.clone(),
        });
        let opened = opened.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().build().expect("runtime");
            let valid = call.valid.clone();
            let answer = runtime.block_on(nexus_desktop_mcp::browser::handle(
                &call.message,
                "test",
                |name, args| async move {
                    if !valid() {
                        return Err("This browser tool credential is invalid or expired.".to_string());
                    }
                    match name.as_str() {
                        "browser_open" => {
                            let url = args["url"].as_str().ok_or("An address is required.")?.to_string();
                            nexus_browser::preview::browser_url(&url)?;
                            *opened.lock().expect("opened") = Some(url);
                            Ok(Value::Null)
                        }
                        "browser_read" => {
                            let url = opened
                                .lock()
                                .expect("opened")
                                .clone()
                                .ok_or("Use browser_open with the website's address first.")?;
                            let address = url.trim_start_matches("http://").trim_end_matches('/');
                            let mut stream =
                                std::net::TcpStream::connect(address).map_err(|error| error.to_string())?;
                            write!(stream, "GET / HTTP/1.1\r\nhost: {address}\r\nconnection: close\r\n\r\n")
                                .map_err(|error| error.to_string())?;
                            let mut page = String::new();
                            std::io::Read::read_to_string(&mut stream, &mut page).map_err(|error| error.to_string())?;
                            let title = page
                                .split("<title>")
                                .nth(1)
                                .and_then(|rest| rest.split("</title>").next())
                                .unwrap_or_default();
                            let text = page
                                .split("<h1>")
                                .nth(1)
                                .and_then(|rest| rest.split("</h1>").next())
                                .unwrap_or_default();
                            Ok(json!({"url": url, "title": title, "text": text, "elements": []}))
                        }
                        other => Err(format!("{other} is not in this test.")),
                    }
                },
            ));
            let _ = call.reply.send(answer.unwrap_or(Value::Null));
        });
        true
    })
}

#[test]
fn the_browser_tools_answer_a_runs_client_against_a_local_page_until_the_run_ends() {
    let (runner, dir, scope) = runner("browser", Settings::default());
    let scope = scope.as_str();
    let seen: Arc<Mutex<Vec<SeenBrowserCall>>> = Arc::default();
    let socket = AppSocket::bind(
        runner.host.data.clone(),
        runner.runtime.clone(),
        runner.secrets.clone(),
        {
            let scope = scope.to_string();
            Arc::new(move || (scope.clone(), PERSON.to_string()))
        },
        fake_browser(seen.clone()),
    )
    .expect("bound");
    let (address, token) = socket.grant(SESSION, scope, RUN, PERSON).expect("a credential");
    let site = test_site();
    // The browser server relays each line with no `tools`, as `--browser-mcp` does.
    let ask = |message: Value| -> Value {
        let line =
            nexus_desktop_mcp::app_socket::relay(&address, &json!({"token": token, "data": "/wrong-store", "scope": "wrong-scope", "origin": {"data": "/wrong-store", "scope": "wrong-scope"}, "message": message}), REPLY_WAIT)
                .expect("an answer");
        serde_json::from_str(line.trim()).expect("JSON")
    };
    let list = ask(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
    assert_eq!(list["result"]["tools"].as_array().map(Vec::len), Some(12));
    let opened = ask(tool_call(2, "browser_open", json!({"url": format!("http://{site}/")})));
    assert_eq!(opened["result"]["isError"], false, "{opened}");
    let read = ask(tool_call(3, "browser_read", json!({})));
    assert_eq!(read["result"]["isError"], false, "{read}");
    let text = read["result"]["content"][0]["text"].as_str().expect("text");
    let page: Value = serde_json::from_str(text).expect("the page as JSON");
    assert_eq!(page["title"], "Check page");
    assert_eq!(page["text"], "Hello agent");
    let refused = ask(tool_call(4, "browser_open", json!({"url": "file:///etc/hostname"})));
    assert_eq!(refused["result"]["isError"], true);
    for call in seen.lock().expect("seen").iter() {
        assert_eq!(call.thread, SESSION);
        assert_eq!(call.run, RUN);
        assert_eq!(call.origin.data, dir);
        assert_eq!(call.origin.scope, scope);
    }
    // Another credential, or the run's end, and the browser does not answer.
    let wrong = nexus_desktop_mcp::app_socket::relay(
        &address,
        &json!({"token": "nope", "message": tool_call(5, "browser_read", json!({}))}),
        REPLY_WAIT,
    )
    .expect("an answer");
    assert!(wrong.contains("-32001"), "{wrong}");
    let guard = seen
        .lock()
        .expect("seen")
        .first()
        .expect("real socket call")
        .valid
        .clone();
    let request = crate::side::browser::agent::REQUESTS.fixture_pending(guard);
    assert!(crate::side::browser::agent::REQUESTS.fixture_contains_unchecked(&request.id));
    socket.end_run(RUN);
    assert!(
        !crate::side::browser::agent::REQUESTS.fixture_contains_unchecked(&request.id),
        "actual end_run eagerly prunes the revoked producer guard after releasing Grants"
    );
    let ended = ask(tool_call(6, "browser_read", json!({})));
    assert_eq!(ended["error"]["code"], -32001, "{ended}");
    assert_eq!(
        seen.lock().expect("seen").len(),
        4,
        "a refused credential never reaches the browser"
    );
    drop(runner);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn source_guard_remembers_scope_changes_even_when_the_original_scope_returns() {
    let (runner, dir, original) = runner("source-guard", Settings::default());
    let captured = runner.source_guard();
    assert!(captured(), "the actual runner initially owns its captured source");
    assert_eq!(runner.store_source(), (dir.clone(), original.clone()));

    runner.set_scope(&original);
    assert!(captured(), "rebinding the same scope does not invalidate its owner");
    let other = format!("{GATEWAY}|another-source-guard-org");
    runner.set_scope(&other);
    assert!(!captured(), "a different scope revokes the captured source");
    assert_eq!(runner.store_source(), (dir.clone(), other));

    runner.set_scope(&original);
    assert!(
        !captured(),
        "returning to the original scope cannot revive its earlier generation"
    );
    let fresh = runner.source_guard();
    assert!(fresh(), "a newly captured guard admits the current original scope");
    assert_eq!(runner.store_source(), (dir.clone(), original));
    drop(runner);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn queued_browser_stop_rechecks_source_and_owner_before_workbench_mutation() {
    let (runner, dir, original) = runner("queued-stop", Settings::default());
    assert!(
        runner.api.remote().is_none(),
        "fixture mutates its real private Workbench"
    );
    for revoke_source in [true, false] {
        let source = runner.source_guard();
        let live = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let owner = live.clone();
        let valid: crate::side::browser::agent::Guard =
            Arc::new(move || owner.load(std::sync::atomic::Ordering::SeqCst) && source());
        assert!(valid(), "the UI admits this current owner before its command queues");
        let (started, waiting) = futures::channel::oneshot::channel();
        let (release, released) = futures::channel::oneshot::channel();
        let command = runner.interrupt_after(SESSION.into(), valid, async move {
            let _ = started.send(());
            released.await.expect("controlled per-call queue released");
        });
        runner.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), waiting)
                .await
                .expect("actual dispatch reached its held queue boundary")
                .expect("dispatch acknowledged its wait");
        });
        if revoke_source {
            runner.set_scope(&format!("{GATEWAY}|queued-other-org"));
            runner.set_scope(&original);
        } else {
            live.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        release.send(()).expect("actual command is still queued");
        let dispatched = runner.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), command)
                .await
                .expect("guarded dispatch completed")
                .expect("guarded task did not panic")
        });
        assert!(!dispatched, "queued source or owner revocation must refuse dispatch");
        assert!(
            runner
                .api
                .workbench()
                .halt(SESSION)
                .expect("actual session halt state")
                .is_none(),
            "a refused command must not persist Workbench's interrupt side effect"
        );
    }
    let valid = runner.source_guard();
    assert!(valid());
    let command = runner.interrupt_after(SESSION.into(), valid, std::future::ready(()));
    assert!(runner.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), command)
            .await
            .expect("current command completed")
            .expect("current task did not panic")
    }));
    assert!(
        runner
            .api
            .workbench()
            .halt(SESSION)
            .expect("real interrupt persisted its halt")
            .is_some(),
        "a valid source must still execute Engine::interrupt against the actual private Workbench"
    );
    assert_eq!(runner.store_source(), (dir.clone(), original));
    drop(runner);
    let _ = std::fs::remove_dir_all(dir);
}
