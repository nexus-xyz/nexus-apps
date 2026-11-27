//! `If-Match` on an edit of an editable resource. Gateway refuses such an edit without `If-Match` (428
//! `if_match_required`) once `NEXUS_IF_MATCH_MODE=enforce`, and a stale one with 412 `version_mismatch`.
//!
//! Each client keeps the `ETag` of the last `GET` of an editable path, and sends it as `If-Match` on that path's edit.
//! So an edit carries the version its caller last read: Nexus Desktop's screens show what those reads answered, and an
//! edit made elsewhere since then is refused, not overwritten. When it never read the path (a one-shot CLI command), or
//! a write under the path may have changed its version (a chat message), the client reads it first. A path read without
//! an `ETag` (a Gateway that does not version it yet) is edited without `If-Match` and not read again. A caller that
//! sends its own `If-Match` keeps it. A connector grant set is versioned with the organization's grant list, so its
//! version is the list's ([`READ_AT`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use reqwest::Method;

/// The edits that need `If-Match`. It mirrors Gateway's own list and the API contract's `x-if-match: required`.
/// Each path's `GET` serves the `ETag`.
// Known gap: a copied list; derive it from the published contract once the contract marks `x-if-match`.
pub(crate) const IF_MATCH_REQUIRED: &[(&str, &str)] = &[
    ("PUT", "/v1/agents/access"),
    ("PATCH", "/v1/chat/sessions/{id}"),
    ("DELETE", "/v1/chat/sessions/{id}"),
    ("PUT", "/v1/notifications/settings"),
    ("PUT", "/v1/orgs/settings"),
    ("PUT", "/v1/people/{userId}/budget"),
    ("PUT", "/v1/policies/agents/{agent}"),
    ("PUT", "/v1/policies/chat-writes"),
    ("PUT", "/v1/policies/grants/{product}/{capability}"),
    ("PUT", "/v1/policies/organization"),
    ("PUT", "/v1/policies/review-events"),
    ("PUT", "/v1/policies/teams/{slug}"),
    ("PUT", "/v1/presence/settings"),
    ("PATCH", "/v1/teams/{slug}"),
    ("DELETE", "/v1/teams/{slug}"),
];

/// The edits whose `ETag` another path's `GET` sends: a connector grant set takes the version of the organization's
/// grants, which their list sends. Every edit of the pattern shares the list's version.
const READ_AT: &[(&str, &str)] = &[("/v1/policies/grants/{product}/{capability}", "/v1/policies/grants")];

/// The path without its query: the resource an `ETag` belongs to.
fn resource(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or(path)
}

/// The path whose `GET` sends the `ETag` an edit of `path` needs: the list it is versioned with ([`READ_AT`]), else
/// its own resource. The client keeps each version under it.
pub(crate) fn read_path(path: &str) -> &str {
    READ_AT
        .iter()
        .find(|(edit, _)| fills(edit, path))
        .map_or_else(|| resource(path), |(_, read)| *read)
}

/// Whether `path` fills `pattern`, where `{name}` stands for one non-empty segment.
fn fills(pattern: &str, path: &str) -> bool {
    let mut want = pattern.split('/');
    let mut have = resource(path).split('/');
    loop {
        match (want.next(), have.next()) {
            (None, None) => return true,
            (Some(w), Some(h)) if w.starts_with('{') => {
                if h.is_empty() {
                    return false;
                }
            }
            (Some(w), Some(h)) if w == h => {}
            _ => return false,
        }
    }
}

/// Whether this call edits an editable resource, so it needs `If-Match`.
pub(crate) fn needs_if_match(method: &Method, path: &str) -> bool {
    IF_MATCH_REQUIRED
        .iter()
        .any(|(verb, pattern)| method.as_str() == *verb && fills(pattern, path))
}

/// Whether some edit of `path` needs `If-Match`, so its `ETag` is worth keeping.
fn editable(path: &str) -> bool {
    IF_MATCH_REQUIRED.iter().any(|(_, pattern)| fills(pattern, path))
        || READ_AT.iter().any(|(_, read)| *read == resource(path))
}

