//! Which links in an agent's answer are screenshots and videos to show inline, and how their bytes are joined from
//! the pieces the session's machine sends (`wb_file_media`, [`crate::output_media::read`]). The Nexus app's rule is
//! the earlier desktop app's TypeScript (with `fileReference` from `output-references.ts`); this is the same
//! rule for the apps written in Rust, and its tests are the TypeScript tests, ported.
//!
//! A Markdown link or image to a local image or video shows inline. A code span stays as it is: a path in a sentence
//! is not a request to show the file. Web links and links to a line of a file stay links.

use std::future::Future;

use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::output_media::{MAX_IMAGE, MAX_VIDEO, MediaChunk};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    /// "image" or "video", for the messages.
    pub fn noun(self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
        }
    }

    /// The largest file of this kind an answer shows inline (the machine's caps).
    pub fn limit(self) -> u64 {
        match self {
            MediaKind::Image => MAX_IMAGE,
            MediaKind::Video => MAX_VIDEO,
        }
    }
}

/// Image or video by the path's extension (`mediaKind`); None for every other file.
pub fn media_kind(path: &str) -> Option<MediaKind> {
    let (_, extension) = path.rsplit_once('.')?;
    match extension.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => Some(MediaKind::Image),
        "mp4" | "webm" | "mov" => Some(MediaKind::Video),
        _ => None,
    }
}

/// A local file a link names, and the line it points at (from 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReference {
    pub path: String,
    pub line: Option<u32>,
}

/// The local file a link's target names (`fileReference` for a link or an image, not a code span): a relative or
/// absolute path, a `file:` URL on this computer, or a `sandbox:/` path, with an optional `#L12` or `:12` line. None
/// for web links, other schemes, anchors and network paths.
pub fn file_reference(raw: &str) -> Option<FileReference> {
    let mut path = raw.trim().to_string();
    if path.is_empty() || path.chars().any(is_control) || path.starts_with('#') || path.starts_with("//") {
        return None;
    }
    if path.starts_with("\\\\") {
        return None;
    }
    if path.get(..5).is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:")) {
        path = file_url_path(&path[5..])?;
    } else if let Some(rest) = path.strip_prefix("sandbox:/") {
        path = format!("/{rest}");
    } else if has_scheme(&path) && !is_drive(&path) && !has_line_after_extension(&path) {
        return None;
    }
    let (rest, line) = split_line(&path);
    path = rest;
    let path = percent_decode(&path)?.replace('\\', "/");
    if path.is_empty() || path.starts_with("//") || path.chars().any(|c| is_control(c) || c == '?') {
        return None;
    }
    Some(FileReference { path, line })
}

/// What kind of Markdown reference a target came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reference {
    Link,
    Image,
    Code,
}

/// A local image or video that a link or image names, without a line (`mediaReference`). A code span stays a file
/// chip, so it is never media.
pub fn media_reference(target: &str, reference: Reference) -> Option<(String, MediaKind)> {
    if reference == Reference::Code {
        return None;
    }
    let file = file_reference(target).filter(|file| file.line.is_none())?;
    let kind = media_kind(&file.path)?;
    Some((file.path, kind))
}

/// One image or video an answer links to or shows, in the order it appears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaLink {
    /// The link's text or the image's alt text; may be empty.
    pub label: String,
    pub path: String,
    pub kind: MediaKind,
}

/// The images and videos an answer's Markdown links to or shows, each path once, in order. Fenced code blocks and code
/// spans are skipped, as the Nexus app's Markdown renderer leaves them as code.
pub fn media_links(markdown: &str) -> Vec<MediaLink> {
    let mut found: Vec<MediaLink> = Vec::new();
    for (label, target, reference) in references(markdown) {
        if let Some((path, kind)) = media_reference(&target, reference)
            && !found.iter().any(|seen| seen.path == path)
        {
            found.push(MediaLink { label, path, kind });
        }
    }
    found
}

/// The local files an answer's Markdown links to or shows, in order. Links in fenced code blocks and code spans are
/// skipped: an example of a link (such as one an agent quotes from its instructions) names no file of the session.
pub fn file_links(markdown: &str) -> Vec<FileReference> {
    references(markdown)
        .into_iter()
        .filter_map(|(_, target, _)| file_reference(&target))
        .collect()
}

/// A loaded file: its bytes, or why it does not show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    Ready {
        bytes: Vec<u8>,
        media_type: String,
        size: u64,
    },
    /// Over the cap of its kind: [`too_large`] says so.
    TooLarge { size: u64 },
}

const CHANGED: &str = "The file changed while it loaded. Try again when the agent is done with it.";

