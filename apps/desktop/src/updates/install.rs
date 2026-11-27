//! Installing a verified update, per platform, as `packaging/` builds each package:
//!
//! - Linux: the `.tar.xz` folder. The new folder is unpacked beside this one, then takes its place, so the launcher entry
//!   (`install.sh`) still points at the same path. The old folder is removed at the next launch.
//! - Windows: the NSIS installer. It runs silently with `/UPDATE` after this app quits, then starts the app again.
//! - macOS: mount the verified `.dmg`, stage and validate its app bundle, then replace this bundle and relaunch it.

#[cfg(target_os = "macos")]
mod macos;

use std::path::{Path, PathBuf};

/// What starts the app again once this copy quits.
#[derive(Debug, PartialEq, Eq)]
pub struct Relaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Started as the app itself, which waits for this copy to quit (`single_instance::claim_after_restart`).
    pub restarted: bool,
}

impl Relaunch {
    pub fn start(&self) -> std::io::Result<()> {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args);
        if self.restarted {
            command.env("NEXUS_RESTARTED", "1");
        }
        command.spawn().map(drop)
    }
}

/// The binary's name in every package.
const BIN: &str = if cfg!(windows) {
    "nexus-desktop-gpui.exe"
} else {
    "nexus-desktop-gpui"
};

/// Whether this copy runs from a package this updater can replace: the unpacked Linux folder (CEF and `install.sh`
/// beside the binary), the Windows install folder (with its uninstaller), or a macOS app bundle. A build run from
/// `target/` is not.
pub fn packaged(exe: &Path) -> bool {
    let Some(dir) = exe.parent() else {
        return false;
    };
    if cfg!(target_os = "linux") {
        dir.join("libcef.so").is_file() && dir.join("install.sh").is_file()
    } else if cfg!(windows) {
        dir.join("uninstall.exe").is_file()
    } else if cfg!(target_os = "macos") {
        dir.ends_with("Contents/MacOS")
    } else {
        false
    }
}

/// Install `bytes`, the verified package for `version`, for this copy at `exe`. Linux replaces the folder now; Windows
/// writes the installer, which runs after this copy quits. macOS replaces the app bundle now.
pub fn install(exe: &Path, bytes: &[u8], version: &str) -> Result<Relaunch, String> {
    if cfg!(target_os = "linux") {
        replace_folder(exe, bytes)
    } else if cfg!(windows) {
        let setup = std::env::temp_dir().join(format!("nexus-desktop-{version}-setup.exe"));
        std::fs::write(&setup, bytes).map_err(|error| format!("Couldn't save the installer: {error}."))?;
        Ok(Relaunch {
            program: setup,
            args: vec!["/S".into(), "/UPDATE".into()],
            restarted: false,
        })
    } else if cfg!(target_os = "macos") {
        #[cfg(target_os = "macos")]
        {
            macos::install(exe, bytes, version)
        }
        #[cfg(not(target_os = "macos"))]
        unreachable!();
    } else {
        Err("This build can't install updates on this system. Download the new version instead.".into())
    }
}

/// The sibling folders an update uses: `.<name>.update` while unpacking, `.<name>.old` for the replaced copy.
fn siblings(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let name = dir.file_name()?.to_str()?;
    let parent = dir.parent()?;
    Some((
        parent.join(format!(".{name}.update")),
        parent.join(format!(".{name}.old")),
    ))
}

/// Remove the folder an earlier update replaced. Called at launch; nothing else runs from it then.
pub fn clean_up(exe: &Path) {
    if let Some((staging, old)) = exe.parent().and_then(siblings) {
        let _ = std::fs::remove_dir_all(staging);
        let _ = std::fs::remove_dir_all(old);
    }
}

/// Unpack the tarball beside this folder, then swap the folders. A running binary keeps its files open on Linux, so
/// this copy runs on until it quits.
fn replace_folder(exe: &Path, bytes: &[u8]) -> Result<Relaunch, String> {
    let dir = exe.parent().ok_or("This copy's folder is unknown.")?;
    let (staging, old) = siblings(dir).ok_or("This copy's folder is unknown.")?;
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&old);
    std::fs::create_dir_all(&staging).map_err(|error| {
        format!(
            "Couldn't write beside {}: {error}. Move the app to a folder you can write to.",
            dir.display()
        )
    })?;
    let result = unpack(&staging, bytes).and_then(|unpacked| {
        std::fs::rename(dir, &old).map_err(|error| format!("Couldn't move the old version aside: {error}."))?;
        if let Err(error) = std::fs::rename(&unpacked, dir) {
            let _ = std::fs::rename(&old, dir);
            return Err(format!("Couldn't move the new version in place: {error}."));
        }
        Ok(())
    });
    let _ = std::fs::remove_dir_all(&staging);
    result?;
    Ok(Relaunch {
        program: dir.join(BIN),
        args: Vec::new(),
        restarted: true,
    })
}

