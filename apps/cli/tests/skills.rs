//! `nexus skills` against a stand-in Gateway (`skills.test.ts`): the requests it sends, a file sent exactly as it is
//! (byte-order mark, CRLF, no final newline), a folder's other files named and not sent, `--update`, every refusal
//! printed with its code and the fix, an export written byte for byte, the editor session of `create`, and usage
//! errors (exit 2) before anything is sent.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_skills";
/// A SKILL.md as a Windows editor leaves it: a byte-order mark, CRLF line ends and no final newline.
const WINDOWS_SKILL: &str = "\u{feff}---\r\nname: changelog\r\ndescription: Write release notes from merged pull requests.\r\n---\r\n\r\nGroup the changes by area.";
const OLD_SKILL: &str = "---\nname: changelog\ndescription: Old.\n---\n";
const NEW_SKILL: &str =
    "---\nname: release-notes\ndescription: Write release notes.\n---\n\n# release-notes\n\nList the changes.\n";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: Value,
    key: Option<String>,
}

fn changelog() -> Value {
    json!({ "id": "skl_1", "name": "changelog", "description": "Write release notes from merged pull requests.", "latestVersion": 2, "createdAt": "2026-10-18T12:00:00.000Z", "updatedAt": "2026-10-18T13:00:00.000Z" })
}

fn pdf() -> Value {
    json!({ "id": "skl_2", "name": "pdf-processing", "description": "Extract text\n  and tables from PDFs.\n", "latestVersion": 1, "createdAt": "2026-10-17T12:00:00.000Z", "updatedAt": "2026-10-17T12:00:00.000Z" })
}

fn version(skill: &str, number: i64, bytes: usize) -> Value {
    json!({ "skillId": skill, "version": number, "name": "changelog", "description": "d", "contentSha256": "a".repeat(64), "bytes": bytes, "createdVia": "cli", "createdAt": "2026-10-18T13:00:00.000Z" })
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn refusal(status: u16, code: &str, message: &str) -> Response {
    answer(
        status,
        json!({ "error": { "message": message, "type": "invalid_request_error", "code": code } }),
    )
}

/// The `name:` line of a SKILL.md, however its lines end.
fn name_of(content: &str) -> String {
    content
        .trim_start_matches('\u{feff}')
        .lines()
        .find_map(|line| line.trim_end().strip_prefix("name: "))
        .unwrap_or("")
        .to_string()
}

fn export(body: &str) -> Response {
    (
        [("content-type", "text/markdown; charset=utf-8")],
        body.as_bytes().to_vec(),
    )
        .into_response()
}

fn route(method: &str, path: &str, body: &Value, skills: &[Value]) -> Response {
    match (method, path) {
        ("GET", "/v1/skills") => answer(200, json!({ "skills": skills })),
        ("POST", "/v1/skills") => {
            let content = body["content"].as_str().unwrap_or("");
            if content.contains("broken") {
                return refusal(
                    400,
                    "invalid_frontmatter",
                    "a SKILL.md starts with a line `---`: the front matter is missing",
                );
            }
            let name = name_of(content);
            if name == "changelog" {
                return answer(
                    409,
                    json!({ "error": { "message": "you already have a skill named changelog", "type": "conflict_error", "code": "duplicate_name", "nexus": { "name": "changelog", "skillId": "skl_1" } } }),
                );
            }
            let mut skill = changelog();
            skill["id"] = json!("skl_new");
            skill["name"] = json!(name);
            skill["latestVersion"] = json!(1);
            answer(
                201,
                json!({ "skill": skill, "version": version("skl_new", 1, content.len()) }),
            )
        }
        // The skill is gone: Gateway refused the name a moment ago, and `changelog` is no longer in the list.
        ("GET", "/v1/skills/skl_1/versions/latest") if !skills.iter().any(|skill| skill["id"] == "skl_1") => {
            refusal(404, "skill_not_found", "you have no skill skl_1")
        }
        // The latest version of `changelog` is version 2, which is WINDOWS_SKILL.
        ("GET", "/v1/skills/skl_1/versions/latest") => {
            let mut latest = version("skl_1", 2, WINDOWS_SKILL.len());
            latest["contentSha256"] = json!(nexus_client::base::sha256_hex(&[WINDOWS_SKILL.as_bytes()]));
            latest["content"] = json!(WINDOWS_SKILL);
            answer(200, latest)
        }
        ("POST", "/v1/skills/skl_1/versions") => {
            let content = body["content"].as_str().unwrap_or("");
            // A version that someone wrote after the latest one was read.
            if content.contains("written meanwhile") {
                return refusal(
                    409,
                    "skill_version_conflict",
                    "version 2 is no longer the latest version of changelog",
                );
            }
            let mut skill = changelog();
            skill["latestVersion"] = json!(3);
            answer(
                201,
                json!({ "skill": skill, "version": version("skl_1", 3, content.len()) }),
            )
        }
        ("GET", "/v1/skills/skl_1/versions/latest/export") | ("GET", "/v1/skills/skl_1/versions/2/export") => {
            export(WINDOWS_SKILL)
        }
        ("GET", "/v1/skills/skl_1/versions/1/export") => export(OLD_SKILL),
        ("GET", "/v1/skills/skl_1/versions/9/export") => {
            refusal(404, "skill_version_not_found", "skill skl_1 has no version 9")
        }
        ("DELETE", "/v1/skills/skl_1") => answer(200, json!({ "deleted": true, "id": "skl_1", "versions": 2 })),
        ("DELETE", "/v1/skills/skl_2") => answer(200, json!({ "deleted": true, "id": "skl_2", "versions": 1 })),
        _ => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
        ),
    }
}