/// An answer's `ETag`, whatever the case of the header's name.
pub(crate) fn etag_of(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
        .map(|(_, value)| value.as_str())
}

/// Whether the caller already sends `If-Match`.
pub(crate) fn has_if_match(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("if-match"))
}

/// What a client knows of an editable path's version: unknown (absent), the last `ETag` read, or read with none
/// (`None`: a Gateway that does not version it yet, so an edit need not read it again).
#[derive(Debug, Clone, Default)]
pub(crate) struct Etags(Arc<Mutex<HashMap<String, Option<String>>>>);

impl Etags {
    /// `None`: never read. `Some(None)`: read, with no `ETag`.
    pub(crate) fn get(&self, path: &str) -> Option<Option<String>> {
        self.0.lock().ok()?.get(read_path(path)).cloned()
    }

    /// Learn from an answer to `method path`. A read's `ETag`, or its lack, is the version now, and so is a
    /// successful edit's `ETag`. A refused version, a deleted resource, or an edit that answers no `ETag` over a known
    /// one is forgotten, so the next edit reads again.
    pub(crate) fn learn(&self, method: &Method, path: &str, status: u16, etag: Option<&str>) {
        if !editable(path) {
            return;
        }
        let Ok(mut known) = self.0.lock() else { return };
        let key = read_path(path).to_string();
        let ok = (200..300).contains(&status);
        let read = *method == Method::GET;
        if status == 412 || (ok && *method == Method::DELETE) || (ok && !read && etag.is_none()) {
            known.remove(&key);
        } else if ok && (read || etag.is_some()) {
            known.insert(key, etag.map(String::from));
        }
    }

