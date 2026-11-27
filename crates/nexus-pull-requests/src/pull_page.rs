//! The pull request page, with no UI toolkit: a session's pull request with its checks, its review threads
//! and their comments, a reply, resolving a thread, and the merge. Everything goes through `gh`
//! ([`pull_request::run`]), each call under a deadline, and every failure is a [`PullError`] the
//! page shows by its kind. Nexus GPUI draws it.
//!
//! The page reads the pull request as the watch does (`pull_request::read_with`), then its threads, the repository's
//! merge methods, auto-merge, the merge queue and which checks are required, in one GraphQL query. [`plan`] says what
//! the merge button does, or why it is disabled. A merge reads the pull request again and refuses when it is blocked,
//! when its head moved since the person confirmed, or when the method is not one the repository allows. It runs
//! `gh pr merge --<method> --match-head-commit <head>`, with `--auto` while required checks are still running. With a
//! merge queue, `gh` adds it to the queue. Remote sessions have no page: their pull request lives on their host. A GitLab
//! repository's session has none either ([`GITLAB`]): its merge request is watched, not shown here.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::pull_request::{self, GhFailure, PullRequestStatus};
use crate::watch::Shell;
use nexus_workbench::source_host::Host;

/// A read: the pull request, then its details.
pub const READ_DEADLINE: Duration = Duration::from_secs(7);
/// A write: a reply, a resolve or a merge, which GitHub can take longer to answer.
pub const WRITE_DEADLINE: Duration = Duration::from_secs(30);
/// The longest reply GitHub takes.
const MAX_REPLY: usize = 65_536;

/// What the page reads besides the pull request itself. One line, so a fake `gh` can match it.
const DETAILS: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed autoMergeAllowed viewerDefaultMergeMethod pullRequest(number:$number){headRefOid mergeStateStatus isMergeQueueEnabled isInMergeQueue mergeQueueEntry{position} autoMergeRequest{mergeMethod enabledBy{login}} commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){nodes{__typename ... on CheckRun{name isRequired(pullRequestNumber:$number)} ... on StatusContext{context isRequired(pullRequestNumber:$number)}}}}}}} reviewThreads(first:100){nodes{id isResolved isOutdated path line originalLine viewerCanReply viewerCanResolve viewerCanUnresolve comments(first:50){nodes{id author{login} body createdAt url}}}}}}}";
const REPLY: &str = "mutation($thread:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$thread,body:$body}){comment{id}}}";
const RESOLVE: &str = "mutation($thread:ID!){resolveReviewThread(input:{threadId:$thread}){thread{isResolved}}}";
const UNRESOLVE: &str = "mutation($thread:ID!){unresolveReviewThread(input:{threadId:$thread}){thread{isResolved}}}";

/// What kind of failure the page shows.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// `gh` is not installed or not found.
    Unavailable,
    /// `gh` is not signed in.
    SignedOut,
    /// GitHub did not answer before the deadline.
    Timeout,
    /// The session's branch has no pull request.
    NotFound,
    /// The session has no folder on this computer.
    Session,
    /// GitHub answered with something the page cannot read.
    Invalid,
    /// The merge is blocked: a draft, a conflict, a review or a check.
    Blocked,
    /// The pull request changed after the person confirmed.
    Changed,
    /// The person's input is not one GitHub takes, such as an empty reply.
    Input,
    /// GitHub refused a write.
    Refused,
    /// Any other failure of a read, in `gh`'s words.
    Github,
}

/// A failure, as the page shows it: its kind, and one or two sentences.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PullError {
    pub kind: ErrorKind,
    pub message: String,
}

impl PullError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// A failed `gh` command. `refused` says what GitHub refused, for a write; None for a read.
    fn from_failure(failure: GhFailure, deadline: Duration, refused: Option<&str>) -> Self {
        match failure {
            GhFailure::Late if refused.is_some() => Self::new(
                ErrorKind::Timeout,
                format!(
                    "GitHub did not answer within {} seconds. Look at the pull request before you try again: it may have gone through.",
                    deadline.as_secs()
                ),
            ),
            GhFailure::Late => Self::new(
                ErrorKind::Timeout,
                format!("GitHub did not answer within {} seconds.", deadline.as_secs()),
            ),
            GhFailure::Missing => Self::new(ErrorKind::Unavailable, pull_request::UNAVAILABLE),
            GhFailure::Failed(text) => Self::from_text(text, refused),
        }
    }

    /// `gh`'s own words, sorted by what they say.
    fn from_text(text: String, refused: Option<&str>) -> Self {
        let lower = text.to_lowercase();
        if lower.contains("gh auth login") || lower.contains("authentication") || lower.contains("http 401") {
            return Self::new(
                ErrorKind::SignedOut,
                "The GitHub CLI is not signed in. Run gh auth login, then try again.",
            );
        }
        if lower.contains("no pull requests found") {
            return Self::new(ErrorKind::NotFound, "This session's branch has no pull request.");
        }
        let text = if text.is_empty() {
            "GitHub gave no reason.".to_string()
        } else {
            text
        };
        match refused {
            Some(what) => Self::new(ErrorKind::Refused, format!("GitHub refused {what}: {text}")),
            None => Self::new(ErrorKind::Github, text),
        }
    }

    /// What `pull_request::read_with` said, which is plain text.
    fn from_read(text: String) -> Self {
        if text == pull_request::UNAVAILABLE {
            return Self::new(ErrorKind::Unavailable, text);
        }
        if text.starts_with("GitHub did not answer") {
            return Self::new(ErrorKind::Timeout, text);
        }
        if text.starts_with("GitHub returned") || text.starts_with("GitHub did not report") {
            return Self::new(ErrorKind::Invalid, text);
        }
        Self::from_text(text, None)
    }
}

