use std::collections::HashMap;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Instant;

use super::*;
use crate::Capture;

fn context() -> Context {
    let dir = std::env::temp_dir().join(format!("nexus-wait-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&dir).expect("temporary directory");
    Context {
        config_path: dir.join("config.json"),
        io: Box::new(Capture::default()),
        color: false,
        env: HashMap::from([(
            "NEXUS_DESKTOP_SOCKET".into(),
            dir.join("sessions.sock").display().to_string(),
        )]),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir,
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    }
}

#[test]
fn final_read_uses_its_own_socket_budget() {
    let ctx = context();
    save_token(&ctx, CLIENT, Some("saved")).expect("save token");
    let listener = UnixListener::bind(socket(&ctx).expect("socket path")).expect("bind");
    let app = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).expect("read");
        let request: Value = serde_json::from_str(&line).expect("request JSON");
        assert_eq!(request["token"], "saved");
        assert_eq!(request["message"]["params"]["name"], "show_session");
        assert_eq!(request["message"]["params"]["arguments"]["session_id"], "t_1");
        let response = json!({"result": {"structuredContent": {"id": "t_1"}}});
        writeln!(stream, "{response}").expect("respond");
    });

    let mut desktop = Desktop::new(&ctx, CLIENT)
        .expect("desktop")
        .with_wait_limit(Instant::now() - Duration::from_secs(2), Duration::from_secs(1));
    desktop.start_final_read();
    assert_eq!(
        desktop.call("show_session", json!({"session_id": "t_1"})),
        Ok(json!({"id": "t_1"}))
    );
    app.join().expect("app");
    std::fs::remove_dir_all(ctx.cwd).expect("clean up");
}

#[test]
fn a_stale_socket_ends_the_wait() {
    let ctx = context();
    let path = socket(&ctx).expect("socket path");
    drop(UnixListener::bind(&path).expect("bind"));
    assert_eq!(
        UnixStream::connect(path).expect_err("listener closed").kind(),
        ErrorKind::ConnectionRefused
    );
    let desktop = Desktop::new(&ctx, CLIENT)
        .expect("desktop")
        .with_wait_limit(Instant::now(), Duration::from_secs(1));
    let Err(Error::Cli(error)) = desktop.call("show_session", json!({"session_id": "t_1"})) else {
        panic!("expected a CLI error");
    };
    assert_eq!(error.code, Some(DESKTOP_UNAVAILABLE));
    assert!(ends_wait(&error));
    std::fs::remove_dir_all(ctx.cwd).expect("clean up");
}
