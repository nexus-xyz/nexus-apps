//! Sign-in cookies from the person's own browser, for a session browser's data store: which profiles there are, and
//! the chosen sites' cookies in one. The app writes them into its own browser (`src/side/browser/cookies.rs`).
//!
//! The person picks a browser profile and the sites to import. Only cookies of those sites (and their subdomains) are
//! read: the SQL query filters by host, so other rows are never decrypted. The browser's cookie database is copied
//! first (it is locked while the browser runs), and the copy is deleted when the import ends. Nothing is sent
//! anywhere, and no cookie name or value is logged.
//!
//! Supported: Chromium-family browsers (Chrome, Chromium, Brave, Edge) and Firefox, on Linux and macOS. Windows and
//! Safari are left out: Chrome's app-bound key on Windows and Safari's Full Disk Access need more than this.
mod chromium;
mod firefox;
pub mod sources;

use serde_json::Value;
use std::path::{Path, PathBuf};

/// The most sites one import names.
const MAX_SITES: usize = 20;

/// One cookie, as the browser stored it. `domain` keeps a domain cookie's leading dot (`.example.com`); a host-only
/// cookie has none.
#[derive(Clone, Debug, PartialEq)]
pub struct Cookie {
    pub domain: String,
    pub name: String,
    pub value: String,
    pub path: String,
    /// Unix seconds; `None` for a session cookie.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SameSite {
    None,
    Lax,
    Strict,
}
/// What one database gave: the cookies, and how many matching rows could not be used (not decryptable, or bound to
/// a partition or a container).
#[derive(Debug, Default)]
pub struct Read {
    pub cookies: Vec<Cookie>,
    pub skipped: usize,
    /// Why some rows could not be decrypted, when a key was missing.
    pub warning: Option<String>,
}

/// The sites the person typed, as bare lowercase domains: an address or `*.` prefix is reduced to its host. Each
/// names the domain and its subdomains.
pub fn sites(input: &Value) -> Result<Vec<String>, String> {
    let list = input.as_array().ok_or("Name the sites to import.")?;
    let mut sites = Vec::new();
    for entry in list {
        let text = entry
            .as_str()
            .ok_or("Name the sites to import.")?
            .trim()
            .to_ascii_lowercase();
        let host = text.split_once("://").map_or(text.as_str(), |(_, rest)| rest);
        let host = host.split(['/', '?', '#']).next().unwrap_or_default();
        let host = host
            .rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
            .map_or(host, |(host, _)| host);
        let host = host
            .trim_start_matches("*.")
            .trim_start_matches('.')
            .trim_end_matches('.');
        if host.is_empty() {
            continue;
        }
        let valid = host.len() <= 253
            && (host.contains('.') || host == "localhost")
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            });
        if !valid {
            return Err(format!("{host} is not a site. Enter domains like github.com."));
        }
        if !sites.iter().any(|site| site == host) {
            sites.push(host.to_owned());
        }
    }
    if sites.is_empty() {
        return Err("Name the sites to import.".into());
    }
    if sites.len() > MAX_SITES {
        return Err(format!("Import at most {MAX_SITES} sites at once."));
    }
    Ok(sites)
}

/// Whether a cookie's host (`example.com` or `.example.com`) is one of `sites` or under one.
pub fn matches(host: &str, sites: &[String]) -> bool {
    let host = host.trim_start_matches('.').to_ascii_lowercase();
    sites
        .iter()
        .any(|site| host == *site || host.strip_suffix(site.as_str()).is_some_and(|rest| rest.ends_with('.')))
}

/// The SQL condition on `column` that keeps only `sites`' cookies, and its parameters. Sites are checked by [`sites`]
/// (letters, digits, dots and dashes), so they hold no `LIKE` wildcard.
fn site_filter(column: &str, sites: &[String]) -> (String, Vec<String>) {
    let mut clauses = Vec::new();
    let mut parameters = Vec::new();
    for site in sites {
        clauses.push(format!("{column} = ? OR {column} = ? OR {column} LIKE ?"));
        parameters.extend([site.clone(), format!(".{site}"), format!("%.{site}")]);
    }
    (format!("({})", clauses.join(" OR ")), parameters)
}