/// The page: the pull request as the watch reads it, and what the page adds.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PullPage {
    pub status: PullRequestStatus,
    #[serde(flatten)]
    pub details: Details,
    pub merge: Merge,
}

/// What the GraphQL query reads.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Details {
    /// The head commit: a merge goes through only while it is still the head.
    pub head: String,
    /// GitHub's `mergeStateStatus`: `CLEAN`, `BLOCKED`, `BEHIND`, `UNSTABLE`, `DIRTY`, `DRAFT` or `UNKNOWN`.
    pub merge_state: String,
    /// The merge methods the repository allows, in GitHub's order: `merge`, `squash`, `rebase`.
    pub methods: Vec<String>,
    /// The method GitHub offers the signed-in person first, when the repository allows it.
    pub default_method: Option<String>,
    pub auto_allowed: bool,
    pub auto_merge: Option<AutoMerge>,
    /// The base branch merges through a merge queue.
    pub queue: bool,
    pub in_queue: bool,
    pub queue_position: Option<u64>,
    /// The checks a merge needs, by name.
    pub required: Vec<String>,
    pub threads: Vec<ReviewThread>,
}

/// Auto-merge is on: it merges when its checks pass.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutoMerge {
    pub method: String,
    pub by: String,
}

/// A review thread: comments on one place in the diff.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub resolved: bool,
    /// The lines it is on changed since.
    pub outdated: bool,
    pub can_reply: bool,
    pub can_resolve: bool,
    pub can_unresolve: bool,
    pub comments: Vec<ThreadComment>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: Option<String>,
    pub url: Option<String>,
}

/// What the merge button does.
#[derive(Debug, Clone, Copy, Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MergeAction {
    /// Merge now.
    Merge,
    /// Turn on auto-merge: it merges when the required checks pass.
    Auto,
    /// Add it to the merge queue (or, with checks still running, let it join when they pass).
    Queue,
}

/// The merge button: what it does, or why it is disabled.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Merge {
    /// None: disabled. `blocked` says why, unless the pull request is closed, merged, queued or set to auto-merge.
    pub action: Option<MergeAction>,
    /// The method the button uses: the person's default when allowed, else the repository's first.
    pub method: Option<String>,
    pub blocked: Vec<String>,
}

/// The repository's host, owner and name, from the pull request's address.
pub(crate) fn repo_of(url: &str) -> Option<(&str, &str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let mut parts = rest.split('/');
    let (host, owner, name, kind) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    (kind == "pull" && !host.is_empty() && !owner.is_empty() && !name.is_empty()).then_some((host, owner, name))
}

/// Reads the GraphQL answer.
pub fn parse_details(value: &Value) -> Result<Details, String> {
    let repo = &value["data"]["repository"];
    let pr = &repo["pullRequest"];
    let head = pr["headRefOid"]
        .as_str()
        .filter(|head| !head.is_empty())
        .ok_or("GitHub did not report the pull request's head commit.")?;
    let methods: Vec<String> = [
        ("mergeCommitAllowed", "merge"),
        ("squashMergeAllowed", "squash"),
        ("rebaseMergeAllowed", "rebase"),
    ]
    .into_iter()
    .filter(|(key, _)| repo[*key].as_bool() == Some(true))
    .map(|(_, method)| method.to_string())
    .collect();
    let default_method = repo["viewerDefaultMergeMethod"]
        .as_str()
        .map(str::to_lowercase)
        .filter(|method| methods.contains(method));
    let auto_merge = pr["autoMergeRequest"].as_object().map(|auto| AutoMerge {
        method: auto
            .get("mergeMethod")
            .and_then(Value::as_str)
            .unwrap_or("MERGE")
            .to_lowercase(),
        by: auto
            .get("enabledBy")
            .and_then(|by| by["login"].as_str())
            .unwrap_or("someone")
            .to_string(),
    });
    let contexts = pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"]
        .as_array()
        .into_iter()
        .flatten();
    let required = contexts
        .filter(|context| context["isRequired"].as_bool() == Some(true))
        .filter_map(|context| context["name"].as_str().or_else(|| context["context"].as_str()))
        .map(String::from)
        .collect();
    let threads = pr["reviewThreads"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|thread| {
            Some(ReviewThread {
                id: thread["id"].as_str().filter(|id| !id.is_empty())?.to_string(),
                path: thread["path"].as_str().map(String::from),
                line: thread["line"].as_u64().or_else(|| thread["originalLine"].as_u64()),
                resolved: thread["isResolved"].as_bool().unwrap_or(false),
                outdated: thread["isOutdated"].as_bool().unwrap_or(false),
                can_reply: thread["viewerCanReply"].as_bool().unwrap_or(false),
                can_resolve: thread["viewerCanResolve"].as_bool().unwrap_or(false),
                can_unresolve: thread["viewerCanUnresolve"].as_bool().unwrap_or(false),
                comments: thread["comments"]["nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|comment| ThreadComment {
                        id: comment["id"].as_str().unwrap_or_default().to_string(),
                        author: comment["author"]["login"].as_str().unwrap_or("someone").to_string(),
                        body: comment["body"].as_str().unwrap_or_default().trim().to_string(),
                        created_at: comment["createdAt"].as_str().map(String::from),
                        url: comment["url"].as_str().map(String::from),
                    })
                    .collect(),
            })
        })
        .collect();
    Ok(Details {
        head: head.to_string(),
        merge_state: pr["mergeStateStatus"].as_str().unwrap_or("UNKNOWN").to_string(),
        methods,
        default_method,
        auto_allowed: repo["autoMergeAllowed"].as_bool().unwrap_or(false),
        auto_merge,
        queue: pr["isMergeQueueEnabled"].as_bool().unwrap_or(false),
        in_queue: pr["isInMergeQueue"].as_bool().unwrap_or(false),
        queue_position: pr["mergeQueueEntry"]["position"].as_u64(),
        required,
        threads,
    })
}

