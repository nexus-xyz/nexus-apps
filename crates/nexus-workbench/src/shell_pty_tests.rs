use super::*;

fn shells(limits: Limits) -> Shells {
    Shells::new(Shell::sh(), Arc::new(String::new), limits)
}

fn open(shells: &Shells, command: Option<&str>) -> String {
    let args = OpenArgs {
        cols: 80,
        rows: 24,
        command: command.map(str::to_string),
        ..OpenArgs::default()
    };
    shells
        .open(std::env::temp_dir(), &args)
        .unwrap_or_else(|e| panic!("open: {e}"))
}

/// Reads shell `id` from `after` until `done` holds for everything read, or ten seconds pass.
async fn read_until(shells: &Shells, id: &str, done: impl Fn(&str, &ReadOut) -> bool) -> (String, ReadOut) {
    let mut text = String::new();
    let mut after = 0;
    let started = Instant::now();
    loop {
        let read = shells
            .read(id, after, Duration::from_millis(500))
            .await
            .unwrap_or_else(|e| panic!("read: {e}"));
        text.push_str(&String::from_utf8_lossy(&read.bytes()));
        after = read.next;
        if done(&text, &read) {
            return (text, read);
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "gave up; read so far: {text:?}"
        );
    }
}

#[tokio::test]
async fn what_is_typed_comes_back_in_order() {
    let shells = shells(Limits::default());
    let id = open(&shells, None);
    shells
        // Quoted apart: only the output, not the echo of the typed line, holds "two".
        .input(&id, b"echo one-$((1+1)); echo \"tw\"\"o\"\n")
        .unwrap_or_else(|e| panic!("{e}"));
    let (text, _) = read_until(&shells, &id, |text, _| text.contains("two\r\n")).await;
    let one = text.find("one-2").unwrap_or_else(|| panic!("{text:?}"));
    assert!(one < text.rfind("two").unwrap_or_default());
    let _ = shells.close(&id);
}

#[tokio::test]
async fn the_shell_has_a_terminal_of_the_size_asked_and_resized() {
    let shells = shells(Limits::default());
    let id = open(&shells, None);
    shells.resize(&id, 101, 33).unwrap_or_else(|e| panic!("{e}"));
    shells
        .input(&id, b"echo \"size:$(stty size) term:$TERM\"\n")
        .unwrap_or_else(|e| panic!("{e}"));
    let (text, _) = read_until(&shells, &id, |text, _| text.contains("term:xterm-256color")).await;
    assert!(text.contains("size:33 101"), "{text:?}");
    let _ = shells.close(&id);
}

#[tokio::test]
async fn a_command_ends_with_its_exit_code_and_all_its_output() {
    let shells = shells(Limits::default());
    let id = open(&shells, Some("printf 'last words'; exit 7"));
    let (text, read) = read_until(&shells, &id, |_, read| read.exited).await;
    assert!(text.contains("last words"), "{text:?}");
    assert_eq!(read.exit_code, Some(7));
    // Read again from the start: the same bytes, still ended.
    let again = shells
        .read(&id, 0, Duration::ZERO)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(String::from_utf8_lossy(&again.bytes()).contains("last words"));
    assert!(again.exited);
}

#[tokio::test]
async fn only_the_newest_output_is_kept_and_a_late_read_says_so() {
    let shells = shells(Limits {
        buffer: 64,
        ..Limits::default()
    });
    let id = open(&shells, Some("head -c 5000 /dev/zero | tr '\\0' x; printf END"));
    let _ = read_until(&shells, &id, |_, read| read.exited).await;
    let late = shells
        .read(&id, 0, Duration::ZERO)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(late.truncated);
    assert!(late.bytes().len() <= 64);
    assert!(String::from_utf8_lossy(&late.bytes()).ends_with("END"));
    assert!(late.next >= 5003);
}

/// Whether `pid` runs: a zombie waits only for its reaper (which may be a subreaper that is slow to reap).
fn alive(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
        Err(_) => nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok(),
    }
}

