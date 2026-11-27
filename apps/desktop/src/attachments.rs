//! Files and images attached in the composer, as the Nexus app attaches them: by the attach button, a paste, or a drop
//! on the composer or anywhere on the window. The rules, the limits and the error text are the Nexus app's, so both
//! apps take and refuse the same files:
//!
//! - Images (PNG, JPEG, GIF, WebP) are prepared as Nexus UI's composer prepares them (`chat/images.tsx`): larger than
//!   [`SEND_SIDE`] on a side, or than Gateway takes, drawn smaller; at most Gateway's limits (`CHAT_IMAGE_LIMITS` in
//!   `api.ts`).
//! - Other files keep their bytes, within the Code sessions' limits (`draft-files.ts`, and
//!   `nexus_workbench::attachments`, which checks them again).
//! - A chat (a session with no project) takes images and data files only, a data file up to one piece
//!   (`attachPlain` in `chat-start.ts`). It sends them in the message's `images` and `files`
//!   ([`chat_body`]).
//! - A Code session sends images whole and files through `nexus_workbench`, a file over one piece uploaded in pieces
//!   first ([`sendable_files`], as `attachment-upload.ts`).

use std::collections::BTreeSet;
use std::future::Future;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use gpui_kit::{ClipboardEntry, ClipboardItem};
use nexus_workbench::attachment_upload::{self, Outgoing, UploadRun};
use nexus_workbench::attachments::{
    CHUNK_BYTES, Chunk, FileInput, MAX_FILE_BYTES, MAX_FILES, MAX_TOTAL_BYTES, Progress,
};
use serde_json::{Value, json};

use crate::artifact_refs::{self, ArtifactRef};

const MB: usize = 1024 * 1024;

/// What Gateway takes in one question (`CHAT_IMAGE_LIMITS`): images, bytes each, bytes in all.
pub const IMAGE_COUNT: usize = 8;
pub const IMAGE_BYTES: usize = 5 * MB;
pub const IMAGE_TOTAL_BYTES: usize = 20 * MB;
/// Longer than this on a side, an image is drawn smaller before it is sent (`SEND_SIDE` in `images.tsx`).
pub const SEND_SIDE: u32 = 2048;
/// The image types a message takes (`IMAGE_TYPES` in `images.tsx`).
pub const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
/// The data files Gateway reads in a chat, by extension (`CHAT_FILE_LIMITS.types` in `api.ts`).
const DATA_FILE_TYPES: [(&str, &str); 7] = [
    ("csv", "text/csv"),
    ("tsv", "text/tab-separated-values"),
    ("json", "application/json"),
    ("txt", "text/plain"),
    ("md", "text/markdown"),
    ("markdown", "text/markdown"),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
];
/// The side of a chip's thumbnail, drawn at up to 4x for sharp displays.
const THUMBNAIL_SIDE: u32 = 64;

/// Where the composer sends: a chat through Gateway, or a Code session through its agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Chat,
    Code,
}

/// A file offered to the composer, before it is read.
#[derive(Debug, Clone)]
pub struct Incoming {
    pub name: String,
    /// An image's type (by its extension, or the clipboard's); `None` for any other file.
    pub media_type: Option<String>,
    pub size: u64,
    pub source: Source,
}

#[derive(Debug, Clone)]
pub enum Source {
    Path(PathBuf),
    Bytes(Arc<Vec<u8>>),
    Folder,
    /// Why the file could not be read.
    Unreadable(String),
}

/// An attached image or file, ready to send.
#[derive(Clone)]
pub struct Attachment {
    pub id: u64,
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    /// An image's type, as sent; `None` for a file.
    pub media_type: Option<&'static str>,
    /// An image's chip thumbnail.
    pub thumbnail: Option<Arc<gpui_kit::Image>>,
    /// A Library artifact referenced by the message, not a file (`artifact_refs.rs`). Its `bytes` are the source a
    /// chat gets as a data file, empty when only the reference goes.
    pub artifact: Option<Arc<ArtifactRef>>,
}

impl std::fmt::Debug for Attachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attachment")
            .field("name", &self.name)
            .field("bytes", &self.bytes.len())
            .field("media_type", &self.media_type)
            .field("artifact", &self.artifact)
            .finish()
    }
}

impl Attachment {
    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    /// A referenced artifact, as a chip in the draft: its title, and the source a chat gets, if any.
    pub fn artifact(reference: ArtifactRef, source: Option<String>) -> Attachment {
        Attachment {
            id: next_id(),
            name: reference.title.clone(),
            bytes: Arc::new(source.map(String::into_bytes).unwrap_or_default()),
            media_type: None,
            thumbnail: None,
            artifact: Some(Arc::new(reference)),
        }
    }

    /// The bytes as a `data:` URL of `media_type`.
    fn data_url(&self, media_type: &str) -> String {
        format!("data:{media_type};base64,{}", STANDARD.encode(self.bytes.as_slice()))
    }
}

/// What the composer holds besides its text.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub images: Vec<Attachment>,
    pub files: Vec<Attachment>,
    /// The send of these files in pieces, once they are sent: its progress shows on their chips, and a chip's ✕
    /// cancels it ([`UploadRun`], shared with the composer).
    pub upload: Option<Arc<UploadRun>>,
}

impl Draft {
    pub fn is_empty(&self) -> bool {
        self.images.is_empty() && self.files.is_empty()
    }

    /// Images first, then files: the order the chips show.
    pub fn all(&self) -> impl Iterator<Item = &Attachment> {
        self.images.iter().chain(&self.files)
    }

    pub fn remove(&mut self, id: u64) {
        self.images.retain(|image| image.id != id);
        self.files.retain(|file| file.id != id);
    }

    /// Whether a file is larger than one piece, so it goes up in pieces and its chip shows progress.
    pub fn sends_in_pieces(&self) -> bool {
        self.files
            .iter()
            .any(|file| file.artifact.is_none() && file.size() > CHUNK_BYTES)
    }

    /// The artifacts the message references, in the order added.
    pub fn artifacts(&self) -> Vec<ArtifactRef> {
        self.files
            .iter()
            .filter_map(|file| file.artifact.as_deref().cloned())
            .collect()
    }