fn gateway(skills: Vec<Value>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let (ready, port) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let app = Router::new().fallback(move |headers: HeaderMap, request: Request| {
                let log = log.clone();
                let skills = skills.clone();
                async move {
                    let method = request.method().to_string();
                    let uri = request.uri().clone();
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: uri.path_and_query().map(|p| p.to_string()).unwrap_or_default(),
                        body: body.clone(),
                        key: headers
                            .get("idempotency-key")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from),
                    });
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
                        return answer(
                            401,
                            json!({ "error": { "message": "invalid API key", "type": "authentication_error" } }),
                        );
                    }
                    route(&method, uri.path(), &body, &skills)
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

/// What a run starts from: the person's skills, the environment, and what the editor does.
struct Setup {
    skills: Vec<Value>,
    env: Vec<(&'static str, &'static str)>,
    /// The text the editor saves; `None`: it saves the file as it found it.
    edit_saves: Option<&'static str>,
    edit_exit: i32,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            skills: vec![changelog(), pdf()],
            env: Vec::new(),
            edit_saves: None,
            edit_exit: 0,
        }
    }
}

struct Run {
    code: i32,
    out: String,
    err: String,
    seen: Vec<Seen>,
    dir: PathBuf,
    /// Each editor run: the command and the file's text when it opened.
    edited: Vec<(Vec<String>, String)>,
}

fn new_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nexus-skills-{}", uuid_like()));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

fn run(argv: &[&str]) -> Run {
    run_in(new_dir(), Setup::default(), argv)
}

fn run_with(setup: Setup, argv: &[&str]) -> Run {
    run_in(new_dir(), setup, argv)
}

/// A run in this working directory: what an earlier run there wrote is still there.
fn run_in(dir: PathBuf, setup: Setup, argv: &[&str]) -> Run {
    let (url, seen) = gateway(setup.skills);
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": KEY, "org": "acme" }).to_string(),
    )
    .expect("config");
    let capture = Capture(std::rc::Rc::new(Captured {
        can_stream: true,
        edit_saves: setup.edit_saves.map(String::from),
        edit_exit: setup.edit_exit,
        ..Captured::default()
    }));
    let ctx = Context {
        config_path,
        io: Box::new(capture.clone()),
        color: false,
        env: setup
            .env
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect::<HashMap<_, _>>(),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir.clone(),
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
        dir,
        edited: capture.0.edited.borrow().clone(),
    }
}

