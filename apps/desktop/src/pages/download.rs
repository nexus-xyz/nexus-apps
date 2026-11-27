//! Saving a file Gateway writes out as text (an export): read it, ask where to save it with the system's save dialog
//! (opened in Downloads), and write it there. The web pages save the same answer with a browser download.

use std::path::PathBuf;

use gpui_kit::*;

/// How a download ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Saved {
    /// Written to this file, with this text.
    File { path: PathBuf, text: String },
    /// The person closed the save dialog.
    Cancelled,
    /// Gateway refused it, or the file could not be written.
    Failed(String),
}

/// Where the save dialog opens: the Downloads folder, else home.
pub fn downloads_folder() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let downloads = home.join("Downloads");
    if downloads.is_dir() { downloads } else { home }
}

/// Today's date in UTC, `2026-11-06`, as the web names its downloads (`toISOString().slice(0, 10)`).
pub fn today_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// A file-name part from an organization's slug: letters, digits, `-` and `_` only, as Gateway names its exports.
pub fn name_part(text: &str) -> String {
    let part: String = text
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if part.is_empty() { "org".into() } else { part }
}

/// A saved file's name, for a note: `nexus-audit-acme-2026-11-06.csv`.
pub fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The HTTP status in `Gateway::text`'s refusal (`Gateway answered 403.`), if it has one.
pub fn status_of(error: &str) -> Option<u16> {
    error
        .strip_prefix("Gateway answered ")?
        .trim_end_matches('.')
        .trim()
        .parse()
        .ok()
}

/// Read `request`, then save its text as `name` where the person picks; `done` gets how it ended.
pub fn save_text<V: 'static>(
    cx: &mut Context<V>,
    request: impl std::future::Future<Output = Result<String, String>> + 'static,
    name: String,
    done: impl FnOnce(&mut V, Saved, &mut Context<V>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let saved = match request.await {
            Err(error) => Saved::Failed(error),
            Ok(text) => {
                let folder = downloads_folder();
                let picked = cx.update(|cx| cx.prompt_for_new_path(&folder, Some(&name)));
                match picked.await {
                    Ok(Ok(Some(path))) => match std::fs::write(&path, &text) {
                        Ok(()) => Saved::File { path, text },
                        Err(error) => Saved::Failed(format!("Couldn't save {}: {error}", path.display())),
                    },
                    Ok(Ok(None)) | Err(_) => Saved::Cancelled,
                    Ok(Err(error)) => Saved::Failed(error.to_string()),
                }
            }
        };
        this.update(cx, |view, cx| {
            done(view, saved, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::{name_part, status_of};

    #[test]
    fn names_files_as_gateway() {
        assert_eq!(name_part("acme"), "acme");
        assert_eq!(name_part("acme corp/../x"), "acmecorpx");
        assert_eq!(name_part(""), "org");
        assert_eq!(status_of("Gateway answered 403."), Some(403));
        assert_eq!(status_of("connection refused"), None);
    }
}
