//! What a double click on a window's title bar does, as the person's system says. Nexus GPUI reads it here.
//!
//! - macOS: System Settings › Desktop & Dock ([`mac`]).
//! - Linux: KDE Plasma's KWin setting, or GNOME's ([`linux`]). Nexus GPUI draws its own title bar and reads the
//!   setting here.
//! - Windows has no such setting: the double click maximizes the window or restores it.
//!
//! This module reads the settings only. It never writes them.

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// What a double click on a title bar does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleClick {
    /// Fill the screen, or back to the window's own size if it already does (maximize or restore on Linux).
    Fill,
    Minimize,
    /// The window manager's menu for the window (Linux).
    Menu,
    Nothing,
}

/// macOS: System Settings' choice (Desktop & Dock). `AppleActionOnDoubleClick` is `Fill` (macOS 15 and later),
/// `Maximize` (shown as Zoom), `Minimize` or `None`. Unset, it is the default, Fill, unless the setting from before
/// macOS 10.11, `AppleMiniaturizeOnDoubleClick`, is on.
pub fn mac(action: Option<&str>, minimize: bool) -> DoubleClick {
    match action {
        Some("Minimize") => DoubleClick::Minimize,
        Some("None") => DoubleClick::Nothing,
        Some(_) => DoubleClick::Fill,
        None if minimize => DoubleClick::Minimize,
        None => DoubleClick::Fill,
    }
}

/// KWin's command for the title bar's double click (System Settings › Window Management › Titlebar Actions), as
/// `kwinrc` names it. KWin's default is Maximize. GPUI cannot shade, lower, close or pin a window from here, so those
/// do nothing; a maximize in one direction fills the screen.
pub fn kde_command(value: &str) -> DoubleClick {
    match value.trim() {
        "Minimize" => DoubleClick::Minimize,
        "Operations menu" => DoubleClick::Menu,
        "Nothing" | "Shade" | "Lower" | "Close" | "OnAllDesktops" => DoubleClick::Nothing,
        _ => DoubleClick::Fill,
    }
}

/// GNOME's `org.gnome.desktop.wm.preferences action-double-click-titlebar`, as `gsettings get` prints it (in quotes).
/// GTK follows the same key on every desktop but KDE. The default is `toggle-maximize`. A shade or a lower does nothing
/// here; a maximize in one direction fills the screen.
pub fn gnome_action(value: &str) -> DoubleClick {
    match value.trim().trim_matches('\'') {
        "minimize" => DoubleClick::Minimize,
        "menu" => DoubleClick::Menu,
        "none" | "lower" | "toggle-shade" => DoubleClick::Nothing,
        _ => DoubleClick::Fill,
    }
}

/// `TitlebarDoubleClickCommand` in a `kwinrc`'s `[Windows]` group: None when the file does not set it.
pub fn kwinrc_command(kwinrc: &str) -> Option<String> {
    let mut in_windows = false;
    let mut found = None;
    for line in kwinrc.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_windows = line == "[Windows]";
            continue;
        }
        if !in_windows || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // `Key[$i]` marks an entry the administrator locked; the value is the same.
        let key = key.trim().trim_end_matches("[$i]");
        if key == "TitlebarDoubleClickCommand" {
            found = Some(value.trim().to_string());
        }
    }
    found
}

/// Linux: the desktop's choice, read now. KDE Plasma (`XDG_CURRENT_DESKTOP` names KDE): KWin's, from `kwinrc` in the
/// config folders. Any other desktop: GNOME's key, through `gsettings`. Nothing readable: fill, the default of both.
pub fn linux() -> DoubleClick {
    linux_with(
        &|name| std::env::var_os(name).filter(|value| !value.is_empty()),
        &["gsettings"],
    )
}

/// How long `gsettings get` may take before the default is used.
const GSETTINGS_LIMIT: Duration = Duration::from_secs(2);

