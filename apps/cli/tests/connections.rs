//! `nexus connections`, `connect` and `disconnect` against a stand-in Gateway: what each sends, and a secret read from stdin.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    query: String,
    body: Value,
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn connection(scope: &str) -> Value {
    json!({
        "connector": "jira", "scope": scope, "scopeId": "usr_ada", "account": "cloud-1", "name": "acme.atlassian.net",
        "identity": "ada@acme.example", "status": "active", "kind": "basic", "last4": "cret", "createdAt": "2026-10-26T00:00:00.000Z",
    })
}

fn route(method: &str, path: &str, body: &Value) -> Response {
    match (method, path) {
        ("GET", "/v1/connectors") => answer(
            200,
            json!({ "connectors": [
                { "name": "jira", "description": "Jira", "operations": [], "usable": true, "connects": ["user", "org"],
                  "form": { "properties": { "site": {}, "email": {}, "token": { "x-nexus-secret": true } } } },
                { "name": "notion", "description": "Notion", "operations": [], "usable": true, "connects": ["user", "org"], "form": null, "oauth": true },
                { "name": "linear", "description": "Linear", "operations": [], "usable": true, "connects": ["user", "org"], "oauth": true,
                  "form": { "properties": { "token": { "x-nexus-secret": true } } } },
                { "name": "zendesk", "description": "Zendesk", "operations": [], "usable": true, "connects": [], "oauth": true,
                  "form": { "properties": { "token": { "x-nexus-secret": true } } } },
                { "name": "microsoft-todo", "description": "Microsoft To Do", "operations": [], "usable": true, "connects": ["user", "org"], "form": null, "oauth": true },
                { "name": "github", "description": "GitHub", "operations": [], "usable": true, "connects": [], "form": null },
                { "name": "datadog", "description": "Datadog", "operations": [], "usable": true, "connects": ["org"],
                  "form": { "properties": { "site": {}, "api_key": { "x-nexus-secret": true }, "application_key": { "x-nexus-secret": true } } } },
            ] }),
        ),
        ("GET", "/v1/connections") => answer(
            200,
            json!({ "connections": [connection("user"), { "connector": "notion", "scope": "org", "scopeId": "ten_1", "account": "ws-1", "name": "Acme", "status": "needs_reconnect", "kind": "oauth2", "createdAt": "2026-10-26T00:00:00.000Z" }] }),
        ),
        ("POST", "/v1/connections/jira") => {
            let scope = body["scope"].as_str().unwrap_or("user");
            answer(200, json!({ "connections": [connection(scope)] }))
        }
        ("POST", "/v1/connections/datadog" | "/v1/connections/snowflake" | "/v1/connections/linear") => {
            let connector = path.rsplit('/').next().unwrap_or_default();
            let mut made = connection(body["scope"].as_str().unwrap_or("user"));
            made["connector"] = json!(connector);
            made["name"] = json!(connector);
            made["identity"] = Value::Null;
            answer(200, json!({ "connections": [made] }))
        }
        ("POST", "/v1/connections/notion/oauth") => answer(
            200,
            json!({ "authorizeUrl": "https://api.notion.com/v1/oauth/authorize?state=s1" }),
        ),
        ("POST", "/v1/connections/microsoft-todo/oauth") => answer(
            200,
            json!({ "authorizeUrl": "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?state=s3" }),
        ),
        ("POST", "/v1/connections/linear/oauth") => answer(
            200,
            json!({ "authorizeUrl": "https://oauth.example.com/authorize?state=s2" }),
        ),
        ("DELETE", "/v1/connections/jira/acme.atlassian.net") => answer(
            200,
            json!({ "disconnected": { "connector": "jira", "account": "cloud-1", "name": "acme.atlassian.net" } }),
        ),
        _ => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "code": "not_found" } }),
        ),
    }
}

fn gateway() -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let (ready, port) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let app = Router::new().fallback(move |request: Request| {
                let log = log.clone();
                async move {
                    let method = request.method().to_string();
                    let path = request.uri().path().to_string();
                    let query = request.uri().query().unwrap_or("").to_string();
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: path.clone(),
                        query,
                        body: body.clone(),
                    });
                    route(&method, &path, &body)
                }
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            ready.send(listener.local_addr().expect("addr").port()).expect("ready");
            axum::serve(listener, app).await.expect("serve");
        });
    });
    let port = port.recv().expect("port");
    (format!("http://127.0.0.1:{port}"), seen)
}

