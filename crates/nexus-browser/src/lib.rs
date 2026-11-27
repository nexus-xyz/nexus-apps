//! The session browser's rules, used by Nexus GPUI (`desktop-gpui/src/side/browser/`): the addresses it opens, the
//! dev-server check, the popup rule, reading sign-in cookies from the person's browser, and what a cloud session's
//! browser may do ([`place`]). The app keeps only its own webview code.

pub mod access;
pub mod control;
pub mod cookies;
pub mod place;
pub mod preview;
pub mod profile;

/// The script every session browser page runs: the console it keeps for the agent and the pane, and the agent's
/// read, click, type and element picker (`window.__nxPreview`). It has no Nexus bridge, credential or app state.
pub const PAGE_SCRIPT: &str = include_str!("page.js");

/// The most tabs one session's browser holds. Its sign-in popups have the same limit.
pub const MAX_TABS: usize = 8;

/// Whether a sign-in popup may open or go to `url`: a blank page, or a web address. Never the app's own pages, a file
/// or a script, so a popup gains no app or file access.
pub fn popup_allowed(url: &str) -> bool {
    url == "about:blank" || preview::browser_url(url).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_documents_never_gain_app_or_file_access() {
        for url in [
            "about:blank",
            "https://accounts.example.com/authorize",
            "http://localhost:8080/callback",
        ] {
            assert!(popup_allowed(url), "{url}");
        }
        for url in [
            "about:config",
            "file:///etc/passwd",
            "app://localhost/app",
            "javascript:alert(1)",
            "https://user:secret@example.com/",
        ] {
            assert!(!popup_allowed(url), "{url}");
        }
    }
}
