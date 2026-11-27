//! Replace a file whole: write the new bytes to a temporary file in the same folder, sync it, then rename it over the
//! target. A crash or a failed write before the rename leaves the old file as it was; a reader sees either the old
//! bytes or the new ones, never half of them. On Windows the rename replaces an existing target too.
//!
//! The folder must already exist: callers create it with the permissions they want.
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Replace `file` with `bytes`. A new file gets the default permissions, as `std::fs::write` gives.
pub fn write_atomic(file: &Path, bytes: &[u8]) -> io::Result<()> {
    write_with(file, bytes, None, |_| Ok(()))
}

/// [`write_atomic`] for a file only its owner may read: 0600 on Unix from the first byte.
pub fn write_atomic_private(file: &Path, bytes: &[u8]) -> io::Result<()> {
    write_with(file, bytes, Some(0o600), |_| Ok(()))
}

/// The write, with `before_rename` run on the synced temporary file just before it replaces `file`: tests stop there.
fn write_with(
    file: &Path,
    bytes: &[u8],
    mode: Option<u32>,
    before_rename: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let temporary = temporary_beside(file)?;
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        // A fresh name every time: never reuses a leftover file and the permissions it had.
        options.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let mut output = options.open(&temporary)?;
        output.write_all(bytes)?;
        output.flush()?;
        output.sync_all()?;
        drop(output);
        before_rename(&temporary)?;
        std::fs::rename(&temporary, file)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// A hidden, unique name in `file`'s folder, so the rename never crosses file systems.
fn temporary_beside(file: &Path) -> io::Result<PathBuf> {
    let name = file
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path names no file"))?;
    let parent = file
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-atomic-file-{name}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_new_file_is_written_and_no_temporary_is_left() {
        let dir = folder("new");
        let file = dir.join("state.json");
        write_atomic(&file, b"one").expect("write");
        assert_eq!(std::fs::read(&file).expect("read"), b"one");
        assert_eq!(entries(&dir), vec!["state.json".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_file_is_replaced() {
        let dir = folder("replace");
        let file = dir.join("state.json");
        std::fs::write(&file, b"an older and longer value").expect("seed");
        write_atomic(&file, b"new").expect("write");
        assert_eq!(std::fs::read(&file).expect("read"), b"new");
        write_atomic_private(&file, b"newer").expect("write again");
        assert_eq!(std::fs::read(&file).expect("read"), b"newer");
        assert_eq!(entries(&dir), vec!["state.json".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_interrupted_before_the_rename_leaves_the_old_file_intact() {
        let dir = folder("interrupted");
        let file = dir.join("state.json");
        std::fs::write(&file, b"old").expect("seed");
        let interrupted = write_with(&file, b"new", None, |temporary| {
            // The new bytes are whole on disk, beside the target, and the target still holds the old ones.
            assert_eq!(temporary.parent(), file.parent());
            assert_eq!(std::fs::read(temporary).expect("temporary"), b"new");
            assert_eq!(std::fs::read(&file).expect("target"), b"old");
            Err(io::Error::other("interrupted"))
        });
        assert!(interrupted.is_err());
        assert_eq!(std::fs::read(&file).expect("read"), b"old");
        assert_eq!(
            entries(&dir),
            vec!["state.json".to_string()],
            "the temporary file is removed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_into_a_missing_folder_fails_and_creates_nothing() {
        let dir = folder("missing");
        let file = dir.join("absent").join("state.json");
        assert!(write_atomic(&file, b"x").is_err());
        assert!(entries(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_private_file_is_owner_only_and_a_default_one_is_not_forced() {
        use std::os::unix::fs::PermissionsExt;
        let dir = folder("modes");
        let private = dir.join("secret.json");
        std::fs::write(&private, b"old").expect("seed");
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        write_atomic_private(&private, b"new").expect("write");
        let mode = std::fs::metadata(&private).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let plain = dir.join("plain");
        write_atomic(&plain, b"x").expect("write");
        std::fs::write(dir.join("reference"), b"x").expect("reference");
        let mode = |path: PathBuf| std::fs::metadata(path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(
            mode(plain),
            mode(dir.join("reference")),
            "the same permissions std::fs::write gives"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
