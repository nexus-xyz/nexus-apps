//! Large Code-composer pastes become files.

use crate::workspace::overlay::OverlaySlot;
use std::sync::Arc;

use gpui_kit::*;

use super::{View, Workspace};
use crate::attachments::{self, Incoming, Source};

const PASTED_TEXT_BYTES: usize = 32 * 1024;

fn attaches_as_file(text: &str) -> bool {
    text.len() >= PASTED_TEXT_BYTES
}

fn pasted_name<'a>(taken: impl Iterator<Item = &'a str>) -> String {
    let taken: std::collections::HashSet<String> = taken.map(str::to_ascii_lowercase).collect();
    for n in 1.. {
        let name = if n == 1 {
            "pasted-text.txt".to_string()
        } else {
            format!("pasted-text-{n}.txt")
        };
        if !taken.contains(&name) {
            return name;
        }
    }
    unreachable!()
}

pub(super) fn paste_as_text_key(key: &Keystroke) -> bool {
    let m = &key.modifiers;
    key.key.eq_ignore_ascii_case("v")
        && m.shift
        && !m.alt
        && if cfg!(target_os = "macos") {
            m.platform && !m.control
        } else {
            m.control && !m.platform
        }
}

/// One large text paste, named against the receiving composer's current draft.
pub(super) fn pasted_text_file<'a>(item: &ClipboardItem, taken: impl Iterator<Item = &'a str>) -> Option<Incoming> {
    let text = item.text().filter(|text| attaches_as_file(text))?;
    Some(Incoming {
        name: pasted_name(taken),
        size: text.len() as u64,
        media_type: None,
        source: Source::Bytes(Arc::new(text.into_bytes())),
    })
}

impl Workspace {
    /// True only when this Code composer's text paste became an attachment.
    pub(super) fn paste_large_text(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        if !matches!(self.view, View::Code(_) | View::NewCode) || !self.composer_shown() {
            return false;
        }
        let Some(file) = pasted_text_file(item, self.draft.all().map(|file| file.name.as_str())) else {
            return false;
        };
        // This only checks and wraps bytes already in memory. No disk I/O or image preparation runs here.
        let prepared = attachments::prepare(vec![file]);
        self.composer_note = attachments::merge(&mut self.draft, prepared).map(Into::into);
        cx.notify();
        true
    }

    /// Insert clipboard text directly so Shift+V cannot become a file or paste twice through a platform binding.
    pub(super) fn paste_as_text(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !paste_as_text_key(key)
            || !self.composer_shown()
            || self.overlay.image_viewer().is_some()
            || self.open_secret_request().is_some()
            || matches!(&self.view, View::Code(view) if crate::code::pending_approval(&view.blocks).is_some())
            || !self.composer.focus_handle(cx).is_focused(window)
        {
            return false;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.composer.update(cx, |state, cx| state.replace(text, window, cx));
            self.composer_empty = self.composer.read(cx).value().trim().is_empty();
            cx.notify();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{PASTED_TEXT_BYTES, attaches_as_file, paste_as_text_key, pasted_name};
    use gpui_kit::Keystroke;

    #[test]
    fn threshold_counts_utf8_bytes_and_includes_exactly_32_kib() {
        assert!(!attaches_as_file(&"x".repeat(PASTED_TEXT_BYTES - 1)));
        assert!(attaches_as_file(&"x".repeat(PASTED_TEXT_BYTES)));
        assert!(attaches_as_file(&"界".repeat(PASTED_TEXT_BYTES / 3 + 1)));
    }

    #[test]
    fn names_avoid_existing_files_without_case_sensitivity() {
        assert_eq!(pasted_name([].into_iter()), "pasted-text.txt");
        assert_eq!(
            pasted_name(["PASTED-TEXT.TXT", "pasted-text-2.txt"].into_iter()),
            "pasted-text-3.txt"
        );
        assert_eq!(
            pasted_name(["pasted-text.txt", "pasted-text-3.txt"].into_iter()),
            "pasted-text-2.txt"
        );
    }

    #[test]
    fn literal_paste_requires_only_the_platform_paste_modifiers_and_shift() {
        let key = if cfg!(target_os = "macos") {
            "cmd-shift-v"
        } else {
            "ctrl-shift-v"
        };
        assert!(paste_as_text_key(&Keystroke::parse(key).unwrap()));
        for key in ["ctrl-v", "cmd-v", "ctrl-shift-alt-v", "cmd-shift-alt-v", "shift-v"] {
            assert!(!paste_as_text_key(&Keystroke::parse(key).unwrap()));
        }
    }
}
