//! Pre-PR review (S4.4): `nexus robustness run review --local`, and `nexus mcp`'s `review_local` tool, which
//! calls the same functions.
//!
//! The CLI finds the merge base of `HEAD` with the remote default branch and diffs the working tree against it (tracked
//! changes, staged or not, and untracked files that `.gitignore` does not hide). It sends the patches and the changed
//! files' text to Nexus Review's `run` with `local: true`, polls `report/<id>` and prints each finding as
//! `path:line severity title`. Nexus Review reads the rules at the merge base, as for a pull request, and posts nothing.
//! The exit code is 1 when a finding reaches the repository's `check_fails_on`.
//!
//! Before anything is sent: files that look like secrets are left out and named, and a change over
//! [`MAX_BODY_BYTES`] is refused with its largest files named, to leave out with `--exclude`.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The largest request the CLI sends: Gateway refuses a body over 1 MB.
pub const MAX_BODY_BYTES: usize = 900_000;
/// A working-tree file larger than this is sent as its patch only.
pub const MAX_HEAD_TEXT: usize = 256 * 1024;
/// How long the CLI waits for the review.
pub const WAIT: Duration = Duration::from_secs(15 * 60);
/// The title a change has when no commit is ahead of the base.
pub const LOCAL_TITLE: &str = "Local change";

/// What the person asked for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    /// `owner/name`; default: the `origin` remote's.
    pub repo: Option<String>,
    pub model: Option<String>,
    /// Paths or folders left out (`--exclude a,b`).
    pub exclude: Vec<String>,
}

/// One changed file, as the module reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    pub previous_path: Option<String>,
    /// `added`, `removed`, `modified` or `renamed`.
    pub status: &'static str,
    /// The hunks, from the first `@@`; none for a binary file or a mode-only change.
    pub patch: Option<String>,
}

/// A change ready to send: the request body and the files left out, with why.
#[derive(Debug, Clone, PartialEq)]
pub struct Collected {
    pub repo: String,
    pub base_sha: String,
    pub body: Value,
    pub left_out: Vec<(String, String)>,
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(["-c", "core.quotePath=false"])
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `owner/name` from a GitHub remote URL (`https://github.com/o/n.git`, `git@github.com:o/n.git`).
pub fn repo_of_remote(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, name) = rest.split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/')).then(|| format!("{owner}/{name}"))
}

/// The path in `diff --git a/<p> b/<p>` when both sides are the same path (a binary or mode-only change).
fn same_path(header: &str) -> Option<String> {
    let rest = header.strip_prefix("diff --git ")?;
    let length = rest.len().checked_sub(5)? / 2;
    let left = rest.get(2..2 + length)?;
    (rest.starts_with("a/") && rest.get(2 + length..) == Some(&format!(" b/{left}")[..])).then(|| left.to_string())
}

/// The files of `git diff` output, each with its hunks as GitHub's `patch` gives them.
pub fn parse_diff(text: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    for section in text.split("\ndiff --git ").enumerate().map(|(index, part)| {
        if index == 0 {
            part.to_string()
        } else {
            format!("diff --git {part}")
        }
    }) {
        let mut lines = section.lines();
        let Some(header) = lines.next().filter(|line| line.starts_with("diff --git ")) else {
            continue;
        };
        let (mut old, mut new, mut status) = (None::<String>, None::<String>, "modified");
        let mut hunks: Vec<&str> = Vec::new();
        for line in lines {
            if !hunks.is_empty() || line.starts_with("@@") {
                hunks.push(line);
            } else if let Some(path) = line.strip_prefix("rename from ") {
                old = Some(path.to_string());
                status = "renamed";
            } else if let Some(path) = line.strip_prefix("rename to ") {
                new = Some(path.to_string());
                status = "renamed";
            } else if line.starts_with("new file mode") {
                status = "added";
            } else if line.starts_with("deleted file mode") {
                status = "removed";
            } else if let Some(path) = line.strip_prefix("--- a/") {
                old = Some(path.to_string());
            } else if let Some(path) = line.strip_prefix("+++ b/") {
                new = Some(path.to_string());
            }
        }
        let fallback = same_path(header);
        let path = match status {
            "removed" => old.clone().or(fallback.clone()),
            _ => new.clone().or(fallback.clone()).or(old.clone()),
        };
        let Some(path) = path else { continue };
        files.push(DiffFile {
            previous_path: (status == "renamed").then(|| old.clone()).flatten(),
            path,
            status,
            patch: (!hunks.is_empty()).then(|| hunks.join("\n")),
        });
    }
    files
}

