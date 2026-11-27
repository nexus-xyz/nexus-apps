//! Folders and files only their owner may read: 0700 and 0600 on unix. For what holds a person's secrets or their
//! sessions (the store, the key store, account folders, the logs of repository scripts). Elsewhere the calls are the
//! plain `std::fs` ones.

use std::io::{self, Write};
use std::path::Path;

/// Makes `dir` and its missing parents. A folder this makes last (`dir` itself) is 0700; the parents keep the usual
/// mode, and a `dir` that is already there keeps its own.
pub(crate) fn create_dir_all(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    match create_dir(dir) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
        done => done,
    }
}

/// Makes the folder `dir`, 0700. Fails when it is there already, as [`std::fs::create_dir`] does.
pub(crate) fn create_dir(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// Makes the folder `dir` 0700 when it is a real folder (not a link to one).
pub(crate) fn restrict_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    if std::fs::symlink_metadata(dir)?.is_dir() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Makes `file` 0600, creating it empty when it is missing: called before SQLite opens it, so the database (and the
/// `-wal` and `-shm` files SQLite gives the same mode) is never readable by others, even for a moment. A link at `file`
/// is refused, not followed (`O_NOFOLLOW`), as is anything but a plain file, so a link planted there can't get its
/// target's mode changed.
pub(crate) fn restrict_file(file: &Path) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let opened = options.open(file)?;
    if !opened.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a plain file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        opened.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    drop(opened);
    Ok(())
}

/// Writes `bytes` to `file`, 0600, through a new temporary file in the same folder renamed over it. The temporary file
/// is made new (`create_new`, which never follows a link) and the rename replaces a link at `file` rather than
/// writing through it, so a link planted there can't send the write elsewhere.
pub(crate) fn write(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = file
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let temp = file.with_file_name(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&temp).and_then(|mut out| {
        out.write_all(bytes)?;
        out.sync_all()
    });
    let done = written.and_then(|()| std::fs::rename(&temp, file));
    if done.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    done
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path)
            .map(|meta| meta.permissions().mode() & 0o777)
            .unwrap_or_default()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-private-fs-{name}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        dir
    }

    #[test]
    fn folders_are_0700_and_files_0600() {
        let root = scratch("modes");
        let dir = root.join("a/b");
        create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(mode(&dir), 0o700);
        create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        let file = dir.join("db");
        std::fs::write(&file, "x").unwrap_or_else(|e| panic!("{e}"));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap_or_else(|e| panic!("{e}"));
        restrict_file(&file).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(mode(&file), 0o600);
        assert_eq!(std::fs::read_to_string(&file).unwrap_or_default(), "x");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restricting_a_planted_link_leaves_its_target_alone() {
        let root = scratch("restrict-link");
        let target = root.join("target");
        std::fs::write(&target, "keep").unwrap_or_else(|e| panic!("{e}"));
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap_or_else(|e| panic!("{e}"));
        let db = root.join("store.db");
        std::os::unix::fs::symlink(&target, &db).unwrap_or_else(|e| panic!("{e}"));
        assert!(restrict_file(&db).is_err());
        assert_eq!(mode(&target), 0o644);
        assert_eq!(std::fs::read_to_string(&target).unwrap_or_default(), "keep");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_write_replaces_a_planted_link_instead_of_following_it() {
        let root = scratch("link");
        let target = root.join("target");
        std::fs::write(&target, "keep").unwrap_or_else(|e| panic!("{e}"));
        let log = root.join("setup.log");
        std::os::unix::fs::symlink(&target, &log).unwrap_or_else(|e| panic!("{e}"));
        write(&log, b"secret output").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(std::fs::read_to_string(&target).unwrap_or_default(), "keep");
        assert!(
            !std::fs::symlink_metadata(&log)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true)
        );
        assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "secret output");
        assert_eq!(mode(&log), 0o600);
        let _ = std::fs::remove_dir_all(&root);
    }
}
