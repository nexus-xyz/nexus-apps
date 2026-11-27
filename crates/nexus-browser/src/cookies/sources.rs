//! The browser profiles on this computer whose cookies can be imported. A profile is named by an id the app makes
//! (`chrome:Default`); the webview never names a path.
use super::{Read, chromium, firefox};
use std::{
    io,
    path::{Path, PathBuf},
};

/// A Chromium-family browser: where its profiles live, and where its cookie key is.
pub struct Chromium {
    pub key: &'static str,
    pub label: &'static str,
    /// Under `~/.config` on Linux.
    pub linux: &'static str,
    /// The Flatpak's id: its profiles are under `~/.var/app/<id>/config/<linux>`.
    pub flatpak: &'static str,
    /// Under `~/Library/Application Support` on macOS.
    pub mac: &'static str,
    /// The Secret Service item's `application` attribute on Linux.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub secret: &'static str,
    /// KDE Wallet's entry on Linux, when the browser stored its key there: folder and key.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub kwallet: (&'static str, &'static str),
    /// The Keychain item on macOS: service and account.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub keychain: (&'static str, &'static str),
}
pub const CHROMIUM: [Chromium; 4] = [
    Chromium {
        key: "chrome",
        label: "Chrome",
        linux: "google-chrome",
        flatpak: "com.google.Chrome",
        mac: "Google/Chrome",
        secret: "chrome",
        kwallet: ("Chrome Keys", "Chrome Safe Storage"),
        keychain: ("Chrome Safe Storage", "Chrome"),
    },
    Chromium {
        key: "chromium",
        label: "Chromium",
        linux: "chromium",
        flatpak: "org.chromium.Chromium",
        mac: "Chromium",
        secret: "chromium",
        kwallet: ("Chromium Keys", "Chromium Safe Storage"),
        keychain: ("Chromium Safe Storage", "Chromium"),
    },
    Chromium {
        key: "brave",
        label: "Brave",
        linux: "BraveSoftware/Brave-Browser",
        flatpak: "com.brave.Browser",
        mac: "BraveSoftware/Brave-Browser",
        secret: "brave",
        kwallet: ("Brave Keys", "Brave Safe Storage"),
        keychain: ("Brave Safe Storage", "Brave"),
    },
    Chromium {
        key: "edge",
        label: "Edge",
        linux: "microsoft-edge",
        flatpak: "com.microsoft.Edge",
        mac: "Microsoft Edge",
        secret: "msedge",
        kwallet: ("Microsoft Edge Keys", "Microsoft Edge Safe Storage"),
        keychain: ("Microsoft Edge Safe Storage", "Microsoft Edge"),
    },
];

