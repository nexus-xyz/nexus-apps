//! General attachments are copied onto the session's computer, so both local and remote agents can read them. Each
//! session's files live in their own folder (`<root>/<session>/<batch>/`), which goes when the session is deleted.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

// The composer checks the same limits first (FILE_LIMITS, src/workbench/draft-files.ts); change both together.
pub const MAX_FILES: usize = 10;
pub const MAX_FILE_BYTES: usize = 100 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 200 * 1024 * 1024;
/// The size of a piece the composer sends (CHUNK_BYTES in draft-files.ts). A file this small or smaller travels whole.
pub const CHUNK_BYTES: usize = 4 * 1024 * 1024;
/// The most one call takes: twice a piece, so no single message on a host connection is large.
const MAX_CHUNK_BYTES: usize = 2 * CHUNK_BYTES;
/// How long an unfinished or unused upload stays before the next upload deletes it.
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// Uploads wait here until a message takes them. The leading dot keeps it from being a session's folder name.
const UPLOADS: &str = ".uploads";

/// A file of a message: its bytes in `data` (base64, a piece-sized file) or already uploaded in pieces (`upload`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInput {
    pub name: String,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub upload: Option<String>,
}

/// One piece of an upload, as `wb_attachment_chunk` takes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub upload_id: String,
    /// The whole file's size in bytes.
    pub size: u64,
    /// Where this piece starts. 0 starts the upload afresh.
    pub offset: u64,
    /// The piece, base64.
    pub data: String,
    /// The whole file's SHA-256 (hex), checked when the last piece arrives.
    #[serde(default)]
    pub sha256: Option<String>,
    /// The session the file is for: only a message to that session takes it. A new session's is
    /// [`crate::creation_thread_id`] of its creation request. An older app names none: its upload is stored (until the one-day sweep) but `prepare` refuses to attach it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// How far an upload is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub received: u64,
    pub done: bool,
}

