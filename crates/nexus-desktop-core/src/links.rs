//! Prompt links (`nexus://new?prompt=…&repo=…&agent=…&model=…`), without the toolkit: which links the app keeps, and
//! the queue of links no window has taken yet. The shell receives the links from the OS and tells its windows. The UI
//! reads a link and fills the draft; a link never sends anything.

use std::sync::Mutex;

/// The scheme, the phone app's too (`nexus://a/<key>`); this app opens its `new` links.
pub const SCHEME: &str = "nexus";

/// The longest link kept: a prompt of 8,000 characters, every one percent-encoded from four UTF-8 bytes, fits.
pub const MAX_LINK: usize = 100_000;

/// The most links waiting for a window: a page that opens links in a loop cannot fill the memory.
pub const MAX_WAITING: usize = 8;

/// The links no window has taken yet, oldest first.
#[derive(Default)]
pub struct PromptLinks(Mutex<Vec<String>>);

impl PromptLinks {
    /// Keeps `url` for a window when it is one of this app's links. False when it is not, or too many are waiting.
    pub fn push(&self, url: &str) -> bool {
        if !accepted(url) {
            return false;
        }
        let mut waiting = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if waiting.len() >= MAX_WAITING {
            return false;
        }
        waiting.push(url.to_string());
        true
    }

    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|error| error.into_inner()))
    }
}

/// A `nexus:` link of a reasonable size. What it asks for is the webview's to read.
pub fn accepted(url: &str) -> bool {
    url.len() <= MAX_LINK
        && url
            .split_once(':')
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(SCHEME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_its_own_links_of_a_reasonable_size() {
        assert!(accepted("nexus://new?prompt=Fix%20the%20build"));
        assert!(accepted("NEXUS://new"));
        assert!(!accepted("https://nexus.xyz/new?prompt=x"));
        assert!(!accepted("nexusx://new"));
        assert!(!accepted("new?prompt=x"));
        let long = format!("nexus://new?prompt={}", "a".repeat(MAX_LINK));
        assert!(!accepted(&long));
    }

    #[test]
    fn hands_out_the_waiting_links_once_and_caps_them() {
        let links = PromptLinks::default();
        assert!(!links.push("https://example.com"));
        for n in 0..MAX_WAITING {
            assert!(links.push(&format!("nexus://new?prompt={n}")));
        }
        assert!(!links.push("nexus://new?prompt=one-too-many"));
        assert_eq!(links.take().len(), MAX_WAITING);
        assert!(links.take().is_empty());
    }
}