    /// Add references, once each and up to [`artifact_refs::MAX`]; say what did not fit.
    pub fn add_artifacts(&mut self, added: Vec<Attachment>) -> Option<String> {
        let mut over = 0;
        for attachment in added {
            let Some(reference) = attachment.artifact.clone() else {
                continue;
            };
            if self
                .files
                .iter()
                .any(|file| file.artifact.as_ref().is_some_and(|known| known.same(&reference)))
            {
                continue;
            }
            if self.artifacts().len() >= artifact_refs::MAX {
                over += 1;
                continue;
            }
            self.files.push(attachment);
        }
        (over > 0).then(|| format!("A message references up to {} artifacts.", artifact_refs::MAX))
    }

    /// For a Code session: the artifacts leave the draft as references in `text`, and each source the app read goes on
    /// as a file, which the session saves on its computer and lists under "Attached files". The agent then needs no
    /// `nexus` CLI, whose saved Gateway may not be `gateway`, the one this app signed in to. A source not read, or one
    /// past the message's [`MAX_FILES`], goes as its reference only.
    pub fn take_artifacts(&mut self, text: &str, gateway: &str) -> String {
        let refs = self.artifacts();
        let mut room = MAX_FILES.saturating_sub(self.files.iter().filter(|file| file.artifact.is_none()).count());
        let files = std::mem::take(&mut self.files);
        for file in files {
            let Some(reference) = &file.artifact else {
                self.files.push(file);
                continue;
            };
            if file.bytes.is_empty() || room == 0 {
                continue;
            }
            room -= 1;
            self.files.push(Attachment {
                id: file.id,
                name: reference.source_file(),
                bytes: file.bytes.clone(),
                media_type: None,
                thumbnail: None,
                artifact: None,
            });
        }
        artifact_refs::with_block(text, &refs, artifact_refs::Reader::Terminal { gateway })
    }
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Bytes restored from the private draft store get a fresh in-process id; no external source is read again.
pub fn persisted_attachment(name: String, media_type: Option<&str>, bytes: Arc<Vec<u8>>) -> Attachment {
    let media_type = media_type.and_then(|mime| {
        IMAGE_TYPES.iter().copied().find(|known| *known == mime).or_else(|| {
            DATA_FILE_TYPES
                .iter()
                .map(|(_, mime)| *mime)
                .find(|known| *known == mime)
        })
    });
    let thumbnail = media_type
        .filter(|mime| IMAGE_TYPES.contains(mime))
        .and_then(|mime| ::image::load_from_memory_with_format(&bytes, format_of(mime)).ok())
        .and_then(|decoded| image_thumbnail(&decoded));
    Attachment {
        artifact: None,
        id: next_id(),
        name,
        media_type,
        bytes,
        thumbnail,
    }
}

/// An image that came from somewhere other than the composer (a tool's result, an answer's screenshot), for the
/// full-size viewer.
pub fn image_attachment(name: String, media_type: &'static str, bytes: Arc<Vec<u8>>) -> Attachment {
    Attachment {
        artifact: None,
        id: next_id(),
        name,
        bytes,
        media_type: Some(media_type),
        thumbnail: None,
    }
}

/// `1.5 MB`, or `12 KB` under a megabyte (`sizeLabel` in `images.tsx`).
pub fn size_label(bytes: usize) -> String {
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{} KB", ((bytes as f64 / 1024.).round() as u64).max(1))
    }
}

fn extension(name: &str) -> Option<String> {
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
}

/// An image's type from its name's extension.
pub fn image_type(name: &str) -> Option<&'static str> {
    match extension(name)?.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// A data file's type from its name's extension, as Gateway reads it (`dataFileType` in `data-files.tsx`).
pub fn data_file_type(name: &str) -> Option<&'static str> {
    let extension = extension(name)?;
    DATA_FILE_TYPES
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, media_type)| *media_type)
}

fn is_image_type(media_type: Option<&str>) -> bool {
    media_type.is_some_and(|media_type| IMAGE_TYPES.contains(&media_type))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// A file on this computer, picked, dropped or pasted by its path.
pub fn from_path(path: &Path) -> Incoming {
    let name = file_name(path);
    let media_type = image_type(&name).map(str::to_string);
    let (size, source) = match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => (0, Source::Folder),
        Ok(metadata) => (metadata.len(), Source::Path(path.to_path_buf())),
        Err(error) => (0, Source::Unreadable(format!("{name}: {error}"))),
    };
    Incoming {
        name,
        media_type,
        size,
        source,
    }
}

/// What a paste attaches, and whether it takes the paste (the box then inserts no text).
#[derive(Debug, Default)]
pub struct Pasted {
    pub files: Vec<Incoming>,
    pub take: bool,
}

/// A clipboard's images and files. An image copied with text (a screen recording copied for an agent) attaches the
/// image and leaves the text to paste. Files copied in a file manager attach, and their paths are not pasted: they
/// come as paths, or (Wayland) as text of `file://` addresses.
pub fn pasted(item: &ClipboardItem) -> Pasted {
    let mut pasted = Pasted::default();
    let mut text = false;
    for entry in item.entries() {
        match entry {
            ClipboardEntry::Image(image) => {
                let media_type = image.format.mime_type();
                let extension = media_type
                    .trim_start_matches("image/")
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("png");
                pasted.files.push(Incoming {
                    name: format!("Pasted image.{extension}"),
                    media_type: Some(media_type.to_string()),
                    size: image.bytes.len() as u64,
                    source: Source::Bytes(Arc::new(image.bytes.clone())),
                });
            }
            ClipboardEntry::ExternalPaths(paths) => {
                pasted.files.extend(paths.paths().iter().map(|path| from_path(path)));
                pasted.take = true;
            }
            ClipboardEntry::String(string) => match file_uris(string.text()) {
                Some(paths) => {
                    pasted.files.extend(paths.iter().map(|path| from_path(path)));
                    pasted.take = true;
                }
                None => text = true,
            },
        }
    }
    if !pasted.files.is_empty() && !text {
        pasted.take = true;
    }
    pasted
}

/// The paths of a `text/uri-list`: every line a `file://` address (`#` lines are comments). `None` for other text.
fn file_uris(text: &str) -> Option<Vec<PathBuf>> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    if lines.is_empty() {
        return None;
    }
    lines
        .iter()
        .map(|line| {
            line.starts_with("file://")
                .then(|| url::Url::parse(line).ok()?.to_file_path().ok())
                .flatten()
        })
        .collect()
}