fn plain(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn uploads(root: &Path) -> PathBuf {
    root.join(UPLOADS)
}

/// An upload's temporary and finished files. One for a session starts with the session's id, so a message to another
/// session cannot name it. Neither id holds a dot, so the two never run together.
fn upload_paths(root: &Path, thread_id: Option<&str>, id: &str) -> Result<(PathBuf, PathBuf)> {
    if !plain(id) {
        return Err("Invalid upload id.".into());
    }
    let name = match thread_id {
        Some(thread) if plain(thread) => format!("{thread}.{id}"),
        Some(_) => return Err("Invalid session id.".into()),
        None => id.to_string(),
    };
    let dir = uploads(root);
    Ok((dir.join(format!("{name}.part")), dir.join(format!("{name}.done"))))
}

#[cfg(unix)]
fn private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Deletes uploads older than `age` (the last write counts). Nothing there is fine.
pub fn clean_stale(root: &Path, age: Duration) {
    let Ok(entries) = std::fs::read_dir(uploads(root)) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).unwrap_or_default() >= age);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Adds one piece to an upload. Pieces go to a temporary file; it becomes the finished upload (the one a message can
/// name) only once all `size` bytes are there and match the SHA-256, if given. A wrong upload is deleted.
pub fn write_chunk(root: &Path, chunk: &Chunk) -> Result<Progress> {
    let (part, done) = upload_paths(root, chunk.thread_id.as_deref(), &chunk.upload_id)?;
    if chunk.size > MAX_FILE_BYTES as u64 {
        return Err("A file may be at most 100 MB.".into());
    }
    if chunk.data.len() > MAX_CHUNK_BYTES.div_ceil(3) * 4 {
        return Err("A piece of a file is too large.".into());
    }
    let bytes = STANDARD
        .decode(&chunk.data)
        .map_err(|_| "Could not decode a piece of a file.".to_string())?;
    if bytes.len() > MAX_CHUNK_BYTES || chunk.offset.saturating_add(bytes.len() as u64) > chunk.size {
        let _ = std::fs::remove_file(&part);
        return Err("A piece of a file does not fit its size.".into());
    }
    let dir = uploads(root);
    let mut options = std::fs::OpenOptions::new();
    if chunk.offset == 0 {
        // The start of an upload, or of one begun again: nothing of an earlier try stays. Old leftovers go too.
        clean_stale(root, STALE_AFTER);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Could not save attachments: {e}"))?;
        #[cfg(unix)]
        for each in [root, dir.as_path()] {
            private(each)?;
        }
        let _ = std::fs::remove_file(&done);
        options.write(true).create(true).truncate(true);
    } else {
        options.append(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&part)
        .map_err(|_| "The upload is not there. Send the file again.".to_string())?;
    let held = file.metadata().map_err(|e| e.to_string())?.len();
    if held != chunk.offset {
        drop(file);
        let _ = std::fs::remove_file(&part);
        return Err("The pieces of a file arrived out of order. Send the file again.".into());
    }
    let written = file.write_all(&bytes).map_err(|e| e.to_string());
    if let Err(e) = written {
        drop(file);
        let _ = std::fs::remove_file(&part);
        return Err(format!("Could not save attachments: {e}"));
    }
    let received = held + bytes.len() as u64;
    if received < chunk.size {
        return Ok(Progress { received, done: false });
    }
    let checked = file
        .sync_all()
        .map_err(|e| e.to_string())
        .and_then(|()| match &chunk.sha256 {
            Some(want) if !sha256_of(&part)?.eq_ignore_ascii_case(want) => {
                Err("A file did not arrive intact. Send it again.".to_string())
            }
            _ => Ok(()),
        });
    drop(file);
    match checked.and_then(|()| std::fs::rename(&part, &done).map_err(|e| e.to_string())) {
        Ok(()) => Ok(Progress { received, done: true }),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Deletes an upload for session `thread_id` (None: one from an older app), finished or not (nothing there is fine).
pub fn cancel(root: &Path, thread_id: Option<&str>, upload_id: &str) -> Result<()> {
    let (part, done) = upload_paths(root, thread_id, upload_id)?;
    for path in [part, done] {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("Could not delete an upload: {e}"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// A session's attachments folder under `root`, or None for an id that isn't a plain name (never a path).
pub fn session_dir(root: &Path, thread_id: &str) -> Option<PathBuf> {
    let plain = !thread_id.is_empty()
        && thread_id.len() <= 80
        && thread_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    plain.then(|| root.join(thread_id))
}

/// Deletes everything attached to the session (nothing there is fine).
pub fn remove(root: &Path, thread_id: &str) -> Result<()> {
    let Some(dir) = session_dir(root, thread_id) else {
        return Ok(());
    };
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("Could not delete attachments: {e}")),
        _ => Ok(()),
    }
}

/// Keep files outside the repository, in the session's folder under `root`, with private permissions and unique names.
/// Never execute an attachment. Answers the message's text with the files' paths added, and the folder made for them
/// (None without files).
pub fn prepare(root: &Path, thread_id: &str, text: &str, files: &[FileInput]) -> Result<(String, Option<PathBuf>)> {
    if files.is_empty() {
        return Ok((text.into(), None));
    }
    let session = session_dir(root, thread_id).ok_or_else(|| "Invalid session id.".to_string())?;
    if files.len() > MAX_FILES {
        return Err("A message takes at most 10 files.".into());
    }
    let mut total = 0;
    let mut sources = Vec::new();
    for file in files {
        // Validate the entire batch before creating anything. Reject paths, controls and oversized wire data.
        if file.name.is_empty() {
            return Err("An attachment has no file name.".into());
        }
        if file.name.len() > 200 {
            return Err("An attachment's file name is longer than 200 bytes.".into());
        }
        if file.name == "."
            || file.name == ".."
            || file.name.contains(['/', '\\'])
            || file.name.chars().any(char::is_control)
        {
            return Err("An attachment has an invalid file name.".into());
        }
        let source = match (&file.data, &file.upload) {
            (Some(data), None) => {
                // A file sent whole is at most a piece; a larger one goes in pieces. The padded base64 length of a
                // piece: longer data decodes to more, so it is refused before decoding.
                if data.len() > CHUNK_BYTES.div_ceil(3) * 4 {
                    return Err(format!("{} is too large to send in one piece.", file.name));
                }
                let bytes = STANDARD
                    .decode(data)
                    .map_err(|_| format!("Could not decode {}.", file.name))?;
                if bytes.len() > CHUNK_BYTES {
                    return Err(format!("{} is too large to send in one piece.", file.name));
                }
                total += bytes.len();
                Source::Bytes(bytes)
            }
            (None, Some(id)) => {
                // Only an upload made for this session: another session's, or one that names none, is not taken.
                let (_, done) = upload_paths(root, Some(thread_id), id)?;
                let size = std::fs::metadata(&done)
                    .map_err(|_| {
                        let unbound = upload_paths(root, None, id).is_ok_and(|(_, unbound)| unbound.exists());
                        if unbound {
                            format!(
                                "{} was uploaded by an older Nexus app. Update Nexus, then attach it again.",
                                file.name
                            )
                        } else {
                            format!("{} did not finish uploading. Attach it again.", file.name)
                        }
                    })?
                    .len() as usize;
                if size > MAX_FILE_BYTES {
                    return Err(format!("{} exceeds the 100 MB file limit.", file.name));
                }
                total += size;
                Source::Staged(done)
            }
            _ => return Err(format!("{} has no content.", file.name)),
        };
        if total > MAX_TOTAL_BYTES {
            return Err("Files may total at most 200 MB, with at most 100 MB per file.".into());
        }
        sources.push(source);
    }
    let folder = session.join(uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(&folder).map_err(|e| format!("Could not save attachments: {e}"))?;
    #[cfg(unix)]
    for dir in [root, session.as_path(), folder.as_path()] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    let result = (|| {
        let mut prompt = text.to_string();
        prompt.push_str("\n\nAttached files (on this session's computer):");
        for (index, (file, source)) in files.iter().zip(sources).enumerate() {
            let path = folder.join(format!("{}-{}", index + 1, file.name));
            match source {
                Source::Bytes(bytes) => {
                    let mut options = std::fs::OpenOptions::new();
                    options.write(true).create_new(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(0o600);
                    }
                    let mut output = options.open(&path).map_err(|e| e.to_string())?;
                    output.write_all(&bytes).map_err(|e| e.to_string())?;
                    output.sync_all().map_err(|e| e.to_string())?;
                }
                // The finished upload moves into place: it is complete, so the agent never sees a partial file.
                Source::Staged(done) => {
                    std::fs::rename(&done, &path).map_err(|e| format!("Could not save attachments: {e}"))?
                }
            }
            // JSON quoting keeps spaces, quotes and other filename characters unambiguous to the agent.
            prompt.push_str(&format!(
                "\n- {}: {}",
                serde_json::to_string(&file.name).map_err(|e| e.to_string())?,
                serde_json::to_string(&path.to_string_lossy()).map_err(|e| e.to_string())?
            ));
        }
        Ok(prompt)
    })();
    match result {
        Ok(prompt) => Ok((prompt, Some(folder))),
        Err(e) => {
            let _ = std::fs::remove_dir_all(folder);
            Err(e)
        }
    }
}

enum Source {
    Bytes(Vec<u8>),
    Staged(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("nexus-attachments-{}", uuid::Uuid::new_v4()))
    }
    fn inline(name: &str, data: String) -> FileInput {
        FileInput {
            name: name.into(),
            data: Some(data),
            upload: None,
        }
    }
    fn staged(name: &str, id: &str) -> FileInput {
        FileInput {
            name: name.into(),
            data: None,
            upload: Some(id.into()),
        }
    }
    fn piece(id: &str, size: usize, whole: &[u8], offset: usize, len: usize, sha: Option<String>) -> Chunk {
        Chunk {
            upload_id: id.into(),
            size: size as u64,
            offset: offset as u64,
            data: STANDARD.encode(&whole[offset..offset + len]),
            sha256: sha,
            thread_id: Some("t_1".into()),
        }
    }
    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }
    fn leftovers(root: &Path) -> Vec<String> {
        std::fs::read_dir(uploads(root))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn preserves_binary_bytes_and_duplicate_names_outside_the_project() {
        let root = temp_root();
        let file = inline("report.pdf", STANDARD.encode([0, 255, 42]));
        let (prompt, folder) =
            prepare(&root, "t_1", "Read these", &[file.clone(), file]).unwrap_or_else(|e| panic!("{e}"));
        assert!(prompt.starts_with("Read these\n\nAttached files"));
        let folder = folder.unwrap_or_default();
        assert_eq!(folder.parent(), Some(root.join("t_1").as_path()));
        for name in ["1-report.pdf", "2-report.pdf"] {
            assert_eq!(std::fs::read(folder.join(name)).unwrap_or_default(), [0, 255, 42]);
        }
        // Deleting the session takes its files; another session's stay, and a path-like id touches nothing.
        let other = inline("a.txt", "aGk=".into());
        prepare(&root, "t_2", "", std::slice::from_ref(&other)).unwrap_or_else(|e| panic!("{e}"));
        assert!(prepare(&root, "../t_2", "", &[other]).is_err());
        assert_eq!(remove(&root, "t_1"), Ok(()));
        assert!(!root.join("t_1").exists() && root.join("t_2").exists());
        assert_eq!(remove(&root, "t_1"), Ok(()));
        assert_eq!(remove(&root, ".."), Ok(()));
        assert!(root.join("t_2").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn names_a_missing_or_long_file_name_and_caps_whole_files_at_a_piece() {
        let root = temp_root();
        let one = |name: &str, data: String| prepare(&root, "t_1", "", &[inline(name, data)]).map(|(prompt, _)| prompt);
        assert_eq!(one("", "AA==".into()), Err("An attachment has no file name.".into()));
        assert!(
            one(&"a".repeat(201), "AA==".into())
                .unwrap_or_else(|e| e)
                .contains("200 bytes")
        );
        // Past a piece's base64 length, data is refused before decoding; just past a piece, after it.
        assert!(one("big.bin", STANDARD.encode(vec![0u8; CHUNK_BYTES * 2])).is_err());
        assert_eq!(
            one("big.bin", STANDARD.encode(vec![0u8; CHUNK_BYTES + 3])),
            Err("big.bin is too large to send in one piece.".into())
        );
        assert!(one("max.bin", STANDARD.encode(vec![0u8; CHUNK_BYTES])).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_paths_invalid_encoding_and_limits_before_writing() {
        let root = temp_root();
        for name in ["../secret", "dir/file", "dir\\file", "..", "bad\nname"] {
            assert!(prepare(&root, "t_1", "", &[inline(name, "AA==".into())]).is_err());
        }
        assert!(prepare(&root, "t_1", "", &[inline("test.bin", "invalid".into())]).is_err());
        assert!(
            prepare(
                &root,
                "t_1",
                "",
                &vec![inline("test.bin", "AA==".into()); MAX_FILES + 1]
            )
            .is_err()
        );
        // A file with neither its data nor an upload is refused, and so is one with both.
        let both = FileInput {
            name: "x".into(),
            data: Some("AA==".into()),
            upload: Some("u1".into()),
        };
        assert!(prepare(&root, "t_1", "", &[both]).is_err());
        assert!(!root.exists());
    }

    #[test]
    fn limits_are_100_mb_a_file_200_mb_a_message_and_10_files() {
        assert_eq!((MAX_FILES, MAX_FILE_BYTES, MAX_TOTAL_BYTES), (10, 100 << 20, 200 << 20));
        let root = temp_root();
        // A declared size over the file limit is refused with its first piece, and nothing is kept.
        let over = Chunk {
            upload_id: "big".into(),
            size: MAX_FILE_BYTES as u64 + 1,
            offset: 0,
            data: "AA==".into(),
            sha256: None,
            thread_id: Some("t_1".into()),
        };
        assert_eq!(write_chunk(&root, &over), Err("A file may be at most 100 MB.".into()));
        assert!(leftovers(&root).is_empty());
        // A piece past the two-piece cap is refused before decoding.
        let wide = Chunk {
            upload_id: "wide".into(),
            size: MAX_FILE_BYTES as u64,
            offset: 0,
            data: STANDARD.encode(vec![0u8; MAX_CHUNK_BYTES + 3]),
            sha256: None,
            thread_id: Some("t_1".into()),
        };
        assert!(write_chunk(&root, &wide).is_err());
        // Three finished 100 MB uploads would total 300 MB: a message that names them is refused, sparse files stand in.
        for id in ["a", "b", "c"] {
            std::fs::create_dir_all(uploads(&root)).unwrap_or_else(|e| panic!("{e}"));
            let file =
                std::fs::File::create(uploads(&root).join(format!("t_1.{id}.done"))).unwrap_or_else(|e| panic!("{e}"));
            file.set_len(MAX_FILE_BYTES as u64).unwrap_or_else(|e| panic!("{e}"));
        }
        assert_eq!(
            prepare(
                &root,
                "t_1",
                "",
                &[staged("a", "a"), staged("b", "b"), staged("c", "c")]
            )
            .map(|_| ()),
            Err("Files may total at most 200 MB, with at most 100 MB per file.".into())
        );
        assert!(prepare(&root, "t_1", "", &[staged("a", "a"), staged("b", "b")]).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn writes_a_file_piece_by_piece_and_moves_it_into_the_session_folder_only_when_whole() {
        let root = temp_root();
        let whole: Vec<u8> = (0..10_000u32).map(|n| (n % 251) as u8).collect();
        let sha = Some(hex(&whole));
        let first = write_chunk(&root, &piece("up1", whole.len(), &whole, 0, 4_000, sha.clone()))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            first,
            Progress {
                received: 4_000,
                done: false
            }
        );
        // Half there: only a temporary file exists, and a message cannot name it.
        assert_eq!(leftovers(&root), ["t_1.up1.part"]);
        assert!(prepare(&root, "t_1", "", &[staged("v.bin", "up1")]).is_err());
        assert!(!root.join("t_1").exists());
        let second = write_chunk(&root, &piece("up1", whole.len(), &whole, 4_000, 4_000, sha.clone()))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(second.received, 8_000);
        let last =
            write_chunk(&root, &piece("up1", whole.len(), &whole, 8_000, 2_000, sha)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            last,
            Progress {
                received: 10_000,
                done: true
            }
        );
        assert_eq!(leftovers(&root), ["t_1.up1.done"]);
        let (prompt, folder) =
            prepare(&root, "t_1", "Watch", &[staged("v.bin", "up1")]).unwrap_or_else(|e| panic!("{e}"));
        let folder = folder.unwrap_or_default();
        assert_eq!(std::fs::read(folder.join("1-v.bin")).unwrap_or_default(), whole);
        assert!(prompt.contains("1-v.bin"));
        // The upload left its waiting place, and the file is private.
        assert!(leftovers(&root).is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(folder.join("1-v.bin")).map(|m| m.permissions().mode() & 0o777);
            assert_eq!(mode.unwrap_or_default(), 0o600);
        }
        // Deleting the session deletes the moved file.
        assert_eq!(remove(&root, "t_1"), Ok(()));
        assert!(!root.join("t_1").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_a_file_whose_hash_is_wrong_and_keeps_nothing() {
        let root = temp_root();
        let whole = vec![7u8; 3_000];
        let wrong = Some(hex(b"something else"));
        write_chunk(&root, &piece("bad", 3_000, &whole, 0, 1_000, wrong.clone())).unwrap_or_else(|e| panic!("{e}"));
        let result = write_chunk(&root, &piece("bad", 3_000, &whole, 1_000, 2_000, wrong));
        assert_eq!(result, Err("A file did not arrive intact. Send it again.".into()));
        assert!(leftovers(&root).is_empty());
        assert!(prepare(&root, "t_1", "", &[staged("v.bin", "bad")]).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_pieces_that_do_not_match_the_size_or_the_order() {
        let root = temp_root();
        let whole = vec![1u8; 3_000];
        // Pieces that run past the declared size.
        assert!(write_chunk(&root, &piece("s", 2_000, &whole, 0, 3_000, None)).is_err());
        // A piece out of order deletes the upload, so it cannot finish short or twisted.
        write_chunk(&root, &piece("o", 3_000, &whole, 0, 1_000, None)).unwrap_or_else(|e| panic!("{e}"));
        assert!(write_chunk(&root, &piece("o", 3_000, &whole, 2_000, 1_000, None)).is_err());
        assert!(leftovers(&root).is_empty());
        // A piece for an upload that never began.
        assert!(write_chunk(&root, &piece("none", 3_000, &whole, 1_000, 1_000, None)).is_err());
        // An id that is a path touches nothing.
        assert!(write_chunk(&root, &piece("../x", 3_000, &whole, 0, 3_000, None)).is_err());
        // Whole at the declared size and no hash: accepted. Beginning again starts clean.
        write_chunk(&root, &piece("again", 3_000, &whole, 0, 1_000, None)).unwrap_or_else(|e| panic!("{e}"));
        let restarted =
            write_chunk(&root, &piece("again", 3_000, &whole, 0, 3_000, None)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            restarted,
            Progress {
                received: 3_000,
                done: true
            }
        );
        assert_eq!(
            std::fs::metadata(uploads(&root).join("t_1.again.done"))
                .map(|m| m.len())
                .unwrap_or_default(),
            3_000
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_interrupted_upload_leaves_no_file_in_a_session_and_is_deleted() {
        let root = temp_root();
        let whole = vec![9u8; 6_000];
        // Cancelled by the person: both the temporary and a finished file go.
        write_chunk(&root, &piece("c1", 6_000, &whole, 0, 2_000, None)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(cancel(&root, Some("t_1"), "c1"), Ok(()));
        assert_eq!(cancel(&root, Some("t_1"), "c1"), Ok(()));
        assert!(leftovers(&root).is_empty());
        // Dropped half way: nothing is in any session folder, and the next upload deletes what is a day old.
        write_chunk(&root, &piece("d1", 6_000, &whole, 0, 2_000, None)).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !root.join("d1").exists()
                && std::fs::read_dir(&root)
                    .map(|d| d.flatten().count())
                    .unwrap_or_default()
                    == 1
        );
        clean_stale(&root, STALE_AFTER);
        assert_eq!(leftovers(&root), ["t_1.d1.part"], "a fresh upload stays");
        clean_stale(&root, Duration::ZERO);
        assert!(leftovers(&root).is_empty());
        // The hidden folder is not a session's: a session id cannot name it.
        assert!(session_dir(&root, UPLOADS).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    /// An upload belongs to the session it was made for: a message to another session cannot take it, and neither
    /// can one to any session when the upload names none (an older app's).
    #[test]
    fn an_upload_is_taken_only_by_a_message_to_its_own_session() {
        let root = temp_root();
        let whole = vec![7u8; 3_000];
        write_chunk(&root, &piece("mine", 3_000, &whole, 0, 3_000, None)).unwrap_or_else(|e| panic!("{e}"));
        let other = prepare(&root, "t_2", "", &[staged("v.bin", "mine")]);
        assert_eq!(
            other.map(|_| ()),
            Err("v.bin did not finish uploading. Attach it again.".into())
        );
        assert!(
            leftovers(&root).contains(&"t_1.mine.done".to_string()),
            "the other session left it alone"
        );
        let (prompt, _) = prepare(&root, "t_1", "", &[staged("v.bin", "mine")]).unwrap_or_else(|e| panic!("{e}"));
        assert!(prompt.contains("v.bin"));
        // An older app's upload names no session: no message takes it.
        let unbound = Chunk {
            thread_id: None,
            ..piece("old", 3_000, &whole, 0, 3_000, None)
        };
        write_chunk(&root, &unbound).unwrap_or_else(|e| panic!("{e}"));
        let refused = prepare(&root, "t_1", "", &[staged("v.bin", "old")]);
        assert!(refused.is_err_and(|e| e.contains("older Nexus app")));
        assert_eq!(cancel(&root, None, "old"), Ok(()));
        // A session id that is not a plain name never builds a path.
        let crooked = Chunk {
            thread_id: Some("../t_1".into()),
            ..piece("x", 3_000, &whole, 0, 3_000, None)
        };
        assert_eq!(write_chunk(&root, &crooked), Err("Invalid session id.".into()));
        assert!(leftovers(&root).is_empty(), "{:?}", leftovers(&root));
        let _ = std::fs::remove_dir_all(root);
    }
}
