//! A GitLab merge request, read through `glab` into the watch's [`PullRequestStatus`]: `glab mr view`
//! for its state, pipeline and mergeability, then its discussions (`glab api`) for what reviewers asked.
//!
//! GitLab has no review that asks for changes the way GitHub has. A discussion a reviewer opened and nobody resolved
//! stands in for one: each unresolved thread is a `CHANGES_REQUESTED` review, told once, by its own id. A plain comment
//! is a comment. The merge request's author's own notes are left out, as GitHub leaves out the signed-in person's: the
//! watched merge request is usually the person's own, and `glab` does not say who is signed in without one more call.
//! The head pipeline is the one check. A failed discussions read leaves the reviews out of that read, not the read.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use nexus_workbench::source_host::Host;
use serde_json::Value;

use super::{Check, Comment, GhFailure, PullRequestStatus, Review, UNAVAILABLE_GITLAB, run};

/// Reads merge request `target` (a number or a link), else the checked-out branch's.
pub(crate) async fn read(
    program: &OsStr,
    folder: &Path,
    path: &str,
    target: Option<&str>,
    deadline: Duration,
) -> Result<PullRequestStatus, String> {
    let stdout = run(program, folder, path, &nexus_workbench::gitlab::view(target), deadline)
        .await
        .map_err(|failure| match failure {
            GhFailure::Late => "GitLab did not answer within seven seconds.".to_string(),
            GhFailure::Missing => UNAVAILABLE_GITLAB.to_string(),
            GhFailure::Failed(text) => Host::GitLab.error(&text),
        })?;
    let request: Value = serde_json::from_slice(&stdout).map_err(|_| "GitLab returned an invalid response.")?;
    // An open merge request's discussions, by the project's id: a link may name another project than the folder's.
    let discussions = match (
        request["state"].as_str(),
        request["project_id"].as_u64(),
        request["iid"].as_u64(),
    ) {
        (Some("opened"), Some(project), Some(iid)) => {
            let endpoint = format!("projects/{project}/merge_requests/{iid}/discussions?per_page=100");
            run(program, folder, path, &["api", &endpoint], deadline)
                .await
                .ok()
                .and_then(|out| serde_json::from_slice::<Value>(&out).ok())
        }
        _ => None,
    };
    parse(&request, discussions.as_ref())
}