/// What `target` takes of `files`, why it leaves the others out (said only when the rest attaches without a
/// problem: `attachPlain`), and the folders, which no composer takes.
pub fn take(target: Target, files: Vec<Incoming>) -> (Vec<Incoming>, Option<String>, Vec<String>) {
    let mut folders = Vec::new();
    let files: Vec<Incoming> = files
        .into_iter()
        .filter(|file| match file.source {
            Source::Folder => {
                folders.push(format!(
                    "{} is a folder. Drop files, or drop the folder on a terminal for its path.",
                    file.name
                ));
                false
            }
            _ => true,
        })
        .collect();
    if target == Target::Code {
        return (files, None, folders);
    }
    let fits = |file: &Incoming| data_file_type(&file.name).is_none() || file.size as usize <= CHUNK_BYTES;
    let all = files.len();
    let too_large = files.iter().any(|file| !fits(file));
    let taken: Vec<Incoming> = files
        .into_iter()
        .filter(|file| is_image_type(file.media_type.as_deref()) || data_file_type(&file.name).is_some())
        .filter(fits)
        .collect();
    let after = if too_large {
        Some("A session with no project takes data files up to 4 MB. Pick a project to share larger ones.".to_string())
    } else if taken.len() != all {
        Some(
            "A session with no project takes images and data files (CSV, JSON, text). Pick a project to share other \
             files."
                .to_string(),
        )
    } else {
        None
    };
    (taken, after, folders)
}

/// Files read and images prepared, before they join the draft.
#[derive(Debug, Default)]
pub struct Prepared {
    pub images: Vec<Attachment>,
    pub files: Vec<Attachment>,
    /// Why images were left out (`addImages`).
    pub image_problem: Option<String>,
    /// Why files were left out (`fitFiles`, a file that could not be read, a folder).
    pub errors: Vec<String>,
}

fn read(file: &Incoming) -> Result<Arc<Vec<u8>>, String> {
    match &file.source {
        Source::Bytes(bytes) => Ok(bytes.clone()),
        Source::Path(path) => std::fs::read(path)
            .map(Arc::new)
            .map_err(|_| format!("Could not read {}.", file.name)),
        Source::Unreadable(error) => Err(error.clone()),
        Source::Folder => Err(format!("{} is a folder.", file.name)),
    }
}

/// Read and prepare `files` (slow: run it off the UI thread). Images go first by type, the rest as files
/// (`attachImages` in `use-draft.ts`).
pub fn prepare(files: Vec<Incoming>) -> Prepared {
    let (pictures, documents): (Vec<Incoming>, Vec<Incoming>) = files
        .into_iter()
        .partition(|file| is_image_type(file.media_type.as_deref()));
    let mut prepared = Prepared::default();
    let mut problems = Vec::new();
    for picture in pictures {
        if prepared.images.len() >= IMAGE_COUNT {
            problems.push(format!("A message takes at most {IMAGE_COUNT} images."));
            break;
        }
        match read(&picture).and_then(|bytes| prepare_image(&picture, &bytes)) {
            Ok(image) => {
                let total: usize = prepared.images.iter().map(Attachment::size).sum::<usize>() + image.size();
                if total > IMAGE_TOTAL_BYTES {
                    problems.push(format!(
                        "A message's images may total at most {} MB.",
                        IMAGE_TOTAL_BYTES / MB
                    ));
                    break;
                }
                prepared.images.push(image);
            }
            Err(problem) => problems.push(problem),
        }
    }
    if !problems.is_empty() {
        prepared.image_problem = Some(problems.join(" "));
    }
    let (fit, errors) = fit_files(documents);
    prepared.errors = errors;
    for file in fit {
        match read(&file) {
            Ok(bytes) => prepared.files.push(Attachment {
                artifact: None,
                id: next_id(),
                name: if file.name.is_empty() {
                    "attachment".into()
                } else {
                    file.name
                },
                bytes,
                media_type: None,
                thumbnail: None,
            }),
            Err(error) => prepared.errors.push(error),
        }
    }
    prepared
}

/// Which files fit a message, in order, and why the others don't (`fitFiles` in `draft-files.ts`). A file refused for
/// its own size takes nothing from the total, so a smaller one after it still goes in.
fn fit_files(files: Vec<Incoming>) -> (Vec<Incoming>, Vec<String>) {
    let mut errors = Vec::new();
    if files.len() > MAX_FILES {
        errors.push("A message takes at most 10 files.".to_string());
    }
    let mut total = 0;
    let mut fit = Vec::new();
    for file in files.into_iter().take(MAX_FILES) {
        let size = file.size as usize;
        if size > MAX_FILE_BYTES {
            errors.push(format!("{} exceeds the 100 MB file limit.", file.name));
            continue;
        }
        if total + size > MAX_TOTAL_BYTES {
            errors.push("Files may total at most 200 MB.".to_string());
            continue;
        }
        total += size;
        fit.push(file);
    }
    (fit, errors)
}

/// Fitted to `side` on its longer side, keeping its shape, never larger (`fitted` in `images.tsx`).
pub fn fitted(width: u32, height: u32, side: u32) -> (u32, u32) {
    let scale = (side as f64 / width.max(height).max(1) as f64).min(1.);
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

fn format_of(media_type: &str) -> ::image::ImageFormat {
    match media_type {
        "image/jpeg" => ::image::ImageFormat::Jpeg,
        "image/gif" => ::image::ImageFormat::Gif,
        "image/webp" => ::image::ImageFormat::WebP,
        _ => ::image::ImageFormat::Png,
    }
}

fn encode(image: &::image::DynamicImage, media_type: &str, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    match media_type {
        "image/jpeg" => {
            let encoder = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
            image.to_rgb8().write_with_encoder(encoder).ok()?;
        }
        "image/webp" => {
            let encoder = ::image::codecs::webp::WebPEncoder::new_lossless(&mut out);
            image.to_rgba8().write_with_encoder(encoder).ok()?;
        }
        _ => image
            .write_to(&mut Cursor::new(&mut out), ::image::ImageFormat::Png)
            .ok()?,
    }
    Some(out)
}

fn image_thumbnail(decoded: &::image::DynamicImage) -> Option<Arc<gpui_kit::Image>> {
    let (w, h) = fitted(decoded.width(), decoded.height(), THUMBNAIL_SIDE);
    encode(
        &decoded.resize_exact(w, h, ::image::imageops::FilterType::Triangle),
        "image/png",
        0,
    )
    .map(|png| Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png)))
}

