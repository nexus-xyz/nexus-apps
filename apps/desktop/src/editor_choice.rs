//! One saved editor preference for session folders and file previews.
use std::path::{Path, PathBuf};

use nexus_workbench::editors;

fn choice_file() -> Option<PathBuf> {
    crate::paths::own_data_dir().map(|dir| dir.join("editor-choice"))
}

pub(crate) fn saved() -> Option<String> {
    std::fs::read_to_string(choice_file()?).ok()
}

pub(crate) fn save(editor: &str) {
    if let Some(file) = choice_file() {
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = nexus_desktop_core::atomic_file::write_atomic(&file, editor.as_bytes());
    }
}

/// Session tool rows use the same editor preference and launcher as guarded Files previews.
pub(crate) fn open_file(root: &Path, file: &str, line: Option<u32>) -> Result<(), String> {
    open_file_guarded(root, file, line, &|| true)
}

/// Use the saved available editor, then the preferred one, or the system opener.
/// The shared launcher validates containment, preserves argument boundaries, and handles line numbers.
pub(crate) fn open_file_guarded(
    root: &Path,
    file: &str,
    line: Option<u32>,
    valid: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(), String> {
    choose_file(
        root,
        file,
        line,
        valid,
        || {
            let path = crate::code_run::agent_env().remove("PATH").unwrap_or_default();
            (editors::installed(&path), saved(), path)
        },
        |request, path, valid| {
            editors::open_file_guarded(
                &request.root,
                &request.file,
                request.line,
                None,
                request.editor.as_deref(),
                path,
                valid,
            )
        },
    )
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileRequest {
    pub root: PathBuf,
    pub file: String,
    pub line: Option<u32>,
    pub editor: Option<String>,
}
fn choose_file(
    root: &Path,
    file: &str,
    line: Option<u32>,
    valid: &(dyn Fn() -> bool + Send + Sync),
    load: impl FnOnce() -> (Vec<editors::Editor>, Option<String>, String),
    launch: impl FnOnce(FileRequest, &str, &(dyn Fn() -> bool + Send + Sync)) -> Result<(), String>,
) -> Result<(), String> {
    if !valid() {
        return Err("This file view is no longer active.".into());
    }
    let (found, choice, path) = load();
    let selected = editors::preferred(&found, choice.as_deref());
    if !valid() {
        return Err("This file view is no longer active.".into());
    }
    launch(
        FileRequest {
            root: root.into(),
            file: file.into(),
            line,
            editor: selected.map(|entry| entry.id.into()),
        },
        &path,
        valid,
    )
}

#[cfg(test)]
type FixtureLoad = std::sync::Arc<dyn Fn() -> (Vec<editors::Editor>, Option<String>, String) + Send + Sync>;
#[cfg(test)]
type FixtureLaunch =
    std::sync::Arc<dyn Fn(FileRequest, &(dyn Fn() -> bool + Send + Sync)) -> Result<(), String> + Send + Sync>;
/// Per-instance editor dependencies: never reads/writes saved preferences, PATH or launches an application.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct Fixture {
    pub load: FixtureLoad,
    pub launch: FixtureLaunch,
}
#[cfg(test)]
impl Fixture {
    pub fn open(
        &self,
        root: &Path,
        file: &str,
        line: Option<u32>,
        valid: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<(), String> {
        choose_file(
            root,
            file,
            line,
            valid,
            || (self.load)(),
            |request, _, valid| (self.launch)(request, valid),
        )
    }
}
