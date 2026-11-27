//! The person's shells over the host's socket (`wb_shell_*`, nexus-workbench's `shell_pty`): the hello names the
//! feature, a shell opens in the project's folder with the size asked, a command ends with its exit code, a client
//! that drops reads on from its cursor without loss, and a closed shell is gone.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use nexus_host::client::{Connection, connect};
use nexus_workbench::shell_pty::{CLOSED, FEATURE, ReadOut};
use serde_json::{Value, json};
use tokio::net::UnixStream;

mod common;

use common::{TOKEN, start};

async fn open(socket: &PathBuf) -> Connection<UnixStream> {
    let stream = UnixStream::connect(socket).await.unwrap_or_else(|e| panic!("{e}"));
    connect(stream, TOKEN).await.unwrap_or_else(|e| panic!("{e}"))
}

async fn call(connection: &mut Connection<UnixStream>, name: &str, args: Value) -> Value {
    connection
        .call(name, args)
        .await
        .0
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

async fn type_in(connection: &mut Connection<UnixStream>, id: &str, text: &str) {
    call(
        connection,
        "wb_shell_input",
        json!({ "id": id, "data": STANDARD.encode(text) }),
    )
    .await;
}

/// Reads shell `id` after `after` until `done` holds for the text read, for at most ten seconds: the text, the cursor
/// after it and the last read.
async fn read_until(
    connection: &mut Connection<UnixStream>,
    id: &str,
    mut after: u64,
    done: impl Fn(&str, &ReadOut) -> bool,
) -> (String, u64, ReadOut) {
    let started = Instant::now();
    let mut text = String::new();
    loop {
        let read = call(
            connection,
            "wb_shell_read",
            json!({ "id": id, "after": after, "waitMs": 1000 }),
        )
        .await;
        let read: ReadOut = serde_json::from_value(read).unwrap_or_else(|e| panic!("{e}"));
        text.push_str(&String::from_utf8_lossy(&read.bytes()));
        after = read.next;
        if done(&text, &read) {
            return (text, after, read);
        }
        assert!(started.elapsed() < Duration::from_secs(10), "gave up on {text:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_in_the_projects_folder_survives_its_client_dropping() {
    let host = start("shell");
    let mut first = open(&host.socket).await;
    assert!(first.info.features.iter().any(|f| f == FEATURE));
    let project = call(&mut first, "wb_project_add", json!({ "path": host.repo })).await;
    let project = project["id"].as_str().unwrap_or_default().to_string();

    let opened = call(
        &mut first,
        "wb_shell_open",
        json!({ "projectId": project, "cols": 80, "rows": 24 }),
    )
    .await;
    let id = opened["id"].as_str().unwrap_or_default().to_string();
    call(
        &mut first,
        "wb_shell_resize",
        json!({ "id": id, "cols": 132, "rows": 40 }),
    )
    .await;
    type_in(&mut first, &id, "pwd; echo \"term:$TERM size:$(stty size)\"\n").await;
    let (text, cursor, _) = read_until(&mut first, &id, 0, |text, _| text.contains("size:40 132")).await;
    let folder = std::fs::canonicalize(&host.repo).unwrap_or(host.repo.clone());
    assert!(text.contains(&*folder.to_string_lossy()), "{text:?}");
    assert!(text.contains("term:xterm-256color"), "{text:?}");

    // The client goes; what the shell writes meanwhile waits for the next one, from the same cursor.
    // Quoted apart: only the output, not the echo of the typed line, holds the marker.
    type_in(&mut first, &id, "sleep 0.3; echo \"while\"\"-away\"\n").await;
    drop(first);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let mut second = open(&host.socket).await;
    let (text, _, _) = read_until(&mut second, &id, cursor, |text, _| text.contains("while-away\r\n")).await;
    assert!(!text.contains("term:xterm"), "read again what it had: {text:?}");

    call(&mut second, "wb_shell_close", json!({ "id": id })).await;
    let gone = second.call("wb_shell_read", json!({ "id": id, "after": 0 })).await.0;
    assert!(gone.is_err_and(|e| e.starts_with(CLOSED)));
    let _ = host.stop.send(true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_runs_in_the_projects_folder_and_ends_with_its_code() {
    let host = start("shell-cmd");
    let mut connection = open(&host.socket).await;
    let project = call(&mut connection, "wb_project_add", json!({ "path": host.repo })).await;
    let opened = call(
        &mut connection,
        "wb_shell_open",
        json!({ "projectId": project["id"], "cols": 80, "rows": 24, "command": "echo ran-in:$(pwd); exit 3" }),
    )
    .await;
    let id = opened["id"].as_str().unwrap_or_default().to_string();
    let (text, _, read) = read_until(&mut connection, &id, 0, |_, read| read.exited).await;
    assert!(text.contains("ran-in:"), "{text:?}");
    assert_eq!(read.exit_code, Some(3));
    let nowhere = connection
        .call("wb_shell_open", json!({ "cols": 80, "rows": 24 }))
        .await
        .0;
    assert!(nowhere.is_err());
    let _ = host.stop.send(true);
}
