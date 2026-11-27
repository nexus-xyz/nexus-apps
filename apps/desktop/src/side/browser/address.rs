//! The address bar's rules, as the Nexus app's (`preview.ts`, `previewUrl` and
//! `addressEntered`): a typed address becomes a web URL, plain text searches Google, and an unsafe address is refused.
//! The engine applies `nexus_browser`'s rule to every navigation as well.

use std::sync::LazyLock;

use regex::Regex;

/// What the address bar does with what was typed.
#[derive(Debug, PartialEq, Eq)]
pub enum Entered {
    Nothing,
    Open(String),
    Refused,
}

/// The text the address bar shows when the person types an address it refuses.
pub const REFUSED: &str = "This browser opens HTTP and HTTPS addresses without a username or password.";

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

static SCHEME_SLASHES: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^[a-z][a-z0-9+.-]*://"));
static SCHEME: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^[a-z][a-z0-9+.-]*:"));
static HOST_PORT: LazyLock<Regex> = LazyLock::new(|| regex(r"^[^/?#:]+:\d+(?:[/?#]|$)"));
static LOCAL: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)^(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\])(?:[:/?#]|$)"));
static UNSAFE_SCHEME: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)^(?:https?|javascript|data|file|mailto|blob|about|tauri|nexus|vbscript):"));
static PORT: LazyLock<Regex> = LazyLock::new(|| regex(r"^[^\s/?#:]+:\d+(?:[/?#]|$)"));
static ADDRESS: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^(?:https?://|//|localhost(?:[:/?#]|$)|\[[\da-f:]+\])"));
static DOMAIN: LazyLock<Regex> = LazyLock::new(|| regex(r"^[^\s:@/?#]+\.[^\s:@/?#]+(?::[^\s/?#]*)?(?:[/?#]|$)"));
static CREDENTIALS: LazyLock<Regex> = LazyLock::new(|| regex(r"^[^\s/?#@]+:[^\s/?#@]*@"));

/// What was typed, as a web URL: `localhost:3000` is `http://localhost:3000/`, a website `https://`. `None` for
/// anything else, a username or password included.
pub fn preview_url(text: &str) -> Option<String> {
    let mut typed = text.trim().to_string();
    if typed.is_empty() {
        return None;
    }
    if !SCHEME_SLASHES.is_match(&typed) {
        // A host followed by a numeric port is an address, not a custom URL scheme.
        if SCHEME.is_match(&typed) && !HOST_PORT.is_match(&typed) {
            return None;
        }
        let scheme = if LOCAL.is_match(&typed) { "http" } else { "https" };
        typed = format!("{scheme}://{}", typed.strip_prefix("//").unwrap_or(&typed));
    }
    let mut url = url::Url::parse(&typed).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    // The policy cannot name an IPv6 address, and `localhost` reaches a server on `::1` too.
    if matches!(url.host_str(), Some("0.0.0.0" | "[::]" | "[::1]")) {
        url.set_host(Some("localhost")).ok()?;
    }
    Some(url.to_string())
}

/// The person's address bar: open an address, search Google for plain text, or refuse an unsafe URL.
pub fn entered(text: &str) -> Entered {
    let typed = text.trim();
    if typed.is_empty() {
        return Entered::Nothing;
    }
    let url = preview_url(typed);
    let scheme = SCHEME_SLASHES.is_match(typed) || UNSAFE_SCHEME.is_match(typed);
    let port = PORT.is_match(typed);
    let address = ADDRESS.is_match(typed) || DOMAIN.is_match(typed) || port;
    if CREDENTIALS.is_match(typed) {
        return Entered::Refused;
    }
    if address || scheme {
        return url.map_or(Entered::Refused, Entered::Open);
    }
    let mut search = url::Url::parse("https://www.google.com/search").expect("a valid URL");
    search.query_pairs_mut().append_pair("q", typed);
    Entered::Open(search.to_string())
}

/// A tab's name before its page has a title: the address's host and port, or "New tab".
pub fn tab_label(address: &str) -> String {
    if address.is_empty() {
        return "New tab".into();
    }
    url::Url::parse(address)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_string();
            Some(match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_else(|| address.to_string())
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn adds_a_scheme_to_what_was_typed() {
        assert_eq!(preview_url("localhost:5173").as_deref(), Some("http://localhost:5173/"));
        assert_eq!(
            preview_url(" 127.0.0.1:3000/app ").as_deref(),
            Some("http://127.0.0.1:3000/app")
        );
        assert_eq!(
            preview_url("https://localhost:8443").as_deref(),
            Some("https://localhost:8443/")
        );
        assert_eq!(preview_url("[::1]:3000").as_deref(), Some("http://localhost:3000/"));
        assert_eq!(preview_url("google.com").as_deref(), Some("https://google.com/"));
        assert_eq!(
            preview_url("example.com:8443/docs?q=browser#links").as_deref(),
            Some("https://example.com:8443/docs?q=browser#links")
        );
        assert_eq!(
            preview_url("http://192.168.1.4:3000/app").as_deref(),
            Some("http://192.168.1.4:3000/app")
        );
        assert_eq!(
            preview_url("//example.com/docs").as_deref(),
            Some("https://example.com/docs")
        );
    }

    #[test]
    fn refuses_credentials_other_schemes_and_malformed_addresses() {
        for address in [
            "",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "http://user:pw@localhost:3000",
            "https://user:pw@example.com",
            "data:text/html,test",
            "tauri://localhost",
            "nexus://settings",
            "https:example.com",
            "not a website",
            "http://[bad",
        ] {
            assert_eq!(preview_url(address), None, "{address}");
        }
    }

    #[test]
    fn an_empty_field_does_nothing() {
        assert_eq!(entered(""), Entered::Nothing);
        assert_eq!(entered("   "), Entered::Nothing);
    }

    #[test]
    fn opens_local_addresses_and_websites() {
        assert_eq!(
            entered("localhost:5173"),
            Entered::Open("http://localhost:5173/".into())
        );
        assert_eq!(entered("google.com"), Entered::Open("https://google.com/".into()));
        assert_eq!(entered("javascript:alert(1)"), Entered::Refused);
        for text in [
            "https://intranet",
            "localhost:5173/app",
            "[::1]:3000",
            "192.168.1.4:8080",
            "devbox:8080",
            "//example.com/docs",
            "example.com/search?q=a+b#result",
        ] {
            assert_eq!(entered(text), Entered::Open(preview_url(text).expect(text)), "{text}");
        }
    }

    #[test]
    fn plain_text_searches_google() {
        for text in [
            "bruh",
            "what does bruh mean",
            "  café & pão?  ",
            "nexus.inc pricing",
            "2 + 2",
            "hello/there",
            "site:example.com",
            "weather: tomorrow",
            "@nexus",
            "person@example.com",
        ] {
            let Entered::Open(url) = entered(text) else {
                panic!("not opened: {text}");
            };
            let url = url::Url::parse(&url).expect("a URL");
            assert_eq!(url.as_str().split('?').next(), Some("https://www.google.com/search"));
            let pairs: Vec<_> = url.query_pairs().collect();
            assert_eq!(pairs.len(), 1, "{text}");
            assert_eq!(pairs[0].0, "q");
            assert_eq!(pairs[0].1, text.trim());
        }
    }

    #[test]
    fn unsafe_or_malformed_addresses_are_not_searches() {
        for text in [
            "file:///etc/passwd",
            "data:text/html,test",
            "tauri://localhost",
            "https://user:pw@example.com",
            "user:pw@example.com",
            "http://[bad",
            "localhost:99999",
            "https:example.com",
        ] {
            assert_eq!(entered(text), Entered::Refused, "{text}");
        }
    }

    #[test]
    fn a_tab_is_named_by_its_host() {
        assert_eq!(tab_label(""), "New tab");
        assert_eq!(tab_label("http://localhost:3000/a"), "localhost:3000");
        assert_eq!(tab_label("https://github.com/nexus"), "github.com");
    }
}
