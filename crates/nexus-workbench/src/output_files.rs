//! Read one file selected in Overview → Outputs. Unlike project search, the selected file can be outside the
//! session folder. This never walks directories, changes the agent's permissions, or attaches bytes to a prompt.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

pub const FEATURE: &str = "file-previews";
/// A host's hello names this when `wb_file_preview` takes an `offset` and answers images and PDFs in pieces of
/// [`crate::output_media::CHUNK`]: a whole 16 MB file is about 21 MB in base64, over a WebSocket frame.
pub const PIECES_FEATURE: &str = "file-preview-pieces";
pub const MAX_MEDIA: u64 = 16 * 1024 * 1024;
pub const MAX_TEXT: u64 = 1024 * 1024;

/// Whether a preview shows: `ready`, or why not. The wire names are the Nexus app's (`FilePreview["status"]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Ready,
    /// An image or PDF over [`MAX_MEDIA`].
    TooLarge,
    /// Neither text nor an image or PDF.
    Unsupported,
}

/// A `wb_file_preview` answer: written by the session's host, read by the app that shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub path: String,
    pub media_type: String,
    pub size: u64,
    pub outside_project: bool,
    pub text: Option<String>,
    pub data: Option<String>,
    pub truncated: bool,
    pub status: Status,
    /// Where `data` starts, when the caller asked for a piece. Absent for a whole read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

/// An HTML file as a read-only document (`previewHtml` in `html-preview.ts`): its policy allows no script, network,
/// form or base address, only inline styles and embedded (`data:`) images and fonts.
pub fn preview_html(source: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{HTML_POLICY}\"></head><body>{source}</body></html>"
    )
}

/// The policy [`preview_html`] puts first in the document.
const HTML_POLICY: &str =
    "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'";

/// The scale a PDF page of `base_width` by `base_height` points draws at, `width` pixels across at a screen `ratio`
/// (`PdfPreview` in `pdf-preview.tsx`): fitted to the width, never above 2. A malformed or poster-sized page stays
/// under 8192 pixels tall and 16,777,216 pixels in all.
pub fn pdf_scale(base_width: f32, base_height: f32, width: f32, ratio: f32) -> f32 {
    let ratio = ratio.clamp(f32::MIN_POSITIVE, 2.);
    let (base_width, base_height) = (base_width.max(1.), base_height.max(1.));
    2f32.min(width / base_width)
        .min(8192. / base_height / ratio)
        .min((16_777_216. / (base_width * base_height)).sqrt() / ratio)
}

/// Resolve the specific path on the session's machine. A symlink reports its real destination.
pub fn resolve(root: &Path, path: &str) -> Result<PathBuf, String> {
    if path.is_empty() || path.chars().any(char::is_control) || path.starts_with("//") || path.starts_with("\\\\") {
        return Err("Choose a local file path.".into());
    }
    let candidate = if let Some(tail) = path.strip_prefix("~/") {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .ok_or("The home folder is unavailable.")?;
        PathBuf::from(home).join(tail)
    } else {
        root.join(path)
    };
    let full = candidate.canonicalize().map_err(read_error)?;
    if !full.metadata().map_err(read_error)?.is_file() {
        return Err("Only regular files can be previewed. Choose a file, not a folder or device.".into());
    }
    Ok(full)
}

/// The error a read answers when no file is at the path. It crosses the wire as text, from hosts of any version, so
/// an app tells it apart by [`is_missing`] and shows a "File not found" state instead.
pub const MISSING: &str = "This file is missing or has moved. Check its path and try again.";

/// Whether a read failed because no file is at the path ([`MISSING`]).
pub fn is_missing(error: &str) -> bool {
    error == MISSING
}

fn read_error(error: io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => MISSING.into(),
        io::ErrorKind::PermissionDenied => {
            "The operating system denied access to this file. Check its permissions and try again.".into()
        }
        _ => format!("Could not read this file: {error}"),
    }
}

/// The whole file (text, or media up to [`MAX_MEDIA`]). [`read_piece`] reads media in pieces instead.
pub fn read(root: &Path, path: &str) -> Result<Preview, String> {
    read_at(root, path, None)
}

