//! Link context actions shared by native renderers.
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    Url(String),
    Unsupported,
    NoSession,
    Remote,
}
#[derive(Debug, Clone, Copy)]
pub enum Session {
    Local,
    Remote,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkMenu {
    Internal { path: String, copy: String },
    External { url: String, preview: Preview },
}
/// Resolve relative app links and keep non-web schemes outside the menu.
pub fn for_link(href: &str, base: &str, ui_origin: &str, session: Option<Session>) -> Option<LinkMenu> {
    let app = Url::parse(base).ok()?;
    let url = app.join(href).ok()?;
    let ours = url.scheme() == app.scheme()
        && url.host_str().is_some()
        && url.host_str() == app.host_str()
        && url.port_or_known_default() == app.port_or_known_default();
    if ours {
        let path = &url[url::Position::BeforePath..];
        return Some(LinkMenu::Internal {
            path: path.to_string(),
            copy: format!("{}{path}", ui_origin.trim_end_matches('/')),
        });
    }
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let preview = if !url.username().is_empty() || url.password().is_some() {
        Preview::Unsupported
    } else {
        match session {
            None => Preview::NoSession,
            Some(Session::Remote) => Preview::Remote,
            Some(Session::Local) => {
                let mut preview = url.clone();
                if matches!(url.host_str(), Some("0.0.0.0" | "[::]" | "[::1]")) {
                    preview.set_host(Some("localhost")).ok()?;
                }
                Preview::Url(preview.to_string())
            }
        }
    };
    Some(LinkMenu::External {
        url: url.to_string(),
        preview,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const APP: &str = "nexus-app://localhost/app/acme/session";
    #[test]
    fn internal_links_copy_the_web_address() {
        assert_eq!(
            for_link("/settings#session-starts", APP, "https://nexus.test/", None),
            Some(LinkMenu::Internal {
                path: "/settings#session-starts".into(),
                copy: "https://nexus.test/settings#session-starts".into()
            })
        );
        assert_eq!(
            for_link("../other?q=1", APP, "https://nexus.test", None),
            Some(LinkMenu::Internal {
                path: "/app/other?q=1".into(),
                copy: "https://nexus.test/app/other?q=1".into()
            })
        );
    }
    #[test]
    fn external_preview_depends_on_session_and_refuses_credentials() {
        for (session, preview) in [
            (None, Preview::NoSession),
            (Some(Session::Remote), Preview::Remote),
            (Some(Session::Local), Preview::Url("https://example.com/".into())),
        ] {
            assert_eq!(
                for_link("https://example.com", APP, "https://nexus.test", session),
                Some(LinkMenu::External {
                    url: "https://example.com/".into(),
                    preview
                })
            );
        }
        assert!(matches!(
            for_link(
                "https://user:pass@example.com",
                APP,
                "https://nexus.test",
                Some(Session::Local)
            ),
            Some(LinkMenu::External {
                preview: Preview::Unsupported,
                ..
            })
        ));
    }
    #[test]
    fn local_listening_addresses_become_localhost() {
        for url in ["http://0.0.0.0:3000/", "http://[::]:3000/", "http://[::1]:3000/"] {
            assert!(
                matches!(for_link(url,APP,"https://nexus.test",Some(Session::Local)),Some(LinkMenu::External {preview:Preview::Url(url),..}) if url == "http://localhost:3000/")
            );
        }
    }
    #[test]
    fn unrelated_schemes_keep_their_original_behavior() {
        for href in [
            "mailto:hello@example.com",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,hi",
        ] {
            assert_eq!(for_link(href, APP, "https://nexus.test", None), None);
        }
    }
}
