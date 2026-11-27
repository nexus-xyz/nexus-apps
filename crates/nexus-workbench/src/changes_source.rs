//! What the Changes view shows: the whole session or one turn, from its checkpoints
//! ([`session_diff`], [`changed_turns`]), or the branch, one commit, or what is not committed yet
//! ([`crate::diff_source`]). The picker's rules are [`ChangesSource`]. The reads take a folder, not a store, so Nexus GPUI reads them
//! from git while the Nexus app runs the session too.

use std::collections::HashMap;
use std::path::Path;

use crate::diff_source::DiffSource;
use crate::{MAX_DIFF, Result, git};

/// A session's checkpoint refs: `<this><end event number, 10 digits>`, 0 for the folder before the first turn.
pub fn checkpoint_prefix(thread_id: &str) -> String {
    format!("refs/nexus/checkpoints/{thread_id}/")
}

/// A checkpoint ref's event number.
pub(crate) fn seq_of(refname: &str) -> Option<i64> {
    refname.rsplit('/').next()?.parse().ok()
}

/// The turns of session `thread_id` whose changes can be shown, in `dir`: their end events' numbers, oldest first.
pub fn changed_turns(dir: &Path, thread_id: &str) -> Vec<i64> {
    git::refs(dir, &checkpoint_prefix(thread_id))
        .unwrap_or_default()
        .iter()
        .filter_map(|name| seq_of(name))
        .filter(|seq| *seq > 0)
        .collect()
}

/// What session `thread_id` changed in `dir`: in the turn that ended at event `turn`, or (None) everything, against
/// `base` (its worktree's base) or else the folder before the first turn. The diff, and whether it was cut.
pub fn session_diff(dir: &Path, thread_id: &str, base: Option<&str>, turn: Option<i64>) -> Result<(String, bool)> {
    let names = git::refs(dir, &checkpoint_prefix(thread_id))?;
    let (from, to) = match turn {
        Some(turn) => turn_refs(&names, turn)?,
        None => session_refs(dir, &names, base)?,
    };
    git::diff(dir, &from, &to, MAX_DIFF)
}

pub(crate) fn session_refs(dir: &Path, names: &[String], base: Option<&str>) -> Result<(String, String)> {
    let last = names
        .last()
        .cloned()
        .ok_or_else(|| "No changes recorded yet.".to_string())?;
    let from = match base {
        Some(base) => git::merge_base(dir, base, &last)?,
        None => names[0].clone(),
    };
    Ok((from, last))
}

/// The checkpoints a turn's changes lie between, among a session's checkpoint refs `names` (sorted): the one before
/// the turn that ended at event `turn`, and that turn's own.
pub(crate) fn turn_refs(names: &[String], turn: i64) -> Result<(String, String)> {
    let unrecorded = || "That turn's changes weren't recorded.".to_string();
    let at = names
        .iter()
        .position(|name| seq_of(name) == Some(turn))
        .ok_or_else(unrecorded)?;
    let from = at
        .checked_sub(1)
        .map(|before| names[before].clone())
        .ok_or_else(unrecorded)?;
    Ok((from, names[at].clone()))
}

/// The agents' tools that write a file: each one that finishes makes
/// the views that follow edits stale.
pub const FILE_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// What the Changes view compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesSource {
    /// Everything the session changed.
    Session,
    /// The turn that ended at this event.
    Turn(i64),
    /// The folder against where its branch left its base branch.
    Branch,
    /// What one commit changed.
    Commit(String),
    /// The folder against HEAD.
    Uncommitted,
}

impl ChangesSource {
    /// The read for a source that is not the session's own: `wb_diff`'s.
    pub fn diff_source(&self) -> Option<DiffSource> {
        match self {
            ChangesSource::Session | ChangesSource::Turn(_) => None,
            ChangesSource::Branch => Some(DiffSource::Branch),
            ChangesSource::Commit(id) => Some(DiffSource::Commit { id: id.clone() }),
            ChangesSource::Uncommitted => Some(DiffSource::Uncommitted),
        }
    }

    /// Whether the view reads the folder as it is now, so an edit by the agent makes it stale. A turn's diff and a
    /// commit's are fixed.
    pub fn follows_edits(&self) -> bool {
        matches!(
            self,
            ChangesSource::Session | ChangesSource::Branch | ChangesSource::Uncommitted
        )
    }