/// The whole file `path`, read piece by piece through `read` (the machine's answer from an offset) until it has
/// `size` bytes (`readMedia`). A size that changes between pieces means the agent is still writing the file: that is
/// an error, and "Try again" reads it from the start. An image over its cap stays a card even if a machine sends it.
pub async fn read_all<F, Fut>(path: &str, mut read: F) -> Result<Loaded, String>
where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = Result<MediaChunk, String>>,
{
    let kind = media_kind(path).unwrap_or(MediaKind::Image);
    let mut bytes = Vec::new();
    let mut size = None;
    let mut media_type: String;
    loop {
        let offset = bytes.len() as u64;
        let chunk = read(offset).await?;
        if chunk.status == "tooLarge" || chunk.size > kind.limit() {
            return Ok(Loaded::TooLarge { size: chunk.size });
        }
        if size.is_some_and(|size| size != chunk.size) {
            return Err(CHANGED.into());
        }
        let data = chunk.data.as_deref().filter(|_| chunk.offset == offset);
        let piece = data
            .and_then(|data| STANDARD.decode(data).ok())
            .ok_or("The file could not be read in full. Try again.")?;
        size = Some(chunk.size);
        media_type = chunk.media_type;
        bytes.extend_from_slice(&piece);
        if bytes.len() as u64 >= chunk.size {
            break;
        }
        if piece.is_empty() {
            return Err("The file ended early. Try again.".into());
        }
    }
    let size = size.unwrap_or_default();
    if bytes.len() as u64 != size {
        return Err(CHANGED.into());
    }
    Ok(Loaded::Ready {
        bytes,
        media_type,
        size,
    })
}

/// A file's size as the card says it (`mediaSize`): "40 KB", "2.3 MB".
pub fn size_label(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", ((bytes as f64 / 1024.).round() as u64).max(1))
    } else {
        format!("{:.1} MB", bytes as f64 / 1024. / 1024.)
    }
}

/// What a card says for a file over its cap.
pub fn too_large(kind: MediaKind, size: u64) -> String {
    let noun = kind.noun();
    format!(
        "This {noun} is {}. The reply shows {noun}s up to {} MB.",
        size_label(size),
        kind.limit() / (1024 * 1024)
    )
}

fn is_control(c: char) -> bool {
    c <= '\u{1f}' || c == '\u{7f}'
}

/// `^[a-z][\w+.-]*:`
fn has_scheme(path: &str) -> bool {
    let mut chars = path.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    for c in chars {
        if c == ':' {
            return true;
        }
        if !(c.is_alphanumeric() || matches!(c, '_' | '+' | '.' | '-')) {
            return false;
        }
    }
    false
}

/// `^[a-z]:[\\/]`: a Windows drive.
fn is_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\')
}

/// `^.*\.[\w-]+:\d`: a file name with a line, as `main.rs:12`.
fn has_line_after_extension(path: &str) -> bool {
    path.char_indices().any(|(at, c)| {
        if c != ':' || !path[at + 1..].starts_with(|c: char| c.is_ascii_digit()) {
            return false;
        }
        let before = &path[..at];
        let stem = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '-');
        stem.len() < before.len() && stem.ends_with('.')
    })
}

/// A `file:` URL's path (after `file:`), with its `#` part, still percent-encoded: None for another computer's file.
fn file_url_path(rest: &str) -> Option<String> {
    let (rest, hash) = match rest.find('#') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let rest = rest.split('?').next().unwrap_or_default();
    let path = match rest.strip_prefix("//") {
        Some(authority) => {
            let (host, path) = authority
                .find('/')
                .map_or((authority, "/"), |at| authority.split_at(at));
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                return None;
            }
            path
        }
        None => rest,
    };
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    // `/C:/…` on Windows is the drive's path.
    let bytes = path.as_bytes();
    let path = if bytes.len() >= 4 && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' && bytes[3] == b'/' {
        path[1..].to_string()
    } else {
        path
    };
    Some(format!("{path}{hash}"))
}

/// `(?:#L?(\d+)(?:[-:]L?\d+)?|:(\d+)(?::\d+|[-]\d+)?)$`: the path without its line, and the line (0 is none). Without a
/// line, everything from `#` goes.
fn split_line(path: &str) -> (String, Option<u32>) {
    let digits_end = |s: &str| s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    // The leftmost match wins, as in a regular expression: first with a second number (`-12`, `:12`, `-L12`,
    // `:L12`) after the line, then without.
    let mut candidates = Vec::new();
    let tail = digits_end(path);
    if tail > 0 {
        let head = &path[..path.len() - tail];
        let head = head.strip_suffix('L').unwrap_or(head);
        if head.ends_with('-') || head.ends_with(':') {
            candidates.push(head.len() - 1);
        }
    }
    candidates.push(path.len());
    for end in candidates {
        let before = &path[..end];
        let n = digits_end(before);
        if n == 0 {
            continue;
        }
        let number = &before[before.len() - n..];
        let head = &before[..before.len() - n];
        let suffix = &path[end..];
        let line = number.parse::<u32>().ok().filter(|line| *line > 0);
        // `#12` or `#L12`, then any `-12`, `:12`, `-L12` or `:L12`.
        let hash = head.strip_suffix("#L").or_else(|| head.strip_suffix('#'));
        if let Some(rest) = hash {
            return (rest.to_string(), line);
        }
        // `:12`, then any `:12` or `-12` (no `L`).
        if let Some(rest) = head.strip_suffix(':')
            && !suffix.contains('L')
        {
            return (rest.to_string(), line);
        }
    }
    (path.split('#').next().unwrap_or_default().to_string(), None)
}

