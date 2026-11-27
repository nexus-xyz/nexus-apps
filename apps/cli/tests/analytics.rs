//! `nexus analytics --by key` and `nexus spend --by key` against a stand-in Gateway (`analytics.test.ts`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn figures(spent: f64, calls: u64) -> Value {
    json!({
        "spentUsd": spent, "calls": calls, "inputTokens": calls * 100, "outputTokens": calls * 10,
        "callsByStatus": { "settled": calls, "failed": 0, "refused": 0, "released": 0 }, "errorRate": null,
    })
}

fn group(key: &str, label: &str, extra: Value, spent: f64, calls: u64) -> Value {
    let mut group = figures(spent, calls);
    group["key"] = json!(key);
    group["label"] = json!(label);
    // Four decimals: serde_json is built without `float_roundtrip`, so parsing a long float loses its last digit.
    group["share"] = json!((spent / 3.25 * 10_000.0).round() / 10_000.0);
    for (name, value) in extra.as_object().expect("extra").clone() {
        group[name.as_str()] = value;
    }
    group
}

/// What Gateway answers for `groupBy=key`: a named key, a key with no name, a revoked key and an older call.
fn by_key(scope: &str, others: bool) -> Value {
    let mut groups = vec![
        group(
            "key_ci",
            "CI (nxk_ci12)",
            json!({ "name": "CI", "prefix": "nxk_ci12" }),
            1.5,
            6,
        ),
        group("key_bare", "nxk_ab34", json!({ "prefix": "nxk_ab34" }), 0.5, 2),
        group(
            "key_old",
            "Old laptop (nxk_zz99), revoked",
            json!({ "name": "Old laptop", "prefix": "nxk_zz99", "revoked": true }),
            0.75,
            3,
        ),
        group("key_gone", "key_gone", json!({ "deleted": true }), 0.25, 1),
        group("unassigned", "No key (an older call)", json!({}), 0.25, 1),
    ];
    if others {
        groups.push(group("others", "Others in your teams", json!({}), 0.0, 0));
    }
    json!({
        "from": "2026-10-01", "to": "2026-10-28", "groupBy": "key", "scope": scope,
        "totals": figures(3.25, 13), "groups": groups, "series": [],
    })
}

fn route(path: &str, query: &str) -> Response {
    match path {
        // `--team mine` stands in for a member: Gateway then adds the others row.
        "/v1/analytics/usage" if query.contains("groupBy=key") => {
            let member = query.contains("team=mine");
            answer(200, by_key(if member { "member" } else { "org" }, member))
        }
        _ => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
        ),
    }
}

fn gateway() -> (String, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
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
                    let uri = request.uri().clone();
                    log.lock()
                        .expect("log")
                        .push(uri.path_and_query().map(|p| p.to_string()).unwrap_or_default());
                    route(uri.path(), uri.query().unwrap_or(""))
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
    seen: Vec<String>,
}

fn run(argv: &[&str]) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-analytics-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway();
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": "nx_0123456789abcdef", "kind": "api" }).to_string(),
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
        cwd: dir,
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
        err: capture.0.err.borrow().join("\n"),
        seen,
    }
}

/// The row of the table that starts with `label`.
fn row<'a>(out: &'a str, label: &str) -> &'a str {
    out.lines()
        .find(|line| line.trim_start().trim_start_matches("· ").starts_with(label))
        .unwrap_or_else(|| panic!("no row for {label}:\n{out}"))
}

#[test]
fn by_key_asks_for_groupby_key_and_heads_the_first_column_key() {
    let table = run(&["analytics", "--by", "key", "--from", "2026-10-01", "--to", "2026-10-28"]);
    assert_eq!(table.code, 0, "{}{}", table.out, table.err);
    assert_eq!(
        table.seen,
        vec!["/v1/analytics/usage?groupBy=key&from=2026-10-01&to=2026-10-28"]
    );
    assert!(table.out.contains("Usage · by key"), "{}", table.out);
    let header = table
        .out
        .lines()
        .find(|line| line.contains("Tokens in"))
        .expect("header");
    assert!(header.trim_start().starts_with("Key "), "{header}");
}

#[test]
fn a_key_shows_its_label_and_nothing_else_of_it() {
    let table = run(&["analytics", "--by", "key"]);
    assert_eq!(table.code, 0, "{}{}", table.out, table.err);
    assert!(row(&table.out, "CI (nxk_ci12)").contains("$1.50"), "{}", table.out);
    assert!(row(&table.out, "nxk_ab34").contains("$0.50"), "{}", table.out);
    // The key id and the name and prefix fields stay out: the label already holds them.
    assert!(!table.out.contains("key_ci"), "{}", table.out);
    assert!(!table.out.contains("key_bare"), "{}", table.out);
    // A key that no store holds shows as a deleted team does: its label, then "(deleted)".
    assert!(
        row(&table.out, "key_gone").contains("key_gone (deleted)"),
        "{}",
        table.out
    );
    assert!(
        row(&table.out, "No key (an older call)").contains("$0.25"),
        "{}",
        table.out
    );
    assert!(!table.out.contains("Unassigned:"), "{}", table.out);
}

#[test]
fn a_revoked_key_prints_its_label_as_gateway_sent_it() {
    let table = run(&["analytics", "--by", "key"]);
    assert_eq!(table.code, 0, "{}{}", table.out, table.err);
    let line = row(&table.out, "Old laptop (nxk_zz99), revoked");
    assert!(line.contains("$0.75"), "{line}");
    assert!(!line.contains("(deleted)"), "{line}");
    assert_eq!(line.matches("revoked").count(), 1, "{line}");
}

#[test]
fn a_member_sees_the_others_row_as_it_is() {
    let table = run(&["analytics", "--by", "key", "--team", "mine"]);
    assert_eq!(table.code, 0, "{}{}", table.out, table.err);
    assert!(table.out.contains("your usage and your teams'"), "{}", table.out);
    assert!(
        row(&table.out, "Others in your teams").contains("$0.00"),
        "{}",
        table.out
    );
}

#[test]
fn json_prints_the_report_as_gateway_sent_it() {
    let printed = run(&["analytics", "--by", "key", "--json"]);
    assert_eq!(printed.code, 0, "{}{}", printed.out, printed.err);
    let report: Value = serde_json::from_str(&printed.out).expect("json");
    assert_eq!(report, by_key("org", false));
}

#[test]
fn spend_by_key_prints_the_same_table() {
    let spend = run(&["spend", "--by", "key"]);
    let analytics = run(&["analytics", "--by", "key"]);
    assert_eq!(spend.code, 0, "{}{}", spend.out, spend.err);
    assert_eq!(spend.out, analytics.out);
    assert!(
        spend.seen[0].starts_with("/v1/analytics/usage?groupBy=key"),
        "{:?}",
        spend.seen
    );
}

#[test]
fn an_unknown_grouping_is_refused_before_anything_is_sent_and_names_key() {
    for argv in [&["analytics", "--by", "keys"][..], &["spend", "--by", "keys"]] {
        let refused = run(argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
        assert!(
            refused
                .err
                .contains("--by must be one of: team, person, model, provider, key, product, day."),
            "{}",
            refused.err
        );
    }
}

#[test]
fn depth_and_under_still_roll_up_teams_only() {
    let refused = run(&["analytics", "--by", "key", "--depth", "1"]);
    assert_eq!(refused.code, 2, "{}", refused.out);
    assert!(refused.seen.is_empty());
    assert!(refused.err.contains("use them with --by team"), "{}", refused.err);
}
