//! Install the GPUI feed's signed disk image without changing the feed or the manual installer.
//! Stage on the app's volume, swap bundles, and request authorization when necessary.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{BIN, Relaunch};

/// The app bundle's name in the update's disk image, newest first: `Nexus.app`, then the name of builds before the
/// rename. The installed bundle keeps its own path, whatever it is called.
const APPS: [&str; 2] = ["Nexus.app", "Nexus GPUI.app"];

/// The app bundle in a mounted update: the first of [`APPS`] that is a real folder, never a link.
fn app_in(volume: &Path) -> Option<PathBuf> {
    APPS.iter()
        .map(|name| volume.join(name))
        .find(|path| fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir() && !metadata.is_symlink()))
}

/// The main executable's enclosing bundle, including a renamed app. Never mistake a CEF helper for this app.
fn bundle(exe: &Path) -> Option<&Path> {
    let macos = exe.parent()?;
    let app = macos.parent()?.parent()?;
    (exe.file_name()? == BIN && macos.ends_with("Contents/MacOS") && app.extension()? == "app").then_some(app)
}

pub(super) fn install(exe: &Path, bytes: &[u8], version: &str) -> Result<Relaunch, String> {
    let target = bundle(exe).ok_or("This copy is not in a macOS app bundle.")?;
    // Renames must stay on one file system. Prefer the private system temp folder, which also lets an admin-owned
    // /Applications app stage the update before macOS asks for permission to replace it.
    let device = fs::symlink_metadata(target).map_err(|error| error.to_string())?.dev();
    let root = [Some(std::env::temp_dir()), target.parent().map(Path::to_path_buf)]
        .into_iter()
        .flatten()
        .find(|dir| dir.metadata().is_ok_and(|metadata| metadata.dev() == device))
        .ok_or("No staging folder is available on the app's volume.")?;
    let work = tempfile::Builder::new()
        .prefix("nexus-gpui-update-")
        .tempdir_in(root)
        .map_err(|error| format!("Couldn't stage the update: {error}."))?;
    let image = work.path().join("update.dmg");
    fs::write(&image, bytes).map_err(|error| format!("Couldn't save the update: {error}."))?;
    let mount = work.path().join("image");
    fs::create_dir(&mount).map_err(|error| error.to_string())?;
    let mounted = DiskImage::attach(&image, mount)?;
    let source = app_in(&mounted.0).ok_or("The update has no Nexus app bundle.")?;
    let staged = work.path().join(APPS[0]);
    run(
        Command::new("/usr/bin/ditto").arg(&source).arg(&staged),
        "copy the updated app",
    )?;
    validate(&staged, version)?;
    drop(mounted);

    let backup = work.path().join("previous.app");
    if let Err(error) = replace(&staged, target, &backup) {
        // A failed restore must never delete the only remaining copy with the temporary folder.
        if !target.exists() && backup.exists() {
            let kept = work.keep().join("previous.app");
            return Err(format!(
                "Couldn't restore the previous app. It is saved at {}. {error}",
                kept.display()
            ));
        }
        return Err(error);
    }
    // Refresh Launch Services' view of the bundle after replacing it.
    let _ = Command::new("/usr/bin/touch").arg(target).status();
    Ok(Relaunch {
        program: target.join("Contents/MacOS").join(BIN),
        args: Vec::new(),
        restarted: true,
    })
}

/// A private, read-only mount. Always detach it before its staging folder is removed, including on validation errors.
struct DiskImage(PathBuf);

impl DiskImage {
    fn attach(image: &Path, mount: PathBuf) -> Result<Self, String> {
        let mounted = Self(mount);
        run(
            Command::new("/usr/bin/hdiutil")
                .args(["attach", "-readonly", "-nobrowse", "-noautoopen", "-mountpoint"])
                .arg(&mounted.0)
                .arg(image),
            "open the update disk image",
        )?;
        Ok(mounted)
    }
}

impl Drop for DiskImage {
    fn drop(&mut self) {
        if Command::new("/usr/bin/hdiutil")
            .args(["detach", "-quiet"])
            .arg(&self.0)
            .status()
            .is_ok_and(|status| !status.success())
        {
            let _ = Command::new("/usr/bin/hdiutil")
                .args(["detach", "-quiet", "-force"])
                .arg(&self.0)
                .status();
        }
    }
}

fn run(command: &mut Command, action: &str) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("Couldn't {action}: {error}."))?;
    if !output.status.success() {
        return Err(format!(
            "Couldn't {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The feed's minisign check authenticates the bytes before this point. Also check the copied bundle's identity,
/// version, executable and sealed resources. Ad hoc internal packages follow the earlier packages' signing policy.
fn validate(app: &Path, version: &str) -> Result<(), String> {
    for (key, expected) in [
        ("CFBundleIdentifier", "xyz.nexus.desktop-gpui"),
        ("CFBundleExecutable", BIN),
        ("CFBundleShortVersionString", version),
    ] {
        let value = run(
            Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", &format!("Print :{key}")])
                .arg(app.join("Contents/Info.plist")),
            "read the updated app's identity",
        )?;
        if value != expected {
            return Err(format!("The update has an unexpected {key}."));
        }
    }
    let exe = app.join("Contents/MacOS").join(BIN);
    if !fs::symlink_metadata(&exe).is_ok_and(|metadata| metadata.is_file() && !metadata.is_symlink()) {
        return Err("The update has no app executable.".into());
    }
    run(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app),
        "verify the updated app",
    )?;
    Ok(())
}

fn swap(staged: &Path, target: &Path) -> io::Result<()> {
    let staged = CString::new(staged.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    // SAFETY: these are valid NUL-terminated paths. renamex_np reads them and atomically exchanges the two bundles.
    if unsafe { libc::renamex_np(staged.as_ptr(), target.as_ptr(), libc::RENAME_SWAP) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn move_bundle(staged: &Path, target: &Path, backup: &Path) -> io::Result<()> {
    fs::rename(target, backup)?;
    if let Err(error) = fs::rename(staged, target) {
        fs::rename(backup, target)?;
        return Err(error);
    }
    Ok(())
}

fn replace(staged: &Path, target: &Path, backup: &Path) -> Result<(), String> {
    let result = match swap(staged, target) {
        Err(error) if error.kind() != io::ErrorKind::PermissionDenied => move_bundle(staged, target, backup),
        other => other,
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            // osascript owns the native authorization dialog. Each path is a quoted shell argument, then the whole
            // command is an AppleScript string. A cancelled dialog leaves the current bundle in place.
            let script = format!(
                "do shell script {} with administrator privileges",
                applescript(&privileged(staged, target, backup))
            );
            run(
                Command::new("/usr/bin/osascript").args(["-e", &script]),
                "authorize the app update",
            )
            .map(drop)
        }
        Err(error) => Err(format!("Couldn't replace the app: {error}.")),
    }
}

fn privileged(staged: &Path, target: &Path, backup: &Path) -> String {
    let staged = quote(staged);
    let target = quote(target);
    let backup = quote(backup);
    format!("/bin/mv {target} {backup} && {{ /bin/mv {staged} {target} || {{ /bin/mv {backup} {target}; exit 1; }}; }}")
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn applescript(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests;
