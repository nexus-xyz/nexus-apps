//! The plugins folder, `<config>/plugins/<id>/`. Install copies a plugin's folder in, under a
//! temporary name, and renames it into place, so a failed install leaves the installed version as it was. A symbolic
//! link inside a plugin is refused: it could point the plugin's files outside its folder.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use nexus_workbench::plugins::{MANIFEST_FILE, Manifest, ManifestError};

/// One installed plugin: its manifest and its folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub manifest: Manifest,
    pub folder: PathBuf,
}

#[derive(Debug)]
pub enum InstallError {
    /// The folder has no `nexus-plugin.json`.
    NoManifest,
    /// The manifest breaks a rule.
    Manifest(ManifestError),
    /// Its id is a Settings MCP server's name: the two would share tool names.
    NameTaken(String),
    /// A symbolic link at this path inside the plugin.
    Link(PathBuf),
    Io(io::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::NoManifest => write!(f, "the folder has no {MANIFEST_FILE}"),
            InstallError::Manifest(error) => write!(f, "{MANIFEST_FILE}: {error}"),
            InstallError::NameTaken(id) => write!(f, "an MCP server in Settings is already named {id}"),
            InstallError::Link(path) => write!(f, "{} is a symbolic link, which a plugin may not hold", path.display()),
            InstallError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<io::Error> for InstallError {
    fn from(error: io::Error) -> InstallError {
        InstallError::Io(error)
    }
}

/// The manifest in `folder`.
pub fn read(folder: &Path) -> Result<Manifest, InstallError> {
    let text = match std::fs::read_to_string(folder.join(MANIFEST_FILE)) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(InstallError::NoManifest),
        Err(error) => return Err(error.into()),
    };
    Manifest::parse(&text).map_err(InstallError::Manifest)
}

/// Copies the plugin in `source` into `plugins_dir`, replacing an installed version of the same id. `taken` are the
/// names of Settings' MCP servers. The approval is the caller's: a new install starts unapproved.
pub fn install(plugins_dir: &Path, source: &Path, taken: &[String]) -> Result<Installed, InstallError> {
    let manifest = read(source)?;
    if taken.contains(&manifest.id) {
        return Err(InstallError::NameTaken(manifest.id));
    }
    std::fs::create_dir_all(plugins_dir)?;
    let staging = beside(plugins_dir, &manifest.id, "installing");
    if let Err(error) = copy_folder(source, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    let folder = plugins_dir.join(&manifest.id);
    let previous = beside(plugins_dir, &manifest.id, "previous");
    let had_previous = folder.exists();
    if had_previous {
        std::fs::rename(&folder, &previous)?;
    }
    if let Err(error) = std::fs::rename(&staging, &folder) {
        if had_previous {
            let _ = std::fs::rename(&previous, &folder);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error.into());
    }
    if had_previous {
        let _ = std::fs::remove_dir_all(&previous);
    }
    Ok(Installed { manifest, folder })
}

/// Every plugin in `plugins_dir`, in id order, and each folder whose manifest does not read, with why.
pub fn list(plugins_dir: &Path) -> (Vec<Installed>, Vec<(PathBuf, InstallError)>) {
    let mut installed = Vec::new();
    let mut broken = Vec::new();
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return (installed, broken);
    };
    for entry in entries.flatten() {
        let folder = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if hidden || !folder.is_dir() {
            continue;
        }
        match read(&folder) {
            // A folder renamed by hand does not become another plugin.
            Ok(manifest) if folder.file_name() == Some(manifest.id.as_ref()) => {
                installed.push(Installed { manifest, folder })
            }
            Ok(manifest) => broken.push((
                folder,
                InstallError::Manifest(ManifestError(vec![format!(
                    "the folder is not named after its id {}",
                    manifest.id
                )])),
            )),
            Err(error) => broken.push((folder, error)),
        }
    }
    installed.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    broken.sort_by(|a, b| a.0.cmp(&b.0));
    (installed, broken)
}

/// Removes the plugin `id`, and answers its secrets' names so the caller deletes their values from the keychain.
pub fn remove(plugins_dir: &Path, id: &str) -> io::Result<Vec<String>> {
    // An id names one folder inside `plugins_dir`, never a path out of it.
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("no plugin id {id:?}"),
        ));
    }
    let folder = plugins_dir.join(id);
    let secrets = read(&folder).map(|manifest| manifest.secrets).unwrap_or_default();
    match std::fs::remove_dir_all(&folder) {
        Ok(()) => Ok(secrets),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(secrets),
        Err(error) => Err(error),
    }
}

/// A hidden name in `plugins_dir` for `id`'s install in progress or its previous version, fresh each time.
fn beside(plugins_dir: &Path, id: &str, what: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    plugins_dir.join(format!(".{id}.{what}-{}-{n}", std::process::id()))
}

