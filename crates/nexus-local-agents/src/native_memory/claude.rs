use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use nexus_agent_memory::{Entry, NativeEntry, Repository, preview};

use crate::agents::claude_project_name;

use super::{lock, marked, read as read_file, without_frontmatter, write as write_file};

const INDEX: &str = "MEMORY.md";
const EXPORTED_TOPIC: &str = "<!-- nexus-shared-memory:topic -->";

fn folder(home: &Path, checkout: &Path) -> PathBuf {
    home.join("projects").join(claude_project_name(checkout)).join("memory")
}

fn profile_override(home: &Path) -> io::Result<Option<PathBuf>> {
    let Some(settings) = read_file(&home.join("settings.json"))? else {
        return Ok(None);
    };
    let Some(value) = serde_json::from_str::<serde_json::Value>(&settings).ok() else {
        return Ok(None);
    };
    let Some(dir) = value["autoMemoryDirectory"].as_str() else {
        return Ok(None);
    };
    let path = if let Some(relative) = dir.strip_prefix("~/").or_else(|| dir.strip_prefix("~\\")) {
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)
                )
            })
        {
            return Ok(None);
        }
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
            return Ok(None);
        };
        PathBuf::from(home).join(relative)
    } else {
        PathBuf::from(dir)
    };
    Ok((path.is_absolute() && path.parent().is_some()).then_some(path))
}

pub(crate) fn read(home: &Path, cwd: &Path) -> io::Result<Vec<NativeEntry>> {
    let repo = Repository::discover(cwd).ok_or_else(no_repo)?;
    let mut notes = Vec::new();
    let mut folders: BTreeSet<_> = Repository::worktrees(cwd)
        .iter()
        .map(|checkout| folder(home, checkout))
        .collect();
    if let Some(custom) = profile_override(home)? {
        folders.insert(custom);
    }
    let mut seen = HashSet::new();
    for dir in folders {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if !seen.insert(canonical) {
            continue;
        }
        let files = match fs::read_dir(&dir) {
            Ok(files) => files,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let mut files = files.collect::<Result<Vec<_>, _>>()?;
        files.sort_by_key(|file| file.file_name());
        for file in files {
            if !file.file_type()?.is_file() || file.path().extension().is_none_or(|ext| ext != "md") {
                continue;
            }
            let Some(text) = read_file(&file.path())? else { continue };
            if text.lines().any(|line| line == EXPORTED_TOPIC) {
                continue;
            }
            let name = file.file_name().to_string_lossy().into_owned();
            let text = if name == INDEX {
                marked::without(&text, |key| key.is_empty())
            } else {
                without_frontmatter(&text).to_string()
            };
            notes.push(NativeEntry {
                scope: repo.scope(),
                file: file
                    .path()
                    .strip_prefix(home.join("projects"))
                    .unwrap_or(&file.path())
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
                text,
            });
        }
    }
    Ok(notes)
}

pub(crate) fn write(home: &Path, cwd: &Path, entries: &[Entry]) -> io::Result<()> {
    let checkout = Repository::checkout(cwd).ok_or_else(no_repo)?;
    // Claude's canonical Git root follows a linked checkout back to the primary checkout.
    let primary = Repository::worktrees(cwd).into_iter().next().unwrap_or(checkout);
    let dir = profile_override(home)?.unwrap_or_else(|| folder(home, &primary));
    let _lock = lock(&dir)?;
    let index = dir.join(INDEX);
    let old = read_file(&index)?.unwrap_or_default();
    let mut links = String::new();
    let mut exported = HashSet::new();
    for entry in entries {
        let name = format!("nexus-shared-{}", entry.id);
        let filename = format!("{name}.md");
        let summary = serde_json::to_string(&preview(&entry.text))?;
        let topic = format!(
            "---\nname: {name}\ndescription: {summary}\ntype: project\n---\n{EXPORTED_TOPIC}\n\n{}\n",
            entry.text
        );
        write_file(&dir.join(&filename), &topic)?;
        exported.insert(filename.clone());
        links.push_str(&format!("- [{name}]({filename}) - {}\n", preview(&entry.text)));
    }
    if !entries.is_empty() || !old.is_empty() {
        write_file(&index, &marked::replace(&old, "", &links))?;
    }
    for file in fs::read_dir(&dir)? {
        let file = file?;
        if !file.file_type()?.is_file() {
            continue;
        }
        let name = file.file_name();
        let Some(name) = name.to_str() else { continue };
        if exported.contains(name) || !is_export_name(name) {
            continue;
        }
        let path = file.path();
        if read_file(&path)?.is_some_and(|text| text.lines().any(|line| line == EXPORTED_TOPIC)) {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn is_export_name(name: &str) -> bool {
    name.strip_prefix("nexus-shared-")
        .and_then(|name| name.strip_suffix(".md"))
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}

fn no_repo() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Open a Git repository to exchange Claude Code memory.",
    )
}
