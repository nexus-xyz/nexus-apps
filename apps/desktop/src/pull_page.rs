//! The pull request page's model: where a session's page reads and writes, the calls themselves, and the
//! page's words. The reads, the writes, the merge plan and the errors are `nexus_pull_requests::pull_page`'s. The
//! page is drawn by `pages/pull_request.rs`.
//!
//! While this build owns the store, the session is found as the Nexus app finds it: through the runner's workbench and
//! the watch's link (`pull_page::session`). While the Nexus app owns it, the session's folder is read from the store and
//! its pull request is the one the store links. The page works in both, because `gh` writes to GitHub, not the store.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use nexus_pull_requests::pull_page::{self, ErrorKind, MergeAction, PullError, PullPage, ReviewThread, Session};
use nexus_pull_requests::stack_actions::{Action, Done, Layer, Stack};

use crate::code::CodeStore;

/// Where the page for `thread_id` reads and writes: `url`, one of the session's pull requests (the top bar's card
/// picks it), else the one the card shows. Another URL is refused: the page merges with the person's `gh` sign-in.
pub fn locate(store: Option<&CodeStore>, thread_id: &str, url: Option<&str>) -> Result<Session, PullError> {
    if crate::pr_watch::GpuiShell::running() {
        return pull_page::session_for(&crate::pr_watch::GpuiShell, thread_id, url);
    }
    let store = store.ok_or_else(|| PullError::new(ErrorKind::Session, "The workbench is not open."))?;
    let folder = store
        .folder(thread_id)
        .map_err(|message| PullError::new(ErrorKind::Session, message))?;
    let path = crate::code_run::agent_env().remove("PATH").unwrap_or_default();
    let saved = store.pull_request(thread_id);
    // The Nexus app owns the store: only the pull request it links is known here.
    if url.is_some_and(|url| Some(url) != saved.as_deref()) {
        return Err(PullError::new(
            ErrorKind::NotFound,
            "That pull request is not one of this session's.",
        ));
    }
    pull_page::locate(folder.path, path, saved)
}

/// Runs one of the page's calls to the end, on a runtime of its own: call it off the UI thread.
fn block_on<T>(work: impl std::future::Future<Output = Result<T, PullError>>) -> Result<T, PullError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| PullError::new(ErrorKind::Github, error.to_string()))?
        .block_on(work)
}

pub fn read(session: &Session) -> Result<PullPage, PullError> {
    block_on(session.gh().page())
}

pub fn reply(session: &Session, thread: &str, body: &str) -> Result<(), PullError> {
    block_on(session.gh().reply(thread, body))
}

pub fn resolve(session: &Session, thread: &str, resolved: bool) -> Result<(), PullError> {
    block_on(session.gh().resolve(thread, resolved))
}

/// Merges what the person confirmed, then tells the watch to read the pull request soon, as the Nexus app does.
pub fn merge(
    session: &Session,
    thread_id: &str,
    head: &str,
    method: &str,
    action: MergeAction,
) -> Result<(), PullError> {
    block_on(session.gh().merge(head, method, action))?;
    crate::pr_watch::turn_ended(thread_id);
    Ok(())
}

pub fn auto_off(session: &Session, thread_id: &str) -> Result<(), PullError> {
    block_on(session.gh().auto_off())?;
    crate::pr_watch::turn_ended(thread_id);
    Ok(())
}

/// Reads the stack that the session's pull requests `urls` (bottom first) make, for the stack card to ask with.
pub fn stack(session: &Session, urls: &[String]) -> Result<Stack, PullError> {
    block_on(session.gh().stack(urls))
}

/// Runs the stack action the person confirmed, then tells the watch to read the pull requests soon: even one that
/// failed partway may have pushed or merged part of the stack.
pub fn stack_act(
    session: &Session,
    thread_id: &str,
    stack: &Stack,
    action: Action,
    method: &str,
) -> Result<Done, PullError> {
    let done = block_on(session.gh().act(stack, action, method));
    crate::pr_watch::turn_ended(thread_id);
    done
}

