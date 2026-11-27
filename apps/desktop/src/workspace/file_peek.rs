//! The card a file chip shows while the pointer rests on it: the lines around the named line, an image's
//! thumbnail, or why the file has no preview. The hover card builds it only when it opens, and it reads the file off
//! the UI thread, from this computer or the session's server, as the Files tool does (`side/file_view.rs`).

use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::file_refs::{self, Opens};
use nexus_workbench::inline_media::{self, FileReference};
use nexus_workbench::output_files::{self, Preview, Status};

use crate::palette::Palette;
use crate::side::Place;

/// The card's width, and its image's largest height.
const WIDTH: f32 = 420.;
const IMAGE_HEIGHT: f32 = 200.;

/// What the card shows.
enum Peek {
    Reading,
    Text { first: u32, lines: Vec<SharedString> },
    Image(Arc<Image>),
    Note(SharedString),
}

pub(super) struct FilePeek {
    reference: FileReference,
    palette: Palette,
    local: bool,
    peek: Peek,
    _read: Task<()>,
}

impl FilePeek {
    /// `place`: where the session's files are, and the paths its tool calls touched; None once the window or the
    /// session is gone. On this computer the file is looked for first, as a click does (`file_open::locate`): an
    /// answer often names a file by a path from another folder, which as written leads nowhere.
    pub(super) fn new(
        reference: FileReference,
        place: Option<(Place, Vec<String>)>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Self {
        let local = matches!(place, Some((Place::Here(_), _)));
        let path = reference.path.clone();
        let line = reference.line;
        let read = cx.background_executor().spawn(async move {
            let preview = match place {
                Some((Place::Here(Ok(folder)), touched)) => {
                    let found = super::file_open::locate(folder.path.clone(), path.clone(), touched)
                        .await
                        .unwrap_or(path);
                    output_files::read(&folder.path, &found)
                }
                Some((Place::Here(Err(error)), _)) => Err(error),
                Some((Place::Server(server), _)) => server.preview(&path).await.map(|(preview, _)| preview),
                None => Err("This session is no longer open.".into()),
            };
            peek_of(preview, line, local)
        });
        let task = cx.spawn(async move |this, cx| {
            let peek = read.await;
            if this
                .update(cx, |this, cx| {
                    this.peek = peek;
                    cx.notify();
                })
                .is_err()
            {
                // The card closed before the file was read: nothing to show.
            }
        });
        FilePeek {
            reference,
            palette,
            local,
            peek: Peek::Reading,
            _read: task,
        }
    }
}

/// What a read file shows, worked out off the UI thread.
fn peek_of(preview: Result<Preview, String>, line: Option<u32>, local: bool) -> Peek {
    let mut preview = match preview {
        Ok(preview) => preview,
        Err(error) => return Peek::Note(error.into()),
    };
    match preview.status {
        Status::TooLarge => {
            return Peek::Note(
                format!(
                    "{} is over the {} MB preview limit.",
                    inline_media::size_label(preview.size),
                    output_files::MAX_MEDIA / 1024 / 1024
                )
                .into(),
            );
        }
        Status::Unsupported => {
            let open = if file_refs::opens(&preview.path, local) == Opens::App {
                "Click to open it in its app."
            } else {
                "Click to open it in Files."
            };
            return Peek::Note(format!("A preview is unavailable for this format. {open}").into());
        }
        Status::Ready => {}
    }
    if preview.media_type.starts_with("image/") {
        let bytes = preview.bytes();
        preview.data = None;
        return match bytes.and_then(|bytes| crate::side::file_view::image_of(&preview.media_type, bytes)) {
            Some((image, _)) => Peek::Image(image),
            None => Peek::Note("This image's format does not draw here. Click to open it.".into()),
        };
    }
    if preview.media_type == "application/pdf" {
        return Peek::Note(
            format!(
                "PDF, {}. Click to open it in Files.",
                inline_media::size_label(preview.size)
            )
            .into(),
        );
    }
    match &preview.text {
        Some(text) => {
            let (first, lines) = file_refs::peek_lines(text, line);
            if lines.is_empty() {
                return Peek::Note("This file is empty.".into());
            }
            Peek::Text {
                first,
                lines: lines.into_iter().map(SharedString::from).collect(),
            }
        }
        None => Peek::Note("A preview is unavailable for this file.".into()),
    }
}

impl Render for FilePeek {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        let reference = &self.reference;
        let app = file_refs::opens(&reference.path, self.local) == Opens::App;
        let head = h_flex()
            .h(px(32.))
            .px(px(12.))
            .gap(px(8.))
            .border_b_1()
            .border_color(palette.border)
            .text_size(px(12.))
            .text_color(palette.muted)
            .child(Icon::new(Lucide::File).size(px(13.)).text_color(palette.icon))
            .child(div().flex_1().min_w(px(0.)).truncate().child(reference.path.clone()))
            .when_some(reference.line, |head, line| {
                head.child(
                    div()
                        .flex_none()
                        .text_color(palette.faint)
                        .child(format!("line {line}")),
                )
            });
        let body = match &self.peek {
            Peek::Reading => note("Reading…", palette).into_any_element(),
            Peek::Note(text) => note(text.clone(), palette).into_any_element(),
            Peek::Image(image) => div()
                .flex()
                .justify_center()
                .p(px(8.))
                .bg(palette.sidebar)
                .child(
                    img(image.clone())
                        .max_w(px(WIDTH - 16.))
                        .max_h(px(IMAGE_HEIGHT))
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            Peek::Text { first, lines } => {
                let target = reference.line;
                v_flex()
                    .py(px(6.))
                    .bg(palette.sidebar)
                    .font_family(crate::pages::ui::MONO)
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .children(lines.iter().enumerate().map(|(ix, text)| {
                        let number = first + ix as u32;
                        let hit = target == Some(number);
                        h_flex()
                            .w_full()
                            .when(hit, |row| row.bg(palette.selected))
                            .child(
                                div()
                                    .w(px(44.))
                                    .flex_none()
                                    .pr(px(12.))
                                    .text_right()
                                    .text_color(if hit { palette.foreground } else { palette.faint })
                                    .child(number.to_string()),
                            )
                            .child(div().whitespace_nowrap().text_color(palette.strong).child(text.clone()))
                    }))
                    .into_any_element()
            }
        };
        let action = if app { "Open in app" } else { "Open in Files" };
        let foot = h_flex()
            .h(px(30.))
            .px(px(12.))
            .gap(px(14.))
            .border_t_1()
            .border_color(palette.border)
            .text_size(px(11.))
            .text_color(palette.faint)
            .child(format!("Click: {action}"))
            .when(self.local, |foot| {
                foot.child(if cfg!(target_os = "macos") {
                    "⌘-click: Open in editor"
                } else {
                    "Ctrl-click: Open in editor"
                })
            });
        v_flex()
            .w(px(WIDTH))
            .overflow_hidden()
            .rounded(px(10.))
            .border_1()
            .border_color(palette.border)
            .bg(palette.pop)
            .shadow_lg()
            .child(head)
            .child(body)
            .child(foot)
    }
}

fn note(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .px(px(12.))
        .py(px(14.))
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::{Peek, peek_of};
    use nexus_workbench::output_files::{Preview, Status};

    fn preview(media_type: &str, status: Status, text: Option<&str>) -> Preview {
        Preview {
            path: "/p/a".into(),
            media_type: media_type.into(),
            size: 2048,
            outside_project: false,
            text: text.map(str::to_string),
            data: None,
            truncated: false,
            status,
            offset: None,
        }
    }

    /// The card keeps one preview while it is open (`file_chips.rs`), so its read must finish and show the file.
    #[gpui_kit::test]
    fn a_preview_reads_the_sessions_file_and_shows_it(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        let dir = tempfile::tempdir().expect("a temporary folder");
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\nfn b() {}\n").expect("the file");
        let place = crate::side::Place::Here(Ok(crate::code::SessionFolder {
            path: dir.path().to_path_buf(),
            base: None,
        }));
        let reference = nexus_workbench::inline_media::FileReference {
            path: "a.rs".into(),
            line: Some(2),
        };
        let palette = crate::palette::Palette::solid_for(true);
        let peek = cx.new(|cx| super::FilePeek::new(reference, Some((place, Vec::new())), palette, cx));
        cx.run_until_parked();
        peek.read_with(cx, |peek, _| match &peek.peek {
            Peek::Text { first, lines } => {
                assert_eq!(*first, 1);
                assert_eq!(lines[1].as_ref(), "fn b() {}");
            }
            Peek::Reading => panic!("the read never finished"),
            _ => panic!("a text file shows its lines"),
        });

        let gone = cx.new(|cx| {
            super::FilePeek::new(
                nexus_workbench::inline_media::FileReference {
                    path: "a.rs".into(),
                    line: None,
                },
                None,
                palette,
                cx,
            )
        });
        cx.run_until_parked();
        gone.read_with(cx, |peek, _| assert!(matches!(peek.peek, Peek::Note(_))));
    }

    /// An answer names `src/frost.rs` while the file is at `app/src/frost.rs` in the session's folder. A click finds
    /// it (`file_open::locate`), so the preview must find the same file, not say it is missing.
    #[gpui_kit::test]
    fn a_preview_finds_the_file_a_click_opens(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        let dir = tempfile::tempdir().expect("a temporary folder");
        std::fs::create_dir_all(dir.path().join("app/src")).expect("the folders");
        std::fs::write(dir.path().join("app/src/frost.rs"), "fn frost() {}\n").expect("the file");
        let place = || {
            crate::side::Place::Here(Ok(crate::code::SessionFolder {
                path: dir.path().to_path_buf(),
                base: None,
            }))
        };
        let reference = nexus_workbench::inline_media::FileReference {
            path: "src/frost.rs".into(),
            line: None,
        };
        let palette = crate::palette::Palette::solid_for(true);
        let found = cx.new(|cx| super::FilePeek::new(reference.clone(), Some((place(), Vec::new())), palette, cx));
        // An agent's copy in another checkout, named by its tool calls, is found too.
        let other = tempfile::tempdir().expect("another checkout");
        std::fs::create_dir_all(other.path().join("src")).expect("the folder");
        let copy = other.path().join("src/frost.rs");
        std::fs::write(&copy, "fn other() {}\n").expect("the copy");
        let touched = vec![copy.to_string_lossy().into_owned()];
        let elsewhere = cx.new(|cx| super::FilePeek::new(reference, Some((place(), touched)), palette, cx));
        cx.run_until_parked();
        let first_line = |peek: &super::FilePeek| match &peek.peek {
            Peek::Text { lines, .. } => lines[0].to_string(),
            Peek::Note(note) => panic!("the file shows its lines, not: {note}"),
            _ => panic!("the read never finished"),
        };
        found.read_with(cx, |peek, _| assert_eq!(first_line(peek), "fn frost() {}"));
        elsewhere.read_with(cx, |peek, _| assert_eq!(first_line(peek), "fn other() {}"));
    }

    #[test]
    fn text_shows_its_lines_around_the_named_line() {
        let text: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        let Peek::Text { first, lines } =
            peek_of(Ok(preview("text/plain", Status::Ready, Some(&text))), Some(12), true)
        else {
            panic!("text shows as text");
        };
        assert_eq!(first, 10);
        assert_eq!(lines[2].as_ref(), "line 12");
    }

    #[test]
    fn a_file_without_a_preview_says_why_and_where_a_click_opens_it() {
        let note = |peek: Peek| match peek {
            Peek::Note(text) => text.to_string(),
            _ => panic!("a note"),
        };
        let mut sheet = preview("text/plain", Status::Unsupported, None);
        sheet.path = "/p/q4.xlsx".into();
        assert!(note(peek_of(Ok(sheet.clone()), None, true)).ends_with("Click to open it in its app."));
        assert!(note(peek_of(Ok(sheet), None, false)).ends_with("Click to open it in Files."));
        assert!(note(peek_of(Ok(preview("image/png", Status::TooLarge, None)), None, true)).contains("16 MB"));
        assert_eq!(note(peek_of(Err("Missing.".into()), None, true)), "Missing.");
        assert_eq!(
            note(peek_of(Ok(preview("text/plain", Status::Ready, Some(""))), None, true)),
            "This file is empty."
        );
    }
}