struct Run {
    code: i32,
    out: String,
    err: String,
    seen: Vec<Seen>,
}

fn run(argv: &[&str], stdin: &str) -> Run {
    run_with(argv, stdin, &[])
}

/// A run whose working folder holds these files, each with its text and mode.
fn run_with(argv: &[&str], stdin: &str, files: &[(&str, &str, u32)]) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-connections-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway();
    std::fs::create_dir_all(&dir).expect("dir");
    for (name, text, mode) in files {
        let path = dir.join(name);
        std::fs::write(&path, text).expect("file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode)).expect("chmod");
        }
        #[cfg(not(unix))]
        let _ = mode;
    }
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": "nx_test", "kind": "api", "org": "acme" }).to_string(),
    )
    .expect("config");
    let capture = Capture(std::rc::Rc::new(Captured {
        stdin: stdin.to_string(),
        ..Captured::default()
    }));
    let ctx = Context {
        config_path,
        io: Box::new(capture.clone()),
        color: false,
        env: HashMap::new(),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir,
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let seen = seen.lock().expect("seen").clone();
    let out = capture.0.out.borrow().join("\n");
    let err = capture.0.err.borrow().join("\n");
    Run { code, out, err, seen }
}

#[test]
fn connect_sends_the_form_with_the_secret_from_stdin_and_never_prints_it() {
    let argv = [
        "connect",
        "jira",
        "site=acme.atlassian.net",
        "email=ada@acme.example",
        "token=-",
    ];
    let done = run(&argv, "s3cret\n");
    assert_eq!(done.code, 0, "{}", done.err);
    let call = done.seen.last().expect("a call");
    assert_eq!(
        (call.method.as_str(), call.path.as_str()),
        ("POST", "/v1/connections/jira")
    );
    assert_eq!(
        call.body,
        json!({ "site": "acme.atlassian.net", "email": "ada@acme.example", "token": "s3cret", "scope": "user" })
    );
    assert!(
        done.out
            .contains("Connected jira acme.atlassian.net as ada@acme.example."),
        "{}",
        done.out
    );
    assert!(!done.out.contains("s3cret"));

    let org = run(&["connect", "jira", "--org", "site=acme.atlassian.net"], "");
    assert_eq!(org.seen.last().expect("a call").body["scope"], "org");
    assert!(org.out.contains("for the organization."), "{}", org.out);
}

#[test]
fn connect_without_fields_names_the_form_or_starts_oauth() {
    let form = run(&["connect", "jira"], "");
    assert_eq!(form.code, 2);
    assert!(
        form.err
            .contains("nexus connect jira [--org] site=<site> email=<email> token=-"),
        "{}",
        form.err
    );

    let oauth = run(&["connect", "notion", "--org"], "");
    assert_eq!(oauth.code, 0, "{}", oauth.err);
    let call = oauth.seen.last().expect("a call");
    assert_eq!(
        (call.path.as_str(), &call.body),
        ("/v1/connections/notion/oauth", &json!({ "scope": "org" }))
    );
    assert!(oauth.out.contains("https://api.notion.com/v1/oauth/authorize?state=s1"));

    let github = run(&["connect", "github"], "");
    assert_eq!(github.code, 1);
    assert!(github.err.contains("github has nothing to connect"), "{}", github.err);
}

