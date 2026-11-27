//! The Files tool's viewer, as the Nexus app's (`file-view.tsx`): a file opens as a preview read
//! through `nexus_workbench::output_files` (`wb_file_preview` on a server). An image shows fitted to the pane, with
//! Actual size; Markdown shows rendered, with Preview and Source; HTML shows as a read-only document, with Preview and
//! Source; PDF shows a page at a time (`pdf.rs`); other text shows highlighted, and the project's code on this computer, read whole, can be edited and saved
//! (`output_files::write`). A video plays in the engine's player, read as an answer's video
//! is (`wb_file_media`). Files over the preview limits, and formats with no preview, say so as the Nexus app does.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Editor, EditorState, Position};
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::inline_media::{self, Loaded, MediaKind};
use nexus_workbench::output_files::{self, Preview, Status};

use super::file_lifetime::{Guard, Lifetime};
use super::{INSET, Place, icon_button};
use crate::code_media::{self, Media};
use crate::palette::Palette;
use crate::web::WebView;

// Save the edited file (⌘S / Ctrl+S, `main.rs`): bound only while the open file can be saved, so Stash keeps the keys.
actions!(file_view, [SaveFile]);

/// What the viewer asks of the Files tool.
pub enum FileViewEvent {
    /// Back to the tree.
    Close,
    /// A link in a Markdown preview names another file: open it instead.
    Open(String),
}

/// What the file shows once read.
enum Body {
    /// Too large, or no preview for its format: its message says so.
    Nothing,
    /// The image, and its size in pixels when they read (not an SVG's). None: it cannot be drawn here.
    Image(Option<(Arc<Image>, Option<(u32, u32)>)>),
    /// Rendered, and its source.
    Markdown {
        view: Entity<TextViewState>,
        source: Entity<EditorState>,
    },
    /// A read-only document (no script, network or forms), and its source.
    Html {
        page: Entity<WebView>,
        source: Entity<EditorState>,
    },
    /// Read-only text.
    Code(Entity<EditorState>),
    Pdf(PdfPages),
}

/// A PDF's pages, one drawn at a time (`PdfPreview`).
struct PdfPages {
    bytes: Arc<Vec<u8>>,
    /// The page shown, from 1, and the count once the document has read.
    page: usize,
    pages: Option<usize>,
    drawn: Option<(Arc<Image>, (f32, f32))>,
    /// The shown page's text, for a screen reader.
    text: SharedString,
    error: Option<String>,
    /// The page, width and screen ratio last asked for: a change draws again.
    asked: Option<(usize, u32, u32)>,
    _draw: Option<Task<()>>,
}

impl PdfPages {
    /// Ask for the page, width and ratio `asked`. Another one forgets the page shown, its text and its error until it
    /// draws, so a screen reader never reads one page's text under another's name. Answers whether to draw.
    fn ask(&mut self, asked: (usize, u32, u32)) -> bool {
        if self.asked == Some(asked) {
            return false;
        }
        self.asked = Some(asked);
        self.drawn = None;
        self.text = SharedString::default();
        self.error = None;
        true
    }
}

enum State {
    Opening,
    Failed(SharedString),
    Ready {
        preview: Preview,
        body: Body,
    },
    /// A video, read as an answer's video is: it has no text or image preview.
    Video(Clip),
}

/// A video in the viewer, as the Nexus app's full `MediaCard`.
enum Clip {
    Loading,
    /// Playing in the engine's player, from `file`. `size` in bytes.
    Ready {
        page: Entity<WebView>,
        size: u64,
        _file: VideoFile,
    },
    TooLarge(u64),
    Failed(SharedString),
}

/// The file a video plays from: the session's own on this computer, or a copy of a server's, removed with the viewer.
struct VideoFile {
    path: PathBuf,
    copy: bool,
}

