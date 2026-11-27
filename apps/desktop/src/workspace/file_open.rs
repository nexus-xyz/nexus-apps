//! Where a file chip in a Code session's answer opens its file (`file_chips.rs`).
//!
//! A click opens the file in the Workbench's Files tool at its line. A format Files cannot show opens in the system
//! app, when the session runs on this computer and the format is a document one (`output_files::openable`); otherwise
//! Files opens it and says why it has no preview. Ctrl-click opens the editor, as a tool row does (`tool_body.rs`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use nexus_workbench::file_refs::{self, Opens};
use nexus_workbench::inline_media::FileReference;
use nexus_workbench::output_files;

use super::file_chips::{ChipClick, FileChips, PlaceSource};
use super::{View, Workspace};
use crate::palette::Palette;

/// How long a chip's notice stays.
const NOTICE: Duration = Duration::from_secs(4);

/// At most this many paths of the session's tool calls are kept for [`locate`], newest first.
const TOUCHED: usize = 512;

/// The absolute paths the session's tool calls named, sub-agents' included, newest first and once each
/// (`file_origin::touched_paths`): where the agent read and wrote, in any checkout.
pub(super) fn touched_files(blocks: &[crate::code::Block]) -> Vec<String> {
    fn tool(block: &crate::code::Block) -> Option<&crate::code::ToolCall> {
        match block {
            crate::code::Block::Tool { call, .. } => Some(call),
            crate::code::Block::Inside { block, .. } => tool(block),
            _ => None,
        }
    }
    let mut seen = std::collections::HashSet::new();
    blocks
        .iter()
        .rev()
        .filter_map(tool)
        .flat_map(|call| nexus_workbench::file_origin::touched_paths(&call.input))
        .filter(|path| seen.insert(path.clone()))
        .take(TOUCHED)
        .collect()
}

/// Where the file an answer names as `path` is, from the session's `folder` and the paths its calls `touched`
/// (`file_refs::locate`). Blocking: run it off the UI thread.
pub(super) async fn locate(folder: PathBuf, path: String, touched: Vec<String>) -> Option<String> {
    file_refs::locate(&folder, &path, &touched, |folder| {
        nexus_workbench::files::list(folder)
            .map(|list| list.files.to_vec())
            .unwrap_or_default()
    })
}

impl Workspace {
    /// The chips for the open Code session's answers; None outside a stored session.
    pub(super) fn file_chips(&self, palette: Palette, cx: &Context<Self>) -> Option<FileChips> {
        let View::Code(view) = &self.view else {
            return None;
        };
        if view.is_draft() {
            return None;
        }
        let local = crate::machines::get().machine_of(&view.id).is_none();
        let this = cx.entity().downgrade();
        let (session, workspace) = (view.id.clone(), this.clone());
        let place: PlaceSource = Arc::new(move |cx: &App| {
            let workspace = workspace.upgrade()?;
            let workspace = workspace.read(cx);
            let touched = match &workspace.view {
                View::Code(view) if view.id == session => touched_files(&view.blocks),
                _ => Vec::new(),
            };
            Some((workspace.session_place(&session), touched))
        });
        let handler = Arc::new(
            move |reference: &FileReference, click, window: &mut Window, cx: &mut App| {
                let reference = reference.clone();
                let opened = this.update(cx, |this, cx| match click {
                    ChipClick::Open => this.open_file_reference(reference, window, cx),
                    ChipClick::Editor => this.open_reference_in_editor(reference, cx),
                    ChipClick::Menu(position) => this.open_file_menu(reference, position, window, cx),
                });
                if let Err(error) = opened {
                    eprintln!("nexus-desktop-gpui: a file chip's window is gone: {error}");
                }
            },
        );
        Some(FileChips::new(handler, place, palette, local))
    }

