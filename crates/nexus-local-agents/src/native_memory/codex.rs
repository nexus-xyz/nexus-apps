use std::path::Path;
use std::process::Command;
use std::{fs, io};

use nexus_agent_memory::{Entry, NativeEntry, Repository, Scope, markdown};

use super::{lock, marked, note_metadata, read as read_file, write as write_file};

const INSTRUCTIONS: &str = "AGENTS.md";

const PROJECT_MEMORY: &str = ".codex/memory/project";

pub(crate) fn read(home: &Path, _repo: &Path) -> io::Result<Vec<NativeEntry>> {
    let mut entries = Vec::new();
    if let Some(text) = read_file(&home.join(INSTRUCTIONS))? {
        entries.push(NativeEntry {
            scope: Scope::Personal,
            file: INSTRUCTIONS.into(),
            name: None,
            description: None,
            text: marked::without(&text, |_| true),
        });
    }
    // Codex keeps versioned memory directories. Import the latest handbook present.
    for file in ["memories_v2/MEMORY.md", "memories/MEMORY.md"] {
        if let Some(text) = read_file(&home.join(file))? {
            entries.push(NativeEntry {
                scope: Scope::Personal,
                file: file.into(),
                name: None,
                description: None,
                text,
            });
            break;
        }
    }
    Ok(entries)
}

pub(super) fn read_project(cwd: &Path, repository: &Repository) -> io::Result<Vec<(String, NativeEntry)>> {
    let mut entries = Vec::new();
    for checkout in Repository::worktrees(cwd) {
        if !checkout.join(PROJECT_MEMORY).is_dir() {
            continue;
        }
        let mut git = Command::new("git");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            git.creation_flags(0x08000000);
        }
        let files = git
            .current_dir(&checkout)
            .args([
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
                "--",
                PROJECT_MEMORY,
            ])
            .output()?;
        if !files.status.success() {
            return Err(io::Error::other(format!(
                "Couldn't list local Codex notes in {}.",
                checkout.display()
            )));
        }

        let profile = checkout.to_string_lossy().into_owned();
        for file in files
            .stdout
            .split(|byte| *byte == 0)
            .filter_map(|name| std::str::from_utf8(name).ok())
        {
            let relative = Path::new(file);
            if relative.parent() != Some(Path::new(PROJECT_MEMORY))
                || relative.extension().is_none_or(|ext| ext != "md")
            {
                continue;
            }
            let source = checkout.join(relative);
            if fs::symlink_metadata(&source)?.is_file()
                && let Some(text) = read_file(&source)?
            {
                let (metadata, body) = note_metadata(&text);
                entries.push((
                    profile.clone(),
                    NativeEntry {
                        scope: repository.scope(),
                        file: file.into(),
                        name: metadata.name,
                        description: metadata.description,
                        text: body.into(),
                    },
                ));
            }
        }
    }
    Ok(entries)
}

pub(crate) fn write(home: &Path, cwd: &Path, entries: &[Entry]) -> io::Result<()> {
    let _lock = lock(home)?;
    let path = home.join(INSTRUCTIONS);
    let old = read_file(&path)?.unwrap_or_default();
    if entries.is_empty() && old.is_empty() {
        return Ok(());
    }
    let (personal, repository): (Vec<_>, Vec<_>) = entries
        .iter()
        .cloned()
        .partition(|entry| entry.scope == Scope::Personal);
    // The earlier single block is replaced on the next export. The local store retains its entries.
    let old = marked::without(&old, |key| key.is_empty());
    let mut output = marked::replace(&old, "personal", &markdown(&personal));
    if let Some(repo) = Repository::discover(cwd) {
        let key = format!("repository-{}", repo.fingerprint());
        output = marked::replace(&output, &key, &markdown(&repository));
    }
    write_file(&path, &output)
}
