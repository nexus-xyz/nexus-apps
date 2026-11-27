//! A pull request named by what a person typed or pasted in New session's review picker, as the Nexus app reads it:
//! `123`, `#123`, `!123`, a GitHub link ending in `/pull/123`, or a
//! GitLab link ending in `/-/merge_requests/123`.

/// Repository path of an HTTPS, SSH, git, or scp-style remote. Local paths are not hosted repositories.
pub fn repo_of_remote(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let path = if let Some((scheme, rest)) = remote.split_once("://") {
        if !matches!(scheme, "http" | "https" | "ssh" | "git") {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        if authority.is_empty() {
            return None;
        }
        path
    } else {
        let (host, path) = remote.split_once(':')?;
        if !host.contains('.') && !host.contains('@') {
            return None;
        }
        path
    };
    let path = path.trim_matches('/').trim_end_matches(".git");
    let parts: Vec<_> = path.split('/').collect();
    (parts.len() >= 2
        && parts
            .iter()
            .all(|part| plain(part) && !matches!(*part, "." | "..") && !part.contains(['?', '#'])))
    .then(|| path.to_lowercase())
}

fn plain(segment: &str) -> bool {
    !segment.is_empty() && !segment.chars().any(char::is_whitespace)
}

fn starts_with_digit(segment: &str) -> bool {
    segment.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// `owner/name` of a pull request's link (`https://github.com/owner/name/pull/7`, pasted without `https://`, or
/// without the host: `owner/name/pull/7`), lower-cased; None when it is not one. A host has a dot; a GitHub owner never
/// does. A GitLab link gives its project path at any depth of groups (`group/sub/project`).
pub fn repo_of_pull(link: &str) -> Option<String> {
    let trimmed = link.trim();
    let lower = trimmed.to_ascii_lowercase();
    let rest = ["https://", "http://"]
        .iter()
        .find(|scheme| lower.starts_with(*scheme))
        .map_or(trimmed, |scheme| &trimmed[scheme.len()..]);
    let segments: Vec<&str> = rest.split('/').collect();
    // With the host first, then without it, as the pattern's optional host backtracks.
    let host = segments
        .first()
        .is_some_and(|first| first.contains('.') && !first.chars().any(char::is_whitespace));
    let starts = if host { vec![1, 0] } else { vec![0] };
    let github = |start: usize| -> Option<String> {
        let parts = segments.get(start..start + 4)?;
        (plain(parts[0]) && plain(parts[1]) && parts[2].eq_ignore_ascii_case("pull") && starts_with_digit(parts[3]))
            .then(|| format!("{}/{}", parts[0], parts[1]))
    };
    let gitlab = |start: usize| -> Option<String> {
        (start + 2..segments.len()).find_map(|dash| {
            let tail = segments.get(dash..dash + 3)?;
            let path = &segments[start..dash];
            (tail[0] == "-"
                && tail[1].eq_ignore_ascii_case("merge_requests")
                && starts_with_digit(tail[2])
                && path.iter().all(|segment| plain(segment)))
            .then(|| path.join("/"))
        })
    };
    starts
        .iter()
        .find_map(|&start| github(start))
        .or_else(|| starts.iter().find_map(|&start| gitlab(start)))
        .map(|repo| repo.to_lowercase())
}

/// `^[#!]?(\d+)$`.
fn bare(text: &str) -> Option<&str> {
    let digits = text.strip_prefix(['#', '!']).unwrap_or(text);
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

/// `/(?:pull|-/merge_requests)/(\d+)(?:[/?#].*)?$`: the number at a link's end.
fn linked_number(text: &str) -> Option<&str> {
    ["/pull/", "/-/merge_requests/"].iter().find_map(|marker| {
        text.match_indices(marker).find_map(|(at, _)| {
            let after = &text[at + marker.len()..];
            let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
            let rest = &after[end..];
            (end > 0 && (rest.is_empty() || rest.starts_with(['/', '?', '#'])) && !rest.contains('\n'))
                .then(|| &after[..end])
        })
    })
}

/// The largest number JavaScript holds exactly, as the Nexus app's `Number.isSafeInteger` allows.
const MAX_SAFE: u64 = (1 << 53) - 1;

/// The pull request number. A link for another repository than `repo`, or one whose repository cannot be read (when
/// `repo` is known), gives None: the checkout fetches the number from this project's repository, so another
/// repository's #42 would open the wrong pull request.
pub fn pull_number(text: &str, repo: Option<&str>) -> Option<u64> {
    let typed = text.trim();
    let bare = bare(typed);
    let link = linked_number(typed);
    if bare.is_none()
        && link.is_some()
        && let Some(repo) = repo
        && repo_of_pull(typed).as_deref() != Some(repo.to_lowercase().as_str())
    {
        return None;
    }
    let number: u64 = bare.or(link)?.parse().ok()?;
    (number > 0 && number <= MAX_SAFE).then_some(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_remotes_identify_the_repository_without_open_pulls() {
        assert_eq!(
            repo_of_remote("git@github.com:Nexus-Xyz/Nexus.git").as_deref(),
            Some("nexus-xyz/nexus")
        );
        assert_eq!(
            repo_of_remote("ssh://git@gitlab.example.com:2222/group/sub/project.git").as_deref(),
            Some("group/sub/project")
        );
        assert_eq!(
            repo_of_remote("https://github.com/nexus-xyz/nexus.git").as_deref(),
            Some("nexus-xyz/nexus")
        );
        assert_eq!(repo_of_remote("/local/folder"), None);
        assert_eq!(repo_of_remote("C:/repos/project"), None);
    }

    #[test]
    fn reads_a_number_with_or_without_the_hash() {
        assert_eq!(pull_number("42", None), Some(42));
        assert_eq!(pull_number(" #42 ", None), Some(42));
        assert_eq!(pull_number("0", None), None);
        assert_eq!(pull_number("title words", None), None);
    }

    #[test]
    fn reads_a_link_and_a_links_trailing_path() {
        assert_eq!(
            pull_number("https://github.com/nexus-xyz/nexus/pull/42", None),
            Some(42)
        );
        assert_eq!(
            pull_number("https://github.com/nexus-xyz/nexus/pull/42/files?x=1#y", None),
            Some(42)
        );
    }

    #[test]
    fn refuses_a_link_for_another_repository_than_this_projects() {
        let repo = Some("nexus-xyz/nexus");
        assert_eq!(
            pull_number("https://github.com/other-org/other-repo/pull/42", repo),
            None
        );
        assert_eq!(pull_number("github.com/other-org/other-repo/pull/42", repo), None);
        assert_eq!(pull_number("github.com/nexus-xyz/nexus/pull/42", repo), Some(42));
        assert_eq!(
            pull_number("https://github.com/Nexus-XYZ/Nexus/pull/42", repo),
            Some(42)
        );
        assert_eq!(pull_number("42", repo), Some(42));
    }

    #[test]
    fn takes_a_link_as_it_is_while_the_projects_repository_is_not_known() {
        assert_eq!(
            pull_number("https://github.com/other-org/other-repo/pull/42", None),
            Some(42)
        );
    }

    #[test]
    fn repo_of_pull_is_owner_and_name_of_a_pull_request_link() {
        assert_eq!(
            repo_of_pull("https://github.com/nexus-xyz/nexus/pull/7").as_deref(),
            Some("nexus-xyz/nexus")
        );
        assert_eq!(repo_of_pull("not a link"), None);
    }

    #[test]
    fn repo_of_pull_reads_a_link_pasted_without_the_host() {
        assert_eq!(
            repo_of_pull("nexus-xyz/nexus/pull/42").as_deref(),
            Some("nexus-xyz/nexus")
        );
        assert_eq!(
            repo_of_pull("github.com/Other-Org/other.repo/pull/42").as_deref(),
            Some("other-org/other.repo")
        );
    }

    #[test]
    fn refuses_another_repositorys_link_with_no_host_or_one_whose_repository_cannot_be_read() {
        let repo = Some("nexus-xyz/nexus");
        assert_eq!(pull_number("nexus-xyz/nexus/pull/42", repo), Some(42));
        assert_eq!(pull_number("other-org/other-repo/pull/42", repo), None);
        assert_eq!(pull_number("/pull/42", repo), None);
        assert_eq!(pull_number("/pull/42", None), Some(42));
    }

    #[test]
    fn gitlab_merge_request_links_read_the_number_and_the_project_at_any_group_depth() {
        assert_eq!(pull_number("!42", None), Some(42));
        assert_eq!(
            pull_number("https://gitlab.com/team/sub/app/-/merge_requests/42", None),
            Some(42)
        );
        assert_eq!(
            pull_number(
                "https://gitlab.example.com/team/app/-/merge_requests/42/diffs?x=1",
                None
            ),
            Some(42)
        );
        assert_eq!(
            repo_of_pull("https://gitlab.com/Team/Sub/app/-/merge_requests/7").as_deref(),
            Some("team/sub/app")
        );
        assert_eq!(
            repo_of_pull("gitlab.example.com/team/app/-/merge_requests/7").as_deref(),
            Some("team/app")
        );
        assert_eq!(
            pull_number(
                "https://gitlab.com/team/other/-/merge_requests/42",
                Some("team/sub/app")
            ),
            None
        );
        assert_eq!(
            pull_number(
                "https://gitlab.com/team/sub/app/-/merge_requests/42",
                Some("team/sub/app")
            ),
            Some(42)
        );
    }
}
