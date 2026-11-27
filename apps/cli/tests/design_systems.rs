//! `nexus design-system` and `nexus pages check` against a stand-in Gateway (`design-systems.test.ts`): what a sync
//! reads from a folder and sends, the name it picks, the default, and what Check prints.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_ds";

fn brand() -> Value {
    json!({ "id": "ds_nexus_brand", "name": "@nexus/brand", "builtIn": true, "isDefault": true, "source": "brand/v5.0 (Nexus AI)", "variableCount": 280, "componentCount": 0 })
}

fn acme() -> Value {
    json!({ "id": "ds_acme", "name": "acme", "builtIn": false, "isDefault": false, "source": "nexus design-system sync ./web (3 files)", "variableCount": 4, "componentCount": 2, "updatedAt": "2026-11-05T12:00:00.000Z" })
}

fn with(mut base: Value, extra: Value) -> Value {
    if let (Some(map), Value::Object(extra)) = (base.as_object_mut(), extra) {
        map.extend(extra);
    }
    base
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn route(method: &str, path: &str, body: &Value) -> Response {
    match (method, path) {
        ("GET", "/v1/design-systems") => answer(
            200,
            json!({ "designSystems": [brand(), acme()], "defaultId": "ds_nexus_brand" }),
        ),
        ("GET", "/v1/design-systems/ds_nexus_brand") => answer(
            200,
            with(
                brand(),
                json!({ "css": ":root {\n  --nx-page: #000000;\n}\n", "variables": [], "notes": "## Principles\n", "components": [] }),
            ),
        ),
        ("GET", "/v1/design-systems/acme") => answer(
            200,
            with(
                acme(),
                json!({ "css": ":root {\n  --ink: #111;\n}\n", "variables": [], "notes": "", "components": ["Button"] }),
            ),
        ),
        ("POST", "/v1/design-systems") => answer(
            201,
            json!({ "designSystem": with(acme(), json!({ "name": body["name"], "isDefault": body.get("makeDefault") == Some(&json!(true)) })), "created": true, "read": ["src/styles/tokens.css: 1 variables (CSS)"] }),
        ),
        ("POST", "/v1/design-systems/ds_acme/default") => answer(200, with(acme(), json!({ "isDefault": true }))),
        ("POST", "/v1/pages/pag_1/check") => answer(
            200,
            json!({
                "pageId": "pag_1", "version": 2, "designSystem": acme(), "linked": true,
                "findings": [{ "kind": "color", "board": "Home", "boardId": "L1", "layer": "Lede", "selector": "[data-artboard=\"Home\"] > p", "box": { "x": 16, "y": 40, "w": 768, "h": 20 }, "property": "color", "value": "#FF0000", "expected": "--ink", "message": "color #FF0000 is not a token; the nearest is --ink" }],
                "counts": { "color": 1, "fontSize": 0, "spacing": 0, "contrast": 0 }, "truncated": false, "elements": 4,
            }),
        ),
        _ => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
        ),
    }
}

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

fn gateway() -> (String, Seen) {
    let seen: Seen = Arc::default();
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
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    log.lock()
                        .expect("log")
                        .push((method.clone(), path.clone(), body.clone()));
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
    seen: Vec<(String, String, Value)>,
}

