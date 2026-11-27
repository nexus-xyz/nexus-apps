//! A stack's two actions: rebase it onto its trunk, or merge it.
//!
//! A stack is GitHub's own when GitHub lists one for the pull request (`repos/../stacks`), else
//! the one Nexus infers from branch links ([`crate::stack`]), read again here with each layer's head. Both rebase the
//! same way ([`nexus_workbench::restack`]): GitHub has no API for it. GitHub merges its own stack in one request
//! (`merge-async`), through the top layer. An inferred stack merges its bottom pull request with the page's merge
//! ([`Gh::merge`]); then the layers above are rebased onto the trunk, because a squash left the merged commits in them,
//! and the new bottom is retargeted to the trunk. Their checks run again, so the person merges the next one when they
//! pass. Every action reads the stack again first and refuses one that changed after the person confirmed it.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::pull_page::{ErrorKind, Gh, MergeAction, PullError, repo_of};
use nexus_workbench::restack::{self, Stopped};

/// How long a merge of GitHub's own stack is followed before the app says it is still running.
const MERGE_WAIT: Duration = Duration::from_secs(300);

/// One pull request in a stack, as GitHub reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub number: u64,
    pub url: String,
    /// Its head branch, and the commit at its head.
    pub branch: String,
    pub head: String,
    /// `OPEN`, `MERGED` or `CLOSED`, as `gh pr view` says it.
    pub state: String,
    pub draft: bool,
}

impl Layer {
    fn merged(&self) -> bool {
        self.state == "MERGED"
    }
}

/// A stack, bottom first, with the branch its bottom layer merges into.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    /// GitHub's number for the stack; None: Nexus inferred the stack from branch links.
    pub native: Option<u64>,
    pub trunk: String,
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Rebase,
    Merge,
}

/// What an action did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Done {
    pub merged: Vec<u64>,
    /// GitHub put the stack in its merge queue instead of merging it now.
    pub queued: bool,
    pub rebased: usize,
}

/// The layers `action` touches, bottom first, and the merged layer right under them, if any: its head is where the
/// rebase cuts the bottom one ([`restack::restack`]'s `merged_below`). Refuses, as [`ErrorKind::Blocked`] with a
/// sentence the person reads, a stack with nothing left to act on, a merged layer above an unmerged one, a closed
/// layer, and for a merge a draft among the layers that would merge: all of them for GitHub's own stack, only the
/// bottom one for an inferred stack.
fn affected(stack: &Stack, action: Action) -> Result<(Option<&Layer>, &[Layer]), PullError> {
    let blocked = |why: String| PullError::new(ErrorKind::Blocked, why);
    let start = stack
        .layers
        .iter()
        .position(|layer| !layer.merged())
        .ok_or_else(|| blocked("Every pull request in this stack is merged already.".into()))?;
    let open = &stack.layers[start..];
    if let Some(layer) = open.iter().find(|layer| layer.state != "OPEN") {
        return Err(blocked(if layer.merged() {
            format!(
                "#{} is merged, but a pull request under it is not. Look at the stack on GitHub.",
                layer.number
            )
        } else {
            format!("#{} is closed. Reopen it, or take it out of the stack.", layer.number)
        }));
    }
    // GitHub merges its own stack through the top; an inferred stack merges its bottom only.
    let merging = if stack.native.is_some() { open } else { &open[..1] };
    if action == Action::Merge
        && let Some(draft) = merging.iter().find(|layer| layer.draft)
    {
        return Err(blocked(format!(
            "#{} is a draft. Mark it ready for review first.",
            draft.number
        )));
    }
    Ok((start.checked_sub(1).map(|below| &stack.layers[below]), open))
}