pub enum Kind {
    Chromium(&'static Chromium),
    Firefox,
}
pub struct Source {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// The profile's cookie database.
    pub database: PathBuf,
}
impl Source {
    /// The cookies of `sites` (checked by `super::sites`) in this profile.
    pub fn read(&self, sites: &[String]) -> Result<Read, String> {
        match self.kind {
            Kind::Chromium(browser) => chromium::read(&self.database, sites, &mut || chromium::os_key(browser)),
            Kind::Firefox => firefox::read(&self.database, sites),
        }
    }
}

/// Accessible profiles and failures encountered while looking for them. A denied read is not an empty profile list.
#[derive(Default)]
pub struct Discovery {
    pub sources: Vec<Source>,
    pub warnings: Vec<String>,
}
impl Discovery {
    fn warn(&mut self, browser: &str, error: io::Error) {
        // Browsers need not be installed, and profiles need not have created a cookie database yet.
        if error.kind() == io::ErrorKind::NotFound {
            return;
        }
        let message = if error.kind() == io::ErrorKind::PermissionDenied {
            if cfg!(target_os = "macos") {
                format!(
                    "macOS blocked access to {browser} profiles. Check Nexus in System Settings → Privacy & Security, then restart Nexus and try again. You can also sign in directly in this browser."
                )
            } else {
                format!(
                    "Nexus cannot access {browser} profiles. Check the browser folder's permissions, then try again. You can also sign in directly in this browser."
                )
            }
        } else {
            format!("Nexus could not read {browser} profiles. Try again, or sign in directly in this browser.")
        };
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    pub fn select(self, id: Option<&str>) -> Result<Source, String> {
        self.sources
            .into_iter()
            .find(|source| Some(source.id.as_str()) == id)
            .ok_or_else(|| {
                if self.warnings.is_empty() {
                    "Choose a browser profile.".into()
                } else {
                    self.warnings.join(" ")
                }
            })
    }
}

/// The profiles on this computer: under the person's home folder, and on Linux under their config folder
/// (`XDG_CONFIG_HOME` when it is an absolute path, as the browsers read it). `None` without a home folder.
pub fn discover_here() -> Option<Discovery> {
    let home = super::home()?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|config| config.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    Some(discover_with_config(&home, &config))
}

/// The profiles under `home`, with the config folder at `~/.config`.
pub fn discover(home: &Path) -> Discovery {
    discover_with_config(home, &home.join(".config"))
}

/// The profiles under `home` and `config` (the Linux config folder), Chromium-family browsers first. Windows has
/// none: its import is not supported.
pub fn discover_with_config(home: &Path, config: &Path) -> Discovery {
    let mut found = Discovery::default();
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return found;
    }
    for browser in &CHROMIUM {
        let mut roots = Vec::new();
        if cfg!(target_os = "macos") {
            roots.push((home.join("Library/Application Support").join(browser.mac), ""));
        } else {
            roots.push((config.join(browser.linux), ""));
            roots.push((
                home.join(".var/app")
                    .join(browser.flatpak)
                    .join("config")
                    .join(browser.linux),
                "-flatpak",
            ));
        }
        for (root, suffix) in roots {
            let profiles = chromium_profiles(&root, &mut |error| found.warn(browser.label, error));
            for (directory, name, database) in profiles {
                found.sources.push(Source {
                    id: format!("{}{suffix}:{directory}", browser.key),
                    name: format!(
                        "{}{} · {name}",
                        browser.label,
                        if suffix.is_empty() { "" } else { " (Flatpak)" }
                    ),
                    kind: Kind::Chromium(browser),
                    database,
                });
            }
        }
    }
    let firefox_roots = if cfg!(target_os = "macos") {
        vec![(home.join("Library/Application Support/Firefox"), "")]
    } else {
        // Firefox 147 and later keep profiles under the XDG config folder; older ones, and a profile not moved yet,
        // under `~/.mozilla`. A profile found in both is listed once, from the XDG folder.
        vec![
            (config.join("mozilla/firefox"), ""),
            (home.join(".mozilla/firefox"), ""),
            (home.join("snap/firefox/common/.config/mozilla/firefox"), "-snap"),
            (home.join("snap/firefox/common/.mozilla/firefox"), "-snap"),
            (
                home.join(".var/app/org.mozilla.firefox/config/mozilla/firefox"),
                "-flatpak",
            ),
            (home.join(".var/app/org.mozilla.firefox/.mozilla/firefox"), "-flatpak"),
        ]
    };
    for (root, suffix) in firefox_roots {
        let profiles = firefox_profiles(&root, &mut |error| found.warn("Firefox", error));
        for (directory, name, database) in profiles {
            // The same database reached through both folders (a link): listed once.
            let canonical = std::fs::canonicalize(&database).ok();
            if canonical.is_some()
                && found
                    .sources
                    .iter()
                    .any(|source| std::fs::canonicalize(&source.database).ok() == canonical)
            {
                continue;
            }
            // Another profile with the same folder name in the older folder keeps an id of its own.
            let mut id = format!("firefox{suffix}:{directory}");
            if found.sources.iter().any(|source| source.id == id) {
                id = format!("firefox{suffix}-legacy:{directory}");
            }
            found.sources.push(Source {
                id,
                name: format!("Firefox · {name}"),
                kind: Kind::Firefox,
                database,
            });
        }
    }
    found
}

/// A Chromium root's profiles: `Default` and `Profile N`, each with its name from `Local State`.
fn chromium_profiles(root: &Path, warn: &mut impl FnMut(io::Error)) -> Vec<(String, String, PathBuf)> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            warn(error);
            return Vec::new();
        }
    };
    // Display names are optional; an unreadable Local State does not make a cookie database unusable.
    let state: serde_json::Value = std::fs::read(root.join("Local State"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut profiles: Vec<(String, String, PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.map_err(&mut *warn).ok()?;
            let directory = entry.file_name().to_str()?.to_owned();
            if directory != "Default" && !directory.starts_with("Profile ") {
                return None;
            }
            // Chromium 96 moved the database into `Network/`.
            let database = cookie_database(
                [entry.path().join("Network/Cookies"), entry.path().join("Cookies")],
                warn,
            )?;
            let name = state["profile"]["info_cache"][&directory]["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&directory)
                .to_owned();
            Some((directory, name, database))
        })
        .collect();
    profiles.sort();
    profiles
}

/// A Firefox root's profiles from `profiles.ini`: `Name=` and `Path=` (relative to the root unless `IsRelative=0`).
fn firefox_profiles(root: &Path, warn: &mut impl FnMut(io::Error)) -> Vec<(String, String, PathBuf)> {
    let ini = match std::fs::read_to_string(root.join("profiles.ini")) {
        Ok(ini) => ini,
        Err(error) => {
            warn(error);
            return Vec::new();
        }
    };
    let mut profiles = Vec::new();
    let mut section: Option<Section> = None;
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            push(root, &mut profiles, section.take(), warn);
            section = line.starts_with("[Profile").then(|| Section {
                relative: true,
                ..Section::default()
            });
        } else if let Some(open) = section.as_mut()
            && let Some((key, value)) = line.split_once('=')
        {
            match key.trim() {
                "Name" => open.name = Some(value.trim().to_owned()),
                "Path" => open.path = Some(value.trim().to_owned()),
                "IsRelative" => open.relative = value.trim() != "0",
                _ => {}
            }
        }
    }
    push(root, &mut profiles, section, warn);
    profiles
}
/// One `[ProfileN]` section of `profiles.ini`.
#[derive(Default)]
struct Section {
    name: Option<String>,
    path: Option<String>,
    relative: bool,
}
fn push(
    root: &Path,
    profiles: &mut Vec<(String, String, PathBuf)>,
    section: Option<Section>,
    warn: &mut impl FnMut(io::Error),
) {
    let Some(Section {
        name: Some(name),
        path: Some(path),
        relative,
    }) = section
    else {
        return;
    };
    let directory = if relative {
        root.join(&path)
    } else {
        PathBuf::from(&path)
    };
    if let Some(database) = cookie_database([directory.join("cookies.sqlite")], warn) {
        profiles.push((path, name, database));
    }
}

/// `Path::is_file` hides access failures. Keep those failures while checking both Chromium database locations.
fn cookie_database(paths: impl IntoIterator<Item = PathBuf>, warn: &mut impl FnMut(io::Error)) -> Option<PathBuf> {
    for path in paths {
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => return Some(path),
            Ok(_) => {}
            Err(error) => warn(error),
        }
    }
    None
}

#[cfg(test)]
#[path = "sources_tests.rs"]
mod tests;
