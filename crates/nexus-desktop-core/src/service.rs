//! "Keep agents running when Nexus quits": the rules for the background service that runs this computer's Code
//! sessions. Nexus GPUI (its src/service.rs) decides with them whether a launch uses the service, and where it finds
//! it. Starting, reaching and stopping it is nexus-remote's `local` module.

use std::path::{Path, PathBuf};

/// Why the switch cannot be on here, for a person; None when it can. `allow_local_cache` is the managed
/// `AllowLocalCache`: with nothing kept on this computer, sessions can't outlive the app.
pub fn unsupported(allow_local_cache: bool) -> Option<String> {
    if !cfg!(unix) {
        return Some("Not available on Windows yet.".into());
    }
    if !allow_local_cache {
        return Some("Your organization keeps nothing on this computer, so sessions can't outlive the app.".into());
    }
    None
}

/// Whether a launch runs this computer's sessions in the service: the switch (`keep_agents_running` in
/// `settings.json`) is on, and it can be.
pub fn wanted(keep_agents_running: Option<bool>, allow_local_cache: bool) -> bool {
    keep_agents_running.unwrap_or(false) && unsupported(allow_local_cache).is_none()
}

/// The service's socket: in the person's runtime folder when the system has one (`runtime`, Linux's
/// `XDG_RUNTIME_DIR`), else in the app's data folder `data`. Both are private to the person. `identifier` is the
/// app's (`xyz.nexus.desktop`), so a development build's service stays apart from the released app's.
pub fn socket(identifier: &str, data: &Path, runtime: Option<PathBuf>) -> PathBuf {
    match runtime.filter(|dir| dir.is_absolute() && dir.is_dir()) {
        Some(dir) => dir.join(identifier).join("service.sock"),
        None => data.join("run").join("service.sock"),
    }
}

/// [`socket`] with this process's `XDG_RUNTIME_DIR`.
pub fn socket_here(identifier: &str, data: &Path) -> PathBuf {
    socket(identifier, data, std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
}

/// The binary the service runs from: this app's. An AppImage's own path is mounted only while the app runs, so the
/// service starts from the AppImage file, which mounts its own copy.
pub fn program() -> Result<PathBuf, String> {
    if let Some(appimage) = std::env::var_os("APPIMAGE").filter(|path| !path.is_empty()) {
        return Ok(appimage.into());
    }
    std::env::current_exe().map_err(|e| format!("Could not find the app's binary: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_counts_only_where_it_can_be_on() {
        assert!(!wanted(None, true));
        assert!(!wanted(Some(false), true));
        assert_eq!(wanted(Some(true), true), cfg!(unix));
        assert!(!wanted(Some(true), false), "the policy keeps it off");
        if cfg!(unix) {
            assert!(unsupported(false).is_some_and(|reason| reason.contains("organization")));
            assert_eq!(unsupported(true), None);
        }
    }

    #[test]
    fn the_socket_is_in_the_runtime_folder_when_there_is_one() {
        let data = Path::new("/data/xyz.nexus.desktop");
        let runtime = std::env::temp_dir();
        assert_eq!(
            socket("xyz.nexus.desktop", data, Some(runtime.clone())),
            runtime.join("xyz.nexus.desktop").join("service.sock")
        );
        assert_eq!(
            socket("xyz.nexus.desktop", data, None),
            data.join("run").join("service.sock")
        );
        assert_eq!(
            socket("xyz.nexus.desktop", data, Some("relative".into())),
            data.join("run").join("service.sock")
        );
    }
}