/// One image to send, or why not (`prepareImage` in `images.tsx`). A GIF goes as it is (it may move); any other image
/// larger than [`SEND_SIDE`] on a side, or than Gateway takes, is drawn smaller in its own type, and as a JPEG when
/// that is still too large.
fn prepare_image(file: &Incoming, bytes: &[u8]) -> Result<Attachment, String> {
    let name = if file.name.is_empty() {
        "image"
    } else {
        file.name.as_str()
    };
    let Some(media_type) = file
        .media_type
        .as_deref()
        .and_then(|media_type| IMAGE_TYPES.iter().find(|known| **known == media_type))
    else {
        return Err(format!("{name} is not a PNG, JPEG, GIF or WebP image."));
    };
    let mut media_type: &'static str = media_type;
    let decoded = ::image::load_from_memory_with_format(bytes, format_of(media_type))
        .or_else(|_| ::image::load_from_memory(bytes))
        .map_err(|_| "not an image this browser can read".to_string())?;
    let (width, height) = (decoded.width(), decoded.height());
    let mut sent = bytes.to_vec();
    let too_large = bytes.len() > IMAGE_BYTES || width.max(height) > SEND_SIDE;
    if too_large && media_type != "image/gif" {
        let (w, h) = fitted(width, height, SEND_SIDE);
        let smaller = decoded.resize_exact(w, h, ::image::imageops::FilterType::Lanczos3);
        if let Some(own) = encode(&smaller, media_type, 92) {
            sent = own;
        }
        if sent.len() > IMAGE_BYTES
            && let Some(jpeg) = encode(&smaller, "image/jpeg", 85)
        {
            sent = jpeg;
            media_type = "image/jpeg";
        }
    }
    if sent.len() > IMAGE_BYTES {
        return Err(format!(
            "{name} is too large: an image may be at most {} MB.",
            IMAGE_BYTES / MB
        ));
    }
    let thumbnail = image_thumbnail(&decoded);
    Ok(Attachment {
        artifact: None,
        id: next_id(),
        name: name.to_string(),
        bytes: Arc::new(sent),
        media_type: Some(media_type),
        thumbnail,
    })
}

/// Add `prepared` to the draft as it is now, which another attach may have changed meanwhile, and say what was left
/// out (`appendDraftImages`, `appendDraftFiles` and `attachImages`). `None` when everything fit.
pub fn merge(draft: &mut Draft, prepared: Prepared) -> Option<String> {
    let mut image_problem = None;
    for image in prepared.images {
        if draft.images.len() >= IMAGE_COUNT {
            image_problem = Some(format!("A message takes at most {IMAGE_COUNT} images."));
            break;
        }
        if draft.images.iter().map(Attachment::size).sum::<usize>() + image.size() > IMAGE_TOTAL_BYTES {
            image_problem = Some(format!(
                "A message's images may total at most {} MB.",
                IMAGE_TOTAL_BYTES / MB
            ));
            break;
        }
        draft.images.push(image);
    }
    let mut file_problems: Vec<String> = Vec::new();
    // Referenced artifacts are not files: they count toward their own limit (`Draft::add_artifacts`).
    let own = |draft: &Draft| draft.files.iter().filter(|file| file.artifact.is_none()).count();
    let mut total: usize = draft
        .files
        .iter()
        .filter(|file| file.artifact.is_none())
        .map(Attachment::size)
        .sum();
    for file in prepared.files {
        if file.artifact.is_some() {
            file_problems.extend(draft.add_artifacts(vec![file]));
            continue;
        }
        if own(draft) >= MAX_FILES {
            file_problems.push("A message takes at most 10 files.".into());
            break;
        }
        if file.size() > MAX_FILE_BYTES {
            file_problems.push(format!("{} exceeds the 100 MB file limit.", file.name));
            continue;
        }
        if total + file.size() > MAX_TOTAL_BYTES {
            file_problems.push("Files may total at most 200 MB.".into());
            continue;
        }
        total += file.size();
        draft.files.push(file);
    }
    let file_problem = (!file_problems.is_empty()).then(|| unique(file_problems).join(" "));
    let merged = unique(
        image_problem
            .into_iter()
            .chain(file_problem)
            .chain(prepared.errors)
            .collect(),
    )
    .join(" ");
    let all: Vec<String> = prepared
        .image_problem
        .into_iter()
        .chain((!merged.is_empty()).then_some(merged))
        .collect();
    (!all.is_empty()).then(|| all.join(" "))
}

/// Each reason once, in the order first given.
fn unique(problems: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    problems
        .into_iter()
        .filter(|problem| !problem.is_empty() && seen.insert(problem.clone()))
        .collect()
}

/// Why a chat cannot send `draft`: a file attached in a Code session that a chat does not take.
pub fn chat_problem(draft: &Draft) -> Option<String> {
    let files = || draft.files.iter().filter(|file| file.artifact.is_none());
    if files().any(|file| data_file_type(&file.name).is_none()) {
        return Some(
            "A session with no project takes images and data files (CSV, JSON, text). Pick a project to share other \
             files."
                .into(),
        );
    }
    files()
        .any(|file| file.size() > CHUNK_BYTES)
        .then(|| "A session with no project takes data files up to 4 MB. Pick a project to share larger ones.".into())
}

/// The largest artifact source a chat gets as a data file; a larger one goes as its reference only.
pub const ARTIFACT_SOURCE_BYTES: usize = CHUNK_BYTES;

/// A chat message's body (`POST /v1/chat/sessions/{id}/messages`, `SendChatMessageRequest`): the text, the images and
/// data files as `data:` URLs with their names (`toInput`, `toFileInput`), and the model (none for Auto). Referenced
/// artifacts go as a block at the end of the text, and each source read as a data file.
pub fn chat_body(content: &str, model: Option<&str>, draft: &Draft) -> Value {
    let mut body =
        json!({ "content": artifact_refs::with_block(content, &draft.artifacts(), artifact_refs::Reader::Chat) });
    if !draft.images.is_empty() {
        body["images"] = draft
            .images
            .iter()
            .map(|image| json!({ "data": image.data_url(image.media_type.unwrap_or("image/png")), "name": image.name }))
            .collect();
    }
    let files: Vec<Value> = draft
        .files
        .iter()
        .filter_map(|file| {
            if let Some(reference) = &file.artifact {
                let name = reference.source_file();
                let media_type = data_file_type(&name)?;
                return (!file.bytes.is_empty()).then(|| json!({ "data": file.data_url(media_type), "name": name }));
            }
            let media_type = data_file_type(&file.name)?;
            Some(json!({ "data": file.data_url(media_type), "name": file.name }))
        })
        .collect();
    if !files.is_empty() {
        body["files"] = Value::Array(files);
    }
    if let Some(model) = model {
        body["model"] = json!(model);
    }
    body
}