/// Unpack the `.tar.xz` into `staging` with the system's `tar`, and return its one top folder, which holds the binary.
fn unpack(staging: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let archive = staging.join("update.tar.xz");
    std::fs::write(&archive, bytes).map_err(|error| format!("Couldn't save the update: {error}."))?;
    let status = std::process::Command::new("tar")
        .arg("-xJf")
        .arg(&archive)
        .arg("-C")
        .arg(staging)
        .arg("--no-same-owner")
        .status()
        .map_err(|error| format!("Couldn't run tar to unpack the update: {error}."))?;
    let _ = std::fs::remove_file(&archive);
    if !status.success() {
        return Err("The update could not be unpacked.".into());
    }
    let mut folders = std::fs::read_dir(staging)
        .map_err(|error| format!("Couldn't read the unpacked update: {error}."))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir());
    let (Some(folder), None) = (folders.next(), folders.next()) else {
        return Err("The update has an unexpected layout.".into());
    };
    if !folder.join(BIN).is_file() {
        return Err("The update has no app in it.".into());
    }
    Ok(folder)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// A temp folder laid out as an unpacked package: `<root>/<name>/nexus-desktop-gpui`, CEF and `install.sh`.
    struct Install {
        root: PathBuf,
    }

    impl Install {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("nexus-gpui-install-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Install { root }
        }

        fn package(&self, name: &str, marker: &str) -> PathBuf {
            let dir = self.root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(BIN), marker).unwrap();
            std::fs::write(dir.join("libcef.so"), "").unwrap();
            std::fs::write(dir.join("install.sh"), "").unwrap();
            dir
        }

        /// A `.tar.xz` of a package folder holding `marker` in its binary, as `packaging/package.sh` makes one.
        fn tarball(&self, marker: &str) -> Vec<u8> {
            let source = self.root.join("source");
            let name = "nexus-desktop-9.9.9-linux-x86_64";
            let _ = std::fs::remove_dir_all(&source);
            std::fs::create_dir_all(source.join(name)).unwrap();
            std::fs::write(source.join(name).join(BIN), marker).unwrap();
            let out = self.root.join("update.tar.xz");
            let status = std::process::Command::new("tar")
                .arg("-cJf")
                .arg(&out)
                .arg("-C")
                .arg(&source)
                .arg(name)
                .status()
                .unwrap();
            assert!(status.success());
            let bytes = std::fs::read(&out).unwrap();
            std::fs::remove_dir_all(&source).unwrap();
            std::fs::remove_file(&out).unwrap();
            bytes
        }
    }

    impl Drop for Install {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_tarball_replaces_the_folder_and_restarts_from_the_same_path() {
        let folder = Install::new("swap");
        let dir = folder.package("Nexus", "old");
        let exe = dir.join(BIN);
        assert!(packaged(&exe));
        assert!(!packaged(Path::new("/tmp/target/debug/nexus-desktop-gpui")));

        let relaunch = install(&exe, &folder.tarball("new"), "9.9.9").expect("installed");
        assert_eq!(
            relaunch,
            Relaunch {
                program: exe.clone(),
                args: Vec::new(),
                restarted: true
            }
        );
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        let old = folder.root.join(".Nexus.old");
        assert_eq!(std::fs::read_to_string(old.join(BIN)).unwrap(), "old");
        assert!(!folder.root.join(".Nexus.update").exists());
        clean_up(&exe);
        assert!(!old.exists());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
    }

    #[test]
    fn a_broken_tarball_leaves_the_running_folder_alone() {
        let folder = Install::new("broken");
        let dir = folder.package("Nexus", "old");
        let exe = dir.join(BIN);
        let error = install(&exe, b"not a tarball", "9.9.9").expect_err("refused");
        assert_eq!(error, "The update could not be unpacked.");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        assert!(!folder.root.join(".Nexus.update").exists());
        assert!(!folder.root.join(".Nexus.old").exists());
    }
}