/// When the watch last read each session's pull request: the page reads again when it moves, as the Nexus app's page
/// does on `wb:pull`.
fn reads() -> &'static Mutex<HashMap<String, i64>> {
    static READS: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    READS.get_or_init(Mutex::default)
}

/// The watch read a session's pull request (`pr_watch.rs`'s `emit`).
pub fn watched(thread_id: &str, read_at: Option<i64>) {
    if let (Some(at), Ok(mut reads)) = (read_at, reads().lock()) {
        reads.insert(thread_id.to_string(), at);
    }
}

/// When the watch last read the session's pull request.
pub fn watch_read(thread_id: &str) -> Option<i64> {
    reads().lock().ok()?.get(thread_id).copied()
}

/// Whether the topbar's pull request link opens this page: a GitHub pull request. A GitLab merge request opens in the
/// browser, as the Nexus app offers no page for it.
pub fn has_page(url: &str) -> bool {
    url.starts_with("https://") && url.split('/').nth(5) == Some("pull")
}

// The words, as `pull-page-model.ts` says them.

/// The method as GitHub's button names it.
pub fn method_label(method: &str) -> &str {
    match method {
        "squash" => "Squash and merge",
        "merge" => "Merge",
        "rebase" => "Rebase and merge",
        other => other,
    }
}

/// The merge button's label.
pub fn merge_label(action: MergeAction, method: &str) -> String {
    match action {
        MergeAction::Queue => "Add to merge queue".into(),
        MergeAction::Auto => format!("{} when checks pass", method_label(method)),
        MergeAction::Merge => method_label(method).into(),
    }
}

/// What the confirmation asks.
pub fn confirm_text(page: &PullPage, action: MergeAction, method: &str) -> String {
    let head: String = page.details.head.chars().take(7).collect();
    let number = page.status.number;
    match action {
        MergeAction::Queue => {
            format!("Add #{number} to the merge queue at {head}? The queue merges it when its checks pass.")
        }
        MergeAction::Auto => format!(
            "{} #{number} when its required checks pass? It merges {head}, and a new push turns this off.",
            method_label(method)
        ),
        MergeAction::Merge => format!(
            "{} #{number} at {head} now? This can't be undone here.",
            method_label(method)
        ),
    }
}

/// What the page says while there is nothing to press: merged, closed, queued or auto-merging.
pub fn merge_note(page: &PullPage) -> Option<String> {
    if page.status.state == "MERGED" {
        return Some("Merged.".into());
    }
    if page.status.state != "OPEN" {
        return Some("Closed without merging.".into());
    }
    if page.details.in_queue {
        return Some(match page.details.queue_position {
            None => "In the merge queue.".into(),
            Some(position) => format!("In the merge queue, at position {position}."),
        });
    }
    page.details.auto_merge.as_ref().map(|auto| {
        format!(
            "Auto-merge is on ({}, by {}). It merges when the required checks pass.",
            method_label(&auto.method).to_lowercase(),
            auto.by
        )
    })
}