/// What the merge button does, or why it cannot.
pub fn plan(status: &PullRequestStatus, details: &Details) -> Merge {
    let method = details
        .default_method
        .clone()
        .or_else(|| details.methods.first().cloned());
    let mut merge = Merge {
        action: None,
        method,
        blocked: Vec::new(),
    };
    // Closed, merged or queued: nothing to merge. Auto-merge on: it merges on its own, and the page offers to stop it.
    if status.state != "OPEN" || details.in_queue {
        return merge;
    }
    let required = |name: &str| details.required.iter().any(|required| required == name);
    let blocked = &mut merge.blocked;
    if status.draft {
        blocked.push("It is a draft. Mark it ready for review first.".into());
    }
    if status.mergeable == "CONFLICTING" {
        blocked.push("The branch has conflicts with its base branch.".into());
    }
    match status.review_decision.as_deref() {
        Some("CHANGES_REQUESTED") => blocked.push("A reviewer asked for changes.".into()),
        Some("REVIEW_REQUIRED") => blocked.push("It needs an approving review.".into()),
        _ => {}
    }
    let failed: Vec<&str> = status
        .checks
        .iter()
        .filter(|check| check.state == "failure" && required(&check.name))
        .map(|check| check.name.as_str())
        .collect();
    match failed.as_slice() {
        [] => {}
        [one] => blocked.push(format!("The required check \u{201c}{one}\u{201d} failed.")),
        many => blocked.push(format!("{} required checks failed.", many.len())),
    }
    if details.methods.is_empty() {
        blocked.push("The repository allows no merge method.".into());
    }
    if !details.queue && details.merge_state == "BEHIND" {
        blocked.push("The branch is behind its base branch. Update it first.".into());
    }
    if blocked.is_empty() && status.mergeable == "UNKNOWN" {
        blocked.push("GitHub is still checking whether it can merge. Try again in a moment.".into());
    }
    if !blocked.is_empty() || details.auto_merge.is_some() {
        return merge;
    }
    let running = status
        .checks
        .iter()
        .any(|check| check.state == "pending" && required(&check.name));
    // BLOCKED with no reason above: a required check has not reported yet, or a rule waits on one.
    let waiting = running || details.merge_state == "BLOCKED";
    merge.action = if details.queue {
        Some(MergeAction::Queue)
    } else if !waiting {
        Some(MergeAction::Merge)
    } else if details.auto_allowed {
        Some(MergeAction::Auto)
    } else {
        merge.blocked.push(if running {
            "Required checks are still running, and the repository does not allow auto-merge.".into()
        } else {
            "GitHub's branch rules block the merge.".into()
        });
        None
    };
    merge
}

/// `gh` for one session: the program, the session's folder and `PATH`, and the deadlines.
pub struct Gh<'a> {
    pub program: &'a OsStr,
    pub folder: &'a Path,
    pub path: &'a str,
    pub target: &'a str,
    pub read: Duration,
    pub write: Duration,
}

