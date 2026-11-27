//! A project's own Claude settings through the workbench (claude_trust.rs): a session in a project nobody trusted
//! starts Claude Code with `--setting-sources user`, says why once and passes the project's CLAUDE.md on itself; after
//! Trust, the next start has no limit and leaves the CLAUDE.md to Claude Code. The stand-in `claude` records what it
//! was started with (`FAKE_LAUNCH_RECORD`).

mod support;

use std::time::{Duration, Instant};

use nexus_workbench::event::{StoredEvent, WbEvent};
use serde_json::Value;
use support::*;

/// The arguments of the last launch recorded, once `ready` holds for them.
async fn launched(s: &Setup, ready: impl Fn(&[String]) -> bool) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let args: Vec<String> = std::fs::read_to_string(s.root.join("launch.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|value| {
                value["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|arg| arg.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if ready(&args) {
            return args;
        }
        assert!(Instant::now() < deadline, "no launch recorded: {args:?}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn limited(args: &[String]) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == "--setting-sources" && pair[1] == "user")
}

/// The text of the instructions file a launch passes Claude Code.
fn instructions(args: &[String]) -> String {
    let at = args
        .iter()
        .position(|arg| arg == "--append-system-prompt-file")
        .unwrap_or_else(|| panic!("no instructions file: {args:?}"));
    std::fs::read_to_string(&args[at + 1]).unwrap_or_else(|e| panic!("{e}"))
}

fn notices(events: &[StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|stored| match &stored.event {
            WbEvent::Notice { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn project_settings_load_only_after_the_person_trusts_the_project() {
    let s = setup("claude-trust", bounded());
    let repo = s.root.join("repo");
    std::fs::create_dir_all(repo.join(".claude")).expect("a folder");
    std::fs::write(
        repo.join(".claude/settings.json"),
        r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "./run.sh"}]}]}}"#,
    )
    .expect("written");
    std::fs::write(repo.join("CLAUDE.md"), "The project codeword is PELICAN.").expect("written");
    s.host.extra.lock().unwrap_or_else(|p| p.into_inner()).insert(
        "FAKE_LAUNCH_RECORD".into(),
        s.root.join("launch.json").to_string_lossy().into_owned(),
    );

    // Never asked: the question waits, and the session loads only the person's own settings.
    let asked = s.workbench.claude_trust(&s.project).expect("read");
    assert!(asked.asks());
    assert_eq!(asked.found.summary(), "1 hook");
    let first = root(&s, new_root(&s.project, "one")).await;
    let args = launched(&s, |args| !args.is_empty()).await;
    assert!(limited(&args));
    // Claude Code skips the project's CLAUDE.md with that flag, so the launch passes it on itself.
    assert!(instructions(&args).contains("The project codeword is PELICAN."));
    let events = wait_for(&s.workbench, &first.id, |e| turn_ends(e).len() == 1).await;
    let said = notices(&events);
    assert_eq!(
        said.iter().filter(|text| text.contains("(1 hook)")).count(),
        1,
        "{said:?}"
    );

    // Trusted: the next start loads everything, as before.
    std::fs::remove_file(s.root.join("launch.json")).ok();
    let trusted = s.workbench.set_claude_trust(&s.project, true).expect("saved");
    assert_eq!(trusted.trusted, Some(true));
    assert!(!trusted.asks());
    s.workbench.stop(&first.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &first.id).await;
    s.workbench
        .send(&first.id, "again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let args = launched(&s, |args| !args.is_empty()).await;
    assert!(!args.iter().any(|arg| arg == "--setting-sources"), "{args:?}");
    // Claude Code loads the CLAUDE.md itself now: not twice.
    assert!(!instructions(&args).contains("PELICAN"));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
