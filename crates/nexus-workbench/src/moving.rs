//! Moving a session out of its project's folder into a new worktree of its own (`wb_move_to_worktree`), for a session
//! that started in the folder and now needs it free for another. What the folder had not committed goes with it
//! ([`git::move_to_worktree`]): staged, unstaged and untracked files. The folder is left clean, the project's setup
//! script runs in the new worktree as it does for any new one, and the agent goes on there: its next start resumes
//! its own conversation (`--resume`, `thread/resume`) in the new folder, told first that the folder changed.
//!
//! Refused while a turn runs: the person stops it or waits, then moves. Refused too while an agent of one of its side
//! chats runs, since it works in the same folder. Its other side chats follow it into the new worktree (`worktree_of`),
//! and their agents are told at their next start, as the session's is. The turns before the move keep their
//! checkpoints: they are refs of the repository, which the worktree shares, so their changes still show and a revert
//! to one of them puts the new worktree's files back as they were then.

use std::path::{Path, PathBuf};

use crate::Result;
use crate::event::{StoredEvent, ThreadStatus, ThreadSummary, TurnStatus, WbEvent, Worktree};
use crate::{Reverting, StopReason, Workbench, branch_for, checkpoint_prefix, git};

/// What a core that moves sessions into worktrees (`wb_move_to_worktree`) says in its features.
pub const FEATURE: &str = "move-to-worktree";

/// Why a session whose side chat's agent runs is not moved: the agent works in the same folder.
const SIDE_CHATS_LIVE: &str =
    "Stop the agents of this session's side chats first: they work in its folder, which the move cleans.";

/// How the log's notice of a move starts: [`unsaid`] finds it by these words.
pub const MOVED: &str = "Moved to a new worktree";

/// Whether the session moved since its agent last ran a turn: its next start tells it ([`note`]). A turn that failed
/// before the agent said anything may not have read the note, so it waits for the next.
pub(crate) fn unsaid(events: &[StoredEvent]) -> bool {
    let mut moved = false;
    for stored in events {
        match &stored.event {
            WbEvent::Notice { text } if text.starts_with(MOVED) => moved = true,
            WbEvent::TurnEnd { cursor, status, .. } if cursor.is_some() || *status != TurnStatus::Error => {
                moved = false;
            }
            _ => {}
        }
    }
    moved
}

/// What the agent reads ahead of its first message after a move.
pub(crate) fn note(from: &str, worktree: &Worktree) -> String {
    format!(
        "(Note from the Nexus app: the person moved this session into a new git worktree. You now work in {}, on \
         branch {}, not in {from}. The uncommitted changes came with you, and {from} is clean. Work only in the new \
         folder.)",
        worktree.path, worktree.branch
    )
}