/// GitHub's own stack for the pull request, from `repos/../stacks?pull_request=N`: None when it lists none.
fn parse_native(value: &Value, host: &str, owner: &str, name: &str) -> Result<Option<Stack>, String> {
    let stacks = value.as_array().ok_or("GitHub returned an invalid stack.")?;
    let Some(stack) = stacks.first() else {
        return Ok(None);
    };
    let number = stack["number"]
        .as_u64()
        .ok_or("GitHub reported a stack with no number.")?;
    let trunk = stack["base"]
        .as_str()
        .or_else(|| stack["base"]["ref"].as_str())
        .ok_or("GitHub reported a stack with no base branch.")?;
    let mut layers = Vec::new();
    for pull in stack["pull_requests"].as_array().into_iter().flatten() {
        let number = pull["number"]
            .as_u64()
            .ok_or("GitHub reported a stacked pull request with no number.")?;
        let state = if !pull["merged_at"].is_null() {
            "MERGED"
        } else if pull["state"].as_str() == Some("open") {
            "OPEN"
        } else {
            "CLOSED"
        };
        let head = pull["head"]["sha"].as_str().unwrap_or_default();
        if head.is_empty() && state == "OPEN" {
            return Err(format!("GitHub did not report the head of #{number}."));
        }
        layers.push(Layer {
            number,
            url: format!("https://{host}/{owner}/{name}/pull/{number}"),
            branch: pull["head"]["ref"].as_str().unwrap_or_default().to_string(),
            head: head.to_string(),
            state: state.to_string(),
            draft: pull["draft"].as_bool().unwrap_or(false),
        });
    }
    Ok(Some(Stack {
        native: Some(number),
        trunk: trunk.to_string(),
        layers,
    }))
}

/// What a restack that stopped means to the person.
fn stopped(stop: Stopped) -> PullError {
    let done = match stop.done {
        0 => "Nothing was pushed.".to_string(),
        1 => "The layer below it was rebased and pushed.".to_string(),
        n => format!("The {n} layers below it were rebased and pushed."),
    };
    if stop.rebase {
        PullError::new(
            ErrorKind::Blocked,
            format!(
                "#{} does not rebase cleanly onto the layer below it. {done} Resolve #{} and push, then rebase again.\n\n{}",
                stop.number, stop.number, stop.message
            ),
        )
    } else {
        PullError::new(
            ErrorKind::Refused,
            format!("The rebase stopped at #{}. {done}\n\n{}", stop.number, stop.message),
        )
    }
}

fn invalid() -> PullError {
    PullError::new(ErrorKind::Invalid, "GitHub returned an invalid response.")
}