/// Save `text` over the project's file at `path`, only while it still holds `expected` (the text read): a change on
/// disk since then fails, never overwritten. The text goes to a file beside it, renamed over it, so a failed write
/// leaves the old file whole. A symlink keeps pointing at its file, which takes the text.
pub fn write(root: &Path, path: &str, expected: &str, text: &str) -> Result<(), String> {
    let full = resolve(root, path)?;
    if root.canonicalize().map_or(true, |root| !full.starts_with(root)) {
        return Err("Only files in the project can be saved here.".into());
    }
    // The check and the rename are not atomic: a write in that gap is lost. A lock would need the agent to take it too.
    if std::fs::read(&full).map_err(read_error)? != expected.as_bytes() {
        return Err("This file changed on disk since it opened. Read it again, then make your edit.".into());
    }
    let name = full.file_name().unwrap_or_default().to_string_lossy();
    let next = full.with_file_name(format!(".{name}.nexus-save-{}", std::process::id()));
    // A new file only (O_EXCL): anything already at that name, a symlink planted in the project included, fails the
    // save instead of being written through. Only a file this call made is removed.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&next)
        .map_err(|error| format!("Could not save this file: {error}"))?;
    let saved = file
        .write_all(text.as_bytes())
        .and_then(|()| file.set_permissions(std::fs::metadata(&full)?.permissions()))
        .and_then(|()| std::fs::rename(&next, &full));
    saved.map_err(|error| {
        let _ = std::fs::remove_file(&next);
        format!("Could not save this file: {error}")
    })
}

/// As [`read`], but an image or PDF comes as the piece from `offset`, at most [`crate::output_media::CHUNK`] bytes;
/// `size` says how many there are in all. Text is read whole as before (it is at most [`MAX_TEXT`]).
pub fn read_piece(root: &Path, path: &str, offset: u64) -> Result<Preview, String> {
    read_at(root, path, Some(offset))
}

impl Preview {
    /// A host without [`FEATURE`] answers `wb_file_read` only: its text as plain text, a binary file unsupported, as
    /// the Nexus app's `file-view.tsx` shows an older machine's file.
    pub fn from_text(file: crate::project_files::FileText) -> Preview {
        Preview {
            path: file.path,
            media_type: "text/plain".into(),
            size: file.size,
            outside_project: false,
            status: if file.binary {
                Status::Unsupported
            } else {
                Status::Ready
            },
            text: file.text,
            data: None,
            truncated: file.truncated,
            offset: None,
        }
    }

    /// The image's or PDF's bytes, when they came.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        STANDARD.decode(self.data.as_deref()?).ok()
    }
}

const CHANGED: &str = "The file changed while it loaded. Read the file again.";

/// The whole preview through `read`, a call to the session's host (`wb_file_preview`, its offset or none). With
/// `pieces` (the host names [`PIECES_FEATURE`]), an image or PDF comes in pieces and they are joined here, so a large
/// one fits a remote connection's messages; without, one read as before. Text and refusals come as they are. The
/// Nexus app's `readPreview` (`preview-pieces.ts`).
pub async fn read_whole<F, Fut>(mut read: F, pieces: bool) -> Result<Preview, String>
where
    F: FnMut(Option<u64>) -> Fut,
    Fut: std::future::Future<Output = Result<Preview, String>>,
{
    if !pieces {
        return read(None).await;
    }
    let first = read(Some(0)).await?;
    if first.offset.is_none() || first.data.is_none() || first.status != Status::Ready {
        return Ok(first);
    }
    let mut all = Vec::new();
    let mut piece = first.clone();
    loop {
        let data = piece
            .data
            .as_deref()
            .filter(|_| piece.size == first.size && piece.offset == Some(all.len() as u64));
        let bytes = data.and_then(|data| STANDARD.decode(data).ok()).ok_or(CHANGED)?;
        if (all.len() + bytes.len()) as u64 > first.size {
            return Err(CHANGED.into());
        }
        all.extend_from_slice(&bytes);
        if all.len() as u64 >= first.size {
            break;
        }
        if bytes.is_empty() {
            return Err("The file ended early. Read the file again.".into());
        }
        piece = read(Some(all.len() as u64)).await?;
    }
    Ok(Preview {
        data: Some(STANDARD.encode(all)),
        offset: None,
        ..first
    })
}