/// `#1`, `#1 and #2`, `#1, #2 and #3`.
fn numbers(numbers: &[u64]) -> String {
    let refs: Vec<String> = numbers.iter().map(|number| format!("#{number}")).collect();
    match refs.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// What the stack card's confirmation asks.
pub fn stack_confirm_text(stack: &Stack, action: Action, method: &str) -> String {
    let open: Vec<&Layer> = stack.layers.iter().filter(|layer| layer.state != "MERGED").collect();
    let all = numbers(&open.iter().map(|layer| layer.number).collect::<Vec<_>>());
    let trunk = &stack.trunk;
    match (action, stack.native) {
        (Action::Rebase, _) => format!(
            "Rebase {all} onto {trunk}? Each one that moves is force-pushed, unless someone pushed to it since."
        ),
        (Action::Merge, Some(_)) => format!(
            "{} {all} into {trunk} now, as one stack? This can't be undone here.",
            method_label(method)
        ),
        (Action::Merge, None) => {
            let Some((bottom, rest)) = open.split_first() else {
                return String::new();
            };
            let merge = format!("{} #{} into {trunk} now", method_label(method), bottom.number);
            if rest.is_empty() {
                return format!("{merge}? This can't be undone here.");
            }
            let rest = numbers(&rest.iter().map(|layer| layer.number).collect::<Vec<_>>());
            format!("{merge}, then rebase {rest} onto it? Their checks run again: merge the next one when they pass.")
        }
    }
}

/// What the stack card says after an action.
pub fn stack_done_text(done: &Done, trunk: &str) -> String {
    if done.queued {
        return "GitHub added the stack to its merge queue.".into();
    }
    match (done.merged.as_slice(), done.rebased) {
        ([], _) => format!("The stack is on {trunk}."),
        (merged, 0) => format!("Merged {}.", numbers(merged)),
        (merged, _) => format!("Merged {}. The rest of the stack is on {trunk}.", numbers(merged)),
    }
}

/// A failure's title: what kind it is, in a few words.
pub fn error_title(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Unavailable => "GitHub CLI not found",
        ErrorKind::SignedOut => "Not signed in to GitHub",
        ErrorKind::Timeout => "GitHub did not answer",
        ErrorKind::NotFound => "No pull request",
        ErrorKind::Blocked => "Can't merge yet",
        ErrorKind::Changed => "The pull request changed",
        ErrorKind::Refused => "GitHub refused",
        _ => "Couldn't read the pull request",
    }
}

/// Whether a failed write leaves what the person confirmed out of date: the page reads again to show what is there.
pub fn rereads_after(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::Changed | ErrorKind::Blocked)
}

/// Open threads first, then resolved ones, each in GitHub's order.
pub fn sort_threads(threads: &[ReviewThread]) -> Vec<&ReviewThread> {
    let (open, resolved): (Vec<_>, Vec<_>) = threads.iter().partition(|thread| !thread.resolved);
    open.into_iter().chain(resolved).collect()
}

/// Where a thread is: its file and line, if it is on one.
pub fn thread_place(thread: &ReviewThread) -> String {
    match (&thread.path, thread.line) {
        (None, _) => "Pull request".into(),
        (Some(path), None) => path.clone(),
        (Some(path), Some(line)) => format!("{path}:{line}"),
    }
}

/// What a review did, after its author's name.
pub fn review_word(state: &str) -> String {
    match state {
        "APPROVED" => "approved".into(),
        "CHANGES_REQUESTED" => "asked for changes".into(),
        "COMMENTED" => "commented".into(),
        "DISMISSED" => "was dismissed".into(),
        other => other.to_lowercase(),
    }
}

/// The checks' count: "3 of 4 passed", or nothing with no checks.
pub fn checks_summary(page: &PullPage) -> String {
    let checks = &page.status.checks;
    if checks.is_empty() {
        return String::new();
    }
    let passed = checks.iter().filter(|check| check.state == "success").count();
    format!("{passed} of {} passed", checks.len())
}

/// The threads' count: "1 open of 2", or nothing with no threads.
pub fn threads_summary(page: &PullPage) -> String {
    let threads = &page.details.threads;
    if threads.is_empty() {
        return String::new();
    }
    let open = threads.iter().filter(|thread| !thread.resolved).count();
    format!("{open} open of {}", threads.len())
}

/// A comment's time as the Nexus app shows it, in the computer's time zone: "Nov 5, 2:03 PM". The text as it came
/// when it does not read.
pub fn comment_time(at: &str) -> String {
    comment_time_in(at, &chrono::Local)
}

