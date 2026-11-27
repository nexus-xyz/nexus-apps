// The version for build scripts (`include!`d by the build.rs files that need it): one source of
// truth, the `VERSION` file (a bare semver, e.g. `0.3.0`), plus a build suffix, the git short sha:
// `NEXUS_BUILD_SHA` when set (a Docker or CI build without .git), else `git rev-parse --short=7 HEAD`; with neither
// the version is the bare semver. `factory_version(manifest)` returns `(semver, version)`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The `VERSION` file, from a crate's manifest directory.
fn version_file(manifest: &Path) -> PathBuf {
    manifest.join("../../VERSION")
}

/// `(semver, semver+sha)`; panics, naming the file, when `VERSION` is missing or not a semver.
fn factory_version(manifest: &Path) -> (String, String) {
    let file = version_file(manifest);
    println!("cargo:rerun-if-changed={}", file.display());
    println!("cargo:rerun-if-env-changed=NEXUS_BUILD_SHA");
    let semver = std::fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("{}: {error} (the repository's version file)", file.display()))
        .trim()
        .to_string();
    let core = semver.split('-').next().unwrap_or_default();
    let valid = core.split('.').count() == 3
        && core
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        && semver
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !valid {
        panic!(
            "{}: {semver:?} is not a semver (MAJOR.MINOR.PATCH[-prerelease], no build)",
            file.display()
        );
    }
    let sha = match std::env::var("NEXUS_BUILD_SHA") {
        Ok(sha) => sha.trim().to_string(),
        Err(_) => {
            watch_git_head(manifest);
            git(manifest, &["rev-parse", "--short=7", "HEAD"]).unwrap_or_default()
        }
    };
    let sha: String = sha.chars().take(12).collect();
    if !sha.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        panic!("NEXUS_BUILD_SHA must be letters, digits and hyphens: {sha}");
    }
    let version = if sha.is_empty() {
        semver.clone()
    } else {
        format!("{semver}+{sha}")
    };
    (semver, version)
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).current_dir(dir).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Rebuild when HEAD moves (a commit or a checkout), in a plain checkout or a worktree.
fn watch_git_head(dir: &Path) {
    if let Some(head) = git(dir, &["rev-parse", "--git-path", "HEAD"]) {
        let head = dir.join(head);
        println!("cargo:rerun-if-changed={}", head.display());
        if let Ok(text) = std::fs::read_to_string(&head)
            && let Some(reference) = text.strip_prefix("ref: ")
            && let Some(path) = git(dir, &["rev-parse", "--git-path", reference.trim()])
        {
            let reference = dir.join(path);
            if reference.exists() {
                println!("cargo:rerun-if-changed={}", reference.display());
            } else if let Some(packed) = git(dir, &["rev-parse", "--git-common-dir"]) {
                println!(
                    "cargo:rerun-if-changed={}",
                    dir.join(packed).join("packed-refs").display()
                );
            }
        }
    }
}