#[tokio::test]
async fn closing_ends_the_shell_and_what_it_started() {
    let shells = shells(Limits::default());
    let id = open(&shells, None);
    shells
        // Quoted apart, so the echo of the typed line does not hold the marker.
        .input(&id, b"sleep 300 & echo \"pi\"\"ds:$$:$!:\"\n")
        .unwrap_or_else(|e| panic!("{e}"));
    let (text, _) = read_until(&shells, &id, |text, _| {
        text.split("pids:")
            .nth(1)
            .is_some_and(|rest| rest.matches(':').count() >= 2)
    })
    .await;
    let mut pids = text
        .split("pids:")
        .nth(1)
        .unwrap_or_default()
        .split(':')
        .filter_map(|pid| pid.trim().parse::<i32>().ok());
    let (shell, sleep) = (pids.next().unwrap_or(0), pids.next().unwrap_or(0));
    assert!(shell > 1 && sleep > 1, "{text:?}");
    assert!(alive(shell) && alive(sleep), "{text:?}");
    shells.close(&id).unwrap_or_else(|e| panic!("{e}"));
    let started = Instant::now();
    while (alive(shell) || alive(sleep)) && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    // A zombie still answers a signal until it is reaped: the shell's reaper reaps it, the sleep's is init.
    assert!(!alive(sleep), "the background job outlived its shell");
    assert!(shells.read(&id, 0, Duration::ZERO).await.is_err());
    assert!(shells.is_empty());
}

#[tokio::test]
async fn an_unused_shell_is_reaped_and_a_full_host_refuses_more() {
    let shells = shells(Limits {
        shells: 1,
        orphan: Duration::from_secs(3600),
        ..Limits::default()
    });
    let id = open(&shells, None);
    let refused = shells.open(
        std::env::temp_dir(),
        &OpenArgs {
            cols: 80,
            rows: 24,
            ..OpenArgs::default()
        },
    );
    assert!(refused.is_err_and(|e| e.contains("1 shells open")));
    shells.reap();
    assert_eq!(shells.len(), 1);
    let orphans = Shells {
        limits: Limits {
            orphan: Duration::ZERO,
            ..shells.limits
        },
        ..shells.clone()
    };
    orphans.reap();
    assert!(shells.is_empty());
    assert!(shells.input(&id, b"x").is_err_and(|e| e.starts_with(CLOSED)));
}

#[tokio::test]
async fn the_calls_answer_in_their_wire_shape() {
    let shells = shells(Limits::default());
    let dir = std::env::temp_dir();
    let opened = shells
        .answer(
            OPEN,
            json!({ "projectId": "p1", "cols": 90, "rows": 20, "command": "pwd" }),
            |thread, project| {
                assert_eq!((thread, project), (None, Some("p1")));
                Ok(dir.clone())
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let id = opened["id"].as_str().unwrap_or_default().to_string();
    assert!(id.starts_with("sh_"));
    let mut text = String::new();
    let mut after = 0;
    loop {
        let read = shells
            .answer(
                READ,
                json!({ "id": id, "after": after, "waitMs": 2000 }),
                |_, _| unreachable!(),
            )
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let read: ReadOut = serde_json::from_value(read).unwrap_or_else(|e| panic!("{e}"));
        text.push_str(&String::from_utf8_lossy(&read.bytes()));
        after = read.next;
        if read.exited {
            assert_eq!(read.exit_code, Some(0));
            break;
        }
    }
    let canonical = std::fs::canonicalize(&dir).unwrap_or(dir);
    assert!(text.contains(&*canonical.to_string_lossy()), "{text:?}");
    let input = shells
        .answer(INPUT, json!({ "id": "sh_gone", "data": "eA==" }), |_, _| unreachable!())
        .await;
    assert!(input.is_err_and(|e| e.starts_with(CLOSED)));
    let bad = shells
        .answer(INPUT, json!({ "id": id, "data": "not base64!" }), |_, _| unreachable!())
        .await;
    assert!(bad.is_err());
    assert!(
        shells
            .answer(CLOSE, json!({ "id": id }), |_, _| unreachable!())
            .await
            .is_ok()
    );
}