impl Workbench {
    /// Moves a session that works in its project's folder into a new worktree, with what the folder had not
    /// committed, and leaves the folder clean. The session's agent, between turns, is stopped; the next message
    /// resumes it in the new folder. Refused while it, its sub-agents or its commands work, while an agent of one of its
    /// side chats runs, and while another session's agent works in the same folder (its files would change under it).
    pub async fn move_to_worktree(&self, thread_id: &str) -> Result<ThreadSummary> {
        self.checkpoint_written(thread_id).await;
        let busy = |this: &Workbench| -> Result<bool> {
            let live_busy = this.live(thread_id).is_some_and(|live| {
                let state = live.state();
                !state.turns.is_empty() || !state.background.is_empty()
            });
            Ok(live_busy || this.record(thread_id)?.summary.status.busy())
        };
        if busy(self)? {
            return Err(
                "The agent, its sub-agents or its commands are still working. Stop them or wait for them to finish, then move the session."
                    .into(),
            );
        }
        let record = self.record(thread_id)?;
        if record.summary.side_chat {
            return Err("A side chat works in its session's folder. Move the session instead.".into());
        }
        if record.summary.worktree.is_some() {
            return Err("This session already works in a worktree of its own.".into());
        }
        if self.side_chat_live(thread_id) {
            return Err(SIDE_CHATS_LIVE.into());
        }
        let root = self
            .config()
            .worktrees
            .ok_or_else(|| "Worktrees aren't available here.".to_string())?;
        let project = self
            .store()
            .project(&record.summary.project_id)?
            .ok_or_else(|| "This session's project is no longer open.".to_string())?;
        let repo = PathBuf::from(&project.path);
        if !git::is_repo(&repo) {
            return Err("Only a project at the top of a git repository can move a session into a worktree.".into());
        }
        let threads = self.threads()?;
        // A side chat works where its session does.
        let in_folder = |thread: &ThreadSummary| {
            let owner = match (&thread.source_thread_id, thread.side_chat) {
                (Some(source), true) => threads.iter().find(|other| &other.id == source),
                _ => Some(thread),
            };
            owner.is_some_and(|owner| owner.worktree.is_none())
        };
        let others = threads.iter().any(|thread| {
            thread.id != thread_id
                && thread.project_id == record.summary.project_id
                && in_folder(thread)
                && self.is_live(&thread.id)
                && thread.status != ThreadStatus::Idle
        });
        if others {
            return Err("Another session's agent is working in the project's folder. Wait for its turn to end.".into());
        }
        // Until the move is logged, a message to the session is refused: it would start the agent in the old folder.
        let _moving = Reverting::hold(self, thread_id)?;
        if let Some(live) = self.live(thread_id) {
            self.stop_live(&live, StopReason::Stop, None);
        }
        self.settle(thread_id).await?;
        // The last turn's checkpoint, if still being written, must snapshot the folder before the move cleans it.
        self.checkpoint_written(thread_id).await;
        if busy(self)? {
            return Err("The agent started working again. Wait for the turn to end, then move the session.".into());
        }
        if self.side_chat_live(thread_id) {
            return Err(SIDE_CHATS_LIVE.into());
        }
        let named = branch_for(&self.config().branch_prefix, &record.summary.title, thread_id);
        let (path, branch, moved) = tokio::task::spawn_blocking({
            let repo = repo.clone();
            move || {
                let (path, branch) = free_place(&root, &repo, &named);
                git::move_to_worktree(&repo, &path, &branch).map(|moved| (path, branch, moved))
            }
        })
        .await
        .map_err(|error| error.to_string())??;
        let worktree = Worktree {
            path: path.to_string_lossy().into_owned(),
            branch,
            base: moved.base,
            removed: false,
            pull_request: None,
            pull_requests: Vec::new(),
            review: None,
        };
        let summary = self.change(thread_id, |record| {
            // The pull requests its agent opened in the folder are still the session's.
            let mut moved_to = worktree.clone();
            for url in std::mem::take(&mut record.summary.pull_requests) {
                moved_to.add_pull(&url);
            }
            record.summary.worktree = Some(moved_to);
        })?;
        let mut text = format!(
            "{MOVED} at {}, on branch {}. The uncommitted changes came with it, and the agent goes on there at the next \
             message.",
            worktree.path, worktree.branch
        );
        match &moved.left {
            Some(error) => text.push_str(&format!(
                " The project's folder could not be cleaned, so it still holds some of them: {error}"
            )),
            None => text.push_str(" The project's folder is clean."),
        }
        let notice = self.append(thread_id, WbEvent::Notice { text })?;
        // Its side chats work in its folder, so they follow it; their agents are told at their next start.
        for side in self.reader().side_chats(thread_id)? {
            let text = format!(
                "{MOVED} with its session, at {}, on branch {}. The side chat goes on there at the next message.",
                worktree.path, worktree.branch
            );
            self.append(&side.summary.id, WbEvent::Notice { text })?;
        }
        // The folder as the move left it: the next turn's changes count from here. A copy of what moved, too, which no
        // folder's removal takes away. Named apart from turn ends (a letter after the number), as a revert's is.
        let refname = format!("{}{:010}m", checkpoint_prefix(thread_id), notice.seq);
        let snapshot = moved.snapshot;
        let dir = path.clone();
        let _ = tokio::task::spawn_blocking(move || git::set_ref(&dir, &refname, &snapshot)).await;
        // The project's setup script, as in any new worktree; the next message waits for it.
        self.begin_setup(&summary, &project.path)?;
        self.record(thread_id).map(|record| record.summary)
    }
}