/// [`linux`] with its environment and its `gsettings` command (the program and its first arguments) given.
fn linux_with(env: &dyn Fn(&str) -> Option<OsString>, gsettings: &[&str]) -> DoubleClick {
    let kde = env("XDG_CURRENT_DESKTOP")
        .and_then(|desktops| desktops.into_string().ok())
        .is_some_and(|desktops| desktops.split(':').any(|desktop| desktop.eq_ignore_ascii_case("KDE")));
    if kde {
        return kwinrc_files(env)
            .iter()
            .find_map(|file| {
                std::fs::read_to_string(file)
                    .ok()
                    .and_then(|text| kwinrc_command(&text))
            })
            .map_or(DoubleClick::Fill, |command| kde_command(&command));
    }
    read_gsettings(gsettings).map_or(DoubleClick::Fill, |value| gnome_action(&value))
}

/// The `kwinrc` files, the person's first: `$XDG_CONFIG_HOME` (else `~/.config`), then each of `$XDG_CONFIG_DIRS`
/// (else `/etc/xdg`), as KConfig reads them. The first that sets the key wins.
fn kwinrc_files(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let home = env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|home| Path::new(&home).join(".config")));
    let dirs = env("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".into());
    home.into_iter()
        .chain(std::env::split_paths(&dirs))
        .map(|dir| dir.join("kwinrc"))
        .collect()
}