    /// What the view says when the diff is empty, naming the base branch when it is known.
    pub fn empty_note(&self, base: Option<&str>) -> String {
        match self {
            ChangesSource::Session => "No changes yet.".into(),
            ChangesSource::Turn(_) => "No changes in this turn.".into(),
            ChangesSource::Branch => match base {
                Some(base) => format!("No changes against {base}."),
                None => "No changes against the base branch.".into(),
            },
            ChangesSource::Uncommitted => "Nothing uncommitted.".into(),
            ChangesSource::Commit(_) => "This commit changed no files.".into(),
        }
    }

    /// What a line comment adds after its lines, so the agent knows which version they are from: `of commit a1b2c3d`
    /// for a commit. The other sources show the folder as it is now (their new-side lines are the files' own).
    pub fn comment_place(&self) -> Option<String> {
        match self {
            ChangesSource::Commit(id) => Some(format!("of commit {}", short(id))),
            _ => None,
        }
    }
}

fn short(id: &str) -> &str {
    id.get(..7).unwrap_or(id)
}

/// A commit as the picker lists it: `a1b2c3d Fix the retry`.
pub fn commit_label(short: &str, subject: &str) -> String {
    if subject.is_empty() {
        short.to_string()
    } else {
        format!("{short} {subject}")
    }
}

/// The commit picked, as the picker shows it: its label from `commits` when it is there, else its short id alone (it
/// fell out of the newest 100, or the list has not loaded yet). None for a source that is not a commit.
pub fn picked_commit(source: &ChangesSource, commits: &[crate::diff_source::Commit]) -> Option<(String, String)> {
    let ChangesSource::Commit(id) = source else {
        return None;
    };
    Some(match commits.iter().find(|commit| &commit.id == id) {
        Some(commit) => (commit.id.clone(), commit_label(&commit.short, &commit.subject)),
        None => (id.clone(), short(id).to_string()),
    })
}