/// A Code session's images as its agent takes them: the bytes in base64 and their type (`toWb` in `composer.tsx`).
pub fn session_images(draft: &Draft) -> Vec<nexus_workbench::event::Image> {
    draft
        .images
        .iter()
        .map(|image| nexus_workbench::event::Image {
            media_type: image.media_type.unwrap_or("image/png").to_string(),
            data: STANDARD.encode(image.bytes.as_slice()),
            name: Some(image.name.clone()),
        })
        .collect()
}

/// A sent message's images returned to a draft. Stable source ids avoid duplicates after repeated restores.
pub fn restored_images(thread: &str, seq: i64, images: &[nexus_workbench::event::Image]) -> Draft {
    use std::hash::{Hash, Hasher};
    let images = images
        .iter()
        .enumerate()
        .filter_map(|(index, image)| {
            let bytes = STANDARD.decode(&image.data).ok()?;
            let media_type = match image.media_type.as_str() {
                "image/png" => "image/png",
                "image/jpeg" => "image/jpeg",
                "image/gif" => "image/gif",
                "image/webp" => "image/webp",
                _ => return None,
            };
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            ("restored", thread, seq, index).hash(&mut hash);
            Some(Attachment {
                artifact: None,
                id: hash.finish() | (1 << 63),
                name: image.name.clone().unwrap_or_else(|| "Image".into()),
                bytes: Arc::new(bytes),
                media_type: Some(media_type),
                thumbnail: None,
            })
        })
        .collect();
    Draft {
        images,
        ..Default::default()
    }
}

/// Keep current attachments first, then source images not already restored into this draft.
pub fn append_restored(draft: &mut Draft, restored: Draft) {
    for image in restored.images {
        if !draft.images.iter().any(|known| known.id == image.id) {
            draft.images.push(image);
        }
    }
}

/// An image drawn full size (`image_viewer.rs`): its bytes as GPUI draws them, and its size in pixels when it reads.
pub struct FullSize {
    pub image: Arc<gpui_kit::Image>,
    pub pixels: Option<(u32, u32)>,
}

/// `attachment` full size; `None` for a file that is not an image.
pub fn full_size(attachment: &Attachment) -> Option<FullSize> {
    let format = match attachment.media_type? {
        "image/jpeg" => gpui_kit::ImageFormat::Jpeg,
        "image/gif" => gpui_kit::ImageFormat::Gif,
        "image/webp" => gpui_kit::ImageFormat::Webp,
        _ => gpui_kit::ImageFormat::Png,
    };
    let pixels = ::image::ImageReader::new(Cursor::new(attachment.bytes.as_slice()))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok());
    Some(FullSize {
        image: Arc::new(gpui_kit::Image::from_bytes(format, attachment.bytes.to_vec())),
        pixels,
    })
}

/// What the viewer says of an image: `1200 × 800 · 1.5 MB`, or only its size when its pixels did not read
/// (`detailOf` in `images.tsx`).
pub fn image_detail(pixels: Option<(u32, u32)>, bytes: usize) -> String {
    match pixels {
        Some((width, height)) => format!("{width} × {height} · {}", size_label(bytes)),
        None => size_label(bytes),
    }
}

/// Where the viewer's save dialog opens: the person's Downloads folder, or their home folder without one.
pub fn downloads_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
}