fn comment_time_in<Zone: chrono::TimeZone>(at: &str, zone: &Zone) -> String
where
    Zone::Offset: std::fmt::Display,
{
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|time| time.with_timezone(zone).format("%b %-d, %-I:%M %p").to_string())
        .unwrap_or_else(|_| at.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use nexus_pull_requests::pull_page::{AutoMerge, Details, Merge};
    use nexus_pull_requests::pull_request::PullRequestStatus;

    fn page(state: &str) -> PullPage {
        PullPage {
            status: PullRequestStatus {
                number: 42,
                title: "Sample".into(),
                url: "https://github.com/example/repo/pull/42".into(),
                draft: false,
                state: state.into(),
                checks: Vec::new(),
                review_decision: None,
                reviews: Vec::new(),
                mergeable: "MERGEABLE".into(),
                host: Default::default(),
                head_branch: String::new(),
                base_branch: String::new(),
                comments: Vec::new(),
            },
            details: Details {
                head: "abc1234def".into(),
                ..Details::default()
            },
            merge: Merge::default(),
        }
    }

    #[test]
    fn says_the_merge_as_the_nexus_app_does() {
        let open = page("OPEN");
        assert_eq!(merge_label(MergeAction::Merge, "squash"), "Squash and merge");
        assert_eq!(
            merge_label(MergeAction::Auto, "rebase"),
            "Rebase and merge when checks pass"
        );
        assert_eq!(merge_label(MergeAction::Queue, "squash"), "Add to merge queue");
        assert_eq!(
            confirm_text(&open, MergeAction::Merge, "squash"),
            "Squash and merge #42 at abc1234 now? This can't be undone here."
        );
        assert_eq!(merge_note(&open), None);
        assert_eq!(merge_note(&page("MERGED")).as_deref(), Some("Merged."));
        assert_eq!(merge_note(&page("CLOSED")).as_deref(), Some("Closed without merging."));
        let mut auto = page("OPEN");
        auto.details.auto_merge = Some(AutoMerge {
            method: "squash".into(),
            by: "alice".into(),
        });
        assert_eq!(
            merge_note(&auto).as_deref(),
            Some("Auto-merge is on (squash and merge, by alice). It merges when the required checks pass.")
        );
        let mut queued = page("OPEN");
        queued.details.in_queue = true;
        queued.details.queue_position = Some(2);
        assert_eq!(
            merge_note(&queued).as_deref(),
            Some("In the merge queue, at position 2.")
        );
    }

    #[test]
    fn titles_each_kind_of_failure() {
        assert_eq!(error_title(ErrorKind::SignedOut), "Not signed in to GitHub");
        assert_eq!(error_title(ErrorKind::Blocked), "Can't merge yet");
        assert_eq!(error_title(ErrorKind::Input), "Couldn't read the pull request");
        assert!(rereads_after(ErrorKind::Changed) && !rereads_after(ErrorKind::Refused));
    }

    #[test]
    fn opens_only_a_github_pull_request_here() {
        assert!(has_page("https://github.com/example/repo/pull/42"));
        assert!(!has_page("https://gitlab.com/example/repo/-/merge_requests/7"));
        assert!(!has_page("https://github.com/example/repo/issues/42"));
    }

    #[test]
    fn reads_a_comment_time() {
        assert_eq!(comment_time_in("2026-11-05T14:03:00Z", &chrono::Utc), "Nov 5, 2:03 PM");
        let brasilia = chrono::FixedOffset::west_opt(3 * 3600).expect("offset");
        assert_eq!(comment_time_in("2026-01-09T03:07:00Z", &brasilia), "Jan 9, 12:07 AM");
        assert_eq!(comment_time("soon"), "soon");
    }

    /// The page against recorded `gh` and `glab` answers (`fixtures/pull-page/`), through a fake CLI that logs each
    /// call. Nothing reaches GitHub or GitLab.
    #[cfg(unix)]
    mod recorded {
        use super::*;
        use nexus_pull_requests::pull_page::{self, ErrorKind, MergeAction, Session};
        use std::path::{Path, PathBuf};

        const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/pull-page");

        /// A folder with a fake `gh` and `glab`, answering from the fixtures, and the session the page reads.
        struct Fake {
            dir: PathBuf,
            session: Session,
        }

        impl Fake {
            /// `view` names the `gh pr view` fixture; `host` is written to the repository's git config.
            fn new(view: &str, host: &str) -> Fake {
                let dir = std::env::temp_dir().join(format!("nexus-gpui-pull-{}", uuid::Uuid::new_v4()));
                let bin = dir.join("bin");
                let repo = dir.join("repo");
                std::fs::create_dir_all(&bin).expect("bin");
                std::fs::create_dir_all(repo.join(".git")).expect("repo");
                std::fs::write(repo.join(".git/config"), format!("[nexus]\n\thost = {host}\n")).expect("config");
                let gh = format!(
                    r#"#!/bin/sh
[ -n "$FAKE_PROBE" ] && exit 0
printf '%s\n' "$*" >> "{log}"
case "$1 $2" in
"pr view") cat "{fx}/{view}" ;;
"api graphql")
  case "$*" in
  *addPullRequestReviewThreadReply*) cat "{fx}/gh-graphql-reply.json" ;;
  *unresolveReviewThread*) cat "{fx}/gh-graphql-unresolve.json" ;;
  *resolveReviewThread*) cat "{fx}/gh-graphql-resolve.json" ;;
  *) cat "{fx}/gh-graphql-details.json" ;;
  esac ;;