impl Drop for VideoFile {
    fn drop(&mut self) {
        if self.copy {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The user's file selection; renderers and native preview engines are always rebuilt.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct FileViewState {
    pub path: String,
    pub line: Option<u32>,
    pub source: bool,
    pub zoom: bool,
    pub pdf_page: Option<usize>,
}

#[cfg(test)]
type PreviewReply = futures::channel::oneshot::Receiver<Result<(Preview, bool), String>>;

pub struct FileView {
    lifetime: Lifetime,
    #[cfg(test)]
    pending_preview: Option<PreviewReply>,
    #[cfg(test)]
    pending_video: Option<futures::channel::oneshot::Receiver<Result<VideoFile, Clip>>>,
    #[cfg(test)]
    pending_pdf: Option<futures::channel::oneshot::Receiver<Result<super::pdf::Drawn, String>>>,
    #[cfg(test)]
    editor_fixture: Option<crate::editor_choice::Fixture>,
    pdf_page: Option<usize>,
    place: Place,
    /// The path as opened: relative to the session's folder, or absolute.
    path: String,
    state: State,
    /// The host previews images and PDFs (always here; a server says so in its hello).
    previews: bool,
    /// Markdown's Source instead of its Preview.
    source: bool,
    /// An image at its actual size instead of fitted.
    zoom: bool,
    notice: Option<SharedString>,
    /// The preview's width, measured as it draws: an image fits it.
    width: f32,
    _read: Option<Task<()>>,
    line: Option<u32>,
    /// Back was pressed once over unsaved edits: the next press drops them.
    leaving: bool,
}

impl EventEmitter<FileViewEvent> for FileView {}

impl FileView {
    pub fn new(place: Place, path: String, window: &mut Window, cx: &mut Context<Self>) -> FileView {
        Self::restore(
            place,
            FileViewState {
                path,
                ..Default::default()
            },
            Arc::new(|| true),
            window,
            cx,
        )
    }

    pub(crate) fn restore(
        place: Place,
        saved: FileViewState,
        guard: Guard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::unread(place, saved, guard);
        view.read(window, cx);
        view
    }
    fn unread(place: Place, saved: FileViewState, guard: Guard) -> Self {
        Self {
            lifetime: Lifetime::new(guard),
            #[cfg(test)]
            pending_preview: None,
            #[cfg(test)]
            pending_video: None,
            #[cfg(test)]
            pending_pdf: None,
            #[cfg(test)]
            editor_fixture: None,
            pdf_page: saved.pdf_page,
            place,
            path: saved.path,
            state: State::Opening,
            previews: true,
            source: saved.source,
            zoom: saved.zoom,
            notice: None,
            width: 0.,
            _read: None,
            line: saved.line,
            leaving: false,
        }
    }
    #[cfg(test)]
    pub(crate) fn fixture_preview(
        place: Place,
        saved: FileViewState,
        guard: Guard,
        pending: PreviewReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::unread(place, saved, guard);
        view.pending_preview = Some(pending);
        view.read(window, cx);
        view
    }
    #[cfg(test)]
    pub(crate) fn fixture_path(&self) -> &str {
        &self.path
    }
    #[cfg(test)]
    pub(crate) fn fixture_preview_text(&self) -> Option<&str> {
        if !self.is_current() {
            return None;
        }
        match &self.state {
            State::Ready { preview, .. } => preview.text.as_deref(),
            _ => None,
        }
    }

    /// Invoke production callbacks after native close and report only observed task/notice state.
    #[cfg(test)]
    pub(crate) fn fixture_attempt_actions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (bool, Option<String>) {
        self.read(window, cx);
        self.copy_path(cx);
        self.open_in_editor(cx);
        (self._read.is_some(), self.notice.as_ref().map(ToString::to_string))
    }

    pub(crate) fn is_current(&self) -> bool {
        self.lifetime.current()
    }
    pub(crate) fn is_retired(&self) -> bool {
        self.lifetime.retired()
    }
    pub(crate) fn snapshot(&self) -> FileViewState {
        let pdf_page = match &self.state {
            State::Ready {
                body: Body::Pdf(pdf), ..
            } => self.pdf_page.or(Some(pdf.page)),
            _ => self.pdf_page,
        };
        FileViewState {
            path: self.path.clone(),
            line: self.line,
            source: self.source,
            zoom: self.zoom,
            pdf_page,
        }
    }
    pub(crate) fn retire(&mut self, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        self.lifetime.retire();
        self._read = None;
        self.release_document(cx);
        self.state = State::Failed("This file view is no longer active.".into());
        self.notice = None;
        cx.notify();
    }

    fn release_document(&self, cx: &mut Context<Self>) {
        // A rendered child or an old test/tool handle may still retain the WebView entity.
        // Explicitly dispose a CEF document owner before dropping its presentation state.
        let page = match &self.state {
            State::Ready {
                body: Body::Html { page, .. },
                ..
            }
            | State::Video(Clip::Ready { page, .. }) => Some(page.clone()),
            _ => None,
        };
        if let Some(page) = page {
            page.update(cx, |page, _| drop(page.retain()));
        }
    }

    /// A file picker or search result requests a source line, counted from one.
    pub fn open_line(&mut self, line: Option<u32>) {
        if !self.is_current() {
            return;
        }
        self.line = line;
        self.source = line.is_some();
    }

    fn remote(&self) -> bool {
        matches!(self.place, Place::Server(_))
    }

    /// Whether the file plays as a video here: a video, where the session's machine reads media (`useInlineMedia`).
    fn video(&self) -> bool {
        inline_media::media_kind(&self.path) == Some(MediaKind::Video)
            && match &self.place {
                Place::Here(_) => true,
                Place::Server(server) => server.shows_media(),
            }
    }

    /// Read the video (again), off the UI thread, and open it in a player.
    fn read_video(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        let permit = self.lifetime.next();
        self.release_document(cx);
        self.state = State::Video(Clip::Loading);
        self.notice = None;
        let path = self.path.clone();
        let valid = permit.guard();
        let read = self.video_read(path, valid, cx);
        self._read = Some(cx.spawn_in(window, async move |this, cx| {
            let read = read.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !permit.current() {
                    return;
                }
                this.state = State::Video(match read {
                    Err(clip) => clip,
                    Ok(file) => match url::Url::from_file_path(&file.path) {
                        Ok(url) => {
                            let size = file.path.metadata().map(|meta| meta.len()).unwrap_or_default();
                            let page = cx.new(|cx| WebView::document(url.as_str(), window, cx));
                            Clip::Ready {
                                page,
                                size,
                                _file: file,
                            }
                        }
                        Err(()) => Clip::Failed("The video could not be read.".into()),
                    },
                });
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn video_read(&mut self, path: String, valid: Guard, cx: &mut Context<Self>) -> Task<Result<VideoFile, Clip>> {
        #[cfg(test)]
        if let Some(read) = self.pending_video.take() {
            return cx
                .background_executor()
                .spawn(async move { read.await.expect("fixture video reply must be completed") });
        }
        match &self.place {
            Place::Here(Err(error)) => Task::ready(Err(Clip::Failed(error.clone().into()))),
            Place::Here(Ok(folder)) => {
                let root = folder.path.clone();
                cx.background_executor().spawn(async move {
                    if !valid() {
                        return Err(Clip::Failed("This file view is no longer active.".into()));
                    }
                    match code_media::load(code_media::Source::Here(root), path, MediaKind::Video).await {
                        Media::Video { file: Some(path), .. } => Ok(VideoFile { path, copy: false }),
                        Media::TooLarge(size) => Err(Clip::TooLarge(size)),
                        Media::Failed(error) => Err(Clip::Failed(error.into())),
                        _ => Err(Clip::Failed("The video could not be read.".into())),
                    }
                })
            }
            Place::Server(server) => {
                let server = server.clone();
                cx.background_executor().spawn(async move {
                    if !valid() {
                        return Err(Clip::Failed("This file view is no longer active.".into()));
                    }
                    let read = inline_media::read_all(&path, |offset| {
                        let server = server.clone();
                        let path = path.clone();
                        async move { server.media(&path, offset).await }
                    })
                    .await;
                    match read {
                        Err(error) => Err(Clip::Failed(error.into())),
                        Ok(Loaded::TooLarge { size }) => Err(Clip::TooLarge(size)),
                        Ok(Loaded::Ready { bytes, .. }) => {
                            if !valid() {
                                return Err(Clip::Failed("This file view is no longer active.".into()));
                            }
                            let dir = crate::paths::cache_dir().join("file-videos");
                            copy_of(&dir, &path, &bytes).map_err(|error| Clip::Failed(error.into()))
                        }
                    }
                })
            }
        }
    }

    /// Read the file (again), off the UI thread.
    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        if self.video() {
            return self.read_video(window, cx);
        }
        let permit = self.lifetime.next();
        self.release_document(cx);
        self.state = State::Opening;
        self.notice = None;
        let path = self.path.clone();
        let read = self.preview_read(path, permit.guard(), cx);
        self._read = Some(cx.spawn_in(window, async move |this, cx| {
            let read = read.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !permit.current() {
                    return;
                }
                this.state = match read {
                    Err(error) => State::Failed(error.into()),
                    Ok((mut preview, previews)) => {
                        this.previews = previews;
                        let mut body = body_of(&mut preview, window, cx);
                        if let Body::Pdf(pdf) = &mut body {
                            pdf.page = 1;
                        }
                        if let Some(line) = this.line {
                            let editor = match &body {
                                Body::Code(editor) | Body::Markdown { source: editor, .. } => Some(editor),
                                _ => None,
                            };
                            if let Some(editor) = editor {
                                editor.update(cx, |editor, cx| {
                                    editor.set_cursor_position(
                                        Position {
                                            line: line.saturating_sub(1),
                                            character: 0,
                                        },
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }
                        State::Ready { preview, body }
                    }
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn preview_read(
        &mut self,
        path: String,
        valid: Guard,
        cx: &mut Context<Self>,
    ) -> Task<Result<(Preview, bool), String>> {
        #[cfg(test)]
        if let Some(read) = self.pending_preview.take() {
            return cx
                .background_executor()
                .spawn(async move { read.await.expect("fixture preview reply must be completed") });
        }
        match &self.place {
            Place::Here(Err(error)) => Task::ready(Err(error.clone())),
            Place::Here(Ok(folder)) => {
                let dir = folder.path.clone();
                cx.background_executor().spawn(async move {
                    if !valid() {
                        return Err("This file view is no longer active.".into());
                    }
                    output_files::read(&dir, &path).map(|preview| (preview, true))
                })
            }
            Place::Server(server) => {
                let server = server.clone();
                cx.background_executor().spawn(async move {
                    if !valid() {
                        return Err("This file view is no longer active.".into());
                    }
                    server.preview(&path).await
                })
            }
        }
    }

    fn copy_path(&mut self, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        let path = match &self.state {
            State::Ready { preview, .. } => preview.path.clone(),
            _ => self.path.clone(),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(path));
        self.notice = Some("Path copied.".into());
        cx.notify();
    }

    /// Open the file in its own app, a document or image format only, on this computer.
    fn open_in_app(&mut self, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        let Place::Here(Ok(folder)) = &self.place else {
            return;
        };
        match output_files::openable(&folder.path, &self.path) {
            Ok(file) => {
                if !self.is_current() {
                    return;
                }
                self.notice = None;
                cx.open_with_system(&file);
            }
            Err(error) => self.notice = Some(error.into()),
        }
        cx.notify();
    }

    /// The preview already carries the session's folder, including its selected worktree.
    fn open_in_editor(&mut self, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        let permit = self.lifetime.capture();
        let guard = permit.guard();
        let Place::Here(Ok(folder)) = &self.place else {
            return;
        };
        let preview = match &self.state {
            State::Ready { preview, .. } => Some(preview),
            _ => None,
        };
        if !can_edit(false, preview) {
            return;
        }
        let root = folder.path.clone();
        let path = self.path.clone();
        let line = self.line;
        self.notice = None;
        #[cfg(test)]
        let fixture = self.editor_fixture.clone();
        let open = cx.background_executor().spawn(async move {
            #[cfg(test)]
            if let Some(fixture) = fixture {
                return fixture.open(&root, &path, line, guard.as_ref());
            }
            crate::editor_choice::open_file_guarded(&root, &path, line, guard.as_ref())
        });
        cx.spawn(async move |this, cx| {
            let result = open.await;
            let _ = this.update(cx, |this, cx| {
                if !permit.current() {
                    return;
                }
                this.notice = result.err().map(Into::into);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn fixture_editor(&mut self, fixture: crate::editor_choice::Fixture) {
        self.editor_fixture = Some(fixture);
    }

    /// The open code's text, when it may be saved here and differs from what was last read or saved.
    // Compares up to MAX_TEXT on each draw; keep a dirty flag from the editor's Change event if that shows.
    fn unsaved(&self, cx: &App) -> Option<SharedString> {
        let State::Ready {
            preview,
            body: Body::Code(editor),
        } = &self.state
        else {
            return None;
        };
        let text = editor.read(cx).value();
        (can_save(self.remote(), preview) && preview.text.as_deref() != Some(text.as_ref())).then_some(text)
    }

    /// Write the edited text over the file, unless it changed on disk since it was read: that fails, and says so.
    fn save(&mut self, _: &SaveFile, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        let Some(text) = self.unsaved(cx) else {
            return;
        };
        let (Place::Here(Ok(folder)), State::Ready { preview, .. }) = (&self.place, &self.state) else {
            return;
        };
        let (root, path) = (folder.path.clone(), self.path.clone());
        let expected = preview.text.clone().unwrap_or_default();
        let permit = self.lifetime.capture();
        self.notice = Some("Saving…".into());
        let written = text.clone();
        let write = cx
            .background_executor()
            .spawn(async move { output_files::write(&root, &path, &expected, &written) });
        cx.spawn(async move |this, cx| {
            let result = write.await;
            let _ = this.update(cx, |this, cx| {
                if !permit.current() {
                    return;
                }
                this.notice = Some(match result {
                    Ok(()) => {
                        if let State::Ready { preview, .. } = &mut this.state {
                            preview.size = text.len() as u64;
                            preview.text = Some(text.to_string());
                        }
                        this.leaving = false;
                        "Saved.".into()
                    }
                    Err(error) => error.into(),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Back to the tree. Over unsaved edits, the first press warns and the second drops them.
    fn back(&mut self, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        if self.unsaved(cx).is_some() && !self.leaving {
            self.leaving = true;
            self.notice = Some("Unsaved changes. Save, or press Back again to discard them.".into());
            return cx.notify();
        }
        cx.emit(FileViewEvent::Close);
    }

    fn link_opener(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&SharedString, &ClickEvent, &mut Window, &mut App) + Send + Sync + 'static {
        let this = cx.entity().downgrade();
        let current = self.path.clone();
        let guard = self.lifetime.guard();
        move |href, _, _, cx| {
            if !guard() {
                return;
            }
            match link_target(&current, href) {
                Link::Web(url) => crate::external_link::open(&url, cx),
                Link::File(path) => {
                    let _ = this.update(cx, |_, cx| cx.emit(FileViewEvent::Open(path)));
                }
                Link::Nothing => {}
            }
        }
    }
}

/// The view of a preview just read. An image's bytes move into the image; the preview keeps none.
fn body_of(preview: &mut Preview, window: &mut Window, cx: &mut Context<FileView>) -> Body {
    if preview.status != Status::Ready {
        return Body::Nothing;
    }
    if preview.media_type == "application/pdf" {
        let Some(bytes) = preview.bytes() else {
            return Body::Nothing;
        };
        preview.data = None;
        return Body::Pdf(PdfPages {
            bytes: Arc::new(bytes),
            page: 1,
            pages: None,
            drawn: None,
            text: SharedString::default(),
            error: None,
            asked: None,
            _draw: None,
        });
    }
    if preview.media_type.starts_with("image/") {
        let bytes = preview.bytes();
        preview.data = None;
        return Body::Image(bytes.and_then(|bytes| image_of(&preview.media_type, bytes)));
    }
    let Some(text) = preview.text.clone() else {
        return Body::Nothing;
    };
    let code = cx.new(|cx| {
        EditorState::new(window, cx)
            .language(super::files::language_of(&preview.path))
            .line_number(true)
            .soft_wrap(false)
            .searchable(true)
            .default_value(text.clone())
    });
    if preview.media_type == "text/markdown" {
        let view = cx.new(|cx| TextViewState::markdown(&text, cx));
        return Body::Markdown { view, source: code };
    }
    if preview.media_type == "text/html" {
        let page = cx.new(|cx| WebView::document(&html_url(&text), window, cx));
        return Body::Html { page, source: code };
    }
    Body::Code(code)
}

/// An HTML file's text as the address of its read-only document (`previewHtml`).
fn html_url(text: &str) -> String {
    format!(
        "data:text/html;base64,{}",
        STANDARD.encode(output_files::preview_html(text))
    )
}

/// A server's video, copied into `dir` on this computer for the player (the app's cache). The copy goes with the
/// viewer.
fn copy_of(dir: &std::path::Path, path: &str, bytes: &[u8]) -> Result<VideoFile, String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let name = file_name(path);
    let extension = name.rsplit_once('.').map(|(_, extension)| extension).unwrap_or("mp4");
    let copy = dir.join(format!("{}.{extension}", uuid::Uuid::new_v4()));
    std::fs::write(&copy, bytes).map_err(|error| error.to_string())?;
    Ok(VideoFile { path: copy, copy: true })
}

/// The image GPUI draws for `bytes`, and its pixels. None for a format GPUI does not draw (AVIF) or a raster image
/// whose header does not read: the Nexus app's web view fails on those too, and says so.
pub(crate) fn image_of(media_type: &str, bytes: Vec<u8>) -> Option<(Arc<Image>, Option<(u32, u32)>)> {
    let format = match media_type {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/svg+xml" => ImageFormat::Svg,
        _ => return None,
    };
    let pixels = if format == ImageFormat::Svg {
        None
    } else {
        let pixels = ::image::ImageReader::new(Cursor::new(bytes.as_slice()))
            .with_guessed_format()
            .ok()
            .and_then(|reader| reader.into_dimensions().ok())?;
        Some(pixels)
    };
    Some((Arc::new(Image::from_bytes(format, bytes)), pixels))
}

/// Where a link in a Markdown preview leads.
#[derive(Debug, PartialEq)]
enum Link {
    Web(String),
    /// A file, relative to the session's folder or absolute.
    File(String),
    Nothing,
}

/// A link in the Markdown file at `current`: a web address opens in the browser; a path opens in the viewer, from the
/// file's own folder unless absolute (`OutputLinks` in `output-links.tsx`).
fn link_target(current: &str, href: &str) -> Link {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    if ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
    {
        return Link::Web(href.to_string());
    }
    // Any other scheme, or an anchor in this page.
    let path = href.split(['#', '?']).next().unwrap_or_default();
    if path.is_empty() || (lower.contains("://") || (path.contains(':') && !is_drive(path))) {
        return Link::Nothing;
    }
    let path = path.strip_prefix("./").unwrap_or(path);
    if path.starts_with('/') || path.starts_with("~/") || is_drive(path) {
        return Link::File(path.to_string());
    }
    match current.rsplit_once('/') {
        Some((dir, _)) => Link::File(format!("{dir}/{path}")),
        None => Link::File(path.to_string()),
    }
}

/// `C:/` or `C:\`: a Windows path, not a scheme.
fn is_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\')
}

/// 900 as "900 B", 2048 as "2 KB", 1.5 MiB as "1.5 MB" (`size` in `file-view.tsx`).
fn size_label(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", (bytes as f64 / 1024.).round())
    } else {
        format!("{:.1} MB", bytes as f64 / 1024. / 1024.)
    }
}

/// The line under the header: where the file is, and its size.
fn meta(preview: &Preview, remote: bool) -> String {
    format!(
        "{}\n{}{}{}",
        preview.path,
        if remote { "Remote machine · " } else { "" },
        if preview.outside_project {
            "Outside project · "
        } else {
            ""
        },
        size_label(preview.size)
    )
}

/// What a file over the preview limit says.
fn too_large(remote: bool) -> String {
    format!(
        "This file exceeds the {} MB preview limit. {}",
        output_files::MAX_MEDIA / 1024 / 1024,
        if remote {
            "Open it on its machine."
        } else {
            "Use Open in app to view the full file."
        }
    )
}

/// What a format with no preview says; `previews` off: the machine's Nexus is older than image and PDF previews.
fn unsupported(remote: bool, previews: bool) -> String {
    format!(
        "A preview is unavailable for this format. {}{}",
        if remote {
            "Open it on its machine."
        } else {
            "Use Open in app to view it."
        },
        if previews {
            ""
        } else {
            " Update Nexus on this machine for image and PDF previews."
        }
    )
}

/// `width` × `height` fitted to `room` across, never larger than it is (`max-width: 100%` in `outputs.css`).
fn fit(width: u32, height: u32, room: f32) -> (f32, f32) {
    let (width, height) = (width as f32, height as f32);
    if width <= 0. || room <= 0. {
        return (width, height);
    }
    let scale = (room / width).min(1.);
    (width * scale, height * scale)
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A text button, pressed or not, as the Nexus app's `link-button`s over a preview.
fn link_button(id: &'static str, label: &'static str, pressed: bool, palette: Palette) -> Stateful<Div> {
    super::chip(id, label, pressed, palette)
}

impl FileView {
    fn source_toggle(&self, palette: Palette, cx: &Context<Self>) -> AnyElement {
        h_flex()
            .flex_none()
            .gap(px(4.))
            .child(
                link_button("file-preview", "Preview", !self.source, palette).on_click(cx.listener(
                    |this, _, _, cx| {
                        if !this.is_current() {
                            return;
                        }
                        this.source = false;
                        cx.notify();
                    },
                )),
            )
            .child(
                link_button("file-source", "Source", self.source, palette).on_click(cx.listener(|this, _, _, cx| {
                    if !this.is_current() {
                        return;
                    }
                    this.source = true;
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    fn image_view(&self, image: &Option<(Arc<Image>, Option<(u32, u32)>)>, palette: Palette) -> AnyElement {
        let Some((image, pixels)) = image else {
            return message("This image could not be displayed. Try Open in app.", palette).into_any_element();
        };
        let room = (self.width - 2. * INSET).max(0.);
        let drawn = img(ImageSource::Image(image.clone())).with_fallback(move || {
            message("This image could not be displayed. Try Open in app.", palette).into_any_element()
        });
        let drawn = match pixels {
            Some((width, height)) if self.zoom => drawn.w(px(*width as f32)).h(px(*height as f32)),
            Some((width, height)) => {
                let (width, height) = fit(*width, *height, room);
                drawn.w(px(width)).h(px(height))
            }
            None => drawn.max_w(px(room.max(1.))),
        };
        div()
            .flex()
            .justify_center()
            .min_w_full()
            .child(drawn.flex_none())
            .into_any_element()
    }
}

impl FileView {
    /// A video as the Nexus app's full `MediaCard`: the player over its caption, or a card while it loads or fails.
    fn video_view(&self, clip: &Clip, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let name = file_name(&self.path).to_string();
        let caption = |detail: String| {
            h_flex()
                .flex_none()
                .gap(px(6.))
                .px(px(INSET))
                .py(px(6.))
                .text_size(px(12.))
                .text_color(palette.foreground)
                .child(
                    gpui_kit::component::Icon::new(Lucide::Film)
                        .size(px(13.))
                        .text_color(palette.faint),
                )
                .child(div().min_w(px(0.)).truncate().child(name.clone()))
                .child(div().flex_none().text_color(palette.faint).child(detail))
        };
        match clip {
            Clip::Ready { page, size, .. } => v_flex()
                .flex_1()
                .min_h(px(0.))
                .child(div().flex_1().min_h(px(0.)).child(page.clone()))
                .child(caption(inline_media::size_label(*size)))
                .into_any_element(),
            Clip::Loading => caption("Loading video…".into()).into_any_element(),
            Clip::TooLarge(size) => caption(inline_media::too_large(MediaKind::Video, *size)).into_any_element(),
            Clip::Failed(error) => self.failure(error, palette, cx),
        }
    }

    /// Why the file did not open, centred in the pane as the Files tool's other notes are: an icon, a title, the path,
    /// what to do, then Try again and Back to Files.
    fn failure(&self, error: &str, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let failure = Failure::of(error, &self.path);
        let buttons = h_flex()
            .gap(px(8.))
            .mt(px(10.))
            .child(
                crate::pages::ui::primary_button("file-try-again", "Try again", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.read(window, cx))),
            )
            .child(
                crate::pages::ui::button("file-failed-back", "Back to Files", palette)
                    .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
            );
        v_flex()
            .flex_1()
            .min_h(px(0.))
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .px(px(32.))
            .pb(px(40.))
            .child(
                gpui_kit::component::Icon::new(failure.icon)
                    .size(px(28.))
                    .text_color(palette.faint)
                    .mb(px(6.)),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.foreground)
                    .child(failure.title),
            )
            .when_some(failure.path, |column, path| {
                column.child(
                    div()
                        .max_w(px(320.))
                        .text_size(px(12.))
                        .line_height(px(17.))
                        .text_color(palette.faint)
                        .text_center()
                        .child(path),
                )
            })
            .child(
                div()
                    .max_w(px(320.))
                    .text_size(px(13.))
                    .line_height(px(19.))
                    .text_color(palette.muted)
                    .text_center()
                    .child(failure.detail),
            )
            .child(buttons)
            .into_any_element()
    }
}

/// What the failed state says (`FileView::failure`), apart from its drawing so tests can read it.
#[derive(Debug, PartialEq)]
struct Failure {
    icon: Lucide,
    title: &'static str,
    /// The path asked for, when no file is there: the person checks it against the agent's reply.
    path: Option<String>,
    detail: String,
}

impl Failure {
    fn of(error: &str, path: &str) -> Self {
        if output_files::is_missing(error) {
            return Failure {
                icon: Lucide::FileSearch,
                title: "File not found",
                path: Some(path.to_string()),
                detail:
                    "The agent may not have saved it yet, or saved it in another folder. Try again once it exists, \
                         or ask the agent where it is."
                        .into(),
            };
        }
        Failure {
            icon: Lucide::CircleAlert,
            title: "This file could not open",
            path: None,
            detail: error.to_string(),
        }
    }
}

impl FileView {
    /// Draw the PDF's page for the pane's width, unless it is drawn (or being drawn) for it already.
    fn draw_pdf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        let permit = self.lifetime.capture();
        let State::Ready {
            body: Body::Pdf(pdf), ..
        } = &mut self.state
        else {
            return;
        };
        // The preview's width less its padding, as the Nexus app's box less 24.
        let width = (self.width - 2. * INSET).floor().max(100.);
        let ratio = window.scale_factor().clamp(1., 2.);
        let asked = (pdf.page, width as u32, (ratio * 100.) as u32);
        if self.width <= 0. || !pdf.ask(asked) {
            return;
        }
        let (bytes, page) = (pdf.bytes.clone(), pdf.page);
        #[cfg(test)]
        let draw = match self.pending_pdf.take() {
            Some(read) => cx
                .background_executor()
                .spawn(async move { read.await.expect("fixture PDF reply must be completed") }),
            None => cx
                .background_executor()
                .spawn(async move { super::pdf::draw(bytes, page, width, ratio) }),
        };
        #[cfg(not(test))]
        let draw = cx
            .background_executor()
            .spawn(async move { super::pdf::draw(bytes, page, width, ratio) });
        pdf._draw = Some(cx.spawn(async move |this, cx| {
            let drawn = draw.await;
            let _ = this.update(cx, |this, cx| {
                if !permit.current() {
                    return;
                }
                if let State::Ready {
                    body: Body::Pdf(pdf), ..
                } = &mut this.state
                    && pdf.asked == Some(asked)
                {
                    match drawn {
                        Ok(drawn) => {
                            pdf.pages = Some(drawn.pages);
                            let desired = this.pdf_page.take().unwrap_or(pdf.page).clamp(1, drawn.pages.max(1));
                            if pdf.page != desired {
                                pdf.page = desired;
                                pdf.asked = None;
                                cx.notify();
                                return;
                            }
                            pdf.drawn = Some((drawn.image, drawn.size));
                            pdf.text = drawn.text.into();
                            pdf.error = None;
                        }
                        Err(error) => {
                            pdf.error = Some(error);
                            pdf.drawn = None;
                            pdf.text = SharedString::default();
                        }
                    }
                    cx.notify();
                }
            });
        }));
    }

    /// Go `step` pages back or forward.
    fn turn(&mut self, step: isize, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        if let State::Ready {
            body: Body::Pdf(pdf), ..
        } = &mut self.state
            && let Some(pages) = pdf.pages
        {
            pdf.page = pdf.page.saturating_add_signed(step).clamp(1, pages);
            cx.notify();
        }
    }
}

/// The shown PDF page's text as a screen reader reads it, as the Nexus app's hidden paragraph (`sr-only`) under the
/// page: none when the page has no text.
fn page_text(text: &str) -> Option<accesskit::Node> {
    (!text.is_empty()).then(|| {
        let mut node = accesskit::Node::new(accesskit::Role::Label);
        node.set_value(text);
        node
    })
}

/// "Page 2 of 5", or "Page 1 of …" before the document has read (`PdfPreview`).
fn page_label(page: usize, pages: Option<usize>) -> String {
    match pages {
        Some(pages) => format!("Page {page} of {pages}"),
        None => format!("Page {page} of …"),
    }
}

fn message(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .px(px(INSET))
        .py(px(INSET))
        .text_size(px(13.))
        .line_height(px(19.))
        .text_color(palette.foreground)
        .child(text.into())
}

impl Render for FileView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<crate::pages::Theme>().0;
        if !self.is_current() {
            self.retire(cx);
            return message("This file view is no longer active.", palette).into_any_element();
        }
        self.draw_pdf(window, cx);
        let remote = self.remote();
        let shown_path = match &self.state {
            State::Ready { preview, .. } => preview.path.clone(),
            _ => self.path.clone(),
        };
        // Open in app: on this computer, for any file but the project's code (the Nexus app opens that in an editor).
        let openable = !remote
            && match &self.state {
                State::Ready { preview, .. } => !is_code(preview),
                State::Video(_) => true,
                _ => false,
            };
        let editable = can_edit(
            remote,
            match &self.state {
                State::Ready { preview, .. } => Some(preview),
                _ => None,
            },
        );
        let saveable =
            matches!(&self.state, State::Ready { preview, body: Body::Code(_) } if can_save(remote, preview));
        let unsaved = self.unsaved(cx).is_some();
        let header = h_flex()
            .flex_none()
            .h(px(36.))
            .pl(px(INSET - 6.))
            .pr(px(INSET - 6.))
            .gap(px(2.))
            .child(
                icon_button("file-back", Lucide::ArrowLeft, palette)
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Back to Files").build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.foreground)
                    .child(file_name(&shown_path).to_string())
                    .when(unsaved, |title| title.child(" •")),
            )
            .when(unsaved, |header| {
                header.child(
                    icon_button("file-save", Lucide::Save, palette)
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Save").build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.save(&SaveFile, window, cx))),
                )
            })
            .when(
                matches!(
                    &self.state,
                    State::Ready { preview, body: Body::Markdown { .. } | Body::Html { .. } }
                        if preview.status == Status::Ready
                ),
                |header| header.child(self.source_toggle(palette, cx)),
            )
            .child(
                icon_button("file-copy-path", Lucide::Copy, palette)
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Copy file path").build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.copy_path(cx))),
            )
            .child(
                icon_button("file-read-again", Lucide::RotateCw, palette)
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Read the file again").build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.read(window, cx))),
            )
            .when(editable, |header| {
                header.child(
                    icon_button("file-open-editor", Lucide::SquarePen, palette)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Open in editor").build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.open_in_editor(cx))),
                )
            })
            .when(openable, |header| {
                header.child(
                    icon_button("file-open-app", Lucide::ArrowUpRight, palette)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Open in app").build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.open_in_app(cx))),
                )
            });
        let faint = |text: String| {
            div()
                .flex_none()
                .px(px(INSET))
                .pb(px(6.))
                .text_size(px(11.5))
                .line_height(px(16.))
                .text_color(palette.faint)
                .child(text)
        };
        let mut column = v_flex()
            .size_full()
            .when(saveable, |column| {
                column.key_context("FileView").on_action(cx.listener(Self::save))
            })
            .child(header);
        let preview = match &self.state {
            State::Opening => {
                return column
                    .child(faint(format!("Opening {}…", file_name(&self.path))))
                    .into_any_element();
            }
            State::Failed(error) => {
                let error = error.clone();
                return column.child(self.failure(&error, palette, cx)).into_any_element();
            }
            State::Ready { preview, body } => (preview, body),
            State::Video(clip) => return column.child(self.video_view(clip, palette, cx)).into_any_element(),
        };
        let (preview, body) = preview;
        column = column.child(faint(meta(preview, remote)));
        if let Some(notice) = &self.notice {
            column = column.child(faint(notice.to_string()));
        }
        match preview.status {
            Status::TooLarge => return column.child(message(too_large(remote), palette)).into_any_element(),
            Status::Unsupported => {
                return column
                    .child(message(unsupported(remote, self.previews), palette))
                    .into_any_element();
            }
            Status::Ready => {}
        }
        if preview.truncated {
            column = column.child(faint("Only the first 1 MB is shown.".into()));
        }
        let actions = |row: Div| row.flex_none().px(px(INSET - 4.)).pb(px(6.)).gap(px(4.));
        let entity = cx.entity().downgrade();
        // The preview's width, for an image to fit.
        let measure = canvas(
            move |bounds, _, cx| {
                let width = f32::from(bounds.size.width);
                let _ = entity.update(cx, |this, cx| {
                    if (this.width - width).abs() > 0.5 {
                        this.width = width;
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let scroller = |id: &'static str| div().id(id).relative().flex_1().min_h(px(0.)).overflow_scroll();
        let content: AnyElement = match body {
            Body::Nothing => message(unsupported(remote, self.previews), palette).into_any_element(),
            Body::Image(image) => {
                column = column.child(
                    actions(h_flex()).child(
                        link_button(
                            "file-zoom",
                            if self.zoom { "Fit image" } else { "Actual size" },
                            self.zoom,
                            palette,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.is_current() {
                                return;
                            }
                            this.zoom = !this.zoom;
                            cx.notify();
                        })),
                    ),
                );
                scroller("file-image")
                    .child(measure)
                    .child(div().p(px(INSET)).child(self.image_view(image, palette)))
                    .into_any_element()
            }
            Body::Markdown { view, source } => {
                if self.source {
                    code_view(source, true)
                } else {
                    scroller("file-markdown")
                        .child(
                            div()
                                .px(px(INSET))
                                .py(px(INSET))
                                .text_size(px(14.))
                                .line_height(px(22.))
                                .child(crate::workspace::prose(
                                    crate::external_link::text_view(view)
                                        .selectable(true)
                                        .on_link_click(self.link_opener(cx)),
                                    palette,
                                )),
                        )
                        .into_any_element()
                }
            }
            Body::Html { page, source } => {
                column = column.child(faint("Read-only HTML · embedded assets only".into()));
                if self.source {
                    code_view(source, true)
                } else {
                    div().flex_1().min_h(px(0.)).child(page.clone()).into_any_element()
                }
            }
            Body::Code(editor) => code_view(editor, !saveable),
            Body::Pdf(pdf) => {
                let (page, pages) = (pdf.page, pdf.pages);
                let first = pages.is_none() || page == 1;
                let last = pages.is_none_or(|pages| page >= pages);
                let turn = |id: &'static str, label: &'static str, off: bool, step: isize| {
                    link_button(id, label, false, palette)
                        .when(off, |button| button.opacity(0.4).cursor_default())
                        .when(!off, |button| {
                            button.on_click(cx.listener(move |this, _, _, cx| this.turn(step, cx)))
                        })
                };
                column = column.child(
                    actions(h_flex())
                        .child(turn("pdf-previous", "Previous", first, -1))
                        .child(
                            div()
                                .px(px(4.))
                                .text_size(px(12.))
                                .text_color(palette.foreground)
                                .child(page_label(page, pages)),
                        )
                        .child(turn("pdf-next", "Next", last, 1)),
                );
                if let Some(error) = &pdf.error {
                    column = column.child(message(format!("This PDF could not be displayed. {error}"), palette));
                }
                let shown = pdf.drawn.clone().map(|(image, (width, height))| {
                    img(ImageSource::Image(image)).w(px(width)).h(px(height)).flex_none()
                });
                let text = pdf.text.clone();
                scroller("file-pdf")
                    .child(measure)
                    .child(
                        div()
                            .id("file-pdf-page")
                            .role(accesskit::Role::Image)
                            .aria_label(format!("PDF page {page}"))
                            .a11y_synthetic_children(move |builder| {
                                if let Some(node) = page_text(&text) {
                                    builder.push_child(builder.synthetic_node_id("text"), node);
                                }
                            })
                            .p(px(INSET))
                            .flex()
                            .justify_center()
                            .min_w_full()
                            .children(shown),
                    )
                    .into_any_element()
            }
        };
        column.child(content).into_any_element()
    }
}

fn code_view(editor: &Entity<EditorState>, readonly: bool) -> AnyElement {
    div()
        .flex_1()
        .min_h(px(0.))
        .text_size(px(12.5))
        .child(Editor::new(editor).readonly(readonly).size_full().appearance(false))
        .into_any_element()
}

/// Text of the project that is not Markdown or HTML: the Nexus app offers an editor for it, not Open in app.
fn is_code(preview: &Preview) -> bool {
    preview.text.is_some()
        && preview.media_type != "text/markdown"
        && preview.media_type != "text/html"
        && !preview.outside_project
}

/// Only a local code file can be edited; documents keep Open in app.
fn can_edit(remote: bool, preview: Option<&Preview>) -> bool {
    !remote && preview.is_some_and(is_code)
}

/// Edited and saved in place: the project's code on this computer, read whole. A truncated read would cut it short.
fn can_save(remote: bool, preview: &Preview) -> bool {
    can_edit(remote, Some(preview)) && !preview.truncated
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn a_missing_file_says_not_found_with_its_path_and_other_errors_say_themselves() {
        let missing = Failure::of(output_files::MISSING, ".nexus/artifacts/demo.gif");
        assert_eq!(missing.title, "File not found");
        assert_eq!(missing.icon, Lucide::FileSearch);
        assert_eq!(missing.path.as_deref(), Some(".nexus/artifacts/demo.gif"));
        assert!(!missing.detail.contains("missing or has moved"));
        let denied = "The operating system denied access to this file. Check its permissions and try again.";
        let other = Failure::of(denied, "a.md");
        assert_eq!(other.title, "This file could not open");
        assert_eq!((other.path, other.detail.as_str()), (None, denied));
    }

    #[test]
    fn editor_action_is_for_local_project_text_only() {
        let mut preview = Preview {
            path: "/project with spaces/src/nested file.rs".into(),
            media_type: "text/plain".into(),
            size: 10,
            outside_project: false,
            text: Some("fn main() {}".into()),
            data: None,
            truncated: false,
            status: Status::Ready,
            offset: None,
        };
        assert!(can_edit(false, Some(&preview)));
        assert!(!can_edit(true, Some(&preview)), "remote paths stay on their machine");
        assert!(!can_edit(false, None), "loading and failed reads have no action");
        preview.outside_project = true;
        assert!(!can_edit(false, Some(&preview)));
        preview.outside_project = false;
        for media_type in ["text/markdown", "text/html"] {
            preview.media_type = media_type.into();
            assert!(!can_edit(false, Some(&preview)), "documents open in their own app");
        }
        preview.media_type = "image/png".into();
        preview.text = None;
        assert!(!can_edit(false, Some(&preview)));
        preview.media_type = "text/plain".into();
        preview.text = Some(String::new());
        assert!(can_edit(false, Some(&preview)), "empty source files remain editable");
        assert!(can_save(false, &preview));
        assert!(!can_save(true, &preview), "remote files are not saved from here");
        preview.truncated = true;
        assert!(
            !can_save(false, &preview),
            "saving a truncated read would cut the file short"
        );
        assert!(
            can_edit(false, Some(&preview)),
            "an outside editor still opens it whole"
        );
    }

    #[test]
    fn sizes_read_as_the_nexus_app_writes_them() {
        assert_eq!(size_label(900), "900 B");
        assert_eq!(size_label(2048), "2 KB");
        assert_eq!(size_label(1536 * 1024), "1.5 MB");
    }

    #[test]
    fn the_messages_are_the_nexus_apps() {
        assert_eq!(
            too_large(false),
            "This file exceeds the 16 MB preview limit. Use Open in app to view the full file."
        );
        assert_eq!(
            too_large(true),
            "This file exceeds the 16 MB preview limit. Open it on its machine."
        );
        assert_eq!(
            unsupported(false, true),
            "A preview is unavailable for this format. Use Open in app to view it."
        );
        assert_eq!(
            unsupported(true, false),
            "A preview is unavailable for this format. Open it on its machine. Update Nexus on this machine for image and PDF previews."
        );
        let preview = Preview {
            path: "/p/a.md".into(),
            media_type: "text/markdown".into(),
            size: 2048,
            outside_project: true,
            text: Some(String::new()),
            data: None,
            truncated: false,
            status: Status::Ready,
            offset: None,
        };
        assert_eq!(meta(&preview, true), "/p/a.md\nRemote machine · Outside project · 2 KB");
    }

    #[test]
    fn markdown_links_open_files_from_the_files_folder_and_web_pages_in_the_browser() {
        assert_eq!(link_target("docs/a.md", "b.md"), Link::File("docs/b.md".into()));
        assert_eq!(
            link_target("docs/a.md", "./img/c.png#x"),
            Link::File("docs/img/c.png".into())
        );
        assert_eq!(link_target("a.md", "../b.md"), Link::File("../b.md".into()));
        assert_eq!(link_target("docs/a.md", "/etc/hosts"), Link::File("/etc/hosts".into()));
        assert_eq!(
            link_target("docs/a.md", "https://nexus.xyz"),
            Link::Web("https://nexus.xyz".into())
        );
        assert_eq!(link_target("docs/a.md", "#usage"), Link::Nothing);
        assert_eq!(link_target("docs/a.md", "javascript:alert(1)"), Link::Nothing);
        assert_eq!(link_target("docs/a.md", "C:/x.md"), Link::File("C:/x.md".into()));
    }

    #[test]
    fn html_opens_as_the_nexus_apps_read_only_document() {
        let url = html_url("<p>é</p>");
        let data = url.strip_prefix("data:text/html;base64,").expect("a data address");
        let page = String::from_utf8(STANDARD.decode(data).expect("base64")).expect("UTF-8");
        assert_eq!(page, output_files::preview_html("<p>é</p>"));
    }

    #[test]
    fn a_servers_video_copy_goes_with_the_viewer_and_the_sessions_own_file_stays() {
        let dir = std::env::temp_dir().join(format!("nexus-file-view-{}", uuid::Uuid::new_v4()));
        let copy = copy_of(&dir, "out/flow.webm", &[1, 2, 3]).expect("copy the video");
        let path = copy.path.clone();
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("webm"));
        assert_eq!(std::fs::read(&path).expect("read the copy"), [1, 2, 3]);
        drop(copy);
        assert!(!path.exists());
        let _ = std::fs::remove_dir(&dir);
        let own = std::env::temp_dir().join(format!("nexus-file-view-{}.webm", uuid::Uuid::new_v4()));
        std::fs::write(&own, [1]).expect("write the video");
        drop(VideoFile {
            path: own.clone(),
            copy: false,
        });
        assert!(own.exists());
        let _ = std::fs::remove_file(own);
    }

    #[test]
    fn the_pdf_page_line_reads_as_the_nexus_apps() {
        assert_eq!(page_label(1, None), "Page 1 of …");
        assert_eq!(page_label(2, Some(5)), "Page 2 of 5");
    }

    #[test]
    fn turning_the_pdf_page_forgets_the_last_pages_text_until_the_new_one_draws() {
        let mut pdf = PdfPages {
            bytes: Arc::new(Vec::new()),
            page: 1,
            pages: Some(2),
            drawn: None,
            text: "Quarterly report".into(),
            error: None,
            asked: Some((1, 400, 100)),
            _draw: None,
        };
        assert!(!pdf.ask((1, 400, 100)));
        assert_eq!(pdf.text.as_ref(), "Quarterly report");
        assert!(pdf.ask((2, 400, 100)));
        assert_eq!(pdf.text.as_ref(), "");
        assert!(pdf.drawn.is_none() && pdf.error.is_none());
    }

    #[test]
    fn a_screen_reader_reads_the_pdf_pages_text_and_nothing_for_a_page_without_text() {
        let node = page_text("Quarterly report Revenue grew").expect("a node");
        assert_eq!(node.role(), accesskit::Role::Label);
        assert_eq!(node.value(), Some("Quarterly report Revenue grew"));
        assert!(page_text("").is_none());
    }

    #[test]
    fn an_image_fits_the_pane_and_never_grows() {
        assert_eq!(fit(1000, 500, 400.), (400., 200.));
        assert_eq!(fit(100, 50, 400.), (100., 50.));
    }

    #[test]
    fn gpui_draws_the_nexus_apps_image_formats_but_avif() {
        let mut png = Vec::new();
        ::image::RgbaImage::new(3, 2)
            .write_to(&mut Cursor::new(&mut png), ::image::ImageFormat::Png)
            .expect("encode a PNG");
        let (_, pixels) = image_of("image/png", png).expect("a PNG draws");
        assert_eq!(pixels, Some((3, 2)));
        assert!(image_of("image/png", b"not a png".to_vec()).is_none());
        assert!(image_of("image/avif", vec![0; 8]).is_none());
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>"#.to_vec();
        assert_eq!(image_of("image/svg+xml", svg).map(|(_, pixels)| pixels), Some(None));
    }
}

#[cfg(test)]
mod retention_tests;
