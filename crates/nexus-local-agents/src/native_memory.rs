//! Explicit exchanges with the formats of locally installed coding agents.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use nexus_agent_memory::{Repository, Store};
use nexus_agents::Agent;

use crate::agents::local;

pub(crate) mod claude;
pub(crate) mod codex;
mod marked;

pub fn supported(agent: Agent) -> bool {
    matches!(agent, Agent::Claude | Agent::Codex)
}

pub(crate) fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "This agent has no native memory format in Nexus. Use Markdown export.",
    ))
}

pub fn import(store: &Store, agent: Agent, profiles: &[impl AsRef<Path>], cwd: &Path) -> io::Result<usize> {
    if !store.enabled()? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Shared memory is off. Turn it on to import entries.",
        ));
    }
    let repo = Repository::discover(cwd);
    let mut notes = Vec::new();
    for profile in profiles {
        let profile = profile.as_ref();
        let key = profile
            .canonicalize()
            .unwrap_or_else(|_| profile.to_path_buf())
            .to_string_lossy()
            .into_owned();
        notes.extend(
            local(agent)
                .memory(profile, cwd)?
                .into_iter()
                .map(|note| (key.clone(), note)),
        );
    }
    if agent == Agent::Codex
        && let Some(repo) = &repo
    {
        notes.extend(codex::read_project(cwd, repo)?);
    }
    store.import(repo.as_ref(), agent.as_str(), notes)
}

pub fn export(store: &Store, agent: Agent, profile: &Path, cwd: &Path) -> io::Result<usize> {
    let repo = Repository::discover(cwd);
    let mut entries = store.entries(repo.as_ref())?;
    entries.retain(|entry| {
        !matches!(&entry.source,
        nexus_agent_memory::Source::Native { agent: imported, .. } if imported == agent.as_str())
    });
    local(agent).write_memory(profile, cwd, &entries)?;
    Ok(entries.len())
}

fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn lock(dir: &Path) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    // Match the lock used by nexus-cloud's agent_md_in for profile instructions.
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".nexus-md.lock"))?;
    lock.lock()?;
    Ok(lock)
}

fn write(path: &Path, text: &str) -> io::Result<()> {
    // Profiles can link their instructions to a dotfiles checkout.
    let existing = fs::symlink_metadata(path);
    let target = match existing {
        Ok(_) => path.canonicalize()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => return Err(error),
    };
    let mut file = tempfile::NamedTempFile::new_in(target.parent().expect("memory file parent"))?;
    file.write_all(text.as_bytes())?;
    if let Ok(metadata) = fs::metadata(&target) {
        file.as_file().set_permissions(metadata.permissions())?;
    }
    file.as_file().sync_all()?;
    file.persist(target)?;
    Ok(())
}

fn without_frontmatter(text: &str) -> &str {
    let mut lines = text.split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return text;
    }
    let mut offset = text.find('\n').map_or(text.len(), |index| index + 1);
    for line in lines {
        offset += line.len();
        if line.trim_end() == "---" {
            return &text[offset..];
        }
    }
    text
}

#[cfg(test)]
mod tests;