/// Linear's shape: OAuth when Gateway has Nexus's app, and a key form beside it. The link comes first,
/// as in Nexus UI, and the key form is named under it.
#[test]
fn connect_with_oauth_and_a_form_starts_oauth_and_names_the_key_form_under_the_link() {
    let mine = run(&["connect", "linear"], "");
    assert_eq!(mine.code, 0, "{}", mine.err);
    let call = mine.seen.last().expect("a call");
    assert_eq!(
        (call.method.as_str(), call.path.as_str(), &call.body),
        ("POST", "/v1/connections/linear/oauth", &json!({ "scope": "user" }))
    );
    let lines: Vec<&str> = mine.out.lines().collect();
    assert_eq!(
        lines,
        [
            "Open this link to connect linear. It works once, for ten minutes:",
            "  https://oauth.example.com/authorize?state=s2",
            "Or paste a key instead: nexus connect linear token=-",
        ]
    );

    let org = run(&["connect", "linear", "--org"], "");
    assert_eq!(org.code, 0, "{}", org.err);
    assert_eq!(org.seen.last().expect("a call").body, json!({ "scope": "org" }));
    assert!(
        org.out
            .contains("Or paste a key instead: nexus connect linear --org token=-"),
        "{}",
        org.out
    );

    // --json keeps its one field, so a script reads the link alone.
    let json = run(&["connect", "linear", "--json"], "");
    assert_eq!(json.code, 0, "{}", json.err);
    let printed: Value = serde_json::from_str(&json.out).expect("json");
    assert_eq!(
        printed,
        json!({ "authorizeUrl": "https://oauth.example.com/authorize?state=s2" })
    );

    // Fields given still go to the key form, OAuth or not.
    let key = run(&["connect", "linear", "token=-"], "lin_api_s3cret\n");
    assert_eq!(key.code, 0, "{}", key.err);
    assert_eq!(key.seen.last().expect("a call").path, "/v1/connections/linear");
    assert!(!format!("{}{}", key.out, key.err).contains("s3cret"));

    // One that takes no account keeps its form usage, though the catalog says oauth: no link to a dead end.
    let none = run(&["connect", "zendesk"], "");
    assert_eq!(none.code, 2, "{}", none.err);
    assert!(
        none.err.contains("nexus connect zendesk [--org] token=-"),
        "{}",
        none.err
    );
    assert!(!none.seen.iter().any(|seen| seen.path.ends_with("/oauth")));
}

#[test]
fn connections_list_and_disconnect() {
    let listed = run(&["connections"], "");
    assert_eq!(listed.code, 0, "{}", listed.err);
    assert!(listed.out.contains("acme.atlassian.net"), "{}", listed.out);
    assert!(
        listed.out.contains("needs reconnect: nexus connect notion --org"),
        "{}",
        listed.out
    );

    let gone = run(&["disconnect", "jira", "acme.atlassian.net", "--org"], "");
    assert_eq!(gone.code, 0, "{}", gone.err);
    let call = gone.seen.last().expect("a call");
    assert_eq!(
        (call.method.as_str(), call.path.as_str(), call.query.as_str()),
        ("DELETE", "/v1/connections/jira/acme.atlassian.net", "scope=org")
    );
    assert!(gone.out.contains("Disconnected jira acme.atlassian.net."));
    assert_eq!(run(&["disconnect", "jira"], "").code, 2);
}

/// A PKCS#8 PEM's shape, several lines long: what Snowflake's private_key holds.
const PEM: &str = "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIFHDBOBgkqhkiG9w0BBQ0wQTApBgkqhkiG9w0BBQwwHAQI\npem-s3cret\n-----END ENCRYPTED PRIVATE KEY-----";

#[test]
fn connect_datadog_reads_one_key_from_stdin_and_one_from_a_file_and_prints_neither() {
    let argv = [
        "connect",
        "datadog",
        "--org",
        "site=datadoghq.com",
        "api_key=-",
        "application_key=@dd-app.key",
    ];
    assert!(!argv.join(" ").contains("s3cret"));
    let done = run_with(&argv, "api-s3cret\n", &[("dd-app.key", "app-s3cret\n", 0o600)]);
    assert_eq!(done.code, 0, "{}", done.err);
    let call = done.seen.last().expect("a call");
    assert_eq!(call.path, "/v1/connections/datadog");
    assert_eq!(
        call.body,
        json!({ "site": "datadoghq.com", "api_key": "api-s3cret", "application_key": "app-s3cret", "scope": "org" })
    );
    assert!(!format!("{}{}", done.out, done.err).contains("s3cret"));
}