/// The file "Open in app" opens: a document or image format only ([`can_open`]), never an executable.
pub fn openable(root: &Path, path: &str) -> Result<PathBuf, String> {
    let file = resolve(root, path)?;
    if !can_open(&file) {
        return Err("Open this file from your file manager. Nexus only launches document and image formats.".into());
    }
    Ok(file)
}

fn read_at(root: &Path, path: &str, offset: Option<u64>) -> Result<Preview, String> {
    let full = resolve(root, path)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // A file replaced with a FIFO between resolution and open must not block a worker forever.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NONBLOCK);
    }
    let mut file = options.open(&full).map_err(read_error)?;
    let metadata = file.metadata().map_err(read_error)?;
    if !metadata.is_file() {
        return Err("Only regular files can be previewed.".into());
    }
    let media = media_type(&full);
    let binary_media = media.starts_with("image/") || media == "application/pdf";
    let limit = if binary_media { MAX_MEDIA } else { MAX_TEXT };
    let mut preview = Preview {
        path: full.to_string_lossy().into_owned(),
        media_type: media.into(),
        size: metadata.len(),
        outside_project: root.canonicalize().map_or(true, |root| !full.starts_with(root)),
        text: None,
        data: None,
        truncated: false,
        status: Status::Ready,
        offset: None,
    };
    if binary_media && metadata.len() > limit {
        preview.status = Status::TooLarge;
        return Ok(preview);
    }
    if let (true, Some(offset)) = (binary_media, offset) {
        use std::io::{Seek as _, SeekFrom};
        if offset > metadata.len() {
            return Err("The file changed while it loaded. Load it again.".into());
        }
        file.seek(SeekFrom::Start(offset)).map_err(read_error)?;
        let mut bytes = Vec::new();
        file.take(crate::output_media::CHUNK.min(metadata.len() - offset))
            .read_to_end(&mut bytes)
            .map_err(read_error)?;
        preview.offset = Some(offset);
        preview.data = Some(STANDARD.encode(bytes));
        return Ok(preview);
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(read_error)?;
    preview.truncated = bytes.len() as u64 > limit;
    if binary_media {
        if preview.truncated {
            preview.status = Status::TooLarge;
        } else {
            preview.data = Some(STANDARD.encode(bytes));
        }
    } else if bytes.contains(&0)
        || std::str::from_utf8(&bytes).is_err_and(|error| !preview.truncated || error.error_len().is_some())
    {
        preview.status = Status::Unsupported;
    } else {
        bytes.truncate(limit as usize);
        preview.text = Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    Ok(preview)
}

fn media_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html",
        "md" | "markdown" => "text/markdown",
        _ => "text/plain",
    }
}