fn temp(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "nexus-ds-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

fn run_in(dir: &Path, argv: &[&str]) -> Run {
    let (url, seen) = gateway();
    let config_path = temp("config").join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": KEY, "org": "acme" }).to_string(),
    )
    .expect("config");
    let capture = Capture(std::rc::Rc::new(Captured::default()));
    let ctx = Context {
        config_path,
        io: Box::new(capture.clone()),
        color: false,
        env: HashMap::new(),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir.to_path_buf(),
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let seen = seen.lock().expect("seen").clone();
    Run {
        code,
        out: capture.0.out.borrow().join("\n"),
        seen,
    }
}

/// The same repository as `design-systems.test.ts`.
fn repo() -> PathBuf {
    let dir = temp("repo");
    let put = |path: &str, text: &str| {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        std::fs::write(file, text).expect("file");
    };
    put("package.json", &json!({ "name": "@Acme/Web App" }).to_string());
    put("src/styles/tokens.css", ":root { --ink: #111; }");
    put("src/styles/plain.css", "p { color: red }");
    put(
        "tailwind.config.ts",
        "export default { theme: { spacing: { 4: '16px' } } }",
    );
    put("design/tokens.json", r##"{"color":{"brand":{"$value":"#5B21B6"}}}"##);
    put("DESIGN.md", "# Acme\n\nOne primary action per view.\n");
    put("src/components/Button.tsx", "export function Button() {}");
    put("src/components/Card/index.tsx", "export function Card() {}");
    put("src/components/use-thing.ts", "");
    put("node_modules/x/tokens.json", r##"{"a":{"$value":"#000"}}"##);
    put(".git/tokens.json", r##"{"a":{"$value":"#000"}}"##);
    dir
}

#[test]
fn lists_them_and_shows_the_defaults_link_tokens_and_notes() {
    let dir = temp("list");
    let r = run_in(&dir, &["design-system"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let brand = r
        .out
        .lines()
        .find(|line| line.contains("@nexus/brand"))
        .expect("the brand's row");
    let cells: Vec<&str> = brand.split_whitespace().collect();
    assert_eq!(
        &cells[1..5],
        ["@nexus/brand", "ds_nexus_brand", "280", "yes"],
        "{}",
        r.out
    );
    let r = run_in(&dir, &["design-system", "show"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        r.out
            .contains("<meta name=\"nexus-design-system\" content=\"@nexus/brand\">"),
        "{}",
        r.out
    );
    assert!(r.out.contains("--nx-page: #000000;"), "{}", r.out);
    assert!(r.out.contains("## Principles"), "{}", r.out);
}

#[test]
fn syncs_a_folder_as_the_typescript_cli_does() {
    let r = run_in(&repo(), &["design-system", "sync", ".", "--default"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let (_, path, body) = r.seen.iter().find(|(method, _, _)| method == "POST").expect("a save");
    assert_eq!(path, "/v1/design-systems");
    assert_eq!(body["name"], "@acme/web-app");
    assert_eq!(body["components"], json!(["Button", "Card"]));
    assert_eq!(body["notes"], "# Acme\n\nOne primary action per view.\n");
    assert_eq!(body["makeDefault"], true);
    assert_eq!(body["source"], "nexus design-system sync . (3 files)");
    let files: Vec<&str> = body["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    assert_eq!(
        files,
        ["src/styles/tokens.css", "tailwind.config.ts", "design/tokens.json"]
    );
    assert!(r.out.contains("New designs use it."), "{}", r.out);
}

#[test]
fn refuses_a_folder_with_no_tokens_before_it_sends_anything() {
    let r = run_in(&temp("empty"), &["design-system", "sync", "."]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(r.seen.is_empty());
    assert_eq!(run_in(&temp("usage"), &["design-system", "sync"]).code, 2);
}

#[test]
fn makes_one_the_default_by_name() {
    let r = run_in(&temp("use"), &["design-system", "use", "acme"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let calls: Vec<String> = r
        .seen
        .iter()
        .map(|(method, path, _)| format!("{method} {path}"))
        .collect();
    assert_eq!(
        calls,
        ["GET /v1/design-systems/acme", "POST /v1/design-systems/ds_acme/default"]
    );
    assert!(r.out.contains("New designs use acme."), "{}", r.out);
}

#[test]
fn pages_check_prints_the_counts_and_each_finding() {
    let r = run_in(&temp("check"), &["pages", "check", "pag_1", "2"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.seen[0].1, "/v1/pages/pag_1/check");
    assert_eq!(r.seen[0].2, json!({ "version": 2 }));
    assert!(
        r.out.contains("pag_1 v2 against acme: 4 elements checked."),
        "{}",
        r.out
    );
    assert!(r.out.contains("Home › Lede"), "{}", r.out);
    assert!(r.out.contains("--ink"), "{}", r.out);
}
