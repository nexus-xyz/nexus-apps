//! GitHub's two check formats, kept separate from the session's git diff, and the pull request's reviews and
//! mergeability, which `pr_watch` reads to tell the person or the agent what changed. A GitLab repository's merge
//! request is read through `glab` into the same [`PullRequestStatus`] ([`gitlab`]), so the watch and the
//! windows treat both alike.
use nexus_workbench::source_host::Host;
use serde::Serialize;
use serde_json::Value;
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

pub mod gitlab;

const READ_DEADLINE: Duration = Duration::from_secs(7);
/// What [`read`] says when `gh` is not installed or not found: `pr_watch` stops watching on it, without telling anyone.
pub const UNAVAILABLE: &str = "The GitHub CLI is unavailable. Install gh and sign in.";
/// The same for `glab`, in a GitLab repository.
pub const UNAVAILABLE_GITLAB: &str = "The GitLab CLI is unavailable. Install glab and sign in.";

/// Whether a read failed because the host's CLI is not installed or not found.
pub fn is_unavailable(error: &str) -> bool {
    error == UNAVAILABLE || error == UNAVAILABLE_GITLAB
}

/// Whether a read failed because the branch has no pull request (or merge request), in `gh`'s or `glab`'s words.
pub fn is_none(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("no pull requests found") || error.contains("no open merge request")
}

/// Reads the pull request `target` names (its URL or number, checked by `pr_watch::target`), else the branch's, with
/// the CLI of the repository's host.
pub async fn read(folder: &Path, path: &str, target: Option<&str>) -> Result<PullRequestStatus, String> {
    let host = Host::of_repo(folder);
    read_with(host, OsStr::new(host.program()), folder, path, target, READ_DEADLINE).await
}

pub async fn read_with(
    host: Host,
    program: &OsStr,
    folder: &Path,
    path: &str,
    target: Option<&str>,
    deadline: Duration,
) -> Result<PullRequestStatus, String> {
    if host == Host::GitLab {
        return gitlab::read(program, folder, path, target, deadline).await;
    }
    let mut args = vec!["pr", "view"];
    args.extend(target);
    view(program, folder, path, args, deadline).await
}

/// `gh pr view` with `args` (the pull request, and `--repo` for another computer's repository), as the watch reads it.
pub(crate) async fn view(
    program: &OsStr,
    folder: &Path,
    path: &str,
    mut args: Vec<&str>,
    deadline: Duration,
) -> Result<PullRequestStatus, String> {
    args.extend([
        "--json",
        "number,title,url,isDraft,state,statusCheckRollup,reviewDecision,latestReviews,mergeable,comments,headRefName,baseRefName",
    ]);
    let stdout = run(program, folder, path, &args, deadline)
        .await
        .map_err(|failure| match failure {
            GhFailure::Late => "GitHub did not answer within seven seconds.".to_string(),
            GhFailure::Missing => UNAVAILABLE.to_string(),
            GhFailure::Failed(text) => text,
        })?;
    parse(serde_json::from_slice(&stdout).map_err(|_| "GitHub returned an invalid response.")?)
}

/// Why a `gh` command failed.
#[derive(Debug, Clone, PartialEq)]
pub enum GhFailure {
    /// It did not finish before its deadline.
    Late,
    /// `gh` is not installed or not found.
    Missing,
    /// It ran and failed: what it wrote on stderr, at most 2000 characters.
    Failed(String),
}