/// An untracked file as an added one: every line added.
pub fn added_patch(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut patch = format!("@@ -0,0 +1,{} @@", lines.len());
    for line in &lines {
        patch.push_str("\n+");
        patch.push_str(line);
    }
    if !text.ends_with('\n') {
        patch.push_str("\n\\ No newline at end of file");
    }
    Some(patch)
}

/// Whether the file's name looks like a secret: it is left out and named, never sent to a model.
pub fn looks_secret(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let example = ["example", "sample", "template", "dist"]
        .iter()
        .any(|word| name.ends_with(&format!(".{word}")));
    (name == ".env" || name.starts_with(".env.")) && !example
        || [".pem", ".key", ".p12", ".pfx", ".keystore", ".jks"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name.starts_with("id_ecdsa")
        || [".netrc", ".npmrc", ".pypirc"].contains(&name.as_str())
        || (name.starts_with("credentials") && !example)
        || (name.contains("service-account") && name.ends_with(".json"))
}

fn excluded(path: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|entry| {
        let entry = entry.trim().trim_matches('/');
        !entry.is_empty() && (path == entry || path.starts_with(&format!("{entry}/")))
    })
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// A regular file's bytes. A symbolic link is never followed, so a link with a plain name cannot send a key file.
fn read_file(path: &Path) -> Option<Vec<u8>> {
    if is_symlink(path) {
        return None;
    }
    std::fs::read(path).ok()
}

/// The working tree at `cwd`, against its merge base with the remote default branch, as a request body.
pub fn collect(cwd: &Path, options: &Options) -> Result<Collected, String> {
    let top = git(cwd, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "not in a git repository: run it from the repository you want reviewed".to_string())?;
    let top = Path::new(top.trim());
    let repo = match &options.repo {
        Some(repo) => repo.clone(),
        None => {
            let url = git(top, &["remote", "get-url", "origin"])
                .map_err(|_| "no origin remote: name the repository with --repo owner/name".to_string())?;
            repo_of_remote(&url).ok_or_else(|| {
                format!(
                    "origin ({}) is not a GitHub repository: name it with --repo owner/name",
                    url.trim()
                )
            })?
        }
    };
    let branch = git(top, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
        .ok()
        .and_then(|name| name.trim().strip_prefix("origin/").map(str::to_string))
        .unwrap_or_else(|| "main".to_string());
    let base_sha = git(top, &["merge-base", "HEAD", &format!("origin/{branch}")])
        .map_err(|error| format!("could not find the merge base with origin/{branch} (fetch it first): {error}"))?
        .trim()
        .to_string();
    let diff = git(
        top,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--find-renames",
            "-U3",
            &base_sha,
            "--",
        ],
    )?;
    let mut files = parse_diff(&diff);
    let mut left_out = Vec::new();
    let untracked = git(top, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked.split('\0').filter(|path| !path.is_empty()) {
        if is_symlink(&top.join(path)) {
            left_out.push((path.to_string(), "a symbolic link".to_string()));
            continue;
        }
        let text = read_file(&top.join(path)).and_then(|bytes| String::from_utf8(bytes).ok());
        files.push(DiffFile {
            path: path.to_string(),
            previous_path: None,
            status: "added",
            patch: text.as_deref().and_then(added_patch),
        });
    }
    let mut entries = Vec::new();
    for file in files {
        // A renamed secret still carries its old text as removed lines: its old name counts too.
        if looks_secret(&file.path) || file.previous_path.as_deref().is_some_and(looks_secret) {
            left_out.push((file.path, "looks like a secret".to_string()));
            continue;
        }
        if excluded(&file.path, &options.exclude) {
            left_out.push((file.path, "--exclude".to_string()));
            continue;
        }
        let head_text = (file.status != "removed")
            .then(|| read_file(&top.join(&file.path)))
            .flatten()
            .filter(|bytes| bytes.len() <= MAX_HEAD_TEXT)
            .and_then(|bytes| String::from_utf8(bytes).ok());
        let mut entry = json!({ "path": file.path, "status": file.status });
        if let Some(previous) = &file.previous_path {
            entry["previous_path"] = json!(previous);
        }
        if let Some(patch) = &file.patch {
            entry["patch"] = json!(patch);
        }
        if let Some(text) = head_text {
            entry["head_text"] = json!(text);
        }
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err(format!(
            "nothing to review: the working tree matches its merge base with origin/{branch}"
        ));
    }
    let ahead = git(top, &["log", "--format=%s", &format!("{base_sha}..HEAD")]).unwrap_or_default();
    let title = ahead
        .lines()
        .next()
        .filter(|line| !line.trim().is_empty())
        .unwrap_or(LOCAL_TITLE);
    let mut body = json!({
        "repo": repo,
        "local": true,
        "base_sha": base_sha,
        "base_ref": branch,
        "title": title,
        "files": entries,
    });
    if let Some(model) = &options.model {
        body["model"] = json!(model);
    }
    let size = body.to_string().len();
    if size > MAX_BODY_BYTES {
        return Err(too_big(&body, size));
    }
    Ok(Collected {
        repo,
        base_sha,
        body,
        left_out,
    })
}

/// The refusal for a change over [`MAX_BODY_BYTES`], naming its five largest files.
fn too_big(body: &Value, size: usize) -> String {
    let mut sizes: Vec<(usize, &str)> = body["files"]
        .as_array()
        .map(|files| {
            files
                .iter()
                .map(|file| (file.to_string().len(), file["path"].as_str().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    sizes.sort_by(|left, right| right.cmp(left));
    let largest: Vec<String> = sizes
        .iter()
        .take(5)
        .map(|(bytes, path)| format!("{path} ({} KB)", bytes.div_ceil(1000)))
        .collect();
    format!(
        "the change is {} KB, over the {} KB a local review sends: leave files out with --exclude <path,…>. Largest: {}",
        size.div_ceil(1000),
        MAX_BODY_BYTES / 1000,
        largest.join(", ")
    )
}

/// Sends a request to Gateway: `(method, path, body)`.
pub type Send<'a> = dyn Fn(reqwest::Method, &str, Option<Value>) -> Result<Value, String> + 'a;

/// Start the run and wait for its report (polling every `poll`); the report as Gateway answers it.
pub fn review(send: &Send<'_>, product: &str, collected: &Collected, poll: Duration) -> Result<Value, String> {
    let started = send(
        reqwest::Method::POST,
        &format!("/v1/{product}/review/run"),
        Some(collected.body.clone()),
    )?;
    let id = started["id"]
        .as_str()
        .ok_or_else(|| format!("the run answer has no id: {started}"))?
        .to_string();
    let deadline = Instant::now() + WAIT;
    loop {
        let report = send(
            reqwest::Method::GET,
            &format!("/v1/{product}/review/report/{}", nexus_client::url::enc(&id)),
            None,
        )?;
        // The report's status is the module contract's: a queued or paused run is `running` there. Only
        // `succeeded` and `failed` end the wait; anything else is not a review the exit code may pass.
        match report["status"].as_str() {
            Some("succeeded" | "failed") => return Ok(report),
            Some("running") => {}
            other => return Err(format!("run {id} answered an unknown status {other:?}: {report}")),
        }
        if Instant::now() > deadline {
            return Err(format!(
                "run {id} is still running after {} minutes: follow it with nexus {product} report review {id}",
                WAIT.as_secs() / 60
            ));
        }
        std::thread::sleep(poll);
    }
}

/// What `--json` prints and the MCP tool answers: the run, its findings, whether one fails the check, what was left
/// out.
pub fn result(report: &Value, collected: &Collected) -> Value {
    let local = &report["local"];
    json!({
        "id": report["id"],
        "status": report["status"],
        "label": report["label"],
        "error": report["error"],
        "repo": collected.repo,
        "base_sha": collected.base_sha,
        "cost_usd": report["costUsd"],
        "check_fails_on": local["check_fails_on"],
        "fails": local["fails"].as_bool().unwrap_or(false),
        "findings": local["findings"].as_array().cloned().unwrap_or_default(),
        "left_out": collected
            .left_out
            .iter()
            .map(|(path, why)| json!({ "path": path, "why": why }))
            .collect::<Vec<_>>(),
    })
}

/// One finding as a line: `path:line severity title`.
pub fn finding_line(finding: &Value) -> String {
    let start = finding["line_start"].as_u64().unwrap_or(0);
    let end = finding["line_end"].as_u64().unwrap_or(start);
    let lines = if end > start {
        format!("{start}-{end}")
    } else {
        start.to_string()
    };
    format!(
        "{}:{lines} {} {}",
        finding["path"].as_str().unwrap_or_default(),
        finding["severity"].as_str().unwrap_or_default(),
        finding["title"].as_str().unwrap_or_default()
    )
}

/// The exit code: 1 when the run failed or a finding reaches `check_fails_on`.
pub fn exit_code(result: &Value) -> i32 {
    i32::from(result["status"] == "failed" || result["fails"] == true)
}

/// `nexus <product> run review --local [--repo owner/name] [--model id] [--exclude path,…] [--json]`.
pub fn command(product: &str, rest: &[String], cmd: &crate::cli::Cmd) -> crate::error::Result<i32> {
    use crate::error::CliError;
    let capability = rest
        .get(1)
        .map(String::as_str)
        .or(cmd.flag("local").filter(|value| !value.is_empty() && *value != "true"))
        .unwrap_or("review");
    if capability != "review" {
        return crate::error::usage(
            format!("only review runs --local, not {capability}"),
            Some(&format!("nexus {product} run review --local")),
        );
    }
    let options = Options {
        repo: cmd.flag_set_str("repo"),
        model: cmd.flag_set_str("model"),
        exclude: cmd
            .flag_set_str("exclude")
            .map(|list| list.split(',').map(|entry| entry.trim().to_string()).collect())
            .unwrap_or_default(),
    };
    let collected = collect(&cmd.ctx.cwd, &options).map_err(CliError::new)?;
    let style = cmd.style;
    if !cmd.json {
        for (path, why) in &collected.left_out {
            cmd.out(format!("{} left out {path}: {why}", style.warn));
        }
    }
    let send = |method: reqwest::Method, path: &str, body: Option<Value>| {
        cmd.core.call(method, path, body).map_err(|error| match error {
            crate::error::Error::Core(core) => core.message.clone(),
            other => format!("{other:?}"),
        })
    };
    cmd.ctx.io.progress(Some("Reviewing the working tree…"));
    let poll = Duration::from_millis(cmd.ctx.poll_ms.unwrap_or(1500));
    let report = review(&send, product, &collected, poll);
    cmd.ctx.io.progress(None);
    let report = report.map_err(CliError::new)?;
    let result = result(&report, &collected);
    if cmd.json {
        cmd.print(&result);
        return Ok(exit_code(&result));
    }
    if result["status"] == "failed" {
        return Err(CliError::new(format!(
            "the local review failed: {}",
            result["error"].as_str().unwrap_or("no reason given")
        ))
        .into());
    }
    let findings = result["findings"].as_array().cloned().unwrap_or_default();
    for finding in &findings {
        cmd.out(finding_line(finding));
    }
    if let Some(note) = result["error"].as_str() {
        cmd.out(format!("{} {note}", style.warn));
    }
    let fails_on = result["check_fails_on"].as_str().unwrap_or("blocker");
    let verdict = if result["fails"] == true {
        format!("{} a finding reaches {fails_on}: the check would fail", style.fail)
    } else {
        format!("{} nothing reaches {fails_on}", style.ok)
    };
    cmd.out(format!(
        "{verdict} {} {} finding{} {} run {}",
        style.bullet,
        findings.len(),
        if findings.len() == 1 { "" } else { "s" },
        style.bullet,
        result["id"].as_str().unwrap_or_default()
    ));
    Ok(exit_code(&result))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,3 @@
 fn a() {}
-fn b() {}
+fn c() {}
 fn d() {}
diff --git a/old name.rs b/new name.rs
similarity index 90%
rename from old name.rs
rename to new name.rs
index 3333333..4444444 100644
--- a/old name.rs
+++ b/new name.rs
@@ -1 +1 @@
-x
+y
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 5555555..0000000
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/logo.png b/logo.png
new file mode 100644
index 0000000..6666666
Binary files /dev/null and b/logo.png differ
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
";

    #[test]
    fn a_git_diff_reads_as_the_connectors_files() {
        let files = parse_diff(DIFF);
        let summary: Vec<(&str, Option<&str>, &str, bool)> = files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    file.previous_path.as_deref(),
                    file.status,
                    file.patch.is_some(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("src/lib.rs", None, "modified", true),
                ("new name.rs", Some("old name.rs"), "renamed", true),
                ("gone.txt", None, "removed", true),
                ("logo.png", None, "added", false),
                ("run.sh", None, "modified", false),
            ]
        );
        assert_eq!(
            files[0].patch.as_deref(),
            Some("@@ -1,3 +1,3 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n fn d() {}")
        );
    }

    #[test]
    fn an_untracked_file_is_added_line_by_line() {
        assert_eq!(added_patch("a\nb\n").as_deref(), Some("@@ -0,0 +1,2 @@\n+a\n+b"));
        assert_eq!(
            added_patch("a").as_deref(),
            Some("@@ -0,0 +1,1 @@\n+a\n\\ No newline at end of file")
        );
        assert_eq!(added_patch(""), None);
    }

    #[test]
    fn remotes_name_the_repository() {
        for (url, repo) in [
            ("https://github.com/nexus-xyz/nexus.git\n", Some("nexus-xyz/nexus")),
            ("git@github.com:nexus-xyz/nexus.git", Some("nexus-xyz/nexus")),
            ("ssh://git@github.com/acme/app", Some("acme/app")),
            ("https://gitlab.com/acme/app.git", None),
            ("https://github.com/acme", None),
        ] {
            assert_eq!(repo_of_remote(url).as_deref(), repo, "{url}");
        }
    }

    #[test]
    fn secrets_are_left_out_and_examples_are_not() {
        for secret in [
            ".env",
            "app/.env.local",
            "certs/server.pem",
            "id_rsa",
            "deploy/service-account.json",
        ] {
            assert!(looks_secret(secret), "{secret}");
        }
        for plain in [".env.example", "src/env.rs", "keys.md", "docs/credentials.sample"] {
            assert!(!looks_secret(plain), "{plain}");
        }
        assert!(excluded("vendor/lib/a.rs", &["vendor/".into()]));
        assert!(excluded("big.json", &["big.json".into()]));
        assert!(!excluded("vendored.rs", &["vendor".into()]));
    }

    #[test]
    fn a_change_too_big_names_its_largest_files() {
        let body = json!({ "files": [
            { "path": "small.rs", "patch": "x" },
            { "path": "huge.json", "patch": "y".repeat(5_000) },
        ] });
        let message = too_big(&body, 950_000);
        assert!(message.contains("950 KB") && message.contains("--exclude"), "{message}");
        assert!(message.find("huge.json") < message.find("small.rs"), "{message}");
    }

    #[test]
    fn findings_print_as_path_line_severity_title_and_set_the_exit_code() {
        let finding =
            json!({ "path": "src/a.rs", "line_start": 3, "line_end": 5, "severity": "major", "title": "No timeout" });
        assert_eq!(finding_line(&finding), "src/a.rs:3-5 major No timeout");
        assert_eq!(exit_code(&json!({ "status": "succeeded", "fails": false })), 0);
        assert_eq!(exit_code(&json!({ "status": "succeeded", "fails": true })), 1);
        assert_eq!(exit_code(&json!({ "status": "failed", "fails": false })), 1);
    }

    #[test]
    fn review_waits_until_the_run_ends() {
        use std::cell::RefCell;
        let collected = Collected {
            repo: "acme/app".into(),
            base_sha: "b".repeat(40),
            body: json!({}),
            left_out: vec![],
        };
        let answers = |statuses: &[&str]| {
            RefCell::new(
                statuses
                    .iter()
                    .rev()
                    .map(|status| json!({ "id": "run_1", "status": status }))
                    .collect::<Vec<_>>(),
            )
        };
        // A queued run reads `running` in the module contract: the CLI keeps polling until it ends.
        let queue = answers(&["running", "running", "succeeded"]);
        let polls = RefCell::new(0);
        let send = |method: reqwest::Method, _: &str, _: Option<Value>| -> Result<Value, String> {
            if method == reqwest::Method::POST {
                return Ok(json!({ "id": "run_1", "status": "running" }));
            }
            *polls.borrow_mut() += 1;
            queue.borrow_mut().pop().ok_or_else(|| "no more answers".to_string())
        };
        let report = review(&send, "robustness", &collected, Duration::ZERO).expect("ends");
        assert_eq!((report["status"].as_str(), *polls.borrow()), (Some("succeeded"), 3));
        // A report with no status is an error, never a pass.
        let queue = answers(&["running", ""]);
        queue.borrow_mut()[0] = json!({ "id": "run_1" });
        let send = |method: reqwest::Method, _: &str, _: Option<Value>| -> Result<Value, String> {
            if method == reqwest::Method::POST {
                return Ok(json!({ "id": "run_1" }));
            }
            queue.borrow_mut().pop().ok_or_else(|| "no more answers".to_string())
        };
        assert!(review(&send, "robustness", &collected, Duration::ZERO).is_err());
    }

    /// A real repository in a temporary folder: a commit on `main`, then a modified, an untracked and a secret file.
    #[test]
    fn collect_reads_the_working_tree_against_the_merge_base() {
        let dir = std::env::temp_dir().join(format!("nexus-local-review-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        // Isolated from the person's own git config: no signing, no hooks.
        let run = |args: &[&str]| {
            let mut all = vec!["-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null"];
            all.extend_from_slice(args);
            git(&dir, &all).expect("git")
        };
        if git(&dir, &["--version"]).is_err() {
            return;
        }
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(dir.join("a.rs"), "fn a() {}\n").expect("write");
        std::fs::write(dir.join(".env"), "A=1\nB=2\nC=3\nOLD=hunter2\n").expect("write");
        run(&["add", "-f", "a.rs", ".env"]);
        run(&["commit", "-q", "-m", "base"]);
        run(&["remote", "add", "origin", "https://github.com/acme/app.git"]);
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let base = run(&["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(dir.join("a.rs"), "fn a() {}\nfn b() {}\n").expect("write");
        std::fs::write(dir.join("new.rs"), "fn n() {}\n").expect("write");
        std::fs::write(dir.join(".env.local"), "TOKEN=secret\n").expect("write");
        // A renamed secret and an untracked link to a key file are left out too.
        run(&["mv", "-f", ".env", "env.txt"]);
        std::fs::write(dir.join("env.txt"), "A=1\nB=2\nC=3\nOLD=hunter2\nD=4\n").expect("write");
        let key = std::env::temp_dir().join(format!("nexus-local-review-key-{}", std::process::id()));
        std::fs::write(&key, "PRIVATE KEY\n").expect("write");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&key, dir.join("notes.txt")).expect("symlink");
        let collected = collect(&dir, &Options::default()).expect("collects");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&key);
        assert_eq!(
            (collected.repo.as_str(), collected.base_sha.as_str()),
            ("acme/app", base.as_str())
        );
        // `.env.local` is left out by name (or already hidden by the person's global ignore file), `env.txt` by its
        // old name, and the link is never followed.
        assert!(collected.left_out.iter().all(|(path, why)| match path.as_str() {
            ".env.local" | "env.txt" => why == "looks like a secret",
            "notes.txt" => why == "a symbolic link",
            _ => false,
        }));
        assert!(collected.left_out.iter().any(|(path, _)| path == "env.txt"));
        #[cfg(unix)]
        assert!(collected.left_out.iter().any(|(path, _)| path == "notes.txt"));
        let body = &collected.body;
        assert_eq!(
            (body["local"].clone(), body["base_ref"].clone()),
            (json!(true), json!("main"))
        );
        assert_eq!(body["title"], LOCAL_TITLE);
        let files = body["files"].as_array().expect("files");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["path"], "a.rs");
        assert_eq!(files[0]["head_text"], "fn a() {}\nfn b() {}\n");
        assert_eq!(files[0]["patch"], "@@ -1 +1,2 @@\n fn a() {}\n+fn b() {}");
        assert_eq!(
            (files[1]["path"].clone(), files[1]["status"].clone()),
            (json!("new.rs"), json!("added"))
        );
        let sent = body.to_string();
        for secret in ["TOKEN", "hunter2", "PRIVATE KEY"] {
            assert!(!sent.contains(secret), "{secret} is never sent");
        }
    }
}
