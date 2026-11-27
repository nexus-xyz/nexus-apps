//! What a client shares with Gateway's base crate (`nexus-base`), as client-safe copies: the CLI's version order, the
//! chat image limits and three small helpers. The `nexus` CLI uses these instead of linking nexus-base, which would
//! pull Gateway's server crates into it. `tests/base_copies.rs` holds each copy to its original's
//! answers, recorded by nexus-base's `tests/client_copies.rs`.

/// CLI versions: `MAJOR.MINOR.PATCH[-prerelease][+build]`, e.g. `0.2.0+abc1234`. Order follows semver, the
/// build never counts, a prerelease comes before its release; the same as `nexus_base::version`, so "too old" means
/// the same thing to Core (426 `cli_too_old`) and to `nexus update`.
pub mod version {
    use std::cmp::Ordering;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Version {
        pub major: u64,
        pub minor: u64,
        pub patch: u64,
        pub prerelease: Vec<String>,
        pub build: Option<String>,
    }

    impl Version {
        /// A version, or `None` when the text is not one.
        pub fn parse(text: &str) -> Option<Version> {
            let text = text.trim();
            let text = text.strip_prefix('v').unwrap_or(text);
            let (rest, build) = match text.split_once('+') {
                Some((rest, build)) => (rest, Some(identifiers(build)?.to_string())),
                None => (text, None),
            };
            let (core, prerelease) = match rest.split_once('-') {
                Some((core, prerelease)) => (core, identifiers(prerelease)?.split('.').map(String::from).collect()),
                None => (rest, Vec::new()),
            };
            let mut numbers = core.split('.').map(number);
            let version = Version {
                major: numbers.next()??,
                minor: numbers.next()??,
                patch: numbers.next()??,
                prerelease,
                build,
            };
            numbers.next().is_none().then_some(version)
        }

        /// Semver order, ignoring the build.
        pub fn compare(&self, other: &Version) -> Ordering {
            let core = (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));
            if core != Ordering::Equal {
                return core;
            }
            match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
                (true, true) => return Ordering::Equal,
                (true, false) => return Ordering::Greater,
                (false, true) => return Ordering::Less,
                (false, false) => {}
            }
            for (x, y) in self.prerelease.iter().zip(&other.prerelease) {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(a), Ok(b)) => a.cmp(&b),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
            self.prerelease.len().cmp(&other.prerelease.len())
        }
    }

    /// A prerelease or build: one or more of `[0-9A-Za-z.-]`.
    fn identifiers(text: &str) -> Option<&str> {
        let valid = !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        valid.then_some(text)
    }

    /// `0` or a number with no leading zero.
    fn number(text: &str) -> Option<u64> {
        let valid =
            text == "0" || (!text.starts_with('0') && !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()));
        if valid { text.parse().ok() } else { None }
    }

    /// `a` compared with `b`; `None` if either is not a version.
    pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
        Some(Version::parse(a)?.compare(&Version::parse(b)?))
    }

    /// The version a native `nexus` binary declares: a `/* @nexus/cli <version> */` marker anywhere in the file (the
    /// Rust CLI keeps it as a static). `nexus update --rollback` reads it to name the versions it swaps.
    pub fn binary_version(code: &[u8]) -> Option<String> {
        let needle = b"/* @nexus/cli ";
        let mut from = 0;
        while let Some(at) = code[from..].windows(needle.len()).position(|window| window == needle) {
            let start = from + at + needle.len();
            if let Some(length) = code[start..].iter().take(64).position(|&byte| byte == b' ') {
                let end = start + length;
                if code[end..].starts_with(b" */") {
                    let found = String::from_utf8_lossy(&code[start..end]).to_string();
                    if Version::parse(&found).is_some() {
                        return Some(found);
                    }
                }
            }
            from = start;
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn orders_like_semver_and_ignores_builds() {
            let order = |a, b| compare_versions(a, b).expect("versions");
            assert_eq!(order("0.2.0+abc", "0.2.0+def"), Ordering::Equal);
            assert_eq!(order("0.1.9", "0.2.0"), Ordering::Less);
            assert_eq!(order("1.0.0-alpha", "1.0.0"), Ordering::Less);
            assert_eq!(order("1.0.0-alpha.1", "1.0.0-alpha.beta"), Ordering::Less);
            assert_eq!(order("1.0.0-beta.11", "1.0.0-beta.2"), Ordering::Greater);
            assert_eq!(order("1.0.0-alpha", "1.0.0-alpha.1"), Ordering::Less);
            assert!(compare_versions("1.0", "1.0.0").is_none());
            assert!(Version::parse("01.0.0").is_none());
            let mut binary = vec![0u8; 5000];
            binary.extend_from_slice(b"/* @nexus/cli nope */ /* @nexus/cli 0.3.0+abc1234 */");
            assert_eq!(binary_version(&binary).as_deref(), Some("0.3.0+abc1234"));
            assert_eq!(binary_version(b"no marker"), None);
        }
    }
}

/// The images a chat question may carry, as Gateway checks them (`nexus_base::images`).
pub mod images {
    /// Images per message.
    pub const MAX_IMAGES: usize = 8;
    /// Bytes per image: Anthropic's limit for one image.
    pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
    /// Bytes of images in one message.
    pub const MAX_TOTAL_BYTES: i64 = 20 * 1024 * 1024;
}

/// Lower-case hex SHA-256 of several parts, hashed in order (`nexus_base::crypto::sha256_hex`).
pub fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    for part in parts {
        context.update(part);
    }
    context
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `decodeURIComponent`, or `None` for a malformed escape or bytes that are not UTF-8 (`nexus_base::http`).
pub fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