/// A directory name of its own per run: tests run in parallel, so the process id and a counter.
fn uuid_like() -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    )
}

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

fn requests(r: &Run) -> Vec<String> {
    r.seen
        .iter()
        .map(|seen| format!("{} {}", seen.method, seen.path))
        .collect()
}

/// The folder `nexus skills create` said it kept the text in: the path in "Your text is kept in <path>."
fn kept_file(err: &str) -> PathBuf {
    let rest = err.split("Your text is kept in ").nth(1).expect("a kept path");
    let end = rest.find("SKILL.md").expect("SKILL.md") + "SKILL.md".len();
    PathBuf::from(&rest[..end])
}

#[test]
fn lists_one_line_per_skill_and_says_so_when_there_are_none() {
    let r = run(&["skills"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(requests(&r), ["GET /v1/skills"]);
    let words = words(&r.out);
    assert!(
        words
            .windows(8)
            .any(|w| w == ["changelog", "v2", "Write", "release", "notes", "from", "merged", "pull"]),
        "{}",
        r.out
    );
    // A description is one line, however it was written.
    assert!(
        words.windows(8).any(|w| w
            == [
                "pdf-processing",
                "v1",
                "Extract",
                "text",
                "and",
                "tables",
                "from",
                "PDFs."
            ]),
        "{}",
        r.out
    );
    let r = run(&["skills", "list", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let printed: Value = serde_json::from_str(r.out.trim()).expect("json");
    assert_eq!(printed, json!({ "skills": [changelog(), pdf()] }));
    let none = Setup {
        skills: Vec::new(),
        ..Setup::default()
    };
    let r = run_with(none, &["skills"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "No skills yet. Import one: nexus skills import ./my-skill");
}

#[test]
fn imports_a_file_and_names_what_a_folder_left_out() {
    let dir = new_dir();
    std::fs::write(dir.join("one.md"), NEW_SKILL).expect("file");
    std::fs::create_dir_all(dir.join("release-notes/references")).expect("dir");
    std::fs::create_dir_all(dir.join("release-notes/scripts")).expect("dir");
    std::fs::write(dir.join("release-notes/SKILL.md"), NEW_SKILL).expect("file");
    std::fs::write(dir.join("release-notes/references/guide.md"), "guide").expect("file");
    std::fs::write(dir.join("release-notes/scripts/run.sh"), "echo").expect("file");

    let r = run_in(dir.clone(), Setup::default(), &["skills", "import", "one.md"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        (r.seen[0].method.as_str(), r.seen[0].path.as_str()),
        ("POST", "/v1/skills")
    );
    assert_eq!(r.seen[0].body, json!({ "content": NEW_SKILL }));
    assert!(r.seen[0].key.is_some());
    assert_eq!(
        r.out,
        format!("✓ Imported release-notes, version 1 ({} bytes).", NEW_SKILL.len())
    );

    // A folder: SKILL.md alone is sent; the rest is named, sorted, with a warning.
    let r = run_in(dir.clone(), Setup::default(), &["skills", "import", "release-notes"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen.len(), 1);
    assert_eq!(r.seen[0].body, json!({ "content": NEW_SKILL }));
    assert!(
        r.out
            .ends_with("! Sent SKILL.md only. Not sent: references/guide.md, scripts/run.sh."),
        "{}",
        r.out
    );

    let r = run_in(
        dir.clone(),
        Setup::default(),
        &["skills", "import", "release-notes", "--json"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    let printed: Value = serde_json::from_str(r.out.trim()).expect("json");
    assert_eq!(printed["skill"]["name"], "release-notes");
    assert_eq!(printed["notSent"], json!(["references/guide.md", "scripts/run.sh"]));

    // Ten names are shown, then a count.
    std::fs::create_dir_all(dir.join("release-notes/assets")).expect("dir");
    for number in 0..11 {
        std::fs::write(dir.join(format!("release-notes/assets/a{number:02}.txt")), "x").expect("file");
    }
    let r = run_in(dir, Setup::default(), &["skills", "import", "release-notes"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.out.ends_with(
            "Not sent: assets/a00.txt, assets/a01.txt, assets/a02.txt, assets/a03.txt, assets/a04.txt, assets/a05.txt, assets/a06.txt, assets/a07.txt, assets/a08.txt, assets/a09.txt and 3 more."
        ),
        "{}",
        r.out
    );
}

#[test]
fn sends_a_file_exactly_as_it_is() {
    let dir = new_dir();
    std::fs::write(dir.join("SKILL.md"), WINDOWS_SKILL).expect("file");
    let r = run_in(dir, Setup::default(), &["skills", "import", "SKILL.md"]);
    // The name is taken, so Gateway refuses; the request still carries the file as it is.
    assert_eq!(r.code, 1, "{}", r.err);
    assert_eq!(r.seen[0].body["content"], WINDOWS_SKILL);
    assert!(
        r.seen[0].body["content"]
            .as_str()
            .expect("content")
            .starts_with('\u{feff}')
    );
    assert!(r.err.contains("(duplicate_name)"), "{}", r.err);
}

#[test]
fn writes_a_new_version_with_update_when_the_name_is_taken_and_the_file_is_not_the_latest_version() {
    let dir = new_dir();
    let changed = format!("{WINDOWS_SKILL}\r\nThen write the notes.");
    std::fs::write(dir.join("SKILL.md"), &changed).expect("file");
    let r = run_in(dir.clone(), Setup::default(), &["skills", "import", "SKILL.md"]);
    assert_eq!(r.code, 1, "{}", r.err);
    assert!(
        r.err
            .contains("You already have a skill named changelog (duplicate_name)"),
        "{}",
        r.err
    );
    assert!(r.err.contains("nexus skills import <path> --update"), "{}", r.err);
    assert_eq!(requests(&r), ["POST /v1/skills"]);

    let r = run_in(dir, Setup::default(), &["skills", "import", "SKILL.md", "--update"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        requests(&r),
        [
            "POST /v1/skills",
            "GET /v1/skills/skl_1/versions/latest",
            "POST /v1/skills/skl_1/versions"
        ]
    );
    // The new version is built on the version that was read, so a newer one is not written over.
    assert_eq!(r.seen[2].body, json!({ "content": changed, "baseVersion": 2 }));
    assert!(r.seen[2].key.is_some());
    assert_eq!(
        r.out,
        format!("✓ Updated changelog to version 3 ({} bytes).", changed.len())
    );
}

#[test]
fn does_not_write_over_a_version_that_someone_wrote_meanwhile() {
    let dir = new_dir();
    let changed = format!("{WINDOWS_SKILL}\r\nwritten meanwhile");
    std::fs::write(dir.join("SKILL.md"), &changed).expect("file");
    let r = run_in(dir, Setup::default(), &["skills", "import", "SKILL.md", "--update"]);
    // Gateway refuses the version built on version 2, and the CLI does not ask again.
    assert_eq!(r.code, 1, "{}", r.err);
    assert_eq!(
        requests(&r),
        [
            "POST /v1/skills",
            "GET /v1/skills/skl_1/versions/latest",
            "POST /v1/skills/skl_1/versions"
        ]
    );
    assert_eq!(r.seen[2].body, json!({ "content": changed, "baseVersion": 2 }));
    assert!(
        r.err
            .contains("Version 2 is no longer the latest version of changelog (skill_version_conflict)"),
        "{}",
        r.err
    );
    assert!(
        r.err
            .contains("Someone wrote a newer version. Export the latest, make your change on it, and import again."),
        "{}",
        r.err
    );
    assert_eq!(r.out, "");
}

#[test]
fn writes_a_new_version_for_a_file_that_differs_only_by_its_byte_order_mark_or_its_line_ends() {
    // Bytes are compared, not text: to a reader these are the latest version's text, and neither is its bytes.
    let files = [
        (
            "no byte-order mark",
            WINDOWS_SKILL.trim_start_matches('\u{feff}').to_string(),
        ),
        ("LF line ends", WINDOWS_SKILL.replace("\r\n", "\n")),
    ];
    for (label, file) in files {
        assert_ne!(file, WINDOWS_SKILL, "{label}");
        let dir = new_dir();
        std::fs::write(dir.join("SKILL.md"), &file).expect("file");
        let r = run_in(dir, Setup::default(), &["skills", "import", "SKILL.md", "--update"]);
        assert_eq!(r.code, 0, "{label}: {}", r.err);
        assert_eq!(r.seen[2].body, json!({ "content": file, "baseVersion": 2 }), "{label}");
        assert_eq!(
            r.out,
            format!("✓ Updated changelog to version 3 ({} bytes).", file.len()),
            "{label}"
        );
    }
}

#[test]
fn stops_with_the_refusal_when_the_latest_version_cannot_be_read() {
    // The skill was deleted between Gateway's refusal of the name and the read of its latest version.
    let dir = new_dir();
    std::fs::write(dir.join("SKILL.md"), WINDOWS_SKILL).expect("file");
    let setup = Setup {
        skills: vec![pdf()],
        ..Setup::default()
    };
    let r = run_in(dir, setup, &["skills", "import", "SKILL.md", "--update"]);
    assert_eq!(r.code, 1, "{}", r.err);
    assert_eq!(
        requests(&r),
        ["POST /v1/skills", "GET /v1/skills/skl_1/versions/latest"]
    );
    assert!(r.err.contains("(skill_not_found)"), "{}", r.err);
    assert_eq!(r.out, "");
}

#[test]
fn leaves_a_file_that_is_already_the_latest_version_alone_with_update() {
    let dir = new_dir();
    std::fs::write(dir.join("SKILL.md"), WINDOWS_SKILL).expect("file");
    let r = run_in(
        dir.clone(),
        Setup::default(),
        &["skills", "import", "SKILL.md", "--update"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    // Gateway is asked and refuses the name; the latest version is read and is the same bytes: no version is written.
    assert_eq!(
        requests(&r),
        ["POST /v1/skills", "GET /v1/skills/skl_1/versions/latest"]
    );
    assert_eq!(
        r.out,
        format!(
            "✓ changelog is already version 2 ({} bytes). Nothing written.",
            WINDOWS_SKILL.len()
        )
    );

    let r = run_in(
        dir,
        Setup::default(),
        &["skills", "import", "SKILL.md", "--update", "--json"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    let printed: Value = serde_json::from_str(&r.out).expect("JSON");
    assert_eq!(printed["unchanged"], json!(true));
    assert_eq!(printed["skill"], json!({ "id": "skl_1", "name": "changelog" }));
    assert_eq!(printed["version"]["version"], json!(2));
    assert_eq!(
        printed["version"]["contentSha256"],
        json!(nexus_client::base::sha256_hex(&[WINDOWS_SKILL.as_bytes()]))
    );
    assert!(printed["version"].get("content").is_none(), "{printed}");
    // The same shape as a write's result: the files a folder left out are named, here none.
    assert_eq!(printed["notSent"], json!([]));
}

#[test]
fn names_the_other_files_of_a_folder_whose_skill_is_unchanged() {
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("changelog/references")).expect("dir");
    std::fs::write(dir.join("changelog/SKILL.md"), WINDOWS_SKILL).expect("file");
    std::fs::write(dir.join("changelog/references/guide.md"), "guide").expect("file");

    let r = run_in(
        dir.clone(),
        Setup::default(),
        &["skills", "import", "changelog", "--update"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        requests(&r),
        ["POST /v1/skills", "GET /v1/skills/skl_1/versions/latest"]
    );
    assert_eq!(
        r.out,
        format!(
            "✓ changelog is already version 2 ({} bytes). Nothing written.\n! Sent SKILL.md only. Not sent: references/guide.md.",
            WINDOWS_SKILL.len()
        )
    );

    let r = run_in(
        dir,
        Setup::default(),
        &["skills", "import", "changelog", "--update", "--json"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    let printed: Value = serde_json::from_str(&r.out).expect("JSON");
    assert_eq!(printed["unchanged"], json!(true));
    assert_eq!(printed["notSent"], json!(["references/guide.md"]));
}

#[test]
fn prints_the_code_and_the_fix_for_a_refusal() {
    let dir = new_dir();
    std::fs::write(dir.join("SKILL.md"), "broken").expect("file");
    let r = run_in(
        dir.clone(),
        Setup::default(),
        &["skills", "import", "SKILL.md", "--update"],
    );
    assert_eq!(r.code, 1);
    assert!(
        r.err
            .contains("A SKILL.md starts with a line `---`: the front matter is missing (invalid_frontmatter)"),
        "{}",
        r.err
    );
    assert!(
        r.err
            .contains("A SKILL.md starts with a line `---`, then `name:` and `description:`, then a line `---`."),
        "{}",
        r.err
    );
    // A file that is not UTF-8 is refused before anything is sent.
    std::fs::write(dir.join("latin.md"), [0x2d, 0xe9, 0x0a]).expect("file");
    let r = run_in(dir, Setup::default(), &["skills", "import", "latin.md"]);
    assert_eq!(r.code, 2);
    assert!(
        r.err.contains("Could not read latin.md: the file is not valid UTF-8"),
        "{}",
        r.err
    );
    assert!(r.seen.is_empty());
}

#[test]
fn exports_the_bytes_gateway_stored() {
    let r = run(&["skills", "export", "changelog", "--out", "skills"]);
    assert_eq!(r.code, 0, "{}", r.err);
    // The library is read too: a name a library skill has is that skill (no route here, so there is no library).
    assert_eq!(
        requests(&r),
        [
            "GET /v1/skills",
            "GET /v1/skill-library",
            "GET /v1/skills/skl_1/versions/latest/export"
        ]
    );
    assert_eq!(
        std::fs::read(r.dir.join("skills/changelog/SKILL.md")).expect("written"),
        WINDOWS_SKILL.as_bytes()
    );
    assert_eq!(
        r.out,
        format!(
            "✓ Wrote skills/changelog/SKILL.md (version 2, {} bytes).",
            WINDOWS_SKILL.len()
        )
    );

    // By id, a version, into the working directory.
    let r = run(&["skills", "export", "skl_1", "1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    // An id of one of your own skills needs no library.
    assert_eq!(
        requests(&r),
        ["GET /v1/skills", "GET /v1/skills/skl_1/versions/1/export"]
    );
    assert_eq!(
        std::fs::read_to_string(r.dir.join("changelog/SKILL.md")).expect("written"),
        OLD_SKILL
    );
    assert_eq!(
        r.out,
        format!("✓ Wrote changelog/SKILL.md (version 1, {} bytes).", OLD_SKILL.len())
    );

    let r = run(&["skills", "export", "Changelog", "latest", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let printed: Value = serde_json::from_str(r.out.trim()).expect("json");
    assert_eq!(
        printed,
        json!({ "id": "skl_1", "name": "changelog", "version": 2, "path": "changelog/SKILL.md", "bytes": WINDOWS_SKILL.len() })
    );
}

#[test]
fn says_which_skill_or_version_is_missing() {
    let r = run(&["skills", "export", "nope"]);
    assert_eq!(r.code, 1);
    assert_eq!(requests(&r), ["GET /v1/skills", "GET /v1/skill-library"]);
    assert!(
        r.err.contains("You have no skill named nope (skill_not_found)"),
        "{}",
        r.err
    );
    assert!(r.err.contains("`nexus skills` lists your skills."), "{}", r.err);
    let r = run(&["skills", "export", "changelog", "9"]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("(skill_version_not_found)"), "{}", r.err);
    assert!(!r.dir.join("changelog").exists());
}

#[test]
fn writes_no_file_for_a_name_that_is_not_a_skill_name() {
    let mut odd = changelog();
    odd["id"] = json!("skl_9");
    odd["name"] = json!("../outside");
    let setup = Setup {
        skills: vec![odd],
        ..Setup::default()
    };
    let r = run_with(setup, &["skills", "export", "skl_9"]);
    assert_eq!(r.code, 1);
    assert!(
        r.err.contains("Gateway sent \"../outside\" as the skill's name"),
        "{}",
        r.err
    );
    assert_eq!(requests(&r), ["GET /v1/skills"]);
    assert!(!r.dir.join("../outside").exists());
    // The name is text that another person may have written: a terminal does not run it.
    let mut hostile = changelog();
    hostile["id"] = json!("skl_9");
    hostile["name"] = json!("a\u{1b}[2J\u{202e}b\u{9b}");
    let setup = Setup {
        skills: vec![hostile],
        ..Setup::default()
    };
    let r = run_with(setup, &["skills", "export", "skl_9"]);
    assert_eq!(r.code, 1);
    assert!(
        r.err
            .contains("Gateway sent \"a[U+001B][2J[U+202E]b[U+009B]\" as the skill's name"),
        "{}",
        r.err
    );
    assert!(!r.err.contains(['\u{1b}', '\u{202e}', '\u{9b}']), "{}", r.err);
}

#[test]
fn deletes_by_name_or_id_with_yes() {
    let r = run(&["skills", "delete", "changelog"]);
    assert_eq!(r.code, 2);
    assert!(r.seen.is_empty());
    assert!(r.err.contains("nexus skills delete changelog --yes"), "{}", r.err);
    let r = run(&["skills", "delete", "changelog", "--yes"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(requests(&r), ["GET /v1/skills", "DELETE /v1/skills/skl_1"]);
    assert_eq!(r.out, "✓ Deleted changelog and its 2 versions.");
    let r = run(&["skills", "delete", "skl_2", "--yes"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "✓ Deleted pdf-processing and its 1 version.");
    let r = run(&["skills", "delete", "skl_2", "--yes", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let printed: Value = serde_json::from_str(r.out.trim()).expect("json");
    assert_eq!(printed, json!({ "deleted": true, "id": "skl_2", "versions": 1 }));
}

#[test]
fn creates_a_skill_in_the_editor() {
    let setup = Setup {
        env: vec![("EDITOR", "vi"), ("VISUAL", "my-editor --wait")],
        edit_saves: Some(NEW_SKILL),
        ..Setup::default()
    };
    let r = run_with(setup, &["skills", "create", "release-notes"]);
    assert_eq!(r.code, 0, "{}", r.err);
    // $VISUAL first, split on spaces; the file it opens is a valid skeleton.
    assert_eq!(r.edited.len(), 1);
    assert_eq!(r.edited[0].0, ["my-editor", "--wait"]);
    assert!(
        r.edited[0].1.starts_with("---\nname: release-notes\ndescription: "),
        "{}",
        r.edited[0].1
    );
    assert_eq!(requests(&r), ["GET /v1/skills", "POST /v1/skills"]);
    assert_eq!(r.seen[1].body, json!({ "content": NEW_SKILL }));
    assert_eq!(
        r.out,
        format!("✓ Created release-notes, version 1 ({} bytes).", NEW_SKILL.len())
    );

    let setup = Setup {
        env: vec![("EDITOR", "vi")],
        edit_saves: Some(NEW_SKILL),
        ..Setup::default()
    };
    let r = run_with(setup, &["skills", "create", "release-notes"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.edited[0].0, ["vi"]);
}

#[test]
fn creates_nothing_when_the_file_is_not_changed_or_the_editor_fails() {
    let setup = Setup {
        env: vec![("EDITOR", "vi")],
        ..Setup::default()
    };
    let r = run_with(setup, &["skills", "create", "unchanged-skill"]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("Nothing changed, so no skill was created."), "{}", r.err);
    assert_eq!(requests(&r), ["GET /v1/skills"]);

    let setup = Setup {
        env: vec![("EDITOR", "vi")],
        edit_saves: Some(NEW_SKILL),
        edit_exit: 3,
        ..Setup::default()
    };
    let r = run_with(setup, &["skills", "create", "failed-editor"]);
    assert_eq!(r.code, 1);
    assert!(
        r.err
            .contains("vi exited with code 3, so no skill was created. Your text is kept in "),
        "{}",
        r.err
    );
    assert_eq!(requests(&r), ["GET /v1/skills"]);
    let kept = kept_file(&r.err);
    assert_eq!(std::fs::read_to_string(&kept).expect("kept"), NEW_SKILL);
    let _ = std::fs::remove_dir_all(kept.parent().expect("folder"));
}

#[test]
fn keeps_the_text_when_gateway_refuses_it() {
    let setup = Setup {
        env: vec![("EDITOR", "vi")],
        edit_saves: Some("broken"),
        ..Setup::default()
    };
    let r = run_with(setup, &["skills", "create", "refused-skill"]);
    assert_eq!(r.code, 1);
    assert_eq!(requests(&r), ["GET /v1/skills", "POST /v1/skills"]);
    assert!(r.err.contains("(invalid_frontmatter)"), "{}", r.err);
    assert!(r.err.contains("Fix it, then run: nexus skills import "), "{}", r.err);
    let kept = kept_file(&r.err);
    assert_eq!(std::fs::read_to_string(&kept).expect("kept"), "broken");
    let _ = std::fs::remove_dir_all(kept.parent().expect("folder"));
}

#[test]
fn refuses_to_create_without_an_editor_or_over_a_name_you_have() {
    let r = run(&["skills", "create", "release-notes"]);
    assert_eq!(r.code, 2);
    assert!(r.err.contains("No editor: set $VISUAL or $EDITOR."), "{}", r.err);
    assert!(r.seen.is_empty());
    assert!(r.edited.is_empty());
    let taken = Setup {
        env: vec![("EDITOR", "vi")],
        ..Setup::default()
    };
    let r = run_with(taken, &["skills", "create", "Changelog"]);
    assert_eq!(r.code, 2, "Changelog is not a name: {}", r.err);
    let taken = Setup {
        env: vec![("EDITOR", "vi")],
        ..Setup::default()
    };
    let r = run_with(taken, &["skills", "create", "changelog"]);
    assert_eq!(r.code, 2);
    assert!(r.err.contains("You already have a skill named changelog."), "{}", r.err);
    assert!(
        r.err.contains("nexus skills import changelog/SKILL.md --update"),
        "{}",
        r.err
    );
    assert!(r.edited.is_empty());
}

#[test]
fn refuses_bad_usage_before_sending_anything() {
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("empty")).expect("dir");
    for argv in [
        &["skills", "import"][..],
        &["skills", "import", "missing"],
        &["skills", "import", "empty"],
        &["skills", "export"],
        &["skills", "export", "changelog", "0"],
        &["skills", "export", "changelog", "1.5"],
        &["skills", "export", "changelog", "--out"],
        &["skills", "delete"],
        &["skills", "delete", "changelog"],
        &["skills", "create"],
        &["skills", "create", "Bad_Name"],
        &["skills", "create", "--bad"],
        &["skills", "frobnicate"],
    ] {
        let r = run_in(dir.clone(), Setup::default(), argv);
        assert_eq!(r.code, 2, "{argv:?}: {}", r.err);
        assert!(r.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn prints_its_help() {
    let r = run(&["skills", "help"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("Usage  nexus skills <command>"), "{}", r.out);
    assert!(r.out.contains("import <path> [--update]"), "{}", r.out);
    assert!(r.seen.is_empty());
    // Every usage is followed by two spaces at least, and the text of every row starts in one column.
    let mut columns = std::collections::HashSet::new();
    for (name, text) in nexus_cli::catalog::SKILL_COMMANDS {
        let row = r
            .out
            .lines()
            .find(|line| line.starts_with(&format!("  {name} ")))
            .expect(name);
        let start = row.find(text).expect(text);
        assert!(row[..start].ends_with("  "), "{row}");
        columns.insert(start);
    }
    assert_eq!(columns.len(), 1, "{columns:?}");
    let r = run(&["skills", "help", "--json"]);
    let printed: Value = serde_json::from_str(r.out.trim()).expect("json");
    assert_eq!(
        printed,
        json!({ "usage": "nexus skills <command>", "commands": ["list", "create", "import", "export", "delete", "promote", "promotions", "approve", "refuse", "withdraw", "library"] })
    );
}