/// Copies `from` into the new folder `to`, refusing a symbolic link anywhere inside.
fn copy_folder(from: &Path, to: &Path) -> Result<(), InstallError> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_symlink() {
            return Err(InstallError::Link(entry.path()));
        } else if kind.is_dir() {
            copy_folder(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A fresh, empty folder for one test.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-plugins-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// A plugin folder in `root` with the manifest of `id` at `version`, and one program file.
    fn plugin(root: &Path, id: &str, version: &str) -> PathBuf {
        let folder = root.join(format!("source-{id}-{version}"));
        std::fs::create_dir_all(folder.join("bin")).expect("folder");
        let manifest = json!({
            "apiVersion": 0, "id": id, "name": "Acme Notes", "version": version,
            "mcp": { "command": "bin/server" }, "secrets": ["ACME_TOKEN"],
        });
        std::fs::write(folder.join(MANIFEST_FILE), manifest.to_string()).expect("manifest");
        std::fs::write(folder.join("bin/server"), version).expect("program");
        folder
    }

    #[test]
    fn install_copies_the_plugin_and_list_finds_it() {
        let root = scratch("install");
        let plugins = root.join("plugins");
        let installed = install(&plugins, &plugin(&root, "acme-notes", "1.0.0"), &[]).expect("installed");
        assert_eq!(installed.folder, plugins.join("acme-notes"));
        assert_eq!(
            std::fs::read_to_string(installed.folder.join("bin/server")).expect("copied"),
            "1.0.0"
        );
        let (listed, broken) = list(&plugins);
        assert_eq!(listed, [installed]);
        assert!(broken.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_new_version_replaces_the_old_one_whole() {
        let root = scratch("update");
        let plugins = root.join("plugins");
        install(&plugins, &plugin(&root, "acme-notes", "1.0.0"), &[]).expect("first");
        std::fs::write(plugins.join("acme-notes/stale.txt"), "old").expect("stale file");
        let second = install(&plugins, &plugin(&root, "acme-notes", "1.1.0"), &[]).expect("second");
        assert_eq!(second.manifest.version, "1.1.0");
        assert!(
            !second.folder.join("stale.txt").exists(),
            "nothing of the old version is left"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&plugins)
            .expect("dir")
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(leftovers, ["acme-notes"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_bad_manifest_or_a_taken_name_installs_nothing() {
        let root = scratch("refused");
        let plugins = root.join("plugins");
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).expect("empty");
        assert!(matches!(install(&plugins, &empty, &[]), Err(InstallError::NoManifest)));
        let bad = plugin(&root, "acme-notes", "1.0.0");
        std::fs::write(
            bad.join(MANIFEST_FILE),
            json!({ "apiVersion": 0, "id": "X" }).to_string(),
        )
        .expect("bad");
        assert!(matches!(install(&plugins, &bad, &[]), Err(InstallError::Manifest(_))));
        let source = plugin(&root, "acme-notes", "2.0.0");
        let taken = install(&plugins, &source, &["acme-notes".into()]);
        assert!(matches!(taken, Err(InstallError::NameTaken(id)) if id == "acme-notes"));
        assert!(list(&plugins).0.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_inside_a_plugin_is_refused_and_the_installed_version_stays() {
        let root = scratch("link");
        let plugins = root.join("plugins");
        install(&plugins, &plugin(&root, "acme-notes", "1.0.0"), &[]).expect("first");
        let linked = plugin(&root, "acme-notes", "1.1.0");
        std::os::unix::fs::symlink("/etc/passwd", linked.join("bin/passwd")).expect("link");
        assert!(matches!(install(&plugins, &linked, &[]), Err(InstallError::Link(_))));
        let (listed, _) = list(&plugins);
        assert_eq!(listed[0].manifest.version, "1.0.0");
        assert_eq!(
            std::fs::read_dir(&plugins).expect("dir").count(),
            1,
            "no staging folder is left"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn list_reports_a_broken_folder_and_skips_hidden_ones() {
        let root = scratch("list");
        let plugins = root.join("plugins");
        install(&plugins, &plugin(&root, "acme-notes", "1.0.0"), &[]).expect("installed");
        std::fs::create_dir_all(plugins.join("broken")).expect("broken");
        std::fs::create_dir_all(plugins.join(".acme-notes.installing-1-0")).expect("hidden");
        std::fs::rename(plugins.join("acme-notes"), plugins.join("renamed")).expect("renamed");
        let (listed, broken) = list(&plugins);
        assert!(listed.is_empty());
        let names: Vec<_> = broken
            .iter()
            .map(|(folder, _)| folder.file_name().expect("name").to_owned())
            .collect();
        assert_eq!(names, ["broken", "renamed"]);
        assert!(matches!(broken[0].1, InstallError::NoManifest));
        assert!(list(&root.join("missing")).0.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn remove_deletes_the_folder_and_names_its_secrets() {
        let root = scratch("remove");
        let plugins = root.join("plugins");
        install(&plugins, &plugin(&root, "acme-notes", "1.0.0"), &[]).expect("installed");
        assert_eq!(remove(&plugins, "acme-notes").expect("removed"), ["ACME_TOKEN"]);
        assert!(!plugins.join("acme-notes").exists());
        assert!(remove(&plugins, "acme-notes").expect("again").is_empty());
        for id in ["..", "../plugins", "a/b", ""] {
            assert!(remove(&plugins, id).is_err(), "{id}");
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
