//! Moving a session from its project's folder into a new worktree (`Workbench::move_to_worktree`): its uncommitted
//! changes go with it, the folder is left clean, the setup script runs there, and the agent resumes its own
//! conversation in the new folder, told once that it moved. Refused while a turn runs; a failed move changes nothing.

mod support;

use std::path::Path;

use nexus_workbench::event::{SessionAgent, SetupStatus, StoredEvent, ThreadSummary, WbEvent};
use nexus_workbench::{Limits, NewThread};
use support::{Setup, git, launches_of, new_root, setup, thread, turn_ends, until_stopped, wait_for};

fn new_session(s: &Setup, agent: SessionAgent, text: &str) -> NewThread {
    let mut new = new_root(&s.project, text);
    new.agent = agent;
    new.orchestration = false;
    new
}

async fn started(s: &Setup, new: NewThread) -> ThreadSummary {
    let created = s.workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    thread(&s.workbench, &created.id)
}

fn status(dir: &Path) -> String {
    let out = nexus_workbench::process::command("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn arg_after(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn texts(events: &[StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The project's repository with a committed file, `a.txt`.
fn with_a_file(s: &Setup) -> std::path::PathBuf {
    let repo = s.root.join("repo");
    std::fs::write(repo.join("a.txt"), "one\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "--quiet", "-m", "a"]);
    repo
}

#[tokio::test]
async fn moves_a_claude_session_with_its_changes_and_resumes_it_in_the_new_folder() {
    let s = setup("move-claude", Limits::default());
    let repo = with_a_file(&s);
    s.workbench
        .set_project_setup(&s.project, Some("echo ran > setup-ran.txt"))
        .unwrap_or_else(|e| panic!("{e}"));
    let session = started(&s, new_session(&s, SessionAgent::Claude, "Fix the login bug")).await;
    let native = launches_of(&s, &session.id)
        .first()
        .and_then(|launch| arg_after(&launch.args, "--session-id"))
        .unwrap_or_default();

    // Mid-turn: refused, and nothing moves.
    s.workbench
        .send(&session.id, "slow")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let busy = s
        .workbench
        .move_to_worktree(&session.id)
        .await
        .err()
        .unwrap_or_default();
    assert!(busy.contains("still working"), "{busy}");
    s.workbench.interrupt(&session.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &session.id, |e| turn_ends(e).len() == 2).await;

    // Staged, unstaged and untracked changes in the project's folder.
    std::fs::write(repo.join("a.txt"), "staged\n").unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["add", "a.txt"]);
    std::fs::write(repo.join("a.txt"), "staged, then edited\n").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(repo.join("notes.txt"), "mine\n").unwrap_or_else(|e| panic!("{e}"));
    let before = status(&repo);

    let moved = s
        .workbench
        .move_to_worktree(&session.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let worktree = moved.worktree.clone().unwrap_or_else(|| panic!("a worktree"));
    let dir = Path::new(&worktree.path);
    assert!(dir.starts_with(s.root.join("worktrees/repo")), "{}", worktree.path);
    assert!(
        worktree.branch.starts_with("nexus/fix-the-login-bug-"),
        "{}",
        worktree.branch
    );
    assert_eq!(worktree.base, "main");
    assert_eq!(status(dir), before);
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap_or_default(),
        "staged, then edited\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.txt")).unwrap_or_default(),
        "mine\n"
    );
    assert_eq!(status(&repo), "");
    assert!(
        !s.workbench.is_live(&session.id),
        "the agent stops, to start again in the new folder"
    );
    // The setup script ran in the new worktree, as in any new one.
    let events = wait_for(&s.workbench, &session.id, |e| {
        e.iter()
            .any(|e| matches!(&e.event, WbEvent::Setup { status, .. } if *status != SetupStatus::Running))
    })
    .await;
    assert!(dir.join("setup-ran.txt").exists());
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Notice { text } if text.starts_with("Moved to a new worktree")))
    );
    // Moved once: a session with a worktree of its own stays in it.
    let again = s
        .workbench
        .move_to_worktree(&session.id)
        .await
        .err()
        .unwrap_or_default();
    assert!(again.contains("already"), "{again}");

    // The next message resumes the agent's own conversation in the new folder, with a note that it moved.
    s.workbench
        .send(&session.id, "go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &session.id, |e| turn_ends(e).len() == 3).await;
    let launch = launches_of(&s, &session.id)
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("a launch"));
    assert_eq!(launch.cwd, dir);
    assert_eq!(arg_after(&launch.args, "--resume"), Some(native.clone()));
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 3 of this session.")
    );
    let prompts = std::fs::read_to_string(s.root.join("memory").join(format!("{native}.prompts"))).unwrap_or_default();
    let last = prompts.lines().last().unwrap_or_default();
    assert!(
        last.contains("moved this session into a new git worktree") && last.contains(&worktree.path),
        "{prompts}"
    );
    // Told once: the start after that has no note.
    s.workbench.stop(&session.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &session.id).await;
    s.workbench
        .send(&session.id, "more")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &session.id, |e| turn_ends(e).len() == 4).await;
    let prompts = std::fs::read_to_string(s.root.join("memory").join(format!("{native}.prompts"))).unwrap_or_default();
    assert!(
        !prompts.lines().last().unwrap_or_default().contains("moved"),
        "{prompts}"
    );

    // The turns before the move keep their changes: their checkpoints are the repository's, which the worktree shares.
    // A turn's checkpoint is written just after its end shows.
    let mut turns = vec![];
    for _ in 0..400 {
        turns = s.workbench.changed_turns(&session.id).await.unwrap_or_default();
        if turns.len() >= 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(turns.len() >= 4, "{turns:?}");
    let all = s
        .workbench
        .changes(&session.id, None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        all.diff.contains("notes.txt") && all.diff.contains("staged, then edited"),
        "{}",
        all.diff
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_codex_session_resumes_its_thread_in_the_new_folder() {
    let s = setup("move-codex", Limits::default());
    let repo = with_a_file(&s);
    let session = started(&s, new_session(&s, SessionAgent::Codex, "Tidy up")).await;
    std::fs::write(repo.join("b.txt"), "new\n").unwrap_or_else(|e| panic!("{e}"));
    let moved = s
        .workbench
        .move_to_worktree(&session.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let dir = moved.worktree.map(|w| w.path).unwrap_or_default();
    assert!(Path::new(&dir).join("b.txt").exists());
    assert_eq!(status(&repo), "");
    s.workbench
        .send(&session.id, "go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &session.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(
        texts(&events).last().map(String::as_str),
        Some("Message 2 of this thread.")
    );
    let launch = launches_of(&s, &session.id)
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("a launch"));
    assert_eq!(launch.cwd, Path::new(&dir));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_session_with_a_running_side_chat_waits_and_its_side_chats_follow_it() {
    let s = setup("move-side-chat", Limits::default());
    let repo = with_a_file(&s);
    let session = started(&s, new_session(&s, SessionAgent::Claude, "Fix the login bug")).await;
    let side = s
        .workbench
        .side_chat(&session.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .send(&side.id, "why that file?")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &side.id, |e| turn_ends(e).len() == 2).await;

    // Its agent works in the project's folder, which the move would clean: refused, and nothing moves.
    assert!(s.workbench.is_live(&side.id));
    std::fs::write(repo.join("b.txt"), "new\n").unwrap_or_else(|e| panic!("{e}"));
    let refused = s
        .workbench
        .move_to_worktree(&session.id)
        .await
        .err()
        .unwrap_or_default();
    assert!(refused.contains("side chats"), "{refused}");
    assert_eq!(thread(&s.workbench, &session.id).worktree, None);
    assert!(repo.join("b.txt").exists());
    // A side chat is never moved on its own: it works where its session does.
    let alone = s.workbench.move_to_worktree(&side.id).await.err().unwrap_or_default();
    assert!(alone.contains("Move the session instead"), "{alone}");

    // Its agent stopped, the session moves and the side chat follows it.
    s.workbench.stop(&side.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &side.id).await;
    let moved = s
        .workbench
        .move_to_worktree(&session.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let worktree = moved.worktree.unwrap_or_else(|| panic!("a worktree"));
    let dir = Path::new(&worktree.path);
    assert_eq!(s.workbench.folder(&side.id).ok().as_deref(), Some(dir));
    let events = s.workbench.events(&side.id, 0).unwrap_or_default();
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Notice { text } if text.starts_with("Moved to a new worktree")))
    );

    // Its next message resumes it in the new folder, told where it works now.
    s.workbench
        .send(&side.id, "and now?")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &side.id, |e| turn_ends(e).len() == 3).await;
    let launch = launches_of(&s, &side.id)
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("a launch"));
    assert_eq!(launch.cwd, dir);
    let native = arg_after(&launch.args, "--resume").unwrap_or_default();
    let prompts = std::fs::read_to_string(s.root.join("memory").join(format!("{native}.prompts"))).unwrap_or_default();
    let last = prompts.lines().last().unwrap_or_default();
    assert!(
        last.contains("moved this session into a new git worktree") && last.contains(&worktree.path),
        "{prompts}"
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_move_that_cannot_happen_leaves_the_session_and_its_folder_as_they_were() {
    let s = setup("move-refused", Limits::default());
    let repo = with_a_file(&s);
    let session = started(&s, new_session(&s, SessionAgent::Claude, "First")).await;
    std::fs::write(repo.join("a.txt"), "edited\n").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(repo.join("c.txt"), "new\n").unwrap_or_else(|e| panic!("{e}"));
    let before = status(&repo);

    // Another session works in the same folder: its files would change under it.
    let other = started(&s, new_session(&s, SessionAgent::Claude, "Second")).await;
    s.workbench
        .send(&other.id, "slow")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &other.id, |e| {
        e.iter()
            .filter(|e| matches!(e.event, WbEvent::TurnStart { .. }))
            .count()
            == 2
    })
    .await;
    let error = s
        .workbench
        .move_to_worktree(&session.id)
        .await
        .err()
        .unwrap_or_default();
    assert!(error.contains("Another session"), "{error}");
    s.workbench.interrupt(&other.id).unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &other.id, |e| turn_ends(e).len() == 2).await;

    // The worktrees folder can't be made: the move fails, and nothing changed.
    std::fs::write(s.root.join("worktrees"), "in the way").unwrap_or_else(|e| panic!("{e}"));
    assert!(s.workbench.move_to_worktree(&session.id).await.is_err());
    assert!(thread(&s.workbench, &session.id).worktree.is_none());
    assert_eq!(status(&repo), before);
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).unwrap_or_default(),
        "edited\n"
    );

    // The session still runs in the project's folder.
    s.workbench
        .send(&session.id, "still here")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &session.id, |e| turn_ends(e).len() == 2).await;
    let launch = launches_of(&s, &session.id)
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("a launch"));
    assert_eq!(launch.cwd, repo);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