    /// Before a write to `path`: forget the version of each editable resource `path` is under. A write under a
    /// resource can change its version: a message (`POST /v1/chat/sessions/{id}/messages`) raises its chat session's
    /// version, so a rename after it would send a stale `If-Match` and get 412.
    pub(crate) fn touch(&self, method: &Method, path: &str) {
        if *method == Method::GET || *method == Method::HEAD {
            return;
        }
        let path = resource(path);
        if let Ok(mut known) = self.0.lock() {
            known.retain(|key, _| !path.starts_with(&format!("{key}/")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_fifteen_edits_need_if_match() {
        assert!(needs_if_match(&Method::PATCH, "/v1/chat/sessions/s1"));
        assert!(needs_if_match(&Method::PUT, "/v1/policies/grants/robustness/review"));
        assert!(!needs_if_match(&Method::PUT, "/v1/policies/grants/robustness"));
        assert!(needs_if_match(&Method::DELETE, "/v1/teams/web%2Fa"));
        assert!(needs_if_match(&Method::PUT, "/v1/people/me/budget"));
        assert!(needs_if_match(&Method::PUT, "/v1/orgs/settings?x=1"));
        // A sub-resource, another verb, or an empty segment is not one of them.
        assert!(!needs_if_match(&Method::POST, "/v1/chat/sessions/s1/messages"));
        assert!(!needs_if_match(&Method::DELETE, "/v1/chat/sessions/s1/share"));
        assert!(!needs_if_match(&Method::GET, "/v1/orgs/settings"));
        assert!(!needs_if_match(&Method::POST, "/v1/teams"));
        assert!(!needs_if_match(&Method::PATCH, "/v1/teams/"));
        assert!(!needs_if_match(&Method::PUT, "/v1/billing/auto-topup"));
    }

    /// Every connector grant set takes the version of the organization's grant list, read at the list.
    #[test]
    fn a_grant_set_takes_the_version_of_the_grant_list() {
        let etags = Etags::default();
        assert_eq!(
            read_path("/v1/policies/grants/robustness/review"),
            "/v1/policies/grants"
        );
        assert_eq!(read_path("/v1/orgs/settings?x=1"), "/v1/orgs/settings");
        etags.learn(&Method::GET, "/v1/policies/grants", 200, Some("\"3\""));
        let version = |path| etags.get(path).flatten();
        assert_eq!(
            version("/v1/policies/grants/robustness/review").as_deref(),
            Some("\"3\"")
        );
        // A set of one module answers the version every module's next set needs.
        etags.learn(
            &Method::PUT,
            "/v1/policies/grants/robustness/review",
            200,
            Some("\"4\""),
        );
        assert_eq!(
            version("/v1/policies/grants/intelligence/wiki").as_deref(),
            Some("\"4\"")
        );
        etags.learn(
            &Method::PUT,
            "/v1/policies/grants/intelligence/wiki",
            412,
            Some("\"5\""),
        );
        assert_eq!(etags.get("/v1/policies/grants/robustness/review"), None);
    }

    #[test]
    fn keeps_the_last_version_of_an_editable_path() {
        let etags = Etags::default();
        let version = |path| etags.get(path).flatten();
        etags.learn(&Method::GET, "/v1/orgs/settings", 200, Some("\"3\""));
        assert_eq!(version("/v1/orgs/settings?fresh=1").as_deref(), Some("\"3\""));
        // A successful edit answers the new version; one with no `ETag` leaves it unknown, so the next edit reads.
        etags.learn(&Method::PUT, "/v1/orgs/settings", 200, Some("\"4\""));
        assert_eq!(version("/v1/orgs/settings").as_deref(), Some("\"4\""));
        etags.learn(&Method::PUT, "/v1/orgs/settings", 200, None);
        assert_eq!(etags.get("/v1/orgs/settings"), None);
        etags.learn(&Method::PUT, "/v1/orgs/settings", 200, Some("\"4\""));
        // A refused version is forgotten, so the caller reads again.
        etags.learn(&Method::PUT, "/v1/orgs/settings", 412, Some("\"5\""));
        assert_eq!(etags.get("/v1/orgs/settings"), None);
        // A failure other than 412 changes nothing.
        etags.learn(&Method::GET, "/v1/orgs/settings", 200, Some("\"5\""));
        etags.learn(&Method::PUT, "/v1/orgs/settings", 500, None);
        assert_eq!(version("/v1/orgs/settings").as_deref(), Some("\"5\""));
        // A read with no `ETag` is known as unversioned, so an edit does not read it again.
        etags.learn(&Method::GET, "/v1/presence/settings", 200, None);
        assert_eq!(etags.get("/v1/presence/settings"), Some(None));
        // A deleted resource has no version.
        etags.learn(&Method::GET, "/v1/teams/web", 200, Some("\"1\""));
        etags.learn(&Method::DELETE, "/v1/teams/web", 200, None);
        assert_eq!(etags.get("/v1/teams/web"), None);
        // A path no edit needs is not kept.
        etags.learn(&Method::GET, "/v1/skills/x", 200, Some("\"9\""));
        assert_eq!(etags.get("/v1/skills/x"), None);
    }

    #[test]
    fn a_write_under_a_resource_forgets_its_version() {
        let etags = Etags::default();
        etags.learn(&Method::GET, "/v1/chat/sessions/s1", 200, Some("\"3\""));
        etags.learn(&Method::GET, "/v1/chat/sessions/s2", 200, Some("\"8\""));
        // A read under it, or a write elsewhere, keeps it.
        etags.touch(&Method::GET, "/v1/chat/sessions/s1/messages");
        etags.touch(&Method::POST, "/v1/chat/sessions/s10/messages");
        etags.touch(&Method::PATCH, "/v1/chat/sessions/s1");
        assert_eq!(etags.get("/v1/chat/sessions/s1").flatten().as_deref(), Some("\"3\""));
        // A message raises the session's version: the next rename reads it again.
        etags.touch(&Method::POST, "/v1/chat/sessions/s1/messages?stream=1");
        assert_eq!(etags.get("/v1/chat/sessions/s1"), None);
        assert_eq!(etags.get("/v1/chat/sessions/s2").flatten().as_deref(), Some("\"8\""));
    }

    #[test]
    fn finds_headers_whatever_their_case() {
        let headers = vec![("ETag".to_string(), "\"7\"".to_string())];
        assert_eq!(etag_of(&headers), Some("\"7\""));
        assert!(has_if_match(&[("If-Match".to_string(), "*".to_string())]));
        assert!(!has_if_match(&headers));
    }
}