/// A private copy of a browser's cookie database (and its write-ahead log), deleted when dropped.
struct Snapshot {
    directory: PathBuf,
    database: PathBuf,
}
impl Snapshot {
    fn of(database: &Path) -> Result<Self, String> {
        let directory = std::env::temp_dir().join(format!("nexus-cookies-{}", uuid::Uuid::new_v4()));
        let mut create = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut create, 0o700);
        create.create(&directory).map_err(|error| error.to_string())?;
        let snapshot = Snapshot {
            database: directory.join("cookies.sqlite"),
            directory,
        };
        std::fs::copy(database, &snapshot.database).map_err(|_| "The browser's cookie file could not be read.")?;
        for suffix in ["-wal", "-journal"] {
            let mut beside = database.as_os_str().to_owned();
            beside.push(suffix);
            let mut copy = snapshot.database.as_os_str().to_owned();
            copy.push(suffix);
            if Path::new(&beside).exists() {
                std::fs::copy(&beside, &copy).map_err(|_| "The browser's cookie file could not be read.")?;
            }
        }
        Ok(snapshot)
    }
    fn open(&self) -> Result<rusqlite::Connection, String> {
        rusqlite::Connection::open(&self.database).map_err(|_| "The browser's cookie file could not be opened.".into())
    }
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// The person's home folder, where browsers keep their profiles: `HOME`, when it is an absolute path.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sites_are_bare_domains_from_what_the_person_typed() {
        let parsed = sites(&json!([
            "GitHub.com",
            "https://app.example.com:8443/path?q=1",
            "*.atlassian.net",
            ".example.org.",
            "localhost:5173",
            "github.com",
            " ",
        ]))
        .expect("sites");
        assert_eq!(
            parsed,
            [
                "github.com",
                "app.example.com",
                "atlassian.net",
                "example.org",
                "localhost"
            ]
        );
        for wrong in [
            json!(["com"]),
            json!(["exa_mple.com"]),
            json!(["a%.com"]),
            json!(["-a.com"]),
            json!([]),
            json!("x"),
        ] {
            assert!(sites(&wrong).is_err(), "{wrong}");
        }
        let many: Vec<String> = (0..=MAX_SITES).map(|index| format!("site{index}.com")).collect();
        assert!(sites(&json!(many)).is_err());
    }

    #[test]
    fn a_site_takes_its_subdomains_and_nothing_else() {
        let sites = vec!["github.com".to_owned()];
        for host in ["github.com", ".github.com", "api.github.com", ".gist.github.com"] {
            assert!(matches(host, &sites), "{host}");
        }
        for host in ["evilgithub.com", "github.com.evil.net", "com", ".com", "hub.com"] {
            assert!(!matches(host, &sites), "{host}");
        }
        let (condition, parameters) = site_filter("host", &sites);
        assert_eq!(condition, "(host = ? OR host = ? OR host LIKE ?)");
        assert_eq!(parameters, ["github.com", ".github.com", "%.github.com"]);
    }

    #[test]
    fn a_snapshot_copies_the_log_and_leaves_nothing_behind() {
        let folder = std::env::temp_dir().join(format!("nexus-cookies-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder).expect("folder");
        let database = folder.join("Cookies");
        std::fs::write(&database, b"db").expect("db");
        std::fs::write(folder.join("Cookies-wal"), b"wal").expect("wal");
        let snapshot = Snapshot::of(&database).expect("snapshot");
        let copied = snapshot.directory.clone();
        assert_eq!(std::fs::read(&snapshot.database).expect("copy"), b"db");
        assert_eq!(
            std::fs::read(copied.join("cookies.sqlite-wal")).expect("wal copy"),
            b"wal"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&copied).expect("dir").permissions().mode() & 0o777,
                0o700
            );
        }
        drop(snapshot);
        assert!(!copied.exists());
        assert_eq!(std::fs::read(&database).expect("original"), b"db");
        std::fs::remove_dir_all(folder).expect("clean up");
    }
}
