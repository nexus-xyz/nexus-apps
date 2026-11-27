//! GitLab's merge requests through `glab`, as `git.rs` reads GitHub's pull requests through `gh`: the
//! commands' arguments and what their JSON answers mean. The desktop's watch reads a merge request's pipeline and
//! discussions in its own `pull_request/gitlab.rs`. [`crate::source_host`] says which host a repository uses.

use serde_json::Value;

use crate::git::PullRequestInfo;
use crate::source_host::Host;

/// The open merge requests, most recent first (at most 30): `glab mr list` lists only open ones unless told otherwise.
pub const LIST: &[&str] = &["mr", "list", "--per-page", "30", "--output", "json"];

/// `glab mr view` of merge request `target` (a number, a branch or a link), else of the checked-out branch.
pub fn view(target: Option<&str>) -> Vec<&str> {
    let mut args = vec!["mr", "view"];
    args.extend(target);
    args.extend(["--output", "json"]);
    args
}

/// The merged merge request of `branch`, newest first, one only.
pub fn merged_of_branch(branch: &str) -> Vec<&str> {
    vec![
        "mr",
        "list",
        "--merged",
        "--source-branch",
        branch,
        "--per-page",
        "1",
        "--output",
        "json",
    ]
}

/// Opens a merge request from the checked-out branch into `base`, without asking anything.
pub fn create<'a>(base: &'a str, title: &'a str, body: &'a str) -> Vec<&'a str> {
    vec![
        "mr",
        "create",
        "--target-branch",
        base,
        "--title",
        title,
        "--description",
        body,
        "--yes",
    ]
}

/// A merge request as the "!" picker and a review list it.
pub fn info(value: &Value) -> Option<PullRequestInfo> {
    let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
    Some(PullRequestInfo {
        number: value["iid"].as_u64()?,
        title: text("title"),
        url: text("web_url"),
        author: value["author"]["username"].as_str().unwrap_or_default().to_string(),
        head: text("source_branch"),
        base: text("target_branch"),
        host: Host::GitLab,
    })
}

/// The address of a merge request that is still open; None for a merged or closed one.
pub fn open_url(value: &Value) -> Option<String> {
    (value["state"] == "opened")
        .then(|| value["web_url"].as_str().map(String::from))
        .flatten()
        .filter(|url| !url.is_empty())
}

/// The head commit of a merged merge request (`sha`), from `glab mr view` or the first of `glab mr list`.
pub fn merged_head(value: &Value) -> Option<String> {
    let request = value.as_array().map_or(Some(value), |list| list.first())?;
    if request["state"] != "merged" {
        return None;
    }
    request["sha"].as_str().filter(|sha| !sha.is_empty()).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(state: &str) -> Value {
        json!({"iid": 7, "title": "Fix the login", "web_url": "https://gitlab.example.com/team/app/-/merge_requests/7",
            "state": state, "author": {"username": "ada"}, "source_branch": "ada/fix", "target_branch": "main",
            "sha": "abc123"})
    }

    #[test]
    fn reads_a_merge_request_as_the_picker_lists_it() {
        let info = info(&request("opened")).expect("info");
        assert_eq!(
            (
                info.number,
                info.author.as_str(),
                info.head.as_str(),
                info.base.as_str(),
                info.host
            ),
            (7, "ada", "ada/fix", "main", Host::GitLab)
        );
        assert!(
            serde_json::to_string(&info)
                .unwrap_or_default()
                .contains("\"host\":\"gitlab\"")
        );
        assert!(super::info(&json!({"title": "no iid"})).is_none());
    }

    #[test]
    fn an_open_merge_request_has_an_address_and_a_merged_one_a_head() {
        assert_eq!(
            open_url(&request("opened")).as_deref(),
            Some("https://gitlab.example.com/team/app/-/merge_requests/7")
        );
        assert_eq!(open_url(&request("merged")), None);
        assert_eq!(merged_head(&request("merged")).as_deref(), Some("abc123"));
        assert_eq!(merged_head(&json!([request("merged")])).as_deref(), Some("abc123"));
        assert_eq!(merged_head(&request("closed")), None);
        assert_eq!(merged_head(&json!([])), None);
    }
}
