//! Firefox's `cookies.sqlite`: plain values in `moz_cookies`. From schema 16 (`PRAGMA user_version`), `expiry` is in
//! milliseconds. A cookie of a container or a partition (`originAttributes` set) belongs to that context only.
use super::{Cookie, Read, SameSite, Snapshot, site_filter};
use std::path::Path;

pub fn read(path: &Path, sites: &[String]) -> Result<Read, String> {
    let snapshot = Snapshot::of(path)?;
    let connection = snapshot.open()?;
    let unreadable = |_| "Firefox's cookie file could not be read.".to_string();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(unreadable)?;
    let (condition, parameters) = site_filter("host", sites);
    let mut statement = connection
        .prepare(&format!(
            "SELECT host, name, value, path, expiry, isSecure, isHttpOnly, sameSite, originAttributes FROM moz_cookies WHERE {condition}"
        ))
        .map_err(unreadable)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(parameters), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, bool>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(unreadable)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let mut read = Read::default();
    for row in rows {
        let (host, name, value, path, expiry, secure, http_only, same_site, origin) = row.map_err(unreadable)?;
        if !super::matches(&host, sites) {
            continue;
        }
        let expires = (expiry > 0).then_some(if version >= 16 { expiry / 1000 } else { expiry });
        if expires.is_some_and(|expires| expires <= now) {
            continue;
        }
        if !origin.is_empty() {
            read.skipped += 1;
            continue;
        }
        read.cookies.push(Cookie {
            domain: host,
            name,
            value,
            path: if path.is_empty() { "/".into() } else { path },
            expires,
            secure,
            http_only,
            // Unset (256) and unknown values stay unspecified: None would allow cross-site use.
            same_site: match same_site {
                Some(0) => Some(SameSite::None),
                Some(1) => Some(SameSite::Lax),
                Some(2) => Some(SameSite::Strict),
                _ => None,
            },
        });
    }
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(path: &Path, version: i64, expiry: i64) {
        let connection = rusqlite::Connection::open(path).expect("fixture");
        connection
            .execute_batch(&format!(
                "PRAGMA user_version = {version};
                 CREATE TABLE moz_cookies(id INTEGER PRIMARY KEY, originAttributes TEXT NOT NULL DEFAULT '',
                   name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, lastAccessed INTEGER,
                   creationTime INTEGER, isSecure INTEGER, isHttpOnly INTEGER, inBrowserElement INTEGER DEFAULT 0,
                   sameSite INTEGER DEFAULT 0, rawSameSite INTEGER DEFAULT 0, schemeMap INTEGER DEFAULT 0);"
            ))
            .expect("schema");
        for (origin, name, host, same_site) in [
            ("", "session", ".gitlab.com", 1),
            ("", "csrf", "gitlab.com", 256),
            ("^userContextId=2", "container", "gitlab.com", 0),
            ("", "other", "notgitlab.com", 0),
            ("", "far", "example.net", 2),
        ] {
            connection
                .execute(
                    "INSERT INTO moz_cookies(originAttributes, name, value, host, path, expiry, isSecure, isHttpOnly, sameSite)
                     VALUES(?1, ?2, 'value', ?3, '/', ?4, 1, 0, ?5)",
                    rusqlite::params![origin, name, host, expiry, same_site],
                )
                .expect("row");
        }
    }

    #[test]
    fn reads_only_the_chosen_sites_outside_containers() {
        for (version, expiry) in [(15, 4_000_000_000_i64), (16, 4_000_000_000_000)] {
            let folder = std::env::temp_dir().join(format!("nexus-firefox-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&folder).expect("folder");
            let path = folder.join("cookies.sqlite");
            fixture(&path, version, expiry);
            let read = read(&path, &["gitlab.com".into()]).expect("read");
            let names: Vec<&str> = read.cookies.iter().map(|cookie| cookie.name.as_str()).collect();
            assert_eq!(names, ["session", "csrf"], "schema {version}");
            assert_eq!(read.skipped, 1);
            assert_eq!(read.cookies[0].domain, ".gitlab.com");
            assert_eq!(read.cookies[0].expires, Some(4_000_000_000), "schema {version}");
            assert_eq!(read.cookies[0].same_site, Some(SameSite::Lax));
            assert_eq!(read.cookies[1].same_site, None);
            std::fs::remove_dir_all(folder).expect("clean up");
        }
    }

    #[test]
    fn expired_cookies_stay_behind() {
        let folder = std::env::temp_dir().join(format!("nexus-firefox-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder).expect("folder");
        let path = folder.join("cookies.sqlite");
        fixture(&path, 16, 1_000_000_000_000);
        assert!(read(&path, &["gitlab.com".into()]).expect("read").cookies.is_empty());
        std::fs::remove_dir_all(folder).expect("clean up");
    }
}