/// A merge request from `glab mr view --output json`, with its discussions (`GET …/discussions`) when they were read.
pub fn parse(request: &Value, discussions: Option<&Value>) -> Result<PullRequestStatus, String> {
    let text = |key: &str| {
        request[key]
            .as_str()
            .filter(|text| !text.is_empty())
            .map(String::from)
            .ok_or_else(|| format!("GitLab did not report {key}."))
    };
    let number = request["iid"]
        .as_u64()
        .ok_or("GitLab did not report a merge-request number.")?;
    let state = match text("state")?.as_str() {
        "merged" => "MERGED",
        "closed" => "CLOSED",
        // `locked` is a merge request being merged right now: still open until it is.
        _ => "OPEN",
    };
    let pipeline = [&request["head_pipeline"], &request["pipeline"]]
        .into_iter()
        .find(|pipeline| pipeline.is_object());
    let checks = pipeline
        .map(|pipeline| Check {
            name: pipeline["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or("pipeline")
                .to_string(),
            state: match pipeline["status"].as_str().unwrap_or_default() {
                "success" => "success",
                "failed" | "canceled" => "failure",
                // A pipeline that waits on a manual job has run all it runs on its own.
                "skipped" | "manual" => "skipped",
                _ => "pending",
            }
            .to_string(),
            url: pipeline["web_url"]
                .as_str()
                .filter(|url| !url.is_empty())
                .map(String::from),
        })
        .into_iter()
        .collect();
    let author = request["author"]["username"].as_str().unwrap_or_default();
    let mut reviews = Vec::new();
    let mut comments = Vec::new();
    for discussion in discussions.and_then(Value::as_array).into_iter().flatten() {
        let Some(first) = discussion["notes"].as_array().and_then(|notes| notes.first()) else {
            continue;
        };
        let by = first["author"]["username"].as_str().unwrap_or("someone");
        if first["system"] == true || (!author.is_empty() && by == author) {
            continue;
        }
        let body = first["body"].as_str().unwrap_or_default().trim().to_string();
        let open_thread = discussion["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|note| note["resolvable"] == true && note["resolved"] != true);
        if open_thread {
            let Some(id) = discussion["id"].as_str().filter(|id| !id.is_empty()) else {
                continue;
            };
            reviews.push(Review {
                id: format!("discussion:{id}"),
                author: by.to_string(),
                state: "CHANGES_REQUESTED".into(),
                body,
                submitted_at: first["created_at"].as_str().map(String::from),
            });
        } else if discussion["individual_note"] == true
            && let Some(id) = first["id"].as_u64()
        {
            comments.push(Comment {
                id: format!("note:{id}"),
                author: by.to_string(),
                body,
            });
        }
    }
    let merge_status = request["detailed_merge_status"].as_str().unwrap_or_default();
    let review_decision = if !reviews.is_empty() || merge_status == "requested_changes" {
        Some("CHANGES_REQUESTED".to_string())
    } else if merge_status == "not_approved" {
        Some("REVIEW_REQUIRED".to_string())
    } else {
        None
    };
    let mergeable = match merge_status {
        "unchecked" | "checking" | "approvals_syncing" | "preparing" => "UNKNOWN",
        _ if request["has_conflicts"] == true || merge_status == "conflict" => "CONFLICTING",
        _ => "MERGEABLE",
    };
    Ok(PullRequestStatus {
        number,
        title: text("title")?,
        url: text("web_url")?,
        draft: request["draft"]
            .as_bool()
            .or(request["work_in_progress"].as_bool())
            .unwrap_or(false),
        state: state.to_string(),
        checks,
        review_decision,
        reviews,
        mergeable: mergeable.to_string(),
        host: Host::GitLab,
        head_branch: request["source_branch"].as_str().unwrap_or_default().to_string(),
        base_branch: request["target_branch"].as_str().unwrap_or_default().to_string(),
        comments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(state: &str, pipeline: Option<&str>, merge_status: &str, conflicts: bool) -> Value {
        json!({"iid": 7, "project_id": 99, "title": "Fix the login", "state": state, "draft": false,
            "web_url": "https://gitlab.example.com/team/app/-/merge_requests/7", "author": {"username": "ada"},
            "detailed_merge_status": merge_status, "has_conflicts": conflicts,
            "head_pipeline": pipeline.map(|status| json!({"id": 5, "status": status,
                "web_url": "https://gitlab.example.com/team/app/-/pipelines/5"}))})
    }

    #[test]
    fn reads_the_state_the_pipeline_and_the_mergeability() {
        let mut opened = request("opened", Some("failed"), "mergeable", false);
        opened["source_branch"] = json!("fix-login");
        opened["target_branch"] = json!("main");
        let status = parse(&opened, None).expect("status");
        assert_eq!(
            (status.head_branch.as_str(), status.base_branch.as_str()),
            ("fix-login", "main")
        );
        assert_eq!(
            (status.number, status.state.as_str(), status.host),
            (7, "OPEN", Host::GitLab)
        );
        assert_eq!(
            status.checks,
            [Check {
                name: "pipeline".into(),
                state: "failure".into(),
                url: Some("https://gitlab.example.com/team/app/-/pipelines/5".into())
            }]
        );
        assert_eq!(status.mergeable, "MERGEABLE");
        for (pipeline, state) in [
            ("success", "success"),
            ("running", "pending"),
            ("manual", "skipped"),
            ("canceled", "failure"),
        ] {
            let status = parse(&request("opened", Some(pipeline), "mergeable", false), None).expect("status");
            assert_eq!(status.checks[0].state, state, "{pipeline}");
        }
        assert!(
            parse(&request("opened", None, "mergeable", false), None)
                .expect("status")
                .checks
                .is_empty()
        );
        let conflict = parse(&request("opened", None, "conflict", true), None).expect("status");
        assert_eq!(conflict.mergeable, "CONFLICTING");
        assert_eq!(
            parse(&request("opened", None, "checking", true), None)
                .expect("status")
                .mergeable,
            "UNKNOWN"
        );
        assert_eq!(
            parse(&request("merged", None, "not_open", false), None)
                .expect("status")
                .state,
            "MERGED"
        );
        assert_eq!(
            parse(&request("closed", None, "not_open", false), None)
                .expect("status")
                .state,
            "CLOSED"
        );
        assert!(parse(&json!({"title": "no number"}), None).is_err());
        let json = serde_json::to_string(&conflict).expect("json");
        assert!(json.contains("\"host\":\"gitlab\""), "{json}");
    }

    #[test]
    fn an_unresolved_discussion_asks_for_changes_and_a_plain_note_is_a_comment() {
        let discussions = json!([
            {"id": "d1", "individual_note": false, "notes": [
                {"id": 1, "author": {"username": "bob"}, "body": " Rename it. ", "resolvable": true, "resolved": false,
                 "created_at": "2026-10-30T10:00:00Z"},
                {"id": 2, "author": {"username": "ada"}, "body": "On it.", "resolvable": true, "resolved": false}]},
            {"id": "d2", "individual_note": false, "notes": [
                {"id": 3, "author": {"username": "bob"}, "body": "Done already.", "resolvable": true, "resolved": true}]},
            {"id": "d3", "individual_note": true, "notes": [
                {"id": 4, "author": {"username": "cy"}, "body": "Nice.", "resolvable": false}]},
            {"id": "d4", "individual_note": true, "notes": [
                {"id": 5, "author": {"username": "ada"}, "body": "Pushed a fix.", "resolvable": false}]},
            {"id": "d5", "individual_note": true, "notes": [
                {"id": 6, "author": {"username": "bob"}, "body": "added 1 commit", "system": true}]}
        ]);
        let status = parse(
            &request("opened", None, "discussions_not_resolved", false),
            Some(&discussions),
        )
        .expect("status");
        assert_eq!(status.reviews.len(), 1);
        assert_eq!(
            (
                status.reviews[0].id.as_str(),
                status.reviews[0].author.as_str(),
                status.reviews[0].state.as_str()
            ),
            ("discussion:d1", "bob", "CHANGES_REQUESTED")
        );
        assert_eq!(status.reviews[0].body, "Rename it.");
        assert_eq!(status.review_decision.as_deref(), Some("CHANGES_REQUESTED"));
        assert_eq!(
            status.comments,
            [Comment {
                id: "note:4".into(),
                author: "cy".into(),
                body: "Nice.".into()
            }]
        );
    }

    #[cfg(unix)]
    mod command {
        use super::*;
        use std::path::PathBuf;

        struct FakeGlab {
            folder: PathBuf,
            program: PathBuf,
        }

        impl FakeGlab {
            fn new(script: &str) -> Self {
                let folder = std::env::temp_dir().join(format!("nexus-glab-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&folder).expect("fake CLI folder");
                let program = folder.join("glab");
                nexus_workbench::process::write_fake_script(&program, &format!("#!/bin/sh\n{script}\n"));
                Self { folder, program }
            }

            async fn read(&self, target: Option<&str>) -> Result<PullRequestStatus, String> {
                crate::pull_request::read_with(
                    Host::GitLab,
                    self.program.as_os_str(),
                    &self.folder,
                    "",
                    target,
                    Duration::from_secs(5),
                )
                .await
            }
        }

        impl Drop for FakeGlab {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.folder);
            }
        }

        #[tokio::test]
        async fn reads_the_merge_request_then_its_discussions_with_prompts_off() {
            let cli = FakeGlab::new(
                r#"test "$NO_PROMPT" = true && test "$GIT_TERMINAL_PROMPT" = 0 || exit 2
case "$*" in
  "mr view 7 --output json") printf '%s' '{"iid":7,"project_id":99,"title":"Fix","state":"opened","web_url":"https://gitlab.com/t/a/-/merge_requests/7","author":{"username":"ada"},"head_pipeline":{"status":"failed","web_url":"https://gitlab.com/t/a/-/pipelines/5"}}' ;;
  "api projects/99/merge_requests/7/discussions?per_page=100") printf '%s' '[{"id":"d1","notes":[{"id":1,"author":{"username":"bob"},"body":"Why?","resolvable":true,"resolved":false}]}]' ;;
  *) echo "unexpected: $*" >&2; exit 3 ;;
esac"#,
            );
            let status = cli.read(Some("7")).await.expect("status");
            assert_eq!(status.checks[0].state, "failure");
            assert_eq!(status.reviews[0].id, "discussion:d1");
        }

        #[tokio::test]
        async fn a_branch_without_a_merge_request_reads_as_none_and_a_missing_glab_as_unavailable() {
            let cli = FakeGlab::new(r#"echo 'no open merge request available for "ada/fix"' >&2; exit 1"#);
            let error = cli.read(None).await.expect_err("none");
            assert!(crate::pull_request::is_none(&error), "{error}");
            let missing = cli.folder.join("missing-glab");
            let error = crate::pull_request::read_with(
                Host::GitLab,
                missing.as_os_str(),
                &cli.folder,
                "",
                None,
                Duration::from_secs(5),
            )
            .await
            .expect_err("missing");
            assert_eq!(error, UNAVAILABLE_GITLAB);
            assert!(crate::pull_request::is_unavailable(&error));
        }

        #[tokio::test]
        async fn a_signed_out_glab_names_its_sign_in_and_a_failed_discussions_read_keeps_the_rest() {
            let cli = FakeGlab::new("echo 'glab: 401 Unauthorized' >&2; exit 1");
            assert!(
                cli.read(None)
                    .await
                    .expect_err("signed out")
                    .contains("`glab auth login`")
            );
            let cli = FakeGlab::new(
                r#"case "$1" in
  mr) printf '%s' '{"iid":7,"project_id":99,"title":"Fix","state":"opened","web_url":"https://gitlab.com/t/a/-/merge_requests/7"}' ;;
  *) exit 1 ;;
esac"#,
            );
            let status = cli.read(None).await.expect("status");
            assert!(status.reviews.is_empty() && status.comments.is_empty());
        }
    }
}