/// The name the viewer's save dialog offers for `attachment`: its own name's last part, so a name with folders in it
/// cannot point the dialog elsewhere, or `Unknown` when it has none.
pub fn save_name(attachment: &Attachment) -> String {
    Path::new(&attachment.name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Unknown")
        .to_string()
}

/// A chip's percent sent, from a share of 0 to 1: rounded down, so 100 means every piece went (`images.tsx`).
pub fn percent(share: f32) -> u32 {
    ((share.clamp(0., 1.) * 100.).floor() as u32).min(100)
}

/// A Code session's files as a message takes them (`sendableFiles` in `attachment-upload.ts`), through
/// `nexus_workbench::attachment_upload`: a file of one piece or less whole, a larger one first uploaded in pieces
/// through `chunk` and then named by its upload id. The uploads are for session `thread_id` only (a new session's is
/// `nexus_workbench::creation_thread_id` of its creation request). The draft's [`Draft::upload`] counts the pieces for
/// the chips and stops at a cancel; a cancelled or failed upload deletes what it uploaded through `cancel`.
pub async fn sendable_files<C, CF, X, XF>(
    thread_id: &str,
    draft: &Draft,
    chunk: C,
    cancel: X,
) -> Result<Vec<FileInput>, String>
where
    C: FnMut(Chunk) -> CF,
    CF: Future<Output = Result<Progress, String>>,
    X: FnMut(String) -> XF,
    XF: Future<Output = Result<(), String>>,
{
    let files: Vec<Outgoing> = draft
        .files
        .iter()
        .map(|file| Outgoing {
            id: file.id,
            name: &file.name,
            bytes: file.bytes.as_slice(),
        })
        .collect();
    // A retry of a send that ended (a failed new session's Try again) counts afresh, unshown.
    let run = draft
        .upload
        .clone()
        .filter(|run| run.outcome().is_none())
        .unwrap_or_default();
    attachment_upload::send_files(thread_id, &files, &run, chunk, cancel).await
}

/// A path as a shell reads it: as it is when plain, else quoted (double quotes on Windows).
pub fn shell_path(path: &str, windows: bool) -> String {
    let plain = !path.is_empty()
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./\\-".contains(c));
    if plain {
        path.to_string()
    } else if windows {
        format!("\"{path}\"")
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// What a terminal types for dropped paths: each ready for the shell, then a space, as Terminal.app does.
pub fn paths_for_shell(paths: &[PathBuf], windows: bool) -> String {
    let quoted: Vec<String> = paths
        .iter()
        .map(|path| shell_path(&path.to_string_lossy(), windows))
        .collect();
    format!("{} ", quoted.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(name: &str, data: Vec<u8>) -> Incoming {
        Incoming {
            name: name.into(),
            media_type: image_type(name).map(str::to_string),
            size: data.len() as u64,
            source: Source::Bytes(Arc::new(data)),
        }
    }

    /// A file of `size` bytes, never read: the limits refuse it first.
    fn sized(name: &str, size: usize) -> Incoming {
        Incoming {
            name: name.into(),
            media_type: image_type(name).map(str::to_string),
            size: size as u64,
            source: Source::Unreadable("unread".into()),
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = ::image::RgbImage::from_fn(width, height, |x, y| {
            ::image::Rgb([(x % 256) as u8, (y % 256) as u8, 7])
        });
        let mut out = Vec::new();
        ::image::DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut out), ::image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn file(name: &str, size: usize) -> Attachment {
        Attachment {
            artifact: None,
            id: next_id(),
            name: name.into(),
            bytes: Arc::new(vec![b'a'; size]),
            media_type: None,
            thumbnail: None,
        }
    }

    #[test]
    fn restored_images_keep_content_source_identity_and_existing_attachments() {
        let source = vec![nexus_workbench::event::Image {
            media_type: "image/png".into(),
            data: STANDARD.encode(b"image bytes"),
            name: Some("shot.png".into()),
        }];
        let mut draft = Draft {
            images: vec![image_attachment("current.png".into(), "image/png", Arc::new(vec![1]))],
            files: vec![file("notes.txt", 4)],
            ..Default::default()
        };
        let current = draft.images[0].id;
        append_restored(&mut draft, restored_images("thread", 7, &source));
        append_restored(&mut draft, restored_images("thread", 7, &source));
        assert_eq!(draft.images.len(), 2);
        assert_eq!(draft.images[0].id, current);
        assert_eq!(draft.files.len(), 1);
        assert_eq!(session_images(&draft)[1], source[0]);
        append_restored(&mut draft, restored_images("other-thread", 7, &source));
        assert_eq!(draft.images.len(), 3);
        let bad = nexus_workbench::event::Image {
            data: "invalid!".into(),
            ..source[0].clone()
        };
        assert!(restored_images("thread", 8, &[bad]).is_empty());
    }

    fn reference(id: &str) -> ArtifactRef {
        ArtifactRef {
            title: "Rollout plan".into(),
            kind: "Doc".into(),
            target: crate::artifact_refs::Target::Page(id.into()),
            version: Some(2),
            link: None,
        }
    }

    #[test]
    fn a_chat_gets_artifacts_as_a_block_and_their_sources_as_files() {
        let mut draft = Draft::default();
        let added = vec![
            Attachment::artifact(reference("pag_1"), Some("# Plan".into())),
            Attachment::artifact(reference("pag_2"), None),
            Attachment::artifact(reference("pag_1"), None),
        ];
        assert_eq!(draft.add_artifacts(added), None);
        assert_eq!(draft.artifacts().len(), 2, "the same artifact is one chip");
        assert_eq!(chat_problem(&draft), None, "an artifact is not a file a chat refuses");
        let body = chat_body("Check it", None, &draft);
        let (text, refs) = crate::artifact_refs::split(body["content"].as_str().unwrap());
        assert_eq!((text.as_str(), refs.len()), ("Check it", 2));
        let files = body["files"].as_array().unwrap();
        assert_eq!(files.len(), 1, "only a source that was read goes as a file");
        assert_eq!(files[0]["name"], "Rollout plan (pag_1).md");
    }

    #[test]
    fn a_code_session_gets_a_source_it_read_as_a_file() {
        // The failure this fixes: a Code session got the block only, and its agent ran a CLI signed in to another
        // Gateway ("no page"). Now the source the app read goes as a file, and an unread one as its reference only.
        let mut draft = Draft::default();
        let _ = draft.add_artifacts(vec![
            Attachment::artifact(reference("pag_1"), Some("<!doctype html><title>Plan</title>".into())),
            Attachment::artifact(reference("pag_2"), None),
        ]);
        let text = draft.take_artifacts("Read this", "https://api.nexus.inc");
        let refs = crate::artifact_refs::split(&text).1;
        assert_eq!(refs, vec![reference("pag_1"), reference("pag_2")]);
        assert!(
            text.contains("NEXUS_GATEWAY=https://api.nexus.inc nexus pages get"),
            "{text}"
        );
        assert_eq!(draft.files.len(), 1, "only the source that was read goes as a file");
        let file = &draft.files[0];
        assert_eq!(file.name, "Rollout plan (pag_1).md");
        assert!(
            file.artifact.is_none(),
            "it leaves as a plain file, which the session saves on its computer"
        );
        assert_eq!(file.bytes.as_slice(), b"<!doctype html><title>Plan</title>");
    }

    #[test]
    fn a_code_sessions_sources_keep_the_file_limit() {
        let mut draft = Draft::default();
        draft.files = (0..MAX_FILES - 1).map(|n| file(&format!("{n}.txt"), 1)).collect();
        let _ = draft.add_artifacts(vec![
            Attachment::artifact(reference("pag_1"), Some("one".into())),
            Attachment::artifact(reference("pag_2"), Some("two".into())),
        ]);
        let text = draft.take_artifacts("Read", "https://g.example");
        assert_eq!(
            draft.files.len(),
            MAX_FILES,
            "a source past the limit goes as its reference only"
        );
        assert_eq!(crate::artifact_refs::split(&text).1.len(), 2);
    }

    #[test]
    fn artifacts_have_their_own_limit() {
        let mut draft = Draft::default();
        let many = (0..crate::artifact_refs::MAX + 1)
            .map(|n| Attachment::artifact(reference(&format!("pag_{n}")), None))
            .collect();
        assert!(draft.add_artifacts(many).is_some());
        assert_eq!(draft.artifacts().len(), crate::artifact_refs::MAX);
    }

    #[test]
    fn types_come_from_the_extension() {
        assert_eq!(image_type("/x/Shot 1.PNG"), Some("image/png"));
        assert_eq!(image_type("a.jpg"), Some("image/jpeg"));
        assert_eq!(image_type("a.svg"), None);
        assert_eq!(image_type("Makefile"), None);
        assert_eq!(data_file_type("data.CSV"), Some("text/csv"));
        assert_eq!(data_file_type("notes.markdown"), Some("text/markdown"));
        assert_eq!(data_file_type("report.pdf"), None);
    }

    #[test]
    fn sizes_read_as_the_nexus_app_shows_them() {
        assert_eq!(size_label(10), "1 KB");
        assert_eq!(size_label(1536), "2 KB");
        assert_eq!(size_label(3 * MB / 2), "1.5 MB");
    }

    #[test]
    fn a_chat_takes_images_and_small_data_files_only() {
        let (taken, after, _) = take(
            Target::Chat,
            vec![bytes("a.png", png(4, 4)), bytes("b.csv", b"x".to_vec())],
        );
        assert_eq!(taken.len(), 2);
        assert_eq!(after, None);

        let (taken, after, _) = take(Target::Chat, vec![bytes("a.csv", b"x".to_vec()), sized("b.pdf", 10)]);
        assert_eq!(taken.len(), 1);
        assert_eq!(
            after.as_deref(),
            Some(
                "A session with no project takes images and data files (CSV, JSON, text). Pick a project to share \
                 other files."
            )
        );

        let (taken, after, _) = take(Target::Chat, vec![sized("big.json", CHUNK_BYTES + 1)]);
        assert!(taken.is_empty());
        assert_eq!(
            after.as_deref(),
            Some("A session with no project takes data files up to 4 MB. Pick a project to share larger ones.")
        );

        // A Code session takes any file.
        let (taken, after, _) = take(
            Target::Code,
            vec![sized("b.pdf", 10), sized("big.json", CHUNK_BYTES + 1)],
        );
        assert_eq!((taken.len(), after), (2, None));
    }

    #[test]
    fn a_folder_is_refused_with_the_nexus_app_words() {
        let folder = std::env::temp_dir();
        let (taken, _, folders) = take(Target::Code, vec![from_path(&folder)]);
        assert!(taken.is_empty());
        assert!(folders[0].ends_with(" is a folder. Drop files, or drop the folder on a terminal for its path."));
    }

    #[test]
    fn files_keep_the_code_session_limits() {
        let (fit, errors) = fit_files((0..11).map(|i| sized(&format!("{i}.bin"), 1)).collect());
        assert_eq!(fit.len(), 10);
        assert_eq!(errors, ["A message takes at most 10 files."]);

        let (fit, errors) = fit_files(vec![
            sized("huge.bin", MAX_FILE_BYTES + 1),
            sized("a.bin", 90 * MB),
            sized("b.bin", 90 * MB),
            sized("c.bin", 90 * MB),
            sized("d.bin", 10 * MB),
        ]);
        assert_eq!(
            fit.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["a.bin", "b.bin", "d.bin"]
        );
        assert_eq!(
            errors,
            [
                "huge.bin exceeds the 100 MB file limit.",
                "Files may total at most 200 MB."
            ]
        );
    }

    #[test]
    fn images_are_prepared_and_limited_as_gateway_takes_them() {
        let prepared = prepare(vec![bytes("small.png", png(10, 6))]);
        assert_eq!(prepared.images.len(), 1);
        assert_eq!(prepared.images[0].media_type, Some("image/png"));
        assert!(prepared.images[0].thumbnail.is_some());

        // Larger than 2048 px on a side: drawn at 2048.
        let prepared = prepare(vec![bytes("wide.png", png(3000, 100))]);
        let sent = ::image::load_from_memory(&prepared.images[0].bytes).unwrap();
        assert_eq!((sent.width(), sent.height()), (2048, 68));

        // Not an image at all.
        let prepared = prepare(vec![bytes("broken.png", b"not a png".to_vec())]);
        assert!(prepared.images.is_empty());
        assert_eq!(
            prepared.image_problem.as_deref(),
            Some("not an image this browser can read")
        );

        // At most 8 in a message.
        let prepared = prepare((0..9).map(|i| bytes(&format!("{i}.png"), png(2, 2))).collect());
        assert_eq!(prepared.images.len(), 8);
        assert_eq!(
            prepared.image_problem.as_deref(),
            Some("A message takes at most 8 images.")
        );
    }

    #[test]
    fn merging_keeps_the_limits_across_attaches() {
        let mut draft = Draft::default();
        let mut prepared = Prepared::default();
        prepared.files = (0..9).map(|i| file(&format!("{i}.txt"), 1)).collect();
        assert_eq!(merge(&mut draft, prepared), None);
        let mut prepared = Prepared::default();
        prepared.files = vec![file("9.txt", 1), file("10.txt", 1)];
        prepared.errors = vec!["A message takes at most 10 files.".into()];
        assert_eq!(
            merge(&mut draft, prepared).as_deref(),
            Some("A message takes at most 10 files.")
        );
        assert_eq!(draft.files.len(), 10);
    }

    #[test]
    fn a_paste_takes_images_and_copied_files() {
        let image = ClipboardItem {
            entries: vec![ClipboardEntry::Image(gpui_kit::Image::from_bytes(
                gpui_kit::ImageFormat::Png,
                png(2, 2),
            ))],
        };
        let got = pasted(&image);
        assert!(got.take);
        assert_eq!(got.files[0].name, "Pasted image.png");
        assert_eq!(got.files[0].media_type.as_deref(), Some("image/png"));

        // Dolphin copies files as `file://` addresses.
        let dir = std::env::temp_dir().join(format!("nexus-paste-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("my notes.md");
        std::fs::write(&path, "hi").unwrap();
        let uri = url::Url::from_file_path(&path).unwrap();
        let got = pasted(&ClipboardItem::new_string(format!("{uri}\n")));
        assert!(got.take);
        assert_eq!(got.files[0].name, "my notes.md");
        assert_eq!(got.files[0].size, 2);
        std::fs::remove_dir_all(&dir).unwrap();

        // Plain text is left to the box.
        let got = pasted(&ClipboardItem::new_string("file:// is how a path starts".into()));
        assert!(!got.take && got.files.is_empty());
    }

    #[test]
    fn a_chat_body_carries_images_and_data_files_as_data_urls() {
        let mut draft = Draft::default();
        let prepared = prepare(vec![
            bytes("shot.png", png(2, 2)),
            bytes("rows.csv", b"a,b\n1,2\n".to_vec()),
        ]);
        assert_eq!(merge(&mut draft, prepared), None);
        let body = chat_body("What is in these?", Some("model-x"), &draft);
        assert_eq!(body["content"], "What is in these?");
        assert_eq!(body["model"], "model-x");
        assert_eq!(body["images"][0]["name"], "shot.png");
        assert!(
            body["images"][0]["data"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,iVBOR")
        );
        assert_eq!(
            body["files"],
            json!([{ "data": format!("data:text/csv;base64,{}", STANDARD.encode("a,b\n1,2\n")), "name": "rows.csv" }])
        );
        // Text alone: no attachment fields, as before.
        assert_eq!(chat_body("hi", None, &Draft::default()), json!({ "content": "hi" }));
    }

    #[test]
    fn a_chat_refuses_files_it_cannot_send() {
        let mut draft = Draft::default();
        draft.files.push(file("report.pdf", 10));
        assert!(chat_problem(&draft).unwrap().contains("images and data files"));
        draft.files = vec![file("rows.csv", CHUNK_BYTES + 1)];
        assert!(chat_problem(&draft).unwrap().contains("up to 4 MB"));
        draft.files = vec![file("rows.csv", 10)];
        assert_eq!(chat_problem(&draft), None);
    }

    /// The hand-off to a Code session: a small file goes whole, a large one in pieces through the same core the
    /// Nexus app uses, and the agent's prompt then names both on its computer.
    #[test]
    fn a_code_session_gets_its_files_through_nexus_workbench() {
        let root = std::env::temp_dir().join(format!("nexus-attach-{}-{}", std::process::id(), next_id()));
        std::fs::create_dir_all(&root).unwrap();
        let draft = Draft {
            files: vec![file("small.txt", 10), file("large.bin", CHUNK_BYTES * 2 + 5)],
            ..Draft::default()
        };
        let mut pieces = 0;
        let inputs = futures::executor::block_on(sendable_files(
            "t_test",
            &draft,
            |chunk| {
                pieces += 1;
                let result = nexus_workbench::attachments::write_chunk(&root, &chunk);
                async move { result }
            },
            |_| async { Ok(()) },
        ))
        .unwrap();
        assert_eq!(pieces, 3);
        assert!(inputs[0].data.is_some() && inputs[0].upload.is_none());
        assert!(inputs[1].data.is_none() && inputs[1].upload.is_some());
        let (prompt, folder) =
            nexus_workbench::attachments::prepare(&root, "t_test", "Look at these", &inputs).unwrap();
        assert!(prompt.starts_with("Look at these\n\nAttached files (on this session's computer):"));
        assert!(prompt.contains("\"small.txt\"") && prompt.contains("\"large.bin\""));
        let folder = folder.unwrap();
        assert_eq!(
            std::fs::metadata(folder.join("2-large.bin")).unwrap().len() as usize,
            CHUNK_BYTES * 2 + 5
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_failed_upload_cancels_what_it_started() {
        let draft = Draft {
            files: vec![file("large.bin", CHUNK_BYTES + 1)],
            ..Draft::default()
        };
        let mut cancelled = Vec::new();
        let result = futures::executor::block_on(sendable_files(
            "t_test",
            &draft,
            |_| async { Err::<Progress, _>("The disk is full.".to_string()) },
            |id| {
                cancelled.push(id);
                async { Ok(()) }
            },
        ));
        assert_eq!(result.unwrap_err(), "The disk is full.");
        assert_eq!(cancelled.len(), 1);
    }

    /// The draft's upload counts the pieces, so its chips show them; a finished one is not reused by a retry.
    #[test]
    fn the_drafts_upload_counts_its_pieces() {
        let run = Arc::new(UploadRun::default());
        let draft = Draft {
            files: vec![file("large.bin", CHUNK_BYTES * 2)],
            upload: Some(run.clone()),
            ..Draft::default()
        };
        let id = draft.files[0].id;
        let mut shares = Vec::new();
        futures::executor::block_on(sendable_files(
            "t_test",
            &draft,
            |_| {
                shares.push(run.share(id));
                async {
                    Ok(Progress {
                        received: 0,
                        done: false,
                    })
                }
            },
            |_| async { Ok(()) },
        ))
        .unwrap();
        assert_eq!(shares, [Some(0.), Some(0.5)]);
        assert!(draft.sends_in_pieces());
        // Try again after the send ended: it goes on a fresh count, and the ended one stays as it was.
        futures::executor::block_on(sendable_files(
            "t_test",
            &draft,
            |_| async {
                Ok(Progress {
                    received: 0,
                    done: false,
                })
            },
            |_| async { Ok(()) },
        ))
        .unwrap();
        assert_eq!(
            run.outcome(),
            Some(nexus_workbench::attachment_upload::Outcome::Uploaded)
        );
    }

    #[test]
    fn a_chips_percent_rounds_down() {
        assert_eq!(percent(0.), 0);
        assert_eq!(percent(0.999), 99);
        assert_eq!(percent(1.), 100);
        assert_eq!(percent(1.5), 100);
    }

    #[test]
    fn session_images_are_base64_without_a_data_url_head() {
        let mut draft = Draft::default();
        merge(&mut draft, prepare(vec![bytes("a.png", png(2, 2))]));
        let images = session_images(&draft);
        assert_eq!(images[0].media_type, "image/png");
        assert_eq!(images[0].name.as_deref(), Some("a.png"));
        assert!(images[0].data.starts_with("iVBOR"));
    }

    /// The viewer reads an image's own pixels, not its chip's thumbnail, and opens no file that is not an image.
    #[test]
    fn an_image_opens_full_size_with_its_pixels() {
        let mut draft = Draft::default();
        merge(&mut draft, prepare(vec![bytes("a.png", png(300, 200))]));
        let full = full_size(&draft.images[0]).expect("an image opens");
        assert_eq!(full.pixels, Some((300, 200)));
        assert!(full_size(&file("notes.txt", 10)).is_none());
        assert_eq!(image_detail(Some((300, 200)), 1536), "300 × 200 · 2 KB");
        assert_eq!(image_detail(None, 1536), "2 KB");
    }

    /// Download offers the image's own name, without the folders in it, or `Unknown` without one.
    #[test]
    fn download_offers_the_images_last_name_part() {
        let mut draft = Draft::default();
        merge(&mut draft, prepare(vec![bytes("shot.png", png(30, 20))]));
        let mut image = draft.images[0].clone();
        assert_eq!(save_name(&image), "shot.png");
        image.name = "../../escape.png".into();
        assert_eq!(save_name(&image), "escape.png");
        image.name = String::new();
        assert_eq!(save_name(&image), "Unknown");
    }

    #[test]
    fn dropped_paths_are_quoted_for_a_shell() {
        assert_eq!(shell_path("/Users/d/src/app.ts", false), "/Users/d/src/app.ts");
        assert_eq!(
            shell_path("/Users/d/My Files/a.png", false),
            "'/Users/d/My Files/a.png'"
        );
        assert_eq!(shell_path("/tmp/it's here", false), "'/tmp/it'\\''s here'");
        assert_eq!(shell_path("C:\\My Files\\a.png", true), "\"C:\\My Files\\a.png\"");
        assert_eq!(
            paths_for_shell(&[PathBuf::from("/a"), PathBuf::from("/b c")], false),
            "/a '/b c' "
        );
    }
}