"pr merge") exit 0 ;;
*) exit 9 ;;
esac
"#,
                    log = dir.join("gh.log").display(),
                    fx = FIXTURES,
                );
                let glab = format!(
                    "#!/bin/sh\n[ -n \"$FAKE_PROBE\" ] && exit 0\nprintf '%s\\n' \"$*\" >> \"{log}\"\ncat \"{fx}/glab-mr-view.json\"\n",
                    log = dir.join("glab.log").display(),
                    fx = FIXTURES,
                );
                for (name, script) in [("gh", gh), ("glab", glab)] {
                    let program = bin.join(name);
                    nexus_workbench::process::write_fake_script(&program, &script);
                    probe(&program);
                }
                // `cat` is the one outside program the scripts run.
                let path = format!("{}:/bin:/usr/bin", bin.display());
                let target = Some("https://github.com/example/repo/pull/42".to_string());
                let session = match pull_page::locate(repo.clone(), path.clone(), target) {
                    Ok(session) => session,
                    // A GitLab repository has no page: keep a session to show the CLI is never run.
                    Err(_) => Session {
                        folder: repo,
                        path,
                        target: String::new(),
                    },
                };
                Fake { dir, session }
            }

            fn log(&self, name: &str) -> Vec<String> {
                std::fs::read_to_string(self.dir.join(name))
                    .unwrap_or_default()
                    .lines()
                    .map(String::from)
                    .collect()
            }

            /// The writes: every call but the page's two reads.
            fn writes(&self) -> Vec<String> {
                self.log("gh.log")
                    .into_iter()
                    .filter(|line| !line.starts_with("pr view") && !line.contains("reviewThreads(first"))
                    .collect()
            }
        }

        impl Drop for Fake {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        /// Runs a just-written script once until it starts: a fork in another test thread may still hold it open for
        /// writing ("Text file busy").
        fn probe(program: &Path) {
            for _ in 0..200 {
                match nexus_workbench::process::command(program)
                    .env("FAKE_PROBE", "1")
                    .status()
                {
                    Err(error) if error.raw_os_error() == Some(26) => {
                        std::thread::sleep(std::time::Duration::from_millis(5))
                    }
                    started => {
                        started.expect("the fake CLI starts");
                        return;
                    }
                }
            }
        }

        #[test]
        fn shows_the_checks_with_their_logs_and_the_threads_with_their_replies() {
            let fake = Fake::new("gh-pr-view.json", "github");
            let page = read(&fake.session).expect("page");
            let checks: Vec<_> = page
                .status
                .checks
                .iter()
                .map(|check| (check.name.as_str(), check.state.as_str(), check.url.as_deref()))
                .collect();
            assert_eq!(
                checks,
                [
                    (
                        "rust",
                        "success",
                        Some("https://github.com/example/repo/actions/runs/1/job/11")
                    ),
                    (
                        "lint",
                        "failure",
                        Some("https://github.com/example/repo/actions/runs/1/job/12")
                    ),
                    ("deploy/preview", "pending", Some("https://preview.example.com/42")),
                ]
            );
            assert_eq!(page.details.required, ["rust"]);
            assert_eq!(checks_summary(&page), "1 of 3 passed");
            assert_eq!(threads_summary(&page), "1 open of 2");
            let threads = sort_threads(&page.details.threads);
            assert_eq!(thread_place(threads[0]), "src/store.rs:18");
            let replies: Vec<_> = threads[0]
                .comments
                .iter()
                .map(|comment| comment.author.as_str())
                .collect();
            assert_eq!(replies, ["bob", "alice"]);
            // Resolved and outdated: last, on the line it was first left on.
            assert!(threads[1].resolved && threads[1].outdated && threads[1].can_unresolve);
            assert_eq!(thread_place(threads[1]), "README.md:4");
            assert_eq!(review_word(&page.status.reviews[0].state), "commented");
            // A failed check that is not required does not block the merge.
            assert_eq!(page.merge.action, Some(MergeAction::Merge));
            assert_eq!(page.merge.method.as_deref(), Some("squash"));
            assert_eq!(page.details.methods, ["squash", "rebase"]);
            assert!(fake.writes().is_empty());
        }

        #[test]
        fn replies_to_a_thread_and_refuses_an_empty_reply() {
            let fake = Fake::new("gh-pr-view.json", "github");
            reply(&fake.session, "PRRT_kwDOA1", "  Renamed it.  ").expect("replied");
            let writes = fake.writes();
            assert_eq!(writes.len(), 1);
            assert!(
                writes[0].starts_with("api graphql --hostname github.com -f thread=PRRT_kwDOA1 -f body=Renamed it.")
            );
            assert!(writes[0].contains("addPullRequestReviewThreadReply"));
            let empty = reply(&fake.session, "PRRT_kwDOA1", "  ").expect_err("empty");
            assert_eq!(
                (empty.kind, empty.message.as_str()),
                (ErrorKind::Input, "Write a reply first.")
            );
            assert_eq!(fake.writes().len(), 1);
        }

        #[test]
        fn resolves_and_reopens_a_thread() {
            let fake = Fake::new("gh-pr-view.json", "github");
            resolve(&fake.session, "PRRT_kwDOA1", true).expect("resolved");
            resolve(&fake.session, "PRRT_kwDOA2", false).expect("reopened");
            let writes = fake.writes();
            assert_eq!(writes.len(), 2);
            assert!(writes[0].contains("thread=PRRT_kwDOA1") && writes[0].contains("resolveReviewThread"));
            assert!(writes[1].contains("thread=PRRT_kwDOA2") && writes[1].contains("unresolveReviewThread"));
        }

        #[test]
        fn refuses_a_merge_while_a_required_check_fails_or_the_branch_conflicts() {
            let fake = Fake::new("gh-pr-view-blocked.json", "github");
            let page = read(&fake.session).expect("page");
            assert_eq!(page.merge.action, None);
            assert_eq!(
                page.merge.blocked,
                [
                    "The branch has conflicts with its base branch.",
                    "The required check \u{201c}rust\u{201d} failed."
                ]
            );
            let refused =
                merge(&fake.session, "t-1", "abc1234def5678", "squash", MergeAction::Merge).expect_err("refused");
            assert_eq!(refused.kind, ErrorKind::Blocked);
            assert_eq!(error_title(refused.kind), "Can't merge yet");
            assert!(rereads_after(refused.kind));
            assert!(fake.writes().is_empty(), "gh pr merge never ran");
        }

        #[test]
        fn merges_the_confirmed_head_with_the_picked_method() {
            let fake = Fake::new("gh-pr-view.json", "github");
            let page = read(&fake.session).expect("page");
            assert_eq!(
                confirm_text(&page, MergeAction::Merge, "rebase"),
                "Rebase and merge #42 at abc1234 now? This can't be undone here."
            );
            merge(&fake.session, "t-1", &page.details.head, "rebase", MergeAction::Merge).expect("merged");
            assert_eq!(
                fake.writes(),
                ["pr merge https://github.com/example/repo/pull/42 --rebase --match-head-commit abc1234def5678"]
            );
        }

        #[test]
        fn a_gitlab_merge_request_has_no_page_and_glab_is_not_run() {
            let fake = Fake::new("gh-pr-view.json", "gitlab");
            let target = Some("https://gitlab.com/example/repo/-/merge_requests/7".to_string());
            let refused =
                pull_page::locate(fake.session.folder.clone(), fake.session.path.clone(), target).expect_err("GitLab");
            assert_eq!(refused.kind, ErrorKind::Unavailable);
            assert_eq!(refused.message, pull_page::GITLAB);
            assert!(fake.log("glab.log").is_empty() && fake.log("gh.log").is_empty());
            // The watch still reads the merge request's pipeline from the recorded `glab` answer; the topbar link opens
            // it in the browser.
            let recorded: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(format!("{FIXTURES}/glab-mr-view.json")).expect("fixture"),
            )
            .expect("json");
            let status = nexus_pull_requests::pull_request::gitlab::parse(&recorded, None).expect("status");
            assert_eq!(status.checks[0].state, "failure");
            assert!(!has_page(&status.url));
        }
    }

    fn layer(number: u64, state: &str) -> Layer {
        Layer {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            branch: format!("b{number}"),
            head: format!("h{number}"),
            state: state.into(),
            draft: false,
        }
    }

    fn stack(native: bool, states: &[&str]) -> Stack {
        Stack {
            native: native.then_some(7),
            trunk: "main".into(),
            layers: states
                .iter()
                .enumerate()
                .map(|(ix, state)| layer(ix as u64 + 1, state))
                .collect(),
        }
    }

    #[test]
    fn the_stack_confirmation_names_what_moves_and_what_merges() {
        let three = stack(false, &["MERGED", "OPEN", "OPEN"]);
        assert_eq!(
            stack_confirm_text(&three, Action::Rebase, "squash"),
            "Rebase #2 and #3 onto main? Each one that moves is force-pushed, unless someone pushed to it since."
        );
        assert_eq!(
            stack_confirm_text(&three, Action::Merge, "squash"),
            "Squash and merge #2 into main now, then rebase #3 onto it? Their checks run again: merge the next one when they pass."
        );
        assert_eq!(
            stack_confirm_text(&stack(true, &["OPEN", "OPEN", "OPEN"]), Action::Merge, "squash"),
            "Squash and merge #1, #2 and #3 into main now, as one stack? This can't be undone here."
        );
        assert_eq!(
            stack_confirm_text(&stack(false, &["MERGED", "OPEN"]), Action::Merge, "merge"),
            "Merge #2 into main now? This can't be undone here."
        );
    }

    #[test]
    fn the_stack_card_says_what_an_action_did() {
        let done = |merged: Vec<u64>, rebased, queued| Done {
            merged,
            rebased,
            queued,
        };
        assert_eq!(
            stack_done_text(&done(vec![], 2, false), "main"),
            "The stack is on main."
        );
        assert_eq!(
            stack_done_text(&done(vec![1, 2], 0, false), "main"),
            "Merged #1 and #2."
        );
        assert_eq!(
            stack_done_text(&done(vec![1], 2, false), "main"),
            "Merged #1. The rest of the stack is on main."
        );
        assert_eq!(
            stack_done_text(&done(vec![], 0, true), "main"),
            "GitHub added the stack to its merge queue."
        );
    }
}