    /// The open Code session's folder on this computer; None for a server's session or a draft.
    pub(super) fn reference_folder(&self) -> Option<PathBuf> {
        let View::Code(view) = &self.view else {
            return None;
        };
        if view.is_draft() || crate::machines::get().machine_of(&view.id).is_some() {
            return None;
        }
        self.code
            .as_ref()
            .and_then(|code| code.folder(&view.id).ok())
            .map(|folder| folder.path)
    }

    /// Open `reference` where it shows best: its app for a format Files cannot show, else Files at its line. On this
    /// computer the file is looked for first, off the UI thread (`file_refs::locate`): an answer often names a file by
    /// its name alone, or one the agent read in another checkout, and the path as written would open "File not found"
    /// or the session folder's copy.
    pub(super) fn open_file_reference(
        &mut self,
        reference: FileReference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(folder), View::Code(view)) = (self.reference_folder(), &self.view) else {
            self.open_in_files(reference.path, reference.line, window, cx);
            return;
        };
        let session = view.id.clone();
        let located = cx.background_executor().spawn(locate(
            folder.clone(),
            reference.path.clone(),
            touched_files(&view.blocks),
        ));
        cx.spawn_in(window, async move |this, cx| {
            let path = located.await.unwrap_or(reference.path);
            let opened = this.update_in(cx, |this, window, cx| {
                // The person may have moved to another session while the folder was read.
                if matches!(&this.view, View::Code(view) if view.id == session) {
                    this.open_located(&folder, path, reference.line, window, cx);
                }
            });
            if let Err(error) = opened {
                eprintln!("nexus-desktop-gpui: the window closed before a file chip opened: {error}");
            }
        })
        .detach();
    }

    /// Open the file at `path` in `folder`, found by `open_file_reference`: in its app, else in Files.
    fn open_located(
        &mut self,
        folder: &std::path::Path,
        path: String,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if file_refs::opens(&path, true) == Opens::App && self.open_reference_in_app(folder, &path, cx) {
            return;
        }
        self.open_in_files(path, line, window, cx);
    }

    /// Open `path` in the system app for its format. False when it may not be (a missing file, not a document
    /// format): Files then opens it and says why.
    pub(super) fn open_reference_in_app(
        &mut self,
        folder: &std::path::Path,
        path: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        match output_files::openable(folder, path) {
            Ok(file) => {
                cx.open_with_system(&file);
                let name = file_refs::file_name(path);
                self.show_notice(format!("Opened {name} in its app."), None, NOTICE, cx);
                true
            }
            Err(_) => false,
        }
    }

    /// Open `reference` in the person's editor, at its line, as Ctrl-click on a tool row does. The file is looked for
    /// first, as a click does (`open_file_reference`).
    pub(super) fn open_reference_in_editor(&mut self, reference: FileReference, cx: &mut Context<Self>) {
        let (Some(folder), View::Code(view)) = (self.reference_folder(), &self.view) else {
            self.show_notice(
                "Open in editor is available for sessions on this computer.",
                None,
                NOTICE,
                cx,
            );
            return;
        };
        let touched = touched_files(&view.blocks);
        let open = cx.background_executor().spawn(async move {
            let path = locate(folder.clone(), reference.path.clone(), touched)
                .await
                .unwrap_or(reference.path);
            crate::editor_choice::open_file(&folder, &path, reference.line)
        });
        cx.spawn(async move |this, cx| {
            if let Err(error) = open.await
                && this
                    .update(cx, |this, cx| this.show_notice(error, None, NOTICE, cx))
                    .is_err()
            {
                eprintln!("nexus-desktop-gpui: the window closed before the editor answered");
            }
        })
        .detach();
    }

    /// Show `reference`'s file in the file manager.
    pub(super) fn reveal_reference(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(folder) = self.reference_folder() else {
            return;
        };
        match output_files::resolve(&folder, path) {
            Ok(file) => cx.reveal_path(&file),
            Err(error) => {
                self.show_notice(error, None, NOTICE, cx);
            }
        }
    }
}