#[test]
fn connect_snowflake_reads_a_multi_line_pem_from_a_file_and_its_passphrase_from_stdin() {
    let pem = format!("{PEM}\n");
    let argv = [
        "connect",
        "snowflake",
        "account_identifier=acme-xy12345",
        "user=NEXUS",
        "warehouse=COMPUTE_WH",
        "private_key=@rsa_key.p8",
        "passphrase=-",
    ];
    let done = run_with(&argv, "pass-s3cret\n", &[("rsa_key.p8", &pem, 0o400)]);
    assert_eq!(done.code, 0, "{}", done.err);
    let body = &done.seen.last().expect("a call").body;
    assert_eq!(
        (body["private_key"].as_str(), body["passphrase"].as_str()),
        (Some(PEM), Some("pass-s3cret"))
    );
    assert!(!format!("{}{}", done.out, done.err).contains("s3cret"));
}

#[test]
fn connect_refuses_a_missing_empty_or_open_file_and_a_second_stdin_field_before_any_call() {
    let files = [("empty.key", "\n", 0o600), ("open.key", "open-s3cret\n", 0o644)];
    let cases: &[(&str, i32, &str)] = &[
        ("application_key=-", 2, "Only one field can read stdin (-)."),
        ("application_key=@nope.key", 1, "No file nope.key for application_key."),
        (
            "application_key=@empty.key",
            1,
            "No value for application_key in empty.key.",
        ),
        (
            "application_key=@open.key",
            1,
            "Other users can read open.key, so it is not sent as application_key.",
        ),
        (
            "application_key=@",
            2,
            "Give the file after @: application_key=@<file>.",
        ),
    ];
    for (field, code, says) in cases {
        let done = run_with(
            &["connect", "datadog", "site=datadoghq.com", "api_key=-", field],
            "api-s3cret\n",
            &files,
        );
        assert_eq!(done.code, *code, "{field}: {}", done.err);
        assert!(done.err.contains(says), "{field}: {}", done.err);
        assert!(!format!("{}{}", done.out, done.err).contains("s3cret"), "{field}");
        assert!(done.seen.is_empty(), "{field}: no call before the fields are whole");
    }
}

#[test]
fn connect_without_fields_names_the_second_secret_as_a_file() {
    let form = run(&["connect", "datadog"], "");
    assert_eq!(form.code, 2);
    assert!(
        form.err
            .contains("nexus connect datadog [--org] site=<site> api_key=- application_key=@<file>"),
        "{}",
        form.err
    );
}

/// `--write` makes the consent call a write refused with `write_consent_required` names (`access: write`)
/// and prints the vendor's link; nothing else is sent.
#[test]
fn connect_write_asks_the_vendor_for_writes_and_prints_the_link() {
    let mine = run(&["connect", "microsoft-todo", "--write"], "");
    assert_eq!(mine.code, 0, "{}", mine.err);
    let call = mine.seen.last().expect("a call");
    assert_eq!(
        (call.method.as_str(), call.path.as_str(), &call.body),
        (
            "POST",
            "/v1/connections/microsoft-todo/oauth",
            &json!({ "scope": "user", "access": "write" })
        )
    );
    let lines: Vec<&str> = mine.out.lines().collect();
    assert_eq!(
        lines,
        [
            "Open this link to allow writes to microsoft-todo. It works once, for ten minutes:",
            "  https://login.microsoftonline.com/common/oauth2/v2.0/authorize?state=s3",
        ]
    );

    let org = run(&["connect", "microsoft-todo", "--org", "--write", "--json"], "");
    assert_eq!(org.code, 0, "{}", org.err);
    assert_eq!(
        org.seen.last().expect("a call").body,
        json!({ "scope": "org", "access": "write" })
    );
    let printed: Value = serde_json::from_str(&org.out).expect("json");
    assert_eq!(
        printed,
        json!({ "authorizeUrl": "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?state=s3" })
    );

    // Writes come through the vendor's link only: no fields, and no key form instead.
    for argv in [
        ["connect", "linear", "--write", "token=-"],
        ["connect", "linear", "token=-", "--write"],
    ] {
        let fields = run(&argv, "lin_api_s3cret");
        assert_eq!(fields.code, 2, "{argv:?}: {}", fields.err);
        assert!(fields.err.contains("give it no fields"), "{}", fields.err);
        assert!(fields.seen.is_empty(), "{argv:?}: nothing sent");
    }
    let linear = run(&["connect", "linear", "--write"], "");
    assert_eq!(linear.code, 0, "{}", linear.err);
    assert!(!linear.out.contains("paste a key"), "{}", linear.out);
}