/// Where a new worktree for `branch` goes, and its branch: `<root>/<repository>/<branch>` with `/` as `-`, both with
/// `-2`, `-3`… when the folder or the branch is taken.
fn free_place(root: &Path, repo: &Path, branch: &str) -> (PathBuf, String) {
    let parent = root.join(repo.file_name().unwrap_or_default());
    let name = branch.replace('/', "-");
    (1..)
        .map(|n| {
            let suffix = if n == 1 { String::new() } else { format!("-{n}") };
            (parent.join(format!("{name}{suffix}")), format!("{branch}{suffix}"))
        })
        .find(|(path, branch)| !path.exists() && !git::has_branch(repo, branch))
        .unwrap_or_else(|| (parent.join(&name), branch.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seq: i64, event: WbEvent) -> StoredEvent {
        StoredEvent {
            thread_id: "t".into(),
            seq,
            at: 0,
            event,
        }
    }

    fn end(seq: i64, status: TurnStatus, cursor: Option<&str>) -> StoredEvent {
        at(
            seq,
            WbEvent::TurnEnd {
                turn_id: format!("t{seq}"),
                status,
                cost_usd: None,
                duration_ms: 0,
                error: None,
                cursor: cursor.map(String::from),
            },
        )
    }

    fn moved(seq: i64) -> StoredEvent {
        at(
            seq,
            WbEvent::Notice {
                text: format!("{MOVED} at /w, on branch b."),
            },
        )
    }

    #[test]
    fn the_agent_is_told_of_a_move_until_a_turn_runs_after_it() {
        assert!(!unsaid(&[]));
        assert!(!unsaid(&[end(1, TurnStatus::Completed, Some("m1"))]));
        assert!(unsaid(&[end(1, TurnStatus::Completed, Some("m1")), moved(2)]));
        // A turn that failed before the agent said anything: it may not have read the note.
        assert!(unsaid(&[moved(2), end(3, TurnStatus::Error, None)]));
        assert!(!unsaid(&[moved(2), end(3, TurnStatus::Completed, None)]));
        assert!(!unsaid(&[moved(2), end(3, TurnStatus::Error, Some("m3"))]));
        // Another notice is not a move.
        assert!(!unsaid(&[at(
            2,
            WbEvent::Notice {
                text: "Agent restarted.".into()
            }
        )]));
    }

    #[test]
    fn the_note_names_both_folders_and_the_branch() {
        let worktree = Worktree {
            path: "/w/app/nexus-fix".into(),
            branch: "nexus/fix".into(),
            base: "main".into(),
            removed: false,
            pull_request: None,
            pull_requests: Vec::new(),
            review: None,
        };
        let text = note("/src/app", &worktree);
        assert!(
            text.contains("/w/app/nexus-fix") && text.contains("nexus/fix") && text.contains("/src/app"),
            "{text}"
        );
    }

    #[test]
    fn a_taken_place_gets_a_number() {
        let root = std::env::temp_dir().join(format!("nexus-move-place-{}", uuid::Uuid::new_v4().simple()));
        let repo = Path::new("/src/app");
        assert_eq!(
            free_place(&root, repo, "nexus/fix"),
            (root.join("app/nexus-fix"), "nexus/fix".to_string())
        );
        std::fs::create_dir_all(root.join("app/nexus-fix")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            free_place(&root, repo, "nexus/fix"),
            (root.join("app/nexus-fix-2"), "nexus/fix-2".to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Whether the menu offers to move the session into a worktree of its own.
pub fn menu_offered(supported: bool, has_worktree: bool, linked: bool, side_chat: bool, repository: bool) -> bool {
    supported && !has_worktree && !linked && !side_chat && repository
}

pub fn menu_blocked(status: ThreadStatus) -> Option<&'static str> {
    matches!(
        status,
        ThreadStatus::Starting | ThreadStatus::Running | ThreadStatus::Approval
    )
    .then_some("Wait for the turn to end, or stop it, then move the session")
}

#[cfg(test)]
mod menu_tests {
    #[test]
    fn only_supported_folder_sessions_offer_the_move() {
        assert!(super::menu_offered(true, false, false, false, true));
        for args in [
            (false, false, false, false, true),
            (true, true, false, false, true),
            (true, false, true, false, true),
            (true, false, false, true, true),
            (true, false, false, false, false),
        ] {
            assert!(!super::menu_offered(args.0, args.1, args.2, args.3, args.4));
        }
    }

    #[test]
    fn active_turns_are_blocked_but_idle_and_stopped_sessions_can_move() {
        use crate::event::ThreadStatus::*;
        for status in [Starting, Running, Approval] {
            assert!(super::menu_blocked(status).is_some());
        }
        for status in [Idle, Interrupted, Error] {
            assert!(super::menu_blocked(status).is_none());
        }
    }
}