impl Gh<'_> {
    /// The stack the session's pull requests `urls` (bottom first, as [`crate::stack::groups`] links them) belong
    /// to: GitHub's own when it lists one for the bottom one, else the inferred one, each layer read again.
    pub async fn stack(&self, urls: &[String]) -> Result<Stack, PullError> {
        let first = urls
            .first()
            .ok_or_else(|| PullError::new(ErrorKind::Input, "Pick a stack first."))?;
        let (host, owner, name) = repo_of(first)
            .ok_or_else(|| PullError::new(ErrorKind::Invalid, "GitHub reported an address this app cannot read."))?;
        let number = first.rsplit('/').next().unwrap_or_default();
        let path = format!("repos/{owner}/{name}/stacks?pull_request={number}");
        match self.run(&["api", "--hostname", host, &path], self.read, None).await {
            Ok(stdout) => {
                let value: Value = serde_json::from_slice(&stdout).map_err(|_| invalid())?;
                let native = parse_native(&value, host, owner, name)
                    .map_err(|message| PullError::new(ErrorKind::Invalid, message))?;
                if let Some(stack) = native {
                    return Ok(stack);
                }
            }
            // A host with no stacks at all (GitHub Enterprise Server, say).
            Err(error) if error.message.contains("HTTP 404") => {}
            Err(error) => return Err(error),
        }
        self.inferred(urls).await
    }

    async fn inferred(&self, urls: &[String]) -> Result<Stack, PullError> {
        let mut layers: Vec<Layer> = Vec::new();
        let mut trunk = String::new();
        for url in urls {
            let fields = "number,url,headRefName,headRefOid,baseRefName,state,isDraft";
            let stdout = self
                .run(&["pr", "view", url, "--json", fields], self.read, None)
                .await?;
            let pull: Value = serde_json::from_slice(&stdout).map_err(|_| invalid())?;
            let base = pull["baseRefName"].as_str().unwrap_or_default();
            match layers.last() {
                None => trunk = base.to_string(),
                Some(below) if below.branch != base => {
                    return Err(PullError::new(
                        ErrorKind::Changed,
                        format!("{url} no longer sits on #{}. Look at the stack again.", below.number),
                    ));
                }
                Some(_) => {}
            }
            layers.push(Layer {
                number: pull["number"].as_u64().ok_or_else(invalid)?,
                url: pull["url"].as_str().unwrap_or(url).to_string(),
                branch: pull["headRefName"].as_str().unwrap_or_default().to_string(),
                head: pull["headRefOid"].as_str().unwrap_or_default().to_string(),
                state: pull["state"].as_str().unwrap_or_default().to_string(),
                draft: pull["isDraft"].as_bool().unwrap_or(false),
            });
        }
        Ok(Stack {
            native: None,
            trunk,
            layers,
        })
    }

    /// Runs `action` on the stack the person confirmed, merging with `method`. Refuses when the stack changed since.
    pub async fn act(&self, confirmed: &Stack, action: Action, method: &str) -> Result<Done, PullError> {
        let urls: Vec<String> = confirmed.layers.iter().map(|layer| layer.url.clone()).collect();
        let now = self.stack(&urls).await?;
        if &now != confirmed {
            return Err(PullError::new(
                ErrorKind::Changed,
                "The stack changed after you confirmed. Look at it again.",
            ));
        }
        let (below, open) = affected(&now, action)?;
        match (action, now.native) {
            (Action::Rebase, _) => Ok(Done {
                rebased: self.restack(&now.trunk, open, below)?,
                ..Done::default()
            }),
            (Action::Merge, Some(_)) => self.merge_native(open, method).await,
            (Action::Merge, None) => self.merge_bottom(&now.trunk, open, method).await,
        }
    }

    fn restack(&self, trunk: &str, layers: &[Layer], below: Option<&Layer>) -> Result<usize, PullError> {
        let layers: Vec<restack::Layer> = layers
            .iter()
            .map(|layer| restack::Layer {
                number: layer.number,
                branch: &layer.branch,
                head: &layer.head,
            })
            .collect();
        restack::restack(self.folder, trunk, &layers, below.map(|layer| layer.head.as_str())).map_err(stopped)
    }

    /// GitHub merges its own stack through the top of `open`, all at once, and may queue it.
    async fn merge_native(&self, open: &[Layer], method: &str) -> Result<Done, PullError> {
        let top = open.last().ok_or_else(invalid)?;
        let (host, owner, name) = repo_of(&top.url).ok_or_else(invalid)?;
        let path = format!("repos/{owner}/{name}/pulls/{}/merge-async", top.number);
        let (method, sha) = (format!("merge_method={method}"), format!("sha={}", top.head));
        let args = [
            "api",
            "--hostname",
            host,
            "-X",
            "PUT",
            &path,
            "-f",
            &method,
            "-f",
            "merge_action=default",
            "-f",
            &sha,
        ];
        let mut reply: Value = serde_json::from_slice(&self.run(&args, self.write, Some("the stack merge")).await?)
            .map_err(|_| invalid())?;
        let started = Instant::now();
        let mut wait = Duration::from_secs(1);
        while reply["status"] == "pending" && started.elapsed() < MERGE_WAIT {
            let uuid = reply["details"]["uuid"].as_str().ok_or_else(invalid)?;
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(Duration::from_secs(10));
            let poll = format!("{path}/{uuid}");
            reply = serde_json::from_slice(&self.run(&["api", "--hostname", host, &poll], self.read, None).await?)
                .map_err(|_| invalid())?;
        }
        let merged = open.iter().map(|layer| layer.number).collect();
        match reply["status"].as_str() {
            Some("merged") => Ok(Done {
                merged,
                ..Done::default()
            }),
            Some("enqueued") => Ok(Done {
                queued: true,
                ..Done::default()
            }),
            Some("pending") => Err(PullError::new(
                ErrorKind::Timeout,
                "GitHub is still merging the stack. Look at it on GitHub before you try again.",
            )),
            Some("failed") => Err(PullError::new(
                ErrorKind::Refused,
                format!(
                    "GitHub refused the stack merge: {}",
                    reply["details"]["message"].as_str().unwrap_or("it gave no reason.")
                ),
            )),
            _ => Err(invalid()),
        }
    }

    /// Merges the bottom of an inferred stack, rebases the rest onto the trunk, and retargets the new bottom to it.
    async fn merge_bottom(&self, trunk: &str, open: &[Layer], method: &str) -> Result<Done, PullError> {
        let (bottom, rest) = open.split_first().ok_or_else(invalid)?;
        let gh = Gh {
            target: &bottom.url,
            ..*self
        };
        // Only a merge now: with auto-merge or the queue it merges later, and the layers above would be rebased
        // onto a trunk that does not hold it yet.
        let page = gh.page().await?;
        if page.merge.action != Some(MergeAction::Merge) {
            let why = match page.merge.blocked.as_slice() {
                [] => "Its required checks are still running, or it merges through the merge queue.".to_string(),
                reasons => reasons.join(" "),
            };
            return Err(PullError::new(
                ErrorKind::Blocked,
                format!("#{} cannot merge now. {why}", bottom.number),
            ));
        }
        gh.merge(&bottom.head, method, MergeAction::Merge).await?;
        let merged = format!("#{} merged.", bottom.number);
        let rebased = self
            .restack(trunk, rest, Some(bottom))
            .map_err(|error| PullError::new(error.kind, format!("{merged} {}", error.message)))?;
        if let Some(next) = rest.first() {
            let (host, owner, name) = repo_of(&next.url).ok_or_else(invalid)?;
            let path = format!("repos/{owner}/{name}/pulls/{}", next.number);
            let base = format!("base={trunk}");
            self.run(
                &["api", "--hostname", host, "-X", "PATCH", &path, "-f", &base],
                self.write,
                Some(&format!("to retarget #{} to {trunk}", next.number)),
            )
            .await
            .map_err(|error| {
                PullError::new(error.kind, format!("{merged} The stack was rebased. {}", error.message))
            })?;
        }
        Ok(Done {
            merged: vec![bottom.number],
            rebased,
            queued: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn layer(number: u64, state: &str, draft: bool) -> Layer {
        Layer {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            branch: format!("b{number}"),
            head: format!("h{number}"),
            state: state.into(),
            draft,
        }
    }

    fn stack(native: bool, layers: Vec<Layer>) -> Stack {
        Stack {
            native: native.then_some(7),
            trunk: "main".into(),
            layers,
        }
    }

    fn numbers(layers: &[Layer]) -> Vec<u64> {
        layers.iter().map(|layer| layer.number).collect()
    }

    #[test]
    fn affected_layers_are_the_unmerged_ones_with_the_merged_one_under_them() {
        let three = stack(
            false,
            vec![
                layer(1, "MERGED", false),
                layer(2, "OPEN", false),
                layer(3, "OPEN", false),
            ],
        );
        for action in [Action::Rebase, Action::Merge] {
            let (below, open) = affected(&three, action).expect("acts");
            assert_eq!(below.map(|layer| layer.number), Some(1));
            assert_eq!(numbers(open), [2, 3]);
        }
        let fresh = stack(true, vec![layer(1, "OPEN", false), layer(2, "OPEN", false)]);
        let (below, open) = affected(&fresh, Action::Rebase).expect("acts");
        assert_eq!((below, numbers(open)), (None, vec![1, 2]));
    }

    #[test]
    fn affected_refuses_what_cannot_be_acted_on() {
        let refused = |stack: &Stack, action| affected(stack, action).expect_err("refused").kind;
        let all_merged = stack(false, vec![layer(1, "MERGED", false)]);
        let closed = stack(false, vec![layer(1, "OPEN", false), layer(2, "CLOSED", false)]);
        let hole = stack(false, vec![layer(1, "OPEN", false), layer(2, "MERGED", false)]);
        for action in [Action::Rebase, Action::Merge] {
            assert_eq!(refused(&all_merged, action), ErrorKind::Blocked);
            assert_eq!(refused(&closed, action), ErrorKind::Blocked);
            assert_eq!(refused(&hole, action), ErrorKind::Blocked);
        }
    }

    #[test]
    fn affected_drafts_block_only_the_layers_that_would_merge() {
        let draft_on_top = |native| stack(native, vec![layer(1, "OPEN", false), layer(2, "OPEN", true)]);
        // GitHub merges its own stack through the top: a draft anywhere blocks it.
        assert!(affected(&draft_on_top(true), Action::Merge).is_err());
        // An inferred stack merges its bottom only.
        assert!(affected(&draft_on_top(false), Action::Merge).is_ok());
        // A rebase merges nothing.
        assert!(affected(&draft_on_top(true), Action::Rebase).is_ok());
        let draft_at_bottom = stack(false, vec![layer(1, "OPEN", true), layer(2, "OPEN", false)]);
        assert!(affected(&draft_at_bottom, Action::Merge).is_err());
    }

    #[test]
    fn githubs_own_stack_is_read_bottom_first_with_merged_and_open_layers() {
        let reply = json!([{"number": 7, "url": "x", "base": "main", "pull_requests": [
            {"number": 1, "head": {"ref": "a", "sha": "s1"}, "state": "closed", "merged_at": "2026-11-23T00:00:00Z"},
            {"number": 2, "head": {"ref": "b", "sha": "s2"}, "state": "open", "draft": true},
            {"number": 3, "head": {"ref": "c", "sha": "s3"}, "state": "closed", "merged_at": null}
        ]}]);
        let stack = parse_native(&reply, "github.com", "o", "r")
            .expect("reads")
            .expect("a stack");
        assert_eq!(stack.native, Some(7));
        assert_eq!(stack.trunk, "main");
        let states: Vec<(u64, &str, &str, bool)> = stack
            .layers
            .iter()
            .map(|layer| (layer.number, layer.branch.as_str(), layer.state.as_str(), layer.draft))
            .collect();
        assert_eq!(
            states,
            [
                (1, "a", "MERGED", false),
                (2, "b", "OPEN", true),
                (3, "c", "CLOSED", false)
            ]
        );
        assert_eq!(stack.layers[1].url, "https://github.com/o/r/pull/2");
        // A base given as an object, and no stack at all.
        let object = json!([{"number": 8, "base": {"ref": "dev"}, "pull_requests": []}]);
        assert_eq!(
            parse_native(&object, "h", "o", "r").expect("reads").map(|s| s.trunk),
            Some("dev".into())
        );
        assert_eq!(parse_native(&json!([]), "h", "o", "r"), Ok(None));
        // An open layer with no head cannot be acted on safely.
        let headless = json!([{"number": 9, "base": "main", "pull_requests": [{"number": 4, "head": {"ref": "d"}, "state": "open"}]}]);
        assert!(parse_native(&headless, "h", "o", "r").is_err());
    }

    #[test]
    fn a_stopped_rebase_says_which_layer_and_what_was_pushed() {
        let conflict = stopped(Stopped {
            number: 3,
            done: 1,
            rebase: true,
            message: "CONFLICT".into(),
        });
        assert_eq!(conflict.kind, ErrorKind::Blocked);
        assert!(conflict.message.starts_with(
            "#3 does not rebase cleanly onto the layer below it. The layer below it was rebased and pushed."
        ));
        let push = stopped(Stopped {
            number: 2,
            done: 0,
            rebase: false,
            message: "stale info".into(),
        });
        assert_eq!(push.kind, ErrorKind::Refused);
        assert!(push.message.contains("Nothing was pushed."));
    }

    #[cfg(unix)]
    mod with_fake_gh {
        use super::*;
        use std::path::{Path, PathBuf};

        /// A fake `gh` whose body is `script`, logging each call to `$0.log`. Builtins only: it runs with no `PATH`.
        struct FakeGh {
            folder: PathBuf,
            program: PathBuf,
        }

        impl FakeGh {
            fn new(script: &str) -> Self {
                let folder = std::env::temp_dir().join(format!("nexus-stack-actions-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&folder).expect("fake CLI folder");
                let program = folder.join("gh");
                let body = format!(
                    "#!/bin/sh\n[ -n \"$FAKE_GH_PROBE\" ] && exit 0\nprintf '%s\\n' \"$*\" >> \"$0.log\"\n{script}\n"
                );
                nexus_workbench::process::write_fake_script(&program, &body);
                // As pull_page's fake: start it once, uncounted, until it runs (ETXTBSY from a racing fork).
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
                    folder: Path::new(&self.folder),
                    path: "",
                    target: "",
                    read: Duration::from_secs(5),
                    write: Duration::from_secs(5),
                }
            }

            fn calls(&self) -> Vec<String> {
                std::fs::read_to_string(self.folder.join("gh.log"))
                    .unwrap_or_default()
                    .lines()
                    .map(String::from)
                    .collect()
            }
        }

        impl Drop for FakeGh {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.folder);
            }
        }

        fn urls(numbers: &[u64]) -> Vec<String> {
            numbers
                .iter()
                .map(|number| format!("https://github.com/o/r/pull/{number}"))
                .collect()
        }

        #[tokio::test]
        async fn with_no_github_stack_the_branch_links_are_read_again_and_must_still_hold() {
            let view = |number: u64, base: &str| {
                json!({"number": number, "url": format!("https://github.com/o/r/pull/{number}"),
                    "headRefName": format!("b{number}"), "headRefOid": format!("h{number}"), "baseRefName": base,
                    "state": "OPEN", "isDraft": false})
            };
            let linked = FakeGh::new(&format!(
                "case \"$*\" in\n*stacks*) printf '[]' ;;\n*pull/1*) printf '%s' '{}' ;;\n*pull/2*) printf '%s' '{}' ;;\nesac",
                view(1, "main"),
                view(2, "b1")
            ));
            let read = linked.gh().stack(&urls(&[1, 2])).await.expect("reads");
            assert_eq!(read.native, None);
            assert_eq!(read.trunk, "main");
            assert_eq!(numbers(&read.layers), [1, 2]);
            assert_eq!(read.layers[1].head, "h2");

            let broken = FakeGh::new(&format!(
                "case \"$*\" in\n*stacks*) printf 'gh: Not Found (HTTP 404)' >&2; exit 1 ;;\n*pull/1*) printf '%s' '{}' ;;\n*pull/2*) printf '%s' '{}' ;;\nesac",
                view(1, "main"),
                view(2, "main")
            ));
            let error = broken.gh().stack(&urls(&[1, 2])).await.expect_err("no longer linked");
            assert_eq!(error.kind, ErrorKind::Changed);
        }

        #[tokio::test]
        async fn githubs_own_stack_merges_through_its_top_in_one_request_at_the_confirmed_head() {
            let stacks = json!([{"number": 7, "base": "main", "pull_requests": [
                {"number": 1, "head": {"ref": "b1", "sha": "h1"}, "state": "open"},
                {"number": 2, "head": {"ref": "b2", "sha": "h2"}, "state": "open"}
            ]}]);
            let fake = FakeGh::new(&format!(
                "case \"$*\" in\n*stacks*) printf '%s' '{stacks}' ;;\n*merge-async*) printf '%s' '{{\"status\":\"merged\",\"details\":{{}}}}' ;;\nesac"
            ));
            let gh = fake.gh();
            let confirmed = gh.stack(&urls(&[1])).await.expect("reads");
            let done = gh.act(&confirmed, Action::Merge, "squash").await.expect("merges");
            assert_eq!(done.merged, [1, 2]);
            assert!(
                fake.calls().iter().any(|call| call
                    == "api --hostname github.com -X PUT repos/o/r/pulls/2/merge-async -f merge_method=squash -f merge_action=default -f sha=h2"),
                "{:?}",
                fake.calls()
            );
        }

        #[tokio::test]
        async fn a_stack_that_moved_after_the_person_confirmed_is_not_touched() {
            let fake = FakeGh::new(
                "case \"$*\" in\n*stacks*) printf '%s' '[{\"number\":7,\"base\":\"main\",\"pull_requests\":[{\"number\":1,\"head\":{\"ref\":\"b1\",\"sha\":\"new\"},\"state\":\"open\"}]}]' ;;\nesac",
            );
            let confirmed = stack(true, vec![layer(1, "OPEN", false)]);
            let error = fake
                .gh()
                .act(&confirmed, Action::Merge, "squash")
                .await
                .expect_err("changed");
            assert_eq!(error.kind, ErrorKind::Changed);
            assert!(!fake.calls().iter().any(|call| call.contains("merge-async")));
        }
    }
}