/// Runs a `gh` (or `glab`) command in the session's folder, prompts off, under a deadline, and returns its stdout.
/// [`read_with`] and the pull request page's reads and writes (`pull_page`) go through it. `gh` reads
/// `GH_PROMPT_DISABLED`, `glab` reads `NO_PROMPT`.
pub async fn run(
    program: &OsStr,
    folder: &Path,
    path: &str,
    args: &[&str],
    deadline: Duration,
) -> Result<Vec<u8>, GhFailure> {
    let output = tokio::time::timeout(
        deadline,
        nexus_workbench::process::async_command(program)
            .args(args)
            .current_dir(folder)
            .env("PATH", path)
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_PROMPT", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| GhFailure::Late)?
    .map_err(|_| GhFailure::Missing)?;
    if !output.status.success() {
        return Err(GhFailure::Failed(
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(2000)
                .collect(),
        ));
    }
    Ok(output.stdout)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStatus {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub draft: bool,
    pub state: String,
    pub checks: Vec<Check>,
    /// `APPROVED`, `CHANGES_REQUESTED` or `REVIEW_REQUIRED`; None when the repository asks for no review.
    pub review_decision: Option<String>,
    /// Each reviewer's latest review.
    pub reviews: Vec<Review>,
    /// `MERGEABLE`, `CONFLICTING`, or `UNKNOWN` while GitHub computes it.
    pub mergeable: String,
    /// Where it lives; absent: GitHub. The windows name it by it (a merge request, `!42`, on GitLab).
    #[serde(skip_serializing_if = "Host::is_github")]
    pub host: Host,
    /// The branch it merges from, and the branch it merges into; empty when the host did not say. A pull request
    /// whose base is another's head sits on it in a stack ([`crate::stack`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub head_branch: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_branch: String,
    /// The conversation's comments, without the signed-in person's own and the hidden ones: only an agent's own
    /// watch reads them (`pr_watch`), so the windows never get them.
    #[serde(skip)]
    pub comments: Vec<Comment>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Check {
    pub name: String,
    pub state: String,
    pub url: Option<String>,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    /// GitHub's id of the review; a reviewer's next review has a new one.
    pub id: String,
    pub author: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED` or `PENDING`.
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}
pub fn parse(value: Value) -> Result<PullRequestStatus, String> {
    let text = |key: &str| {
        value[key]
            .as_str()
            .filter(|text| !text.is_empty())
            .map(String::from)
            .ok_or_else(|| format!("GitHub did not report {key}."))
    };
    let checks = value["statusCheckRollup"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|check| {
            let raw = check["conclusion"]
                .as_str()
                .or_else(|| check["state"].as_str())
                .unwrap_or("PENDING");
            let state = if check["status"].as_str().is_some_and(|status| status != "COMPLETED") {
                "pending"
            } else {
                match raw {
                    "SUCCESS" => "success",
                    "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE"
                    | "STALE" => "failure",
                    "SKIPPED" | "NEUTRAL" => "skipped",
                    _ => "pending",
                }
            };
            Check {
                name: check["name"]
                    .as_str()
                    .or_else(|| check["context"].as_str())
                    .unwrap_or("GitHub check")
                    .into(),
                state: state.into(),
                url: check["detailsUrl"]
                    .as_str()
                    .or_else(|| check["targetUrl"].as_str())
                    .filter(|url| !url.is_empty())
                    .map(String::from),
            }
        })
        .collect();
    let reviews = value["latestReviews"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|review| {
            let author = review["author"]["login"].as_str().unwrap_or("someone").to_string();
            let submitted_at = review["submittedAt"].as_str().map(String::from);
            Review {
                // An older gh reports no id: the author and the time tell two reviews apart too.
                id: review["id"].as_str().map_or_else(
                    || format!("{author}@{}", submitted_at.as_deref().unwrap_or_default()),
                    String::from,
                ),
                author,
                state: review["state"].as_str().unwrap_or("COMMENTED").to_string(),
                body: review["body"].as_str().unwrap_or_default().trim().to_string(),
                submitted_at,
            }
        })
        .collect();
    let comments = value["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|comment| comment["viewerDidAuthor"] != true && comment["isMinimized"] != true)
        .filter_map(|comment| {
            Some(Comment {
                id: comment["id"].as_str().filter(|id| !id.is_empty())?.to_string(),
                author: comment["author"]["login"].as_str().unwrap_or("someone").to_string(),
                body: comment["body"].as_str().unwrap_or_default().trim().to_string(),
            })
        })
        .collect();
    Ok(PullRequestStatus {
        number: value["number"]
            .as_u64()
            .ok_or("GitHub did not report a pull-request number.")?,
        title: text("title")?,
        url: text("url")?,
        draft: value["isDraft"].as_bool().unwrap_or(false),
        state: text("state")?,
        checks,
        review_decision: value["reviewDecision"]
            .as_str()
            .filter(|decision| !decision.is_empty())
            .map(String::from),
        reviews,
        mergeable: value["mergeable"].as_str().unwrap_or("UNKNOWN").to_string(),
        host: Host::GitHub,
        head_branch: value["headRefName"].as_str().unwrap_or_default().to_string(),
        base_branch: value["baseRefName"].as_str().unwrap_or_default().to_string(),
        comments,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn handles_check_runs_and_legacy_status_contexts_without_inventing_success() {
        let result=parse(json!({"number":42,"title":"Sample","url":"https://github.com/example/repo/pull/42","state":"OPEN","isDraft":true,"statusCheckRollup":[
            {"name":"tests","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://github.com/check/1"},
            {"context":"deploy","state":"ERROR","targetUrl":"https://github.com/check/2"},
            {"name":"queued","status":"QUEUED","conclusion":null},
            {"name":"unknown","status":"COMPLETED","conclusion":null},
            {"name":"optional","status":"COMPLETED","conclusion":"SKIPPED"}
        ]})).expect("status");
        assert!(result.draft);
        assert_eq!(
            result
                .checks
                .iter()
                .map(|check| check.state.as_str())
                .collect::<Vec<_>>(),
            ["success", "failure", "pending", "pending", "skipped"]
        );
        assert_eq!(result.checks[1].name, "deploy");
        assert_eq!(result.mergeable, "UNKNOWN");
        assert!(result.reviews.is_empty() && result.review_decision.is_none());
    }
    #[test]
    fn reads_each_reviewers_latest_review_and_the_mergeability() {
        let result = parse(json!({"number":42,"title":"Sample","url":"https://github.com/example/repo/pull/42","state":"OPEN",
            "reviewDecision":"CHANGES_REQUESTED","mergeable":"CONFLICTING","latestReviews":[
            {"id":"PRR_1","author":{"login":"alice"},"state":"CHANGES_REQUESTED","body":" Rename it. ","submittedAt":"2026-10-29T10:00:00Z"},
            {"author":{"login":"bob"},"state":"APPROVED","body":"","submittedAt":"2026-10-29T11:00:00Z"}
        ]}))
        .expect("status");
        assert_eq!(result.review_decision.as_deref(), Some("CHANGES_REQUESTED"));
        assert_eq!(result.mergeable, "CONFLICTING");
        assert_eq!(result.reviews[0].id, "PRR_1");
        assert_eq!(result.reviews[0].body, "Rename it.");
        assert_eq!(result.reviews[1].id, "bob@2026-10-29T11:00:00Z");
    }
    #[test]
    fn reads_the_comments_of_others_that_are_not_hidden() {
        let result = parse(
            json!({"number":42,"title":"Sample","url":"https://github.com/example/repo/pull/42","state":"OPEN",
                "comments":[
                    {"id":"IC_1","author":{"login":"alice"},"body":" Why this name? ","viewerDidAuthor":false},
                    {"id":"IC_2","author":{"login":"me"},"body":"Pushed a fix.","viewerDidAuthor":true},
                    {"id":"IC_3","author":{"login":"bot"},"body":"spam","isMinimized":true},
                    {"author":{"login":"old"},"body":"no id"}
            ]}),
        )
        .expect("status");
        assert_eq!(
            result.comments,
            [Comment {
                id: "IC_1".into(),
                author: "alice".into(),
                body: "Why this name?".into()
            }]
        );
        assert!(!serde_json::to_string(&result).expect("json").contains("comments"));
    }
    #[test]
    fn reads_the_branches_a_stack_is_built_from() {
        let status = parse(
            json!({"number":43,"title":"On top","url":"https://github.com/example/repo/pull/43",
            "state":"OPEN","headRefName":"feat/b","baseRefName":"feat/a"}),
        )
        .expect("status");
        assert_eq!(
            (status.head_branch.as_str(), status.base_branch.as_str()),
            ("feat/b", "feat/a")
        );
        let older = parse(
            json!({"number":44,"title":"Old gh","url":"https://github.com/example/repo/pull/44",
            "state":"OPEN"}),
        )
        .expect("status");
        assert_eq!((older.head_branch.as_str(), older.base_branch.as_str()), ("", ""));
        let json = serde_json::to_string(&older).expect("json");
        assert!(!json.contains("Branch"), "an unknown branch stays off the wire: {json}");
    }
    #[test]
    fn refuses_a_missing_pull_request_instead_of_showing_an_empty_success() {
        assert!(parse(json!({})).is_err());
    }

    #[cfg(unix)]
    mod command {
        use super::*;
        use std::path::PathBuf;

        struct FakeGh {
            folder: PathBuf,
            program: PathBuf,
        }

        impl FakeGh {
            fn new(script: &str) -> Self {
                let folder = std::env::temp_dir().join(format!("nexus-pr-checks-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&folder).expect("fake CLI folder");
                let program = folder.join("gh");
                nexus_workbench::process::write_fake_script(&program, &format!("#!/bin/sh\n{script}\n"));
                Self { folder, program }
            }

            async fn read(&self) -> Result<PullRequestStatus, String> {
                read_with(
                    Host::GitHub,
                    self.program.as_os_str(),
                    &self.folder,
                    "",
                    None,
                    READ_DEADLINE,
                )
                .await
            }
        }

        impl Drop for FakeGh {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.folder);
            }
        }

        #[tokio::test]
        async fn reads_the_cli_response_with_prompts_disabled() {
            let cli = FakeGh::new(
                r#"
test "$GH_PROMPT_DISABLED" = 1 && test "$GIT_TERMINAL_PROMPT" = 0 || exit 2
test "$1" = pr && test "$2" = view && test "$3" = --json || exit 3
printf '%s' '{"number":42,"title":"Sample","url":"https://example.test/pr/42","state":"OPEN","isDraft":false,"statusCheckRollup":[]}'
"#,
            );
            assert_eq!(cli.read().await.expect("status").number, 42);
        }

        #[tokio::test]
        async fn names_the_pull_request_it_was_given() {
            let cli = FakeGh::new(
                r#"
test "$1" = pr && test "$2" = view && test "$3" = 7 && test "$4" = --json || exit 3
printf '%s' '{"number":7,"title":"Sample","url":"https://example.test/pr/7","state":"OPEN","isDraft":false}'
"#,
            );
            let status = read_with(
                Host::GitHub,
                cli.program.as_os_str(),
                &cli.folder,
                "",
                Some("7"),
                READ_DEADLINE,
            )
            .await;
            assert_eq!(status.expect("status").number, 7);
        }

        #[tokio::test]
        async fn reports_an_unavailable_cli() {
            let cli = FakeGh::new("exit 0");
            let missing = cli.folder.join("missing-gh");
            let error = read_with(Host::GitHub, missing.as_os_str(), &cli.folder, "", None, READ_DEADLINE)
                .await
                .expect_err("fake GitHub command must fail");
            assert_eq!(error, UNAVAILABLE);
        }

        #[tokio::test]
        async fn reports_authentication_errors_without_parsing_them_as_checks() {
            let cli = FakeGh::new("printf '%s' 'authentication required' >&2; exit 1");
            assert_eq!(
                cli.read().await.expect_err("fake GitHub command must fail"),
                "authentication required"
            );
        }

        #[tokio::test]
        async fn refuses_invalid_cli_json() {
            let cli = FakeGh::new("printf '%s' 'not json'");
            assert_eq!(
                cli.read().await.expect_err("fake GitHub command must fail"),
                "GitHub returned an invalid response."
            );
        }

        #[tokio::test]
        async fn bounds_a_cli_that_does_not_finish() {
            let cli = FakeGh::new("exec /bin/sleep 30");
            let error = read_with(
                Host::GitHub,
                cli.program.as_os_str(),
                &cli.folder,
                "",
                None,
                Duration::from_millis(50),
            )
            .await
            .expect_err("fake GitHub command must fail");
            assert_eq!(error, "GitHub did not answer within seven seconds.");
        }
    }
}