/// Each changed turn's number from the session's start (1-based), by counting its `turnEnd` events, given in order as
/// their event numbers. `ends` must hold every turn end from the session's first event. A turn missing from `ends` is
/// not in the map. The picker then numbers every turn by its place in the list, so it never mixes the two counts.
pub fn turn_numbers(turns: &[i64], ends: impl IntoIterator<Item = i64>) -> HashMap<i64, usize> {
    ends.into_iter()
        .enumerate()
        .filter(|(_, seq)| turns.contains(seq))
        .map(|(n, seq)| (seq, n + 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_source::Commit;
    use std::path::PathBuf;

    const SOURCES: [fn() -> ChangesSource; 5] = [
        || ChangesSource::Session,
        || ChangesSource::Turn(12),
        || ChangesSource::Branch,
        || ChangesSource::Commit("a1b2c3d4".into()),
        || ChangesSource::Uncommitted,
    ];

    fn sources() -> Vec<ChangesSource> {
        SOURCES.iter().map(|source| source()).collect()
    }

    #[test]
    fn the_views_of_the_folder_as_it_is_now_follow_edits_not_a_turn_or_a_commit() {
        let follow: Vec<_> = sources().into_iter().filter(ChangesSource::follows_edits).collect();
        assert_eq!(
            follow,
            vec![
                ChangesSource::Session,
                ChangesSource::Branch,
                ChangesSource::Uncommitted
            ]
        );
    }

    #[test]
    fn an_empty_diff_says_what_is_empty_naming_the_base_branch_when_known() {
        assert_eq!(ChangesSource::Session.empty_note(None), "No changes yet.");
        assert_eq!(
            ChangesSource::Branch.empty_note(Some("origin/main")),
            "No changes against origin/main."
        );
        assert_eq!(
            ChangesSource::Branch.empty_note(None),
            "No changes against the base branch."
        );
        assert_eq!(ChangesSource::Uncommitted.empty_note(None), "Nothing uncommitted.");
    }

    fn commits() -> Vec<Commit> {
        vec![Commit {
            id: "a1b2c3d4e5".into(),
            short: "a1b2c3d".into(),
            subject: "Fix the retry".into(),
            author: "You".into(),
            time: 1,
        }]
    }

    #[test]
    fn commits_are_listed_by_short_id_and_subject() {
        assert_eq!(commit_label("a1b2c3d", "Fix the retry"), "a1b2c3d Fix the retry");
        assert_eq!(commit_label("a1b2c3d", ""), "a1b2c3d");
    }

    #[test]
    fn the_commit_picked_stays_in_the_picker_when_it_is_not_among_the_newest() {
        let commits = commits();
        assert_eq!(
            picked_commit(&ChangesSource::Commit("a1b2c3d4e5".into()), &commits),
            Some(("a1b2c3d4e5".into(), "a1b2c3d Fix the retry".into()))
        );
        assert_eq!(
            picked_commit(&ChangesSource::Commit("ffffeeee1111".into()), &commits),
            Some(("ffffeeee1111".into(), "ffffeee".into()))
        );
        assert_eq!(
            picked_commit(&ChangesSource::Commit("ffffeeee1111".into()), &[]).map(|(_, label)| label),
            Some("ffffeee".into())
        );
        assert_eq!(picked_commit(&ChangesSource::Session, &commits), None);
    }

    #[test]
    fn a_commit_names_itself_in_a_line_comment_and_the_files_as_they_are_now_need_no_name() {
        assert_eq!(
            ChangesSource::Commit("a1b2c3d4e5".into()).comment_place(),
            Some("of commit a1b2c3d".into())
        );
        for source in sources()
            .into_iter()
            .filter(|source| !matches!(source, ChangesSource::Commit(_)))
        {
            assert_eq!(source.comment_place(), None, "{source:?}");
        }
    }

    #[test]
    fn only_the_git_sources_read_through_wb_diff() {
        assert_eq!(ChangesSource::Session.diff_source(), None);
        assert_eq!(ChangesSource::Turn(3).diff_source(), None);
        assert_eq!(
            ChangesSource::Commit("abc".into()).diff_source(),
            Some(DiffSource::Commit { id: "abc".into() })
        );
    }

    #[test]
    fn turns_are_numbered_from_the_sessions_start() {
        // Turns end at events 4, 9 and 15; the second changed nothing, so it has no checkpoint.
        let numbers = turn_numbers(&[4, 15], [4, 9, 15]);
        assert_eq!(numbers.get(&4), Some(&1));
        assert_eq!(numbers.get(&15), Some(&3));
        assert_eq!(numbers.get(&9), None);
        // Not read yet: not numbered.
        assert!(turn_numbers(&[4], []).is_empty());
    }

    /// A scratch repository with a fixed identity, removed when dropped.
    struct Repo(PathBuf);

    impl Repo {
        fn new() -> Repo {
            let dir = std::env::temp_dir().join(format!("nexus-changes-source-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
            let repo = Repo(dir);
            for args in [
                &["init", "--quiet", "--initial-branch=main"][..],
                &["config", "user.name", "Test"],
                &["config", "user.email", "test@example.invalid"],
                &["config", "commit.gpgsign", "false"],
            ] {
                git::git(&repo.0, args).unwrap_or_else(|e| panic!("{e}"));
            }
            repo
        }

        fn write(&self, path: &str, text: &str) {
            std::fs::write(self.0.join(path), text).unwrap_or_else(|e| panic!("{e}"));
        }

        fn checkpoint(&self, thread: &str, seq: i64) {
            git::checkpoint(&self.0, &format!("{}{seq:010}", checkpoint_prefix(thread)))
                .unwrap_or_else(|e| panic!("{e}"));
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_turn_shows_its_own_changes_and_the_session_shows_them_all() {
        let repo = Repo::new();
        repo.write("a.txt", "one\n");
        repo.checkpoint("t", 0);
        repo.write("a.txt", "two\n");
        repo.checkpoint("t", 5);
        repo.write("b.txt", "new\n");
        repo.checkpoint("t", 11);
        assert_eq!(changed_turns(&repo.0, "t"), vec![5, 11]);
        assert!(changed_turns(&repo.0, "other").is_empty());

        let (turn, cut) = session_diff(&repo.0, "t", None, Some(11)).unwrap_or_else(|e| panic!("{e}"));
        assert!(!cut);
        assert!(turn.contains("+new") && !turn.contains("+two"), "{turn}");
        let (all, _) = session_diff(&repo.0, "t", None, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(all.contains("+two") && all.contains("+new"), "{all}");
        assert_eq!(
            session_diff(&repo.0, "t", None, Some(0)),
            Err("That turn's changes weren't recorded.".into())
        );
        assert_eq!(
            session_diff(&repo.0, "other", None, None),
            Err("No changes recorded yet.".into())
        );
    }
}
