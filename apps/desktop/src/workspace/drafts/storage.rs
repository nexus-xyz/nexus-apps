//! Private immutable attachment blobs and an atomic per-identity draft index.
use super::ParkedDraft;
use crate::attachments::{self, Attachment};
use crate::i18n::tr;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub(super) struct Change {
    pub key: String,
    pub revision: u64,
    pub draft: ParkedDraft,
}
#[derive(Default)]
struct Written {
    revisions: BTreeMap<String, u64>,
    blobs: HashSet<String>,
}
pub(super) struct DiskStore {
    root: PathBuf,
    enabled: AtomicBool,
    written: Mutex<Written>,
}
#[derive(Default, Serialize, Deserialize)]
struct Index {
    version: u8,
    drafts: BTreeMap<String, SavedDraft>,
}
#[derive(Serialize, Deserialize)]
struct SavedDraft {
    text: String,
    images: Vec<SavedAttachment>,
    files: Vec<SavedAttachment>,
}
#[derive(Serialize, Deserialize)]
struct SavedAttachment {
    name: String,
    media_type: Option<String>,
    blob: String,
    bytes: u64,
    /// A referenced artifact's chip; its source, if any, is the blob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact: Option<crate::artifact_refs::ArtifactRef>,
}
fn invalid(note: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, note)
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
fn blob_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl DiskStore {
    pub fn new(root: PathBuf, enabled: bool) -> Self {
        Self {
            root,
            enabled: AtomicBool::new(enabled),
            written: Mutex::default(),
        }
    }
    pub fn enable(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }
    fn allowed(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    fn index(&self) -> io::Result<Index> {
        let bytes = match std::fs::read(self.root.join("drafts.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Index {
                    version: 1,
                    ..Default::default()
                });
            }
            Err(e) => return Err(e),
        };
        let index: Index = serde_json::from_slice(&bytes).map_err(|_| invalid("draft index is invalid"))?;
        if index.version != 1 {
            return Err(invalid("draft index version is unsupported"));
        }
        Ok(index)
    }
    pub fn read(&self, key: &str) -> io::Result<Option<ParkedDraft>> {
        if !self.allowed() {
            return Ok(None);
        }
        let _guard = self.written.lock().map_err(|_| invalid("draft storage lock failed"))?;
        if !self.allowed() {
            return Ok(None);
        }
        let Some(saved) = self.index()?.drafts.remove(key) else {
            return Ok(None);
        };
        let mut draft = ParkedDraft {
            text: saved.text,
            ..Default::default()
        };
        let mut missing = false;
        for (image, items) in [(true, saved.images), (false, saved.files)] {
            for item in items {
                let read = (|| {
                    if !blob_name(&item.blob) {
                        return Err(invalid("draft attachment reference is invalid"));
                    }
                    if image
                        && !matches!(
                            item.media_type.as_deref(),
                            Some("image/png" | "image/jpeg" | "image/gif" | "image/webp")
                        )
                    {
                        return Err(invalid("draft image type is invalid"));
                    }
                    let bytes = std::fs::read(self.root.join("attachments").join(&item.blob))?;
                    if bytes.len() as u64 != item.bytes || digest(&bytes) != item.blob {
                        return Err(invalid("draft attachment is incomplete"));
                    }
                    let mut restored =
                        attachments::persisted_attachment(item.name, item.media_type.as_deref(), Arc::new(bytes));
                    restored.artifact = item.artifact.map(Arc::new);
                    Ok(restored)
                })();
                match read {
                    Ok(item) => {
                        if image {
                            draft.images.push(item)
                        } else {
                            draft.files.push(item)
                        }
                    }
                    Err(_) => missing = true,
                }
            }
        }
        if missing {
            draft.problem =
                Some(tr("Some attachments could not be restored. Attach them again before sending.").to_string());
        }
        Ok(Some(draft))
    }
    /// Merge changed keys into the current index. A late save cannot replace a newer revision.
    pub fn write(&self, changes: Vec<Change>) -> io::Result<Vec<(String, u64)>> {
        if !self.allowed() {
            return Ok(Vec::new());
        }
        let mut written = self.written.lock().map_err(|_| invalid("draft storage lock failed"))?;
        if !self.allowed() {
            return Ok(Vec::new());
        }
        let mut index = self.index()?;
        let mut applied = Vec::new();
        let mut changed = false;
        for change in changes {
            if written
                .revisions
                .get(&change.key)
                .is_some_and(|seen| *seen >= change.revision)
            {
                applied.push((change.key, change.revision));
                continue;
            }
            if !self.allowed() {
                return Ok(Vec::new());
            }
            changed = true;
            if change.draft.text.trim().is_empty() && change.draft.images.is_empty() && change.draft.files.is_empty() {
                index.drafts.remove(&change.key);
            } else {
                let mut save = |items: &[Attachment]| -> io::Result<Vec<SavedAttachment>> {
                    items
                        .iter()
                        .map(|item| {
                            let blob = digest(&item.bytes);
                            let file = self.root.join("attachments").join(&blob);
                            if !written.blobs.contains(&blob) || !file.is_file() {
                                if !self.allowed() {
                                    return Err(io::Error::new(
                                        io::ErrorKind::PermissionDenied,
                                        "draft persistence is disabled",
                                    ));
                                }
                                write_atomic(&file, &item.bytes)?;
                                written.blobs.insert(blob.clone());
                            }
                            Ok(SavedAttachment {
                                name: item.name.clone(),
                                media_type: item.media_type.map(str::to_string),
                                blob,
                                bytes: item.bytes.len() as u64,
                                artifact: item.artifact.as_deref().cloned(),
                            })
                        })
                        .collect()
                };
                index.drafts.insert(
                    change.key.clone(),
                    SavedDraft {
                        text: change.draft.text,
                        images: save(&change.draft.images)?,
                        files: save(&change.draft.files)?,
                    },
                );
            }
            applied.push((change.key, change.revision));
        }
        if !self.allowed() {
            return Ok(Vec::new());
        }
        if !changed {
            return Ok(applied);
        }
        let bytes = serde_json::to_vec(&index).map_err(|_| invalid("draft index could not be encoded"))?;
        write_atomic(&self.root.join("drafts.json"), &bytes)?;
        for (key, revision) in &applied {
            written
                .revisions
                .entry(key.clone())
                .and_modify(|seen| *seen = (*seen).max(*revision))
                .or_insert(*revision);
        }
        let referenced: HashSet<_> = index
            .drafts
            .values()
            .flat_map(|draft| draft.images.iter().chain(&draft.files))
            .map(|item| item.blob.as_str())
            .collect();
        if let Ok(entries) = std::fs::read_dir(self.root.join("attachments")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if blob_name(&name) && !referenced.contains(name.as_str()) {
                    let _ = std::fs::remove_file(entry.path());
                    written.blobs.remove(&name);
                }
            }
        }
        Ok(applied)
    }
}
fn write_atomic(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = file.parent().ok_or_else(|| invalid("draft storage has no parent"))?;
    let mut dirs = std::fs::DirBuilder::new();
    dirs.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        dirs.mode(0o700);
    }
    dirs.create(parent)?;
    nexus_desktop_core::atomic_file::write_atomic_private(file, bytes)
}

#[cfg(test)]
mod tests;