/// `decodeURIComponent`: None for a malformed escape or bytes that are not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = text.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Every link and image in `markdown` outside code, as (label, target, kind), in order. A link's label is read for
/// images too, so `[![shot](a.png)](a.png)` names the image.
fn references(markdown: &str) -> Vec<(String, String, Reference)> {
    let mut found = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut prose = String::new();
    for line in markdown.lines() {
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'));
        let run = marker.map_or(0, |marker| trimmed.chars().take_while(|c| *c == marker).count());
        match (fence, marker) {
            (None, Some(marker)) if indent < 4 && run >= 3 => {
                inline(&prose, &mut found);
                prose.clear();
                fence = Some((marker, run));
            }
            (Some((open, len)), Some(marker)) if marker == open && run >= len && trimmed[run..].trim().is_empty() => {
                fence = None;
            }
            (Some(_), _) => {}
            (None, _) => {
                prose.push_str(line);
                prose.push('\n');
            }
        }
    }
    if fence.is_none() {
        inline(&prose, &mut found);
    }
    found
}

/// The links and images in prose, skipping code spans.
fn inline(text: &str, found: &mut Vec<(String, String, Reference)>) {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        match chars[at] {
            '\\' => at += 2,
            '`' => at = code_span_end(&chars, at),
            '[' => match link_at(&chars, at) {
                Some((label, target, end)) => {
                    let image = at > 0 && chars[at - 1] == '!' && (at < 2 || chars[at - 2] != '\\');
                    let kind = if image { Reference::Image } else { Reference::Link };
                    let label_text: String = label.iter().collect();
                    // An image inside a link's label comes first, as it reads.
                    inline(&label_text, found);
                    found.push((label_text, target, kind));
                    at = end;
                }
                None => at += 1,
            },
            _ => at += 1,
        }
    }
}

/// Where a code span that opens at `at` ends; an unclosed run of backticks is plain text.
fn code_span_end(chars: &[char], at: usize) -> usize {
    let run = chars[at..].iter().take_while(|c| **c == '`').count();
    let mut scan = at + run;
    while scan < chars.len() {
        if chars[scan] == '`' {
            let close = chars[scan..].iter().take_while(|c| **c == '`').count();
            if close == run {
                return scan + close;
            }
            scan += close;
        } else {
            scan += 1;
        }
    }
    at + run
}

/// The link at `[`: its label, its target, and where it ends. `[label](target "title")`, with `<target>` allowed and
/// balanced parentheses in a bare target.
fn link_at(chars: &[char], at: usize) -> Option<(Vec<char>, String, usize)> {
    let mut depth = 0;
    let mut close = None;
    let mut scan = at;
    while scan < chars.len() {
        match chars[scan] {
            '\\' => scan += 1,
            '`' => {
                scan = code_span_end(chars, scan);
                continue;
            }
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(scan);
                    break;
                }
            }
            '\n' if chars.get(scan + 1) == Some(&'\n') => return None,
            _ => {}
        }
        scan += 1;
    }
    let close = close?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let mut scan = close + 2;
    while chars.get(scan).is_some_and(|c| *c == ' ' || *c == '\t') {
        scan += 1;
    }
    let mut target = String::new();
    if chars.get(scan) == Some(&'<') {
        scan += 1;
        while let Some(&c) = chars.get(scan) {
            if c == '>' {
                break;
            }
            if c == '\n' {
                return None;
            }
            target.push(c);
            scan += 1;
        }
        scan += 1;
    } else {
        let mut parens = 0;
        while let Some(&c) = chars.get(scan) {
            if c.is_whitespace() || (c == ')' && parens == 0) {
                break;
            }
            match c {
                '(' => parens += 1,
                ')' => parens -= 1,
                _ => {}
            }
            target.push(c);
            scan += 1;
        }
    }
    // An optional title, then `)`.
    while let Some(&c) = chars.get(scan) {
        if c == ')' {
            return Some((chars[at + 1..close].to_vec(), target, scan + 1));
        }
        if c == '\n' && chars.get(scan + 1) == Some(&'\n') {
            return None;
        }
        scan += 1;
    }
    None
}

#[cfg(test)]
mod tests;