impl Gh<'_> {
    pub(crate) async fn run(
        &self,
        args: &[&str],
        deadline: Duration,
        refused: Option<&str>,
    ) -> Result<Vec<u8>, PullError> {
        pull_request::run(self.program, self.folder, self.path, args, deadline)
            .await
            .map_err(|failure| PullError::from_failure(failure, deadline, refused))
    }

    async fn status(&self) -> Result<PullRequestStatus, PullError> {
        pull_request::read_with(
            Host::GitHub,
            self.program,
            self.folder,
            self.path,
            Some(self.target),
            self.read,
        )
        .await
        .map_err(PullError::from_read)
    }

    /// Runs a GraphQL query or mutation against the pull request's host.
    async fn graphql(
        &self,
        url: &str,
        query: &str,
        fields: &[(&str, &str)],
        deadline: Duration,
        refused: Option<&str>,
    ) -> Result<Value, PullError> {
        let (host, ..) = repo_of(url)
            .ok_or_else(|| PullError::new(ErrorKind::Invalid, "GitHub reported an address this app cannot read."))?;
        let fields: Vec<String> = fields.iter().map(|(key, value)| format!("{key}={value}")).collect();
        let mut args = vec!["api", "graphql", "--hostname", host];
        for field in &fields {
            // Raw strings: an owner named `true` or a body that starts with `@` stays text.
            args.extend(["-f", field.as_str()]);
        }
        let query = format!("query={query}");
        args.extend(["-f", query.as_str()]);
        let stdout = self.run(&args, deadline, refused).await?;
        serde_json::from_slice(&stdout)
            .map_err(|_| PullError::new(ErrorKind::Invalid, "GitHub returned an invalid response."))
    }

    pub async fn page(&self) -> Result<PullPage, PullError> {
        let status = self.status().await?;
        let (host, owner, name) = repo_of(&status.url)
            .ok_or_else(|| PullError::new(ErrorKind::Invalid, "GitHub reported an address this app cannot read."))?;
        // `number` must be an integer: it alone goes as a typed field.
        let number = format!("number={}", status.number);
        let (owner, name) = (format!("owner={owner}"), format!("name={name}"));
        let query = format!("query={DETAILS}");
        let args = [
            "api",
            "graphql",
            "--hostname",
            host,
            "-f",
            &owner,
            "-f",
            &name,
            "-F",
            &number,
            "-f",
            &query,
        ];
        let stdout = self.run(&args, self.read, None).await?;
        let value: Value = serde_json::from_slice(&stdout)
            .map_err(|_| PullError::new(ErrorKind::Invalid, "GitHub returned an invalid response."))?;
        let details = parse_details(&value).map_err(|message| PullError::new(ErrorKind::Invalid, message))?;
        let merge = plan(&status, &details);
        Ok(PullPage { status, details, merge })
    }

    pub async fn reply(&self, thread: &str, body: &str) -> Result<(), PullError> {
        let body = body.trim();
        if body.is_empty() {
            return Err(PullError::new(ErrorKind::Input, "Write a reply first."));
        }
        if body.chars().count() > MAX_REPLY {
            return Err(PullError::new(ErrorKind::Input, "The reply is too long for GitHub."));
        }
        let status = self.status().await?;
        self.graphql(
            &status.url,
            REPLY,
            &[("thread", thread), ("body", body)],
            self.write,
            Some("the reply"),
        )
        .await
        .map(drop)
    }

    pub async fn resolve(&self, thread: &str, resolved: bool) -> Result<(), PullError> {
        let status = self.status().await?;
        let (query, what) = if resolved {
            (RESOLVE, "to resolve the thread")
        } else {
            (UNRESOLVE, "to reopen the thread")
        };
        self.graphql(&status.url, query, &[("thread", thread)], self.write, Some(what))
            .await
            .map(drop)
    }

    /// Merges what the person confirmed: `head` with `method`, as `action`. Reads the pull request again first.
    pub async fn merge(&self, head: &str, method: &str, action: MergeAction) -> Result<(), PullError> {
        let page = self.page().await?;
        if page.status.state != "OPEN" {
            return Err(PullError::new(
                ErrorKind::Changed,
                format!("The pull request is {} already.", page.status.state.to_lowercase()),
            ));
        }
        let Some(now) = page.merge.action else {
            let why = match page.merge.blocked.as_slice() {
                [] if page.details.in_queue => "It is in the merge queue already.".to_string(),
                [] if page.details.auto_merge.is_some() => "Auto-merge is on already.".to_string(),
                [] => "It cannot be merged now.".to_string(),
                reasons => reasons.join(" "),
            };
            return Err(PullError::new(ErrorKind::Blocked, why));
        };
        if page.details.head != head {
            return Err(PullError::new(
                ErrorKind::Changed,
                "New commits arrived after you confirmed. Look at them, then merge again.",
            ));
        }
        if now != action {
            return Err(PullError::new(
                ErrorKind::Changed,
                "The pull request's checks changed after you confirmed. Look again, then merge.",
            ));
        }
        if !page.details.methods.iter().any(|allowed| allowed == method) {
            return Err(PullError::new(
                ErrorKind::Blocked,
                format!("The repository does not allow {method} merges."),
            ));
        }
        let flag = format!("--{method}");
        let mut args = vec![
            "pr",
            "merge",
            page.status.url.as_str(),
            flag.as_str(),
            "--match-head-commit",
            head,
        ];
        if action == MergeAction::Auto {
            args.push("--auto");
        }
        self.run(&args, self.write, Some("the merge")).await.map(drop)
    }

    pub async fn auto_off(&self) -> Result<(), PullError> {
        let status = self.status().await?;
        self.run(
            &["pr", "merge", status.url.as_str(), "--disable-auto"],
            self.write,
            Some("to turn off auto-merge"),
        )
        .await
        .map(drop)
    }
}
/// Said for a session in a GitLab repository: the page is GitHub's only.
pub const GITLAB: &str = "This page shows GitHub pull requests only. Open the merge request on GitLab.";

/// Where a session's page reads and writes: its folder, the `PATH` that finds `gh`, and its pull request.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub folder: PathBuf,
    pub path: String,
    pub target: String,
}

impl Session {
    /// `gh` for this session, with the page's deadlines.
    pub fn gh(&self) -> Gh<'_> {
        Gh {
            program: OsStr::new("gh"),
            folder: &self.folder,
            path: &self.path,
            target: &self.target,
            read: READ_DEADLINE,
            write: WRITE_DEADLINE,
        }
    }
}