/// `gsettings get org.gnome.desktop.wm.preferences action-double-click-titlebar`, read only, at most
/// [`GSETTINGS_LIMIT`]. None when there is no `gsettings`, it fails, or it takes too long.
fn read_gsettings(command: &[&str]) -> Option<String> {
    let (program, first) = command.split_first()?;
    let mut child = nexus_workbench::process::command(program)
        .args(first)
        .args([
            "get",
            "org.gnome.desktop.wm.preferences",
            "action-double-click-titlebar",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + GSETTINGS_LIMIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out.trim().to_string()).filter(|out| !out.is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn mac_follows_system_settings() {
        assert_eq!(mac(None, false), DoubleClick::Fill);
        assert_eq!(mac(Some("Fill"), false), DoubleClick::Fill);
        assert_eq!(mac(Some("Maximize"), false), DoubleClick::Fill);
        assert_eq!(mac(Some("Minimize"), false), DoubleClick::Minimize);
        assert_eq!(mac(Some("None"), false), DoubleClick::Nothing);
        // The setting from before macOS 10.11 counts only when the current one is unset.
        assert_eq!(mac(None, true), DoubleClick::Minimize);
        assert_eq!(mac(Some("Maximize"), true), DoubleClick::Fill);
    }

    #[test]
    fn kde_and_gnome_values_map_to_what_the_app_can_do() {
        assert_eq!(kde_command("Maximize"), DoubleClick::Fill);
        assert_eq!(kde_command("Maximize (vertical only)"), DoubleClick::Fill);
        assert_eq!(kde_command("Minimize"), DoubleClick::Minimize);
        assert_eq!(kde_command("Operations menu"), DoubleClick::Menu);
        assert_eq!(kde_command("Nothing"), DoubleClick::Nothing);
        assert_eq!(kde_command("Shade"), DoubleClick::Nothing);
        assert_eq!(gnome_action("'toggle-maximize'"), DoubleClick::Fill);
        assert_eq!(gnome_action("'toggle-maximize-vertically'\n"), DoubleClick::Fill);
        assert_eq!(gnome_action("'minimize'"), DoubleClick::Minimize);
        assert_eq!(gnome_action("'menu'"), DoubleClick::Menu);
        assert_eq!(gnome_action("'none'"), DoubleClick::Nothing);
        assert_eq!(gnome_action("'lower'"), DoubleClick::Nothing);
    }

    #[test]
    fn kwinrc_reads_only_the_windows_group() {
        let kwinrc = "[Desktops]\nTitlebarDoubleClickCommand=Minimize\n\n[Windows]\n# a note\nFocusPolicy=ClickToFocus\nTitlebarDoubleClickCommand=Nothing\n[Effect-blur]\n";
        assert_eq!(kwinrc_command(kwinrc).as_deref(), Some("Nothing"));
        assert_eq!(
            kwinrc_command("[Windows]\nTitlebarDoubleClickCommand[$i]=Operations menu\n").as_deref(),
            Some("Operations menu")
        );
        assert_eq!(kwinrc_command("[Windows]\nFocusPolicy=ClickToFocus\n"), None);
    }

    /// A temporary folder of fake config files, removed when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!("nexus-title-bar-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            Temp(dir)
        }

        fn write(&self, path: &str, text: &str) -> PathBuf {
            let file = self.0.join(path);
            std::fs::create_dir_all(file.parent().expect("parent")).expect("dirs");
            std::fs::write(&file, text).expect("write");
            file
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn env(vars: &[(&str, OsString)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let vars: HashMap<String, OsString> = vars.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        move |name| vars.get(name).cloned()
    }

    /// A `gsettings` that prints `value`, without writing a script: `sh -c` ignores the arguments after its name.
    fn fake_gsettings(value: &str) -> [String; 4] {
        [
            "sh".into(),
            "-c".into(),
            format!("printf '%s\\n' \"{value}\""),
            "gsettings".into(),
        ]
    }

    #[test]
    fn kde_reads_the_persons_kwinrc_before_the_systems() {
        let temp = Temp::new("kde");
        temp.write("home/kwinrc", "[Windows]\nTitlebarDoubleClickCommand=Minimize\n");
        temp.write("system/kwinrc", "[Windows]\nTitlebarDoubleClickCommand=Nothing\n");
        let no_gsettings = ["/nonexistent/gsettings"];
        let both = env(&[
            ("XDG_CURRENT_DESKTOP", "KDE".into()),
            ("XDG_CONFIG_HOME", temp.0.join("home").into()),
            ("XDG_CONFIG_DIRS", temp.0.join("system").into()),
        ]);
        assert_eq!(linux_with(&both, &no_gsettings), DoubleClick::Minimize);
        // The person's file without the key: the system's counts.
        temp.write("home/kwinrc", "[Windows]\nFocusPolicy=ClickToFocus\n");
        assert_eq!(linux_with(&both, &no_gsettings), DoubleClick::Nothing);
        // `~/.config` when XDG_CONFIG_HOME is unset; nothing set anywhere is KWin's default, Maximize.
        temp.write(
            "user/.config/kwinrc",
            "[Windows]\nTitlebarDoubleClickCommand=Operations menu\n",
        );
        let home = env(&[
            ("XDG_CURRENT_DESKTOP", "ubuntu:KDE".into()),
            ("HOME", temp.0.join("user").into()),
            ("XDG_CONFIG_DIRS", temp.0.join("none").into()),
        ]);
        assert_eq!(linux_with(&home, &no_gsettings), DoubleClick::Menu);
        let unset = env(&[
            ("XDG_CURRENT_DESKTOP", "KDE".into()),
            ("XDG_CONFIG_HOME", temp.0.join("none").into()),
            ("XDG_CONFIG_DIRS", temp.0.join("none").into()),
        ]);
        assert_eq!(linux_with(&unset, &no_gsettings), DoubleClick::Fill);
    }

    #[test]
    fn other_desktops_read_gnomes_key_through_gsettings() {
        let gnome = env(&[("XDG_CURRENT_DESKTOP", "GNOME".into())]);
        let minimize = fake_gsettings("'minimize'");
        let minimize: Vec<&str> = minimize.iter().map(String::as_str).collect();
        assert_eq!(linux_with(&gnome, &minimize), DoubleClick::Minimize);
        let none = fake_gsettings("'none'");
        let none: Vec<&str> = none.iter().map(String::as_str).collect();
        assert_eq!(linux_with(&env(&[]), &none), DoubleClick::Nothing);
        // No gsettings, or one that fails: the default.
        assert_eq!(linux_with(&gnome, &["/nonexistent/gsettings"]), DoubleClick::Fill);
        assert_eq!(
            linux_with(&gnome, &["sh", "-c", "exit 1", "gsettings"]),
            DoubleClick::Fill
        );
    }
}
