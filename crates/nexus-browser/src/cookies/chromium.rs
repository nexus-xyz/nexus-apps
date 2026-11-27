//! Chromium's `Cookies` database. Values are AES-128-CBC under a key derived with PBKDF2-SHA1 (salt `saltysalt`)
//! from a password the OS keyring holds:
//! - Linux `v11`: the Secret Service item whose `application` is the browser's (`chrome`), one iteration.
//! - Linux `v10`: the fixed password `peanuts`, used when the browser found no keyring.
//! - macOS `v10`: the Keychain item `Chrome Safe Storage`, 1003 iterations.
//!
//! Some Linux clients encrypt with an empty password (crbug.com/1195256), so that key is tried too. From database
//! version 24, the plaintext starts with the SHA-256 of the cookie's host, which binds it to its domain.
use super::{Cookie, Read, SameSite, Snapshot, site_filter};
use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
use std::path::Path;

type Key = [u8; 16];
/// CBC's fixed IV: sixteen spaces.
const IV: [u8; 16] = [b' '; 16];
/// Seconds from 1601-01-01 (Chromium's epoch) to 1970-01-01.
const EPOCH: i64 = 11_644_473_600;

pub fn derive(password: &[u8], iterations: u32) -> Key {
    let mut key = [0; 16];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA1,
        std::num::NonZeroU32::new(iterations).unwrap_or(std::num::NonZeroU32::MIN),
        b"saltysalt",
        password,
        &mut key,
    );
    key
}

/// The key the OS keyring holds for `browser`: the Secret Service on Linux, the Keychain on macOS (which can ask the
/// person to allow it).
#[cfg(target_os = "linux")]
pub fn os_key(browser: &super::sources::Chromium) -> Result<Key, String> {
    use secret_service::{EncryptionType, blocking::SecretService};
    let missing = || {
        format!(
            "{}'s cookie key is not in this computer's keyring (GNOME Keyring or KDE Wallet), or the keyring is locked.",
            browser.label
        )
    };
    let service = SecretService::connect(EncryptionType::Dh).map_err(|_| missing())?;
    for attributes in key_searches(browser) {
        let Ok(found) = service.search_items(attributes) else {
            continue;
        };
        let Some(item) = found.unlocked.into_iter().chain(found.locked).next() else {
            continue;
        };
        item.unlock().map_err(|_| missing())?;
        let secret = item.get_secret().map_err(|_| missing())?;
        return Ok(derive(&secret, 1));
    }
    Err(missing())
}