/// A session at `folder` whose pull request is `target`. Refuses a GitLab repository's session ([`GITLAB`]) and one
/// with no linked pull request.
pub fn locate(folder: PathBuf, path: String, target: Option<String>) -> Result<Session, PullError> {
    // The page reads and writes through GitHub's GraphQL API; a GitLab merge request is watched, not shown here yet.
    if Host::of_repo(&folder) == Host::GitLab {
        return Err(PullError::new(ErrorKind::Unavailable, GITLAB));
    }
    let target =
        target.ok_or_else(|| PullError::new(ErrorKind::NotFound, "This session has no linked pull request."))?;
    Ok(Session { folder, path, target })
}

/// The session's folder and `PATH`, as the watch reads them, and the pull request the watch links to it.
pub fn session(shell: &impl Shell, thread_id: &str) -> Result<Session, PullError> {
    let folder = shell
        .folder(thread_id)
        .map_err(|message| PullError::new(ErrorKind::Session, message))?;
    let target = crate::watch::linked_url(shell, thread_id);
    locate(folder, shell.agent_path(), target)
}

/// [`session`] for `url`, one of the session's pull requests (the page's picker); None: the one the card shows. A URL
/// that is not one of the session's is refused: the page merges and replies with the person's `gh` sign-in.
pub fn session_for(shell: &impl Shell, thread_id: &str, url: Option<&str>) -> Result<Session, PullError> {
    let Some(url) = url else {
        return session(shell, thread_id);
    };
    if !crate::watch::session_pulls(shell, thread_id)
        .iter()
        .any(|known| known == url)
    {
        return Err(PullError::new(
            ErrorKind::NotFound,
            "That pull request is not one of this session's.",
        ));
    }
    let folder = shell
        .folder(thread_id)
        .map_err(|message| PullError::new(ErrorKind::Session, message))?;
    locate(folder, shell.agent_path(), Some(url.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::Check;
    use serde_json::json;

    fn check(name: &str, state: &str) -> Check {
        Check {
            name: name.into(),
            state: state.into(),
            url: None,
        }
    }

    fn status(checks: Vec<Check>) -> PullRequestStatus {
        PullRequestStatus {
            number: 42,
            title: "Sample".into(),
            url: "https://github.com/example/repo/pull/42".into(),
            draft: false,
            state: "OPEN".into(),
            checks,
            review_decision: None,
            reviews: Vec::new(),
            mergeable: "MERGEABLE".into(),
            host: Default::default(),
            head_branch: String::new(),
            base_branch: String::new(),
            comments: Vec::new(),
        }
    }

    fn details() -> Details {
        Details {
            head: "abc123".into(),
            merge_state: "CLEAN".into(),
            methods: vec!["squash".into()],
            default_method: Some("squash".into()),
            auto_allowed: true,
            required: vec!["rust".into()],
            ..Details::default()
        }
    }

    #[test]
    fn a_clean_pull_request_merges_with_the_repositorys_method() {
        let merge = plan(&status(vec![check("rust", "success")]), &details());
        assert_eq!(merge.action, Some(MergeAction::Merge));
        assert_eq!(merge.method.as_deref(), Some("squash"));
        assert!(merge.blocked.is_empty());
    }

    #[test]
    fn each_blocked_state_disables_the_merge_and_says_why() {
        let mut draft = status(vec![]);
        draft.draft = true;
        let mut conflicting = status(vec![]);
        conflicting.mergeable = "CONFLICTING".into();
        let mut changes = status(vec![]);
        changes.review_decision = Some("CHANGES_REQUESTED".into());
        let failed = status(vec![check("rust", "failure")]);
        for (pull, reason) in [
            (draft, "It is a draft. Mark it ready for review first."),
            (conflicting, "The branch has conflicts with its base branch."),
            (changes, "A reviewer asked for changes."),
            (failed, "The required check \u{201c}rust\u{201d} failed."),
        ] {
            let merge = plan(&pull, &details());
            assert_eq!(merge.action, None, "{reason}");
            assert_eq!(merge.blocked, [reason]);
        }
    }

    #[test]
    fn a_failed_check_that_is_not_required_does_not_block() {
        let merge = plan(&status(vec![check("lint", "failure")]), &details());
        assert_eq!(merge.action, Some(MergeAction::Merge));
    }

    #[test]
    fn running_required_checks_turn_on_auto_merge_when_the_repository_allows_it() {
        let pull = status(vec![check("rust", "pending")]);
        assert_eq!(plan(&pull, &details()).action, Some(MergeAction::Auto));
        let no_auto = Details {
            auto_allowed: false,
            ..details()
        };
        let merge = plan(&pull, &no_auto);
        assert_eq!(merge.action, None);
        assert_eq!(merge.blocked.len(), 1);
    }

    #[test]
    fn a_merge_queue_takes_the_pull_request_even_behind_its_base() {
        let queued = Details {
            queue: true,
            merge_state: "BEHIND".into(),
            ..details()
        };
        assert_eq!(plan(&status(vec![]), &queued).action, Some(MergeAction::Queue));
        let behind = Details {
            merge_state: "BEHIND".into(),
            ..details()
        };
        assert_eq!(plan(&status(vec![]), &behind).action, None);
    }

    #[test]
    fn auto_merge_on_or_a_place_in_the_queue_leaves_nothing_to_press_and_nothing_blocked() {
        let auto = Details {
            auto_merge: Some(AutoMerge {
                method: "squash".into(),
                by: "alice".into(),
            }),
            ..details()
        };
        let in_queue = Details {
            queue: true,
            in_queue: true,
            ..details()
        };
        for details in [auto, in_queue] {
            let merge = plan(&status(vec![]), &details);
            assert_eq!(merge.action, None);
            assert!(merge.blocked.is_empty());
        }
    }

    #[test]
    fn a_repository_with_no_merge_method_blocks_it() {
        let none = Details {
            methods: vec![],
            default_method: None,
            ..details()
        };
        assert_eq!(
            plan(&status(vec![]), &none).blocked,
            ["The repository allows no merge method."]
        );
    }

    #[test]
    fn reads_the_allowed_methods_the_threads_and_the_required_checks() {
        let details = parse_details(&json!({"data":{"repository":{
            "mergeCommitAllowed":false,"squashMergeAllowed":true,"rebaseMergeAllowed":false,"autoMergeAllowed":true,
            "viewerDefaultMergeMethod":"MERGE",
            "pullRequest":{"headRefOid":"abc123","mergeStateStatus":"BLOCKED","isMergeQueueEnabled":false,"isInMergeQueue":false,
                "mergeQueueEntry":null,"autoMergeRequest":{"mergeMethod":"SQUASH","enabledBy":{"login":"alice"}},
                "commits":{"nodes":[{"commit":{"statusCheckRollup":{"contexts":{"nodes":[
                    {"__typename":"CheckRun","name":"rust","isRequired":true},
                    {"__typename":"StatusContext","context":"deploy","isRequired":true},
                    {"__typename":"CheckRun","name":"lint","isRequired":false}]}}}}]},
                "reviewThreads":{"nodes":[{"id":"PRRT_1","isResolved":false,"isOutdated":true,"path":"src/a.rs","line":null,
                    "originalLine":12,"viewerCanReply":true,"viewerCanResolve":true,"viewerCanUnresolve":false,
                    "comments":{"nodes":[{"id":"PRRC_1","author":{"login":"bob"},"body":" Rename it. ","createdAt":"2026-10-30T10:00:00Z","url":"https://github.com/c/1"}]}}]}}}}}))
        .expect("details");
        assert_eq!(details.methods, ["squash"]);
        // The person's default is a merge commit, which this repository does not allow.
        assert_eq!(details.default_method, None);
        assert_eq!(details.required, ["rust", "deploy"]);
        assert_eq!(
            details.auto_merge.as_ref().map(|auto| auto.method.as_str()),
            Some("squash")
        );
        let thread = &details.threads[0];
        assert_eq!((thread.line, thread.outdated, thread.can_reply), (Some(12), true, true));
        assert_eq!(thread.comments[0].body, "Rename it.");
    }

    #[test]
    fn refuses_details_without_a_head_commit() {
        assert!(parse_details(&json!({"data":{"repository":{"pullRequest":null}}})).is_err());
    }

    #[test]
    fn reads_the_repository_from_the_address() {
        assert_eq!(
            repo_of("https://github.com/example/repo/pull/42"),
            Some(("github.com", "example", "repo"))
        );
        assert_eq!(repo_of("https://github.com/example/repo/issues/42"), None);
        assert_eq!(repo_of("http://github.com/example/repo/pull/42"), None);
    }

    #[test]
    fn sorts_gh_errors_into_kinds() {
        let signed_out = PullError::from_failure(
            GhFailure::Failed("To get started with GitHub CLI, please run:  gh auth login".into()),
            WRITE_DEADLINE,
            Some("the merge"),
        );
        assert_eq!(signed_out.kind, ErrorKind::SignedOut);
        let late = PullError::from_failure(GhFailure::Late, WRITE_DEADLINE, Some("the merge"));
        assert_eq!(late.kind, ErrorKind::Timeout);
        assert!(late.message.contains("30 seconds") && late.message.contains("may have gone through"));
        assert_eq!(
            PullError::from_read(pull_request::UNAVAILABLE.into()).kind,
            ErrorKind::Unavailable
        );
        assert_eq!(
            PullError::from_read("no pull requests found for branch \"x\"".into()).kind,
            ErrorKind::NotFound
        );
    }

    #[test]
    fn a_gitlab_session_or_one_with_no_pull_request_has_no_page() {
        let folder = std::env::temp_dir().join(format!("nexus-pull-locate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(folder.join(".git")).expect("folder");
        std::fs::write(folder.join(".git/config"), "[nexus]\n\thost = gitlab\n").expect("config");
        let gitlab = locate(
            folder.clone(),
            String::new(),
            Some("https://gitlab.com/a/b/-/merge_requests/1".into()),
        );
        assert_eq!(gitlab, Err(PullError::new(ErrorKind::Unavailable, GITLAB)));
        std::fs::write(folder.join(".git/config"), "[nexus]\n\thost = github\n").expect("config");
        let none = locate(folder.clone(), String::new(), None).expect_err("no pull request");
        assert_eq!(none.kind, ErrorKind::NotFound);
        let found = locate(
            folder.clone(),
            "/bin".into(),
            Some("https://github.com/a/b/pull/1".into()),
        )
        .expect("found");
        assert_eq!(found.gh().target, "https://github.com/a/b/pull/1");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(unix)]
    mod with_fake_gh {
        use super::*;

        /// A fake `gh` that answers by its arguments and logs each call, one per line, to `$0.log`. Builtins only:
        /// it runs with an empty `PATH`.
        struct FakeGh {
            folder: PathBuf,
            program: PathBuf,
        }

        impl FakeGh {
            /// Each argument is the shell snippet that answers: `gh pr view`, the page's GraphQL query, `gh pr merge`.
            fn new(view: &str, details: &str, merge: &str) -> Self {
                let folder = std::env::temp_dir().join(format!("nexus-pull-page-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&folder).expect("fake CLI folder");
                let program = folder.join("gh");
                let script = format!(
                    r#"#!/bin/sh
[ -n "$FAKE_GH_PROBE" ] && exit 0
printf '%s\n' "$*" >> "$0.log"
case "$1 $2" in
"pr view") {view} ;;
"api graphql")
  case "$*" in
  *addPullRequestReviewThreadReply*) printf '%s' '{{"data":{{}}}}' ;;
  *unresolveReviewThread*) printf '%s' '{{"data":{{}}}}' ;;
  *resolveReviewThread*) printf 'gh: Resource not accessible by integration' >&2; exit 1 ;;
  *) {details} ;;
  esac ;;
"pr merge") {merge} ;;
*) exit 9 ;;
esac
"#
                );
                nexus_workbench::process::write_fake_script(&program, &script);
                // A test thread that forks while the script is still open for writing passes the handle to its child,
                // and running the script then fails with ETXTBSY ("Text file busy"). Start it once, uncounted, until it
                // runs: after that no writer is left.
                for _ in 0..200 {
                    match nexus_workbench::process::command(&program)
                        .env("FAKE_GH_PROBE", "1")
                        .status()
                    {
                        Err(error) if error.raw_os_error() == Some(26) => std::thread::sleep(Duration::from_millis(5)),
                        started => {
                            started.expect("the fake CLI starts");
                            break;
                        }
                    }
                }
                Self { folder, program }
            }

            fn gh(&self) -> Gh<'_> {
                Gh {
                    program: self.program.as_os_str(),
                    folder: &self.folder,
                    path: "",
                    target: "https://github.com/example/repo/pull/42",
                    read: Duration::from_secs(5),
                    write: Duration::from_secs(5),
                }
            }

            /// The calls it got, without the page's reads.
            fn writes(&self) -> Vec<String> {
                std::fs::read_to_string(self.folder.join("gh.log"))
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| !line.starts_with("pr view") && !line.contains("reviewThreads(first"))
                    .map(String::from)
                    .collect()
            }
        }

        impl Drop for FakeGh {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.folder);
            }
        }

        fn answer(value: &Value) -> String {
            format!("printf '%s' '{value}'")
        }

        fn view(checks: Value, review: Option<&str>) -> String {
            answer(
                &json!({"number":42,"title":"Sample","url":"https://github.com/example/repo/pull/42","state":"OPEN","isDraft":false,
                "statusCheckRollup":checks,"reviewDecision":review,"latestReviews":[],"mergeable":"MERGEABLE"}),
            )
        }

        fn green() -> String {
            view(
                json!([{"name":"rust","status":"COMPLETED","conclusion":"SUCCESS"}]),
                Some("APPROVED"),
            )
        }

        fn graph(merge_state: &str) -> String {
            answer(
                &json!({"data":{"repository":{"mergeCommitAllowed":false,"squashMergeAllowed":true,"rebaseMergeAllowed":false,
                "autoMergeAllowed":true,"viewerDefaultMergeMethod":"SQUASH",
                "pullRequest":{"headRefOid":"abc123","mergeStateStatus":merge_state,"isMergeQueueEnabled":false,"isInMergeQueue":false,
                "commits":{"nodes":[{"commit":{"statusCheckRollup":{"contexts":{"nodes":[
                    {"__typename":"CheckRun","name":"rust","isRequired":true}]}}}}]},
                "reviewThreads":{"nodes":[{"id":"PRRT_1","isResolved":false,"isOutdated":false,"path":"src/a.rs","line":3,
                    "viewerCanReply":true,"viewerCanResolve":true,"viewerCanUnresolve":false,
                    "comments":{"nodes":[{"id":"PRRC_1","author":{"login":"bob"},"body":"Why?"}]}}]}}}}}),
            )
        }

        #[tokio::test]
        async fn reads_the_page_with_its_threads_and_the_repositorys_method() {
            let gh = FakeGh::new(&green(), &graph("CLEAN"), "exit 0");
            let page = gh.gh().page().await.expect("page");
            assert_eq!(page.details.threads[0].comments[0].author, "bob");
            assert_eq!(page.merge.action, Some(MergeAction::Merge));
            assert_eq!(page.merge.method.as_deref(), Some("squash"));
            let log = std::fs::read_to_string(gh.folder.join("gh.log")).expect("log");
            assert!(log.contains("pr view https://github.com/example/repo/pull/42 --json"));
            assert!(log.contains("api graphql --hostname github.com -f owner=example -f name=repo -F number=42"));
        }

        #[tokio::test]
        async fn merges_the_confirmed_head_with_the_allowed_method() {
            let gh = FakeGh::new(&green(), &graph("CLEAN"), "exit 0");
            gh.gh()
                .merge("abc123", "squash", MergeAction::Merge)
                .await
                .expect("merged");
            assert_eq!(
                gh.writes(),
                ["pr merge https://github.com/example/repo/pull/42 --squash --match-head-commit abc123"]
            );
        }

        #[tokio::test]
        async fn turns_on_auto_merge_while_a_required_check_runs() {
            let running = view(json!([{"name":"rust","status":"IN_PROGRESS"}]), Some("APPROVED"));
            let gh = FakeGh::new(&running, &graph("BLOCKED"), "exit 0");
            assert_eq!(
                gh.gh().page().await.expect("page").merge.action,
                Some(MergeAction::Auto)
            );
            gh.gh()
                .merge("abc123", "squash", MergeAction::Auto)
                .await
                .expect("auto-merge on");
            assert_eq!(
                gh.writes(),
                ["pr merge https://github.com/example/repo/pull/42 --squash --match-head-commit abc123 --auto"]
            );
        }

        #[tokio::test]
        async fn refuses_a_blocked_merge_without_asking_github() {
            let failing = view(
                json!([{"name":"rust","status":"COMPLETED","conclusion":"FAILURE"}]),
                Some("CHANGES_REQUESTED"),
            );
            let gh = FakeGh::new(&failing, &graph("BLOCKED"), "exit 0");
            let error = gh
                .gh()
                .merge("abc123", "squash", MergeAction::Merge)
                .await
                .expect_err("blocked");
            assert_eq!(error.kind, ErrorKind::Blocked);
            assert_eq!(
                error.message,
                "A reviewer asked for changes. The required check \u{201c}rust\u{201d} failed."
            );
            assert!(gh.writes().is_empty());
        }

        #[tokio::test]
        async fn refuses_a_head_that_moved_or_a_method_the_repository_does_not_allow() {
            let gh = FakeGh::new(&green(), &graph("CLEAN"), "exit 0");
            let moved = gh
                .gh()
                .merge("old456", "squash", MergeAction::Merge)
                .await
                .expect_err("moved");
            assert_eq!(moved.kind, ErrorKind::Changed);
            let rebase = gh
                .gh()
                .merge("abc123", "rebase", MergeAction::Merge)
                .await
                .expect_err("not allowed");
            assert_eq!(rebase.kind, ErrorKind::Blocked);
            assert_eq!(rebase.message, "The repository does not allow rebase merges.");
            assert!(gh.writes().is_empty());
        }

        #[tokio::test]
        async fn shows_githubs_reason_when_it_refuses_the_merge() {
            let gh = FakeGh::new(
                &green(),
                &graph("CLEAN"),
                "printf 'X Pull request example/repo#42 is not mergeable: the base branch policy prohibits the merge.' >&2; exit 1",
            );
            let error = gh
                .gh()
                .merge("abc123", "squash", MergeAction::Merge)
                .await
                .expect_err("refused");
            assert_eq!(error.kind, ErrorKind::Refused);
            assert_eq!(
                error.message,
                "GitHub refused the merge: X Pull request example/repo#42 is not mergeable: the base branch policy prohibits the merge."
            );
        }

        #[tokio::test]
        async fn bounds_a_merge_that_does_not_finish() {
            let gh = FakeGh::new(&green(), &graph("CLEAN"), "exec /bin/sleep 30");
            let mut slow = gh.gh();
            slow.write = Duration::from_millis(200);
            let error = slow
                .merge("abc123", "squash", MergeAction::Merge)
                .await
                .expect_err("late");
            assert_eq!(error.kind, ErrorKind::Timeout);
        }

        #[tokio::test]
        async fn replies_to_a_thread_and_refuses_an_empty_reply() {
            let gh = FakeGh::new(&green(), &graph("CLEAN"), "exit 0");
            gh.gh().reply("PRRT_1", "  Renamed it.  ").await.expect("replied");
            let writes = gh.writes();
            assert_eq!(writes.len(), 1);
            assert!(writes[0].starts_with(
                "api graphql --hostname github.com -f thread=PRRT_1 -f body=Renamed it. -f query=mutation"
            ));
            let empty = gh.gh().reply("PRRT_1", "   ").await.expect_err("empty");
            assert_eq!(empty.kind, ErrorKind::Input);
            assert_eq!(gh.writes().len(), 1);
        }

        #[tokio::test]
        async fn shows_githubs_reason_when_it_refuses_to_resolve() {
            let gh = FakeGh::new(&green(), &graph("CLEAN"), "exit 0");
            let error = gh.gh().resolve("PRRT_1", true).await.expect_err("refused");
            assert_eq!(error.kind, ErrorKind::Refused);
            assert_eq!(
                error.message,
                "GitHub refused to resolve the thread: gh: Resource not accessible by integration"
            );
            gh.gh().resolve("PRRT_1", false).await.expect("reopened");
        }

        #[tokio::test]
        async fn a_signed_out_cli_is_its_own_kind() {
            let signed_out = "printf 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4";
            let gh = FakeGh::new(signed_out, &graph("CLEAN"), "exit 0");
            let error = gh.gh().page().await.expect_err("signed out");
            assert_eq!(error.kind, ErrorKind::SignedOut);
        }
    }
}
