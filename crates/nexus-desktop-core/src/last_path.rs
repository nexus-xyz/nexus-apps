//! The page the app window opens at launch: the last one the person was on, when it is in the organization they are
//! signed in to, else the home. Nexus GPUI keeps that page in its settings file, and picks it with [`start_page`]. The
//! paths are Nexus UI's: `/app/<org>/<page>`, with
//! [`split_org_path`] and [`app_path`] as `ui/src/lib/org-path.ts` has them.

/// The first segments after `/app` that are Nexus UI's own pages, never an organization (`RESERVED_SEGMENTS`).
pub const RESERVED_SEGMENTS: [&str; 10] = [
    "login",
    "sign-in",
    "sign-up",
    "select-org",
    "device",
    "api",
    "settings",
    "continue",
    "invite",
    "approvals",
];

/// `/app/<org>` and its `page` (`appPath`): the organization percent-encoded, a page of `""` or `"/"` the
/// organization's root, a page with no leading `/` given one.
pub fn app_path(org: &str, page: &str) -> String {
    let base = format!("/app/{}", encode_component(org));
    if page.is_empty() || page == "/" {
        return base;
    }
    if page.starts_with('?') || page.starts_with('#') || page.starts_with('/') {
        format!("{base}{page}")
    } else {
        format!("{base}/{page}")
    }
}

/// The organization a Nexus UI pathname is in and the page within it (`splitOrgPath`): `/app/acme/spend` is
/// `("acme", "/spend")`. None for `/app` itself and the non-organization pages.
pub fn split_org_path(pathname: &str) -> Option<(String, String)> {
    let rest = pathname.strip_prefix("/app/")?;
    let (segment, page) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    if segment.is_empty() || RESERVED_SEGMENTS.contains(&segment) {
        return None;
    }
    let org = decode_component(segment).unwrap_or_else(|| segment.to_string());
    let page = if page.is_empty() || page == "/" {
        "/".to_string()
    } else {
        let trimmed = page.trim_end_matches('/');
        if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        }
    };
    Some((org, page))
}

/// The page to open at launch from the remembered path `last` (its query kept), for the organization `org`: the
/// remembered page when it is in `org`, else None (the home). As `startPath`: the organization is read from the path
/// without its query.
pub fn start_page(last: &str, org: &str) -> Option<String> {
    let (path, query) = match last.find('?') {
        Some(at) => (&last[..at], &last[at..]),
        None => (last, ""),
    };
    let (in_org, page) = split_org_path(path)?;
    (in_org == org).then(|| format!("{page}{query}"))
}

/// `encodeURIComponent`: every byte but ASCII letters, digits and `-_.!~*'()` as `%XX`.
fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `decodeURIComponent`: None when an escape is broken or the bytes are not UTF-8 (the caller keeps it as written).
fn decode_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            if at + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // `ui/src/lib/org-path.test.ts`, "appPath".
    #[test]
    fn app_path_puts_every_page_under_the_organization() {
        assert_eq!(app_path("acme", ""), "/app/acme");
        assert_eq!(app_path("acme", "/"), "/app/acme");
        assert_eq!(app_path("acme", "/spend"), "/app/acme/spend");
        assert_eq!(app_path("acme", "spend"), "/app/acme/spend");
        assert_eq!(app_path("acme", "/teams/platform"), "/app/acme/teams/platform");
        assert_eq!(
            app_path("acme", "/overview?session=s_1#agent-sessions"),
            "/app/acme/overview?session=s_1#agent-sessions"
        );
        assert_eq!(app_path("a b", "/spend"), "/app/a%20b/spend");
    }

    // `ui/src/lib/org-path.test.ts`, "splitOrgPath".
    #[test]
    fn split_org_path_finds_the_organization_and_the_page() {
        let split = |path: &str| split_org_path(path);
        assert_eq!(split("/app/acme"), Some(("acme".into(), "/".into())));
        assert_eq!(split("/app/acme/"), Some(("acme".into(), "/".into())));
        assert_eq!(split("/app/acme/spend"), Some(("acme".into(), "/spend".into())));
        assert_eq!(
            split("/app/acme/teams/platform/"),
            Some(("acme".into(), "/teams/platform".into()))
        );
        assert_eq!(split("/app/a%20b/spend"), Some(("a b".into(), "/spend".into())));
        assert_eq!(split("/app"), None);
        assert_eq!(split("/docs"), None);
        for segment in RESERVED_SEGMENTS {
            assert_eq!(split(&format!("/app/{segment}")), None);
        }
        assert_eq!(split("/app/sign-in/factor-one"), None);
        // A broken escape keeps the segment as written.
        assert_eq!(split("/app/a%2/spend"), Some(("a%2".into(), "/spend".into())));
    }

    // `startPath`: the last page only in the organization signed in to.
    #[test]
    fn the_last_page_opens_only_in_its_organization() {
        assert_eq!(start_page("/app/acme/code/s_1", "acme").as_deref(), Some("/code/s_1"));
        assert_eq!(
            start_page("/app/acme/models?tab=providers", "acme").as_deref(),
            Some("/models?tab=providers")
        );
        assert_eq!(start_page("/app/acme", "acme").as_deref(), Some("/"));
        assert_eq!(start_page("/app/acme/code/s_1", "beta"), None);
        // Signed out, the window was on the sign-in page: no organization, the home.
        assert_eq!(start_page("/app/login", "acme"), None);
        assert_eq!(start_page("", "acme"), None);
    }
}