/// Where the Secret Service may hold `browser`'s key: libsecret's item (GNOME Keyring, and KDE Wallet when the
/// browser used libsecret), then KDE Wallet's own entry, which KDE's Secret Service shows by its folder (`server`) and
/// key (`user`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn key_searches(browser: &super::sources::Chromium) -> [std::collections::HashMap<&'static str, &'static str>; 2] {
    let (folder, entry) = browser.kwallet;
    [
        std::collections::HashMap::from([("application", browser.secret)]),
        std::collections::HashMap::from([("server", folder), ("user", entry)]),
    ]
}
#[cfg(target_os = "macos")]
pub fn os_key(browser: &super::sources::Chromium) -> Result<Key, String> {
    let (service, account) = browser.keychain;
    let password = keyring::Entry::new(service, account)
        .and_then(|entry| entry.get_password())
        .map_err(|_| format!("Nexus could not read {}'s cookie key from the Keychain.", browser.label))?;
    Ok(derive(password.as_bytes(), 1003))
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn os_key(_: &super::sources::Chromium) -> Result<Key, String> {
    Err("Importing from Chromium browsers is not available on this system.".into())
}

/// Decrypts one `encrypted_value` of `host`'s cookie. `None` when no key opens it.
pub fn decrypt(blob: &[u8], host: &str, version: i64, keys: &[Key]) -> Option<String> {
    let payload = blob.strip_prefix(b"v10").or_else(|| blob.strip_prefix(b"v11"))?;
    keys.iter().find_map(|key| {
        let plain = cbc::Decryptor::<aes::Aes128>::new(&(*key).into(), &IV.into())
            .decrypt_padded_vec::<Pkcs7>(payload)
            .ok()?;
        let plain = if version >= 24 {
            let digest = ring::digest::digest(&ring::digest::SHA256, host.as_bytes());
            plain.strip_prefix(digest.as_ref())?.to_vec()
        } else {
            plain
        };
        String::from_utf8(plain).ok()
    })
}

/// The cookies of `sites` in the database at `path`. `os_key` is asked only when a row needs it.
pub fn read(path: &Path, sites: &[String], os_key: &mut dyn FnMut() -> Result<Key, String>) -> Result<Read, String> {
    let snapshot = Snapshot::of(path)?;
    let connection = snapshot.open()?;
    let unreadable = |_| "The browser's cookie file could not be read.".to_string();
    let version: i64 = connection
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(unreadable)?
        .parse()
        .map_err(|_| "The browser's cookie file has an unknown version.")?;
    let partitioned = connection
        .prepare("SELECT * FROM cookies LIMIT 0")
        .map_err(unreadable)?
        .column_names()
        .any(|name| name == "top_frame_site_key");
    let (condition, parameters) = site_filter("host_key", sites);
    let query = format!(
        "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, samesite, {} FROM cookies WHERE {condition}",
        if partitioned { "top_frame_site_key" } else { "''" }
    );
    let mut statement = connection.prepare(&query).map_err(unreadable)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(parameters), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, bool>(6)?,
                row.get::<_, bool>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .map_err(unreadable)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let mut read = Read::default();
    let mut keys: Option<Vec<Key>> = None;
    for row in rows {
        let (host, name, value, encrypted, path, expires, secure, http_only, same_site, partition) =
            row.map_err(unreadable)?;
        // The SQL filter already kept only these sites; this is the second check.
        if !super::matches(&host, sites) {
            continue;
        }
        let expires = (expires > 0).then(|| expires / 1_000_000 - EPOCH);
        if expires.is_some_and(|expires| expires <= now) {
            continue;
        }
        // A cookie partitioned by its top-level site (CHIPS) belongs to that embedding only.
        if !partition.is_empty() {
            read.skipped += 1;
            continue;
        }
        let value = if encrypted.is_empty() {
            Some(value)
        } else {
            let keys = keys.get_or_insert_with(|| {
                let mut found = Vec::new();
                match os_key() {
                    Ok(key) => found.push(key),
                    Err(warning) => read.warning = Some(warning),
                }
                if cfg!(not(target_os = "macos")) {
                    found.extend([derive(b"peanuts", 1), derive(b"", 1)]);
                }
                found
            });
            decrypt(&encrypted, &host, version, keys)
        };
        let Some(value) = value else {
            read.skipped += 1;
            continue;
        };
        read.cookies.push(Cookie {
            domain: host,
            name,
            value,
            path: if path.is_empty() { "/".into() } else { path },
            expires,
            secure,
            http_only,
            same_site: match same_site {
                0 => Some(SameSite::None),
                1 => Some(SameSite::Lax),
                2 => Some(SameSite::Strict),
                _ => None,
            },
        });
    }
    Ok(read)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use aes::cipher::BlockModeEncrypt;

    pub fn encrypt(plain: &[u8], key: &Key, prefix: &[u8]) -> Vec<u8> {
        let sealed = cbc::Encryptor::<aes::Aes128>::new(&(*key).into(), &IV.into()).encrypt_padded_vec::<Pkcs7>(plain);
        [prefix, &sealed].concat()
    }
    fn bound(host: &str, value: &str) -> Vec<u8> {
        [
            ring::digest::digest(&ring::digest::SHA256, host.as_bytes()).as_ref(),
            value.as_bytes(),
        ]
        .concat()
    }
    /// Microseconds since 1601 for a Unix time.
    fn chromium_time(unix: i64) -> i64 {
        (unix + EPOCH) * 1_000_000
    }

    /// A `Cookies` database in Chromium's format, version 24, with values under known keys.
    fn fixture(path: &Path, keyring: &Key) {
        let peanuts = derive(b"peanuts", 1);
        let connection = rusqlite::Connection::open(path).expect("fixture");
        connection
            .execute_batch(
                "CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
                 INSERT INTO meta VALUES('version', '24');
                 CREATE TABLE cookies(creation_utc INTEGER NOT NULL, host_key TEXT NOT NULL, top_frame_site_key TEXT NOT NULL,
                   name TEXT NOT NULL, value TEXT NOT NULL, encrypted_value BLOB NOT NULL, path TEXT NOT NULL,
                   expires_utc INTEGER NOT NULL, is_secure INTEGER NOT NULL, is_httponly INTEGER NOT NULL,
                   samesite INTEGER NOT NULL);",
            )
            .expect("schema");
        let later = chromium_time(4_000_000_000);
        // Host, name, value, encrypted value, expiry, SameSite, partition.
        type Row<'a> = (&'a str, &'a str, &'a str, Vec<u8>, i64, i64, &'a str);
        let rows: Vec<Row> = vec![
            (
                "github.com",
                "user_session",
                "",
                encrypt(&bound("github.com", "secret-1"), keyring, b"v11"),
                later,
                2,
                "",
            ),
            (
                ".github.com",
                "logged_in",
                "",
                encrypt(&bound(".github.com", "yes"), &peanuts, b"v10"),
                0,
                1,
                "",
            ),
            ("api.github.com", "plain", "visible", Vec::new(), later, -1, ""),
            (
                "github.com",
                "expired",
                "old",
                Vec::new(),
                chromium_time(1_000_000_000),
                1,
                "",
            ),
            (
                "github.com",
                "embedded",
                "",
                encrypt(&bound("github.com", "x"), keyring, b"v11"),
                later,
                0,
                "https://other.example",
            ),
            (
                "github.com",
                "wrong_key",
                "",
                encrypt(&bound("github.com", "x"), &[7; 16], b"v11"),
                later,
                0,
                "",
            ),
            // Outside the chosen sites: never read, so their unreadable values are not even counted.
            (
                "evilgithub.com",
                "stolen",
                "",
                b"v11 not a real ciphertext".to_vec(),
                later,
                0,
                "",
            ),
            (
                ".example.org",
                "other",
                "",
                b"v11 not a real ciphertext".to_vec(),
                later,
                0,
                "",
            ),
        ];
        for (host, name, value, encrypted, expires, same_site, partition) in rows {
            connection
                .execute(
                    "INSERT INTO cookies VALUES(0, ?1, ?2, ?3, ?4, ?5, '/', ?6, 1, 1, ?7)",
                    rusqlite::params![host, partition, name, value, encrypted, expires, same_site],
                )
                .expect("row");
        }
    }

    #[test]
    fn reads_and_decrypts_only_the_chosen_sites() {
        let folder = std::env::temp_dir().join(format!("nexus-chromium-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder).expect("folder");
        let path = folder.join("Cookies");
        let keyring = derive(b"test keyring password", 1);
        fixture(&path, &keyring);
        let mut asked = 0;
        let read = read(&path, &["github.com".into()], &mut || {
            asked += 1;
            Ok(keyring)
        })
        .expect("read");
        assert_eq!(asked, 1, "the keyring is asked once");
        let mut got: Vec<(&str, &str, &str)> = read
            .cookies
            .iter()
            .map(|cookie| (cookie.domain.as_str(), cookie.name.as_str(), cookie.value.as_str()))
            .collect();
        got.sort();
        if cfg!(target_os = "macos") {
            // macOS has no `peanuts` key: the v10 row is skipped there.
            assert_eq!(
                got,
                [
                    ("api.github.com", "plain", "visible"),
                    ("github.com", "user_session", "secret-1")
                ]
            );
            assert_eq!(read.skipped, 3);
        } else {
            assert_eq!(
                got,
                [
                    (".github.com", "logged_in", "yes"),
                    ("api.github.com", "plain", "visible"),
                    ("github.com", "user_session", "secret-1"),
                ]
            );
            // The partitioned row and the row under another key.
            assert_eq!(read.skipped, 2);
        }
        let session = read
            .cookies
            .iter()
            .find(|cookie| cookie.name == "user_session")
            .expect("session");
        assert_eq!(session.expires, Some(4_000_000_000));
        assert_eq!(session.same_site, Some(SameSite::Strict));
        assert!(session.secure && session.http_only);
        assert_eq!(
            read.cookies
                .iter()
                .find(|cookie| cookie.name == "plain")
                .expect("plain")
                .same_site,
            None
        );
        assert!(read.warning.is_none());
        std::fs::remove_dir_all(folder).expect("clean up");
    }

    #[test]
    fn a_missing_keyring_is_a_warning_and_plain_values_still_import() {
        let folder = std::env::temp_dir().join(format!("nexus-chromium-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder).expect("folder");
        let path = folder.join("Cookies");
        fixture(&path, &derive(b"test keyring password", 1));
        let read = read(&path, &["api.github.com".into()], &mut || Err("No keyring.".into())).expect("read");
        assert_eq!(read.cookies.len(), 1);
        assert!(read.warning.is_none(), "no encrypted row, so the keyring is not asked");
        let read = read_all(&path, &["github.com".into()]);
        assert_eq!(read.warning.as_deref(), Some("No keyring."));
        assert!(read.cookies.iter().all(|cookie| cookie.name != "user_session"));
        std::fs::remove_dir_all(folder).expect("clean up");
    }
    fn read_all(path: &Path, sites: &[String]) -> Read {
        read(path, sites, &mut || Err("No keyring.".into())).expect("read")
    }

    #[test]
    fn decrypts_with_the_right_key_and_the_domain_binding() {
        let key = derive(b"peanuts", 1);
        let empty = derive(b"", 1);
        assert_eq!(
            decrypt(&encrypt(b"old", &key, b"v10"), "a.com", 23, &[key]).as_deref(),
            Some("old")
        );
        assert_eq!(
            decrypt(&encrypt(b"old", &empty, b"v11"), "a.com", 23, &[key, empty]).as_deref(),
            Some("old")
        );
        assert_eq!(
            decrypt(&encrypt(&bound("a.com", "new"), &key, b"v10"), "a.com", 24, &[key]).as_deref(),
            Some("new")
        );
        // Bound to another host, or under another key: refused.
        assert_eq!(
            decrypt(&encrypt(&bound("b.com", "new"), &key, b"v10"), "a.com", 24, &[key]),
            None
        );
        assert_eq!(decrypt(&encrypt(b"old", &key, b"v10"), "a.com", 23, &[empty]), None);
        assert_eq!(decrypt(b"v20whatever", "a.com", 24, &[key]), None);
        // The Linux `peanuts` key is PBKDF2-HMAC-SHA1 with one iteration (Chromium's os_crypt_linux.cc).
        assert_eq!(
            key,
            [
                0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27, 0x27, 0x78
            ]
        );
    }
}
