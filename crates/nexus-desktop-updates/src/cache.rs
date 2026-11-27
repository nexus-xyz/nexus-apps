//! One completed download, kept across launches. A fresh feed selects the artifact before its bytes are reused.
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::verify::version_matches;
#[cfg(test)]
use base64::{Engine, engine::general_purpose::STANDARD};

const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 16 * 1024;

/// The selected release's identity, supplied by the current feed rather than trusted from disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub version: String,
    pub target: String,
    pub url: String,
    pub signature: String,
}

#[derive(Clone)]
pub struct Cache {
    directory: PathBuf,
}

impl Cache {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Reverify bytes and their signed version with the current key. Discard invalid entries; preserve read failures.
    pub fn load(&self, expected: &Artifact, pubkey: &str) -> Option<Vec<u8>> {
        let loaded = (|| -> io::Result<Option<Vec<u8>>> {
            let Some(metadata) = bounded_read(&self.directory.join("release.json"), MAX_METADATA_BYTES)? else {
                return Ok(None);
            };
            let Ok(saved) = serde_json::from_slice::<Artifact>(&metadata) else {
                return Ok(None);
            };
            if saved != *expected {
                return Ok(None);
            }
            let Some(bytes) = bounded_read(&self.directory.join("artifact"), MAX_ARTIFACT_BYTES)? else {
                return Ok(None);
            };
            Ok(verify(expected, pubkey, &bytes).map(|()| bytes))
        })();
        match loaded {
            Ok(Some(bytes)) => Some(bytes),
            Err(error) if error.kind() != io::ErrorKind::NotFound => None,
            _ => {
                let _ = self.clear();
                None
            }
        }
    }

    /// Save bytes already verified by the downloader. Publish metadata last; an interrupted save is never reused.
    pub fn save(&self, artifact: &Artifact, bytes: &[u8]) -> io::Result<()> {
        let metadata = serde_json::to_vec(artifact).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_ARTIFACT_BYTES || metadata.len() as u64 > MAX_METADATA_BYTES {
            return Err(io::Error::other("The update is too large to cache."));
        }
        fs::create_dir_all(&self.directory)?;
        self.replace("artifact", bytes)?;
        self.replace("release.json", &metadata)?;
        // Unix supports syncing directory entries after rename. Other platforms revalidate after interruption.
        #[cfg(unix)]
        fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    /// Remove a stale or installed release, including an interrupted write. No other app cache files are touched.
    pub fn clear(&self) -> io::Result<()> {
        for name in ["release.json", "artifact", "release.json.part", "artifact.part"] {
            match fs::remove_file(self.directory.join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn replace(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let pending = self.directory.join(format!("{name}.part"));
        let mut file = fs::File::create(&pending)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(pending, self.directory.join(name))
    }
}

fn bounded_read(path: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > limit {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
}

fn verify(artifact: &Artifact, pubkey: &str, bytes: &[u8]) -> Option<()> {
    crate::verify::verify(bytes, &artifact.signature, pubkey, &artifact.version).ok()
}

#[cfg(test)]
mod tests;