/// Document associations only. Selecting an output must not turn an executable into a launch button.
pub fn can_open(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "pdf"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "avif"
            | "svg"
            | "mp4"
            | "webm"
            | "mov"
            | "txt"
            | "md"
            | "markdown"
            | "html"
            | "htm"
            | "csv"
            | "tsv"
            | "json"
            | "yaml"
            | "yml"
            | "toml"
            | "xml"
            | "doc"
            | "docx"
            | "odt"
            | "rtf"
            | "xls"
            | "xlsx"
            | "ods"
            | "ppt"
            | "pptx"
            | "odp"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_previews_as_the_nexus_apps_read_only_document() {
        assert_eq!(
            preview_html("<p>Hi</p>"),
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'\"></head><body><p>Hi</p></body></html>"
        );
        // A policy in the file adds to this one: the browser enforces both, so it cannot loosen it.
        assert!(preview_html("<meta http-equiv=x>").starts_with("<!doctype html><html><head><meta charset"));
    }
    #[test]
    fn a_pdf_page_fits_the_width_never_above_twice_and_never_poster_sized() {
        // A Letter page in a 500 px pane fits it.
        assert!((pdf_scale(612., 792., 500., 1.) - 500. / 612.).abs() < 1e-6);
        // A small page in a wide pane draws at most twice its size.
        assert_eq!(pdf_scale(100., 100., 2000., 1.), 2.);
        // A tall page stays under 8192 pixels on a 2x screen.
        let scale = pdf_scale(100., 20_000., 500., 2.);
        assert!(20_000. * scale * 2. <= 8192.001);
        // A huge page stays under 16,777,216 pixels in all.
        let scale = pdf_scale(10_000., 8_000., 20_000., 2.);
        assert!((10_000. * scale * 2.) * (8_000. * scale * 2.) <= 16_777_300.);
        // A screen above 2x counts as 2x.
        assert_eq!(pdf_scale(100., 20_000., 500., 3.), pdf_scale(100., 20_000., 500., 2.));
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("nexus-output-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(dir.join("project")).expect("create the fixture project");
            Self(dir)
        }
        fn root(&self) -> PathBuf {
            self.0.join("project")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn previews_selected_absolute_and_parent_paths_without_broadening_search() {
        let f = Fixture::new();
        let outside = f.0.join("résumé draft.md");
        std::fs::write(&outside, "# Ready").expect("write the selected outside file");
        for path in [outside.to_string_lossy().into_owned(), "../résumé draft.md".into()] {
            let preview = read(&f.root(), &path).expect("preview the selected outside file");
            assert_eq!(preview.text.as_deref(), Some("# Ready"));
            assert!(preview.outside_project);
            assert_eq!(preview.media_type, "text/markdown");
            assert!(crate::project_files::read(&f.root(), &path).is_err());
        }
    }

    #[test]
    fn saves_only_an_unchanged_project_file() {
        let f = Fixture::new();
        let file = f.root().join("main.rs");
        std::fs::write(&file, "old").expect("write the project file");
        write(&f.root(), "main.rs", "old", "new").expect("save over the text read");
        assert_eq!(std::fs::read_to_string(&file).expect("read it back"), "new");
        let stale = write(&f.root(), "main.rs", "old", "lost").expect_err("refuse a file changed since it was read");
        assert!(stale.contains("changed on disk"), "{stale}");
        assert_eq!(std::fs::read_to_string(&file).expect("read it back"), "new");
        let outside = f.0.join("outside.txt");
        std::fs::write(&outside, "x").expect("write a file outside the project");
        let error = write(&f.root(), "../outside.txt", "x", "y").expect_err("refuse a file outside the project");
        assert!(error.contains("in the project"), "{error}");
        assert_eq!(
            std::fs::read_dir(f.root()).expect("list the project").count(),
            1,
            "no temporary file is left"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_save_never_writes_through_a_planted_temporary_name() {
        let f = Fixture::new();
        std::fs::write(f.root().join("main.rs"), "old").expect("write the project file");
        let target = f.0.join("victim.txt");
        std::fs::write(&target, "keep").expect("write the file outside the project");
        let planted = f.root().join(format!(".main.rs.nexus-save-{}", std::process::id()));
        std::os::unix::fs::symlink(&target, &planted).expect("plant a symlink at the temporary name");
        write(&f.root(), "main.rs", "old", "new").expect_err("refuse to write through an existing path");
        assert_eq!(std::fs::read_to_string(&target).expect("read the target"), "keep");
        assert_eq!(
            std::fs::read_to_string(f.root().join("main.rs")).expect("read the file"),
            "old"
        );
        assert!(
            planted.symlink_metadata().is_ok(),
            "a path this save did not make stays"
        );
    }

    #[test]
    fn bounds_media_and_text_and_rejects_devices() {
        let f = Fixture::new();
        let big = f.root().join("large.pdf");
        std::fs::File::create(&big)
            .expect("create the oversized media file")
            .set_len(MAX_MEDIA + 1)
            .expect("set the oversized media length");
        let result = read(&f.root(), "large.pdf").expect("report the oversized media file");
        assert_eq!(result.status, Status::TooLarge);
        assert!(result.data.is_none());
        std::fs::write(f.root().join("long.txt"), vec![b'a'; MAX_TEXT as usize + 100])
            .expect("write text beyond the preview limit");
        let result = read(&f.root(), "long.txt").expect("preview text up to the limit");
        assert!(result.truncated);
        assert_eq!(
            result.text.expect("the text preview contains text").len(),
            MAX_TEXT as usize
        );
        assert!(
            read(&f.root(), ".")
                .expect_err("reject a directory")
                .contains("regular files")
        );
        assert!(is_missing(
            &read(&f.root(), "missing.pdf").expect_err("reject a missing file")
        ));
        assert!(!is_missing(&read(&f.root(), ".").expect_err("reject a directory")));
        assert!(resolve(&f.root(), "//server/share/file.pdf").is_err());
        #[cfg(unix)]
        assert!(
            read(&f.root(), "/dev/zero")
                .expect_err("reject a device")
                .contains("regular files")
        );
    }

    #[test]
    fn preserves_media_and_reports_unknown_binary() {
        let f = Fixture::new();
        std::fs::write(f.root().join("image.png"), b"\x89PNG\0\x01").expect("write the image fixture");
        let image = read(&f.root(), "image.png").expect("preview the image fixture");
        assert_eq!(
            STANDARD
                .decode(image.data.expect("the image preview contains base64 data"))
                .expect("decode the image preview"),
            b"\x89PNG\0\x01"
        );
        std::fs::write(f.root().join("data.bin"), b"\0\xff").expect("write the unknown binary fixture");
        assert_eq!(
            read(&f.root(), "data.bin")
                .expect("report the unknown binary file")
                .status,
            Status::Unsupported
        );
        std::fs::write(f.root().join("unicode.txt"), "日本語").expect("write the Unicode text fixture");
        assert_eq!(
            read(&f.root(), "unicode.txt")
                .expect("preview the Unicode text")
                .text
                .as_deref(),
            Some("日本語")
        );
    }

    #[test]
    fn reads_media_in_pieces_and_text_whole() {
        let f = Fixture::new();
        let chunk = crate::output_media::CHUNK;
        let bytes: Vec<u8> = (0..chunk + 7).map(|n| (n % 241) as u8).collect();
        std::fs::write(f.root().join("deck.pdf"), &bytes).expect("write the PDF fixture");
        let first = read_piece(&f.root(), "deck.pdf", 0).expect("read the first piece");
        assert_eq!(
            (first.offset, first.size, first.status),
            (Some(0), chunk + 7, Status::Ready)
        );
        let decode = |p: &Preview| {
            STANDARD
                .decode(p.data.as_deref().expect("the piece has data"))
                .expect("decode")
        };
        assert_eq!(decode(&first), bytes[..chunk as usize]);
        let last = read_piece(&f.root(), "deck.pdf", chunk).expect("read the last piece");
        assert_eq!(decode(&last), bytes[chunk as usize..]);
        assert!(read_piece(&f.root(), "deck.pdf", chunk + 8).is_err());
        std::fs::write(f.root().join("notes.md"), "# Notes").expect("write the text fixture");
        let text = read_piece(&f.root(), "notes.md", 0).expect("read text whole");
        assert_eq!((text.text.as_deref(), text.offset), (Some("# Notes"), None));
        let whole = read(&f.root(), "deck.pdf").expect("read the PDF whole");
        assert_eq!(whole.offset, None);
        assert_eq!(decode(&whole), bytes);
    }

    // `read_whole`: the Nexus app's `preview-pieces.test.ts`.
    fn base() -> Preview {
        Preview {
            path: "/p/deck.pdf".into(),
            media_type: "application/pdf".into(),
            size: 5,
            outside_project: false,
            text: None,
            data: None,
            truncated: false,
            status: Status::Ready,
            offset: None,
        }
    }

    fn piece(offset: u64, bytes: &[u8], size: u64) -> Preview {
        Preview {
            size,
            offset: Some(offset),
            data: Some(STANDARD.encode(bytes)),
            ..base()
        }
    }

    #[test]
    fn joins_the_pieces_into_the_same_preview() {
        let mut asked = Vec::new();
        let preview = futures::executor::block_on(read_whole(
            |offset| {
                asked.push(offset);
                let answer = if offset == Some(0) {
                    piece(0, &[1, 2, 3], 5)
                } else {
                    piece(3, &[4, 5], 5)
                };
                async move { Ok(answer) }
            },
            true,
        ))
        .expect("the joined preview");
        assert_eq!(asked, [Some(0), Some(3)]);
        assert_eq!(preview.offset, None);
        assert_eq!(preview.bytes(), Some(vec![1, 2, 3, 4, 5]));
        assert_eq!(preview.size, 5);
    }

    #[test]
    fn reads_whole_without_pieces_and_keeps_text_and_refusals_as_they_come() {
        let whole = Preview {
            data: Some(STANDARD.encode([1])),
            ..base()
        };
        let mut asked = Vec::new();
        let read = futures::executor::block_on(read_whole(
            |offset| {
                asked.push(offset);
                let whole = whole.clone();
                async move { Ok(whole) }
            },
            false,
        ));
        assert_eq!(read, Ok(whole));
        assert_eq!(asked, [None]);
        let text = Preview {
            media_type: "text/markdown".into(),
            text: Some("# Notes".into()),
            ..base()
        };
        let answer = text.clone();
        let read = futures::executor::block_on(read_whole(move |_| std::future::ready(Ok(answer.clone())), true));
        assert_eq!(read, Ok(text));
        let big = Preview {
            status: Status::TooLarge,
            size: 17 * 1024 * 1024,
            ..base()
        };
        let answer = big.clone();
        let read = futures::executor::block_on(read_whole(move |_| std::future::ready(Ok(answer.clone())), true));
        assert_eq!(read, Ok(big));
    }

    #[test]
    fn fails_when_the_file_changes_while_it_loads() {
        let join = |rest: Preview| {
            futures::executor::block_on(read_whole(
                move |offset| {
                    std::future::ready(Ok(if offset == Some(0) {
                        piece(0, &[1, 2, 3], 5)
                    } else {
                        rest.clone()
                    }))
                },
                true,
            ))
        };
        assert!(
            join(piece(3, &[4, 5], 9))
                .expect_err("grew")
                .contains("changed while it loaded")
        );
        assert!(join(piece(3, &[], 5)).expect_err("ended").contains("ended early"));
        assert!(
            join(piece(3, &[4, 5, 6], 5))
                .expect_err("longer")
                .contains("changed while it loaded")
        );
    }

    #[test]
    fn opens_documents_and_images_never_executables() {
        let f = Fixture::new();
        std::fs::write(f.root().join("report.pdf"), b"%PDF").expect("write the document");
        std::fs::write(f.root().join("run.sh"), b"echo").expect("write the script");
        assert!(openable(&f.root(), "report.pdf").is_ok());
        assert!(
            openable(&f.root(), "run.sh")
                .expect_err("refuse a script")
                .contains("file manager")
        );
    }

    #[test]
    fn a_preview_answer_reads_back_as_written() {
        let preview = piece(4, &[1], 5);
        let wire = serde_json::to_value(&preview).expect("serialize");
        assert_eq!(wire["status"], "ready");
        assert_eq!(wire["mediaType"], "application/pdf");
        let back: Preview = serde_json::from_value(wire).expect("deserialize");
        assert_eq!(back, preview);
        let old: Preview = serde_json::from_value(serde_json::json!({
            "path": "a.md", "mediaType": "text/markdown", "size": 1, "outsideProject": false,
            "text": "#", "data": null, "truncated": false, "status": "tooLarge"
        }))
        .expect("an answer without an offset");
        assert_eq!((old.status, old.offset), (Status::TooLarge, None));
    }

    #[test]
    fn an_older_hosts_file_is_plain_text_or_unsupported() {
        let text = crate::project_files::FileText {
            path: "a.md".into(),
            text: Some("# A".into()),
            binary: false,
            truncated: true,
            size: 3,
        };
        let preview = Preview::from_text(text.clone());
        assert_eq!(preview.media_type, "text/plain");
        assert_eq!((preview.status, preview.truncated), (Status::Ready, true));
        assert_eq!(preview.text.as_deref(), Some("# A"));
        let binary = Preview::from_text(crate::project_files::FileText {
            text: None,
            binary: true,
            ..text
        });
        assert_eq!(binary.status, Status::Unsupported);
    }

    #[cfg(unix)]
    #[test]
    fn outside_symlinks_resolve_to_the_actual_file() {
        let f = Fixture::new();
        let outside = f.0.join("outside.md");
        std::fs::write(&outside, "outside").expect("write the symlink's outside target");
        std::os::unix::fs::symlink(&outside, f.root().join("link.md")).expect("link the outside target");
        let preview = read(&f.root(), "link.md").expect("preview the selected symlink");
        assert!(preview.outside_project);
        assert_eq!(
            preview.path,
            outside
                .canonicalize()
                .expect("resolve the outside target")
                .to_string_lossy()
        );
        assert!(crate::project_files::read(&f.root(), "link.md").is_err());
    }
}
