//! The person's shells on a real server, run by hand (ignored in CI): a host there, its socket forwarded here with
//! `ssh -N -L <local.sock>:<remote host.sock> <server>`. It prints what each step saw.
//!
//! ```sh
//! NEXUS_E2E_SOCKET=/tmp/e2e.sock NEXUS_E2E_TOKEN=... NEXUS_E2E_REPO=/tmp/e2e-repo NEXUS_E2E_SSH=myserver \
//!   cargo test -p nexus-host --test shell_e2e -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use nexus_host::client::{Connection, connect};
use nexus_workbench::shell_pty::{CLOSED, FEATURE, ReadOut};
use serde_json::{Value, json};
use tokio::net::UnixStream;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
}

async fn open() -> Connection<UnixStream> {
    let stream = UnixStream::connect(env("NEXUS_E2E_SOCKET"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    connect(stream, &env("NEXUS_E2E_TOKEN"))
        .await
        .unwrap_or_else(|e| panic!("{e}"))
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
            json!({ "id": id, "after": after, "waitMs": 2000 }),
        )
        .await;
        let read: ReadOut = serde_json::from_value(read).unwrap_or_else(|e| panic!("{e}"));
        text.push_str(&String::from_utf8_lossy(&read.bytes()));
        after = read.next;
        if done(&text, &read) {
            return (text, after, read);
        }
        assert!(started.elapsed() < Duration::from_secs(30), "gave up on {text:?}");
    }
}

/// `ps` on the server, for the processes in session `sid`.
fn ps_session(sid: &str) -> String {
    let out = nexus_workbench::process::command("ssh")
        .args([&env("NEXUS_E2E_SSH"), "ps", "-o", "pid,pgid,sid,stat,cmd", "-s", sid])
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The text with its escape sequences shown, one line a line.
fn shown(text: &str) -> String {
    format!("{text:?}").replace("\\r\\n", "\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a real host: see the file's header"]
async fn a_shell_on_a_real_server() {
    let mut connection = open().await;
    println!(
        "== hello: version {} features {:?}",
        connection.info.version, connection.info.features
    );
    assert!(connection.info.features.iter().any(|f| f == FEATURE));

    let project = call(
        &mut connection,
        "wb_project_add",
        json!({ "path": env("NEXUS_E2E_REPO") }),
    )
    .await;
    println!("== project: {} at {}", project["id"], project["path"]);

    // NEXUS_E2E_SHELL: run this as the shell instead of the login shell, when the server's own startup files would
    // change what the test reads (a zsh that starts tmux, say).
    let opened = call(
        &mut connection,
        "wb_shell_open",
        json!({
            "projectId": project["id"], "cols": 80, "rows": 24, "command": std::env::var("NEXUS_E2E_SHELL").ok()
        }),
    )
    .await;
    let id = opened["id"].as_str().unwrap_or_default().to_string();
    println!("== wb_shell_open -> {opened}");
    call(
        &mut connection,
        "wb_shell_resize",
        json!({ "id": id, "cols": 100, "rows": 30 }),
    )
    .await;
    println!("== wb_shell_resize 100x30");
    type_in(
        &mut connection,
        &id,
        "pwd; echo $TERM; tput cols; echo \"pi\"\"d:$$\"\n",
    )
    .await;
    let (text, cursor, _) = read_until(&mut connection, &id, 0, |text, _| text.contains("pid:")).await;
    println!("== typed `pwd; echo $TERM; tput cols`, read:\n{}", shown(&text));
    assert!(text.contains(&env("NEXUS_E2E_REPO")));
    assert!(text.contains("xterm-256color"));
    assert!(text.contains("\r\n100\r\n"));
    let pid: String = text
        .rsplit("pid:")
        .next()
        .unwrap_or_default()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    println!("== shell pid {pid}; its session on the server:\n{}", ps_session(&pid));

    type_in(&mut connection, &id, "sleep 2; echo \"after\"\"-drop\"\n").await;
    drop(connection);
    println!("== dropped the connection at cursor {cursor}; waiting 4 s");
    tokio::time::sleep(Duration::from_secs(4)).await;
    let mut connection = open().await;
    let (text, _, _) = read_until(&mut connection, &id, cursor, |text, _| text.contains("after-drop\r\n")).await;
    println!("== reconnected, read from cursor {cursor}:\n{}", shown(&text));
    assert!(!text.contains("xterm-256color"), "read again what it had");

    let command = call(
        &mut connection,
        "wb_shell_open",
        json!({ "projectId": project["id"], "cols": 80, "rows": 24, "command": "echo from-command in $(pwd); exit 5" }),
    )
    .await;
    let command_id = command["id"].as_str().unwrap_or_default().to_string();
    let (text, _, read) = read_until(&mut connection, &command_id, 0, |_, read| read.exited).await;
    println!(
        "== command `echo from-command in $(pwd); exit 5`: exited {} code {:?}, read:\n{}",
        read.exited,
        read.exit_code,
        shown(&text)
    );
    assert_eq!(read.exit_code, Some(5));

    type_in(&mut connection, &id, "sleep 600 &\n").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!(
        "== a background job started; the session before close:\n{}",
        ps_session(&pid)
    );
    call(&mut connection, "wb_shell_close", json!({ "id": id })).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let after = ps_session(&pid);
    println!("== after wb_shell_close, the session:\n{after}");
    assert!(after.lines().count() <= 1, "left running: {after}");
    let gone = connection
        .call("wb_shell_read", json!({ "id": id, "after": 0 }))
        .await
        .0;
    println!("== wb_shell_read after close -> {gone:?}");
    assert!(gone.is_err_and(|e| e.starts_with(CLOSED)));
}
