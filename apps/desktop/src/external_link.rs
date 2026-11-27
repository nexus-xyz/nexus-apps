//! Links that leave the app for the operating system's handler.
//!
//! Every link the app hands to the OS goes through [`open`] or [`open_popup`]. Much of what the app shows comes from
//! outside it: a model's answer, a web page, a shared chat, a file or a Gateway answer. The OS opens any scheme it gets,
//! so a `file:`, `search-ms:` or `smb:` link, a UNC path or another app's scheme would run on one click. Only the
//! schemes below open. A refused link does what a dead link does: nothing. The log names its scheme, never the link.
//!
//! gpui-base's `TextView` opens a link itself when it has no handler, with no check. So a text view starts from
//! [`text_view`], and a view that has its own handler replaces this one. A test below keeps every `open_url` call and
//! every `TextView::new` in this module.

use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::{App, ClickEvent, Entity, MouseButton};

/// What a link may open: a web page or a new mail.
pub const LINK: &[&str] = &["http", "https", "mailto"];
/// What a web page's popup may open in the person's browser: a web page only.
pub const POPUP: &[&str] = &["http", "https"];

/// What a desktop draft's link may open: this app, through its own `nexus:` scheme.
pub const APP: &[&str] = &["nexus"];

/// Why a link did not open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Not an absolute URL: an empty string, a relative path or a UNC path.
    NotAbsolute,
    /// A scheme outside the list. It holds the scheme, lowercased and cut to 32 characters.
    Scheme(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NotAbsolute => f.write_str("not an absolute URL"),
            Refused::Scheme(scheme) => write!(f, "scheme {scheme}"),
        }
    }
}

/// `link` parsed, when its scheme is in `schemes`. The OS gets the parsed form, so it reads what this checked.
/// The parser lowercases the scheme: `HTTPS:` is `https:`, and `FiLe:` is still `file:`.
pub fn checked(link: &str, schemes: &[&str]) -> Result<url::Url, Refused> {
    let url = url::Url::parse(link).map_err(|_| Refused::NotAbsolute)?;
    if schemes.contains(&url.scheme()) {
        Ok(url)
    } else {
        Err(Refused::Scheme(url.scheme().chars().take(32).collect()))
    }
}

/// Open a link in the person's browser or mail app: `http`, `https` and `mailto` only.
pub fn open(link: &str, cx: &App) {
    open_with(link, LINK, cx);
}

/// Open a web page's popup in the person's browser: `http` and `https` only.
pub fn open_popup(link: &str, cx: &App) {
    open_with(link, POPUP, cx);
}

/// Open a link to this app: `nexus:` only.
pub fn open_app(link: &str, cx: &App) {
    open_with(link, APP, cx);
}

fn open_with(link: &str, schemes: &[&str], cx: &App) {
    match checked(link, schemes) {
        Ok(url) => cx.open_url(url.as_str()),
        Err(refused) => eprintln!("nexus-desktop-gpui: a link was refused: {refused}"),
    }
}

/// A text view whose links open through [`open`]. It opens on the clicks gpui-base's own fallback opens on: a left
/// or middle click, a key, or a tap that is not a long press.
pub fn text_view(state: &Entity<TextViewState>) -> TextView {
    TextView::new(state).on_link_click(|href, event, _, cx| {
        if opens(event) {
            open(href, cx);
        }
    })
}

fn opens(event: &ClickEvent) -> bool {
    match event {
        ClickEvent::Mouse(click) => matches!(click.up.button, MouseButton::Left | MouseButton::Middle),
        ClickEvent::Keyboard(_) => true,
        ClickEvent::Touch(click) => !click.long_press,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_and_mail_links_open() {
        for link in [
            "http://example.com/a",
            "https://example.com/a?b=c#d",
            "mailto:someone@example.com",
        ] {
            assert!(checked(link, LINK).is_ok(), "{link}");
        }
    }

    #[test]
    fn the_os_gets_the_parsed_link() {
        assert_eq!(
            checked("HTTPS://Example.COM", LINK).unwrap().as_str(),
            "https://example.com/"
        );
    }

    #[test]
    fn other_schemes_are_refused() {
        for (link, scheme) in [
            ("file:///etc/passwd", "file"),
            ("file://server/share/run.exe", "file"),
            ("javascript:alert(1)", "javascript"),
            ("data:text/html,<script>alert(1)</script>", "data"),
            ("search-ms:query=invoice&crumb=location:\\\\evil\\share", "search-ms"),
            ("smb://evil.example/share", "smb"),
            ("vscode://file/etc/passwd", "vscode"),
            ("FiLe:///etc/passwd", "file"),
            ("JavaScript:alert(1)", "javascript"),
            ("C:\\Windows\\System32\\calc.exe", "c"),
        ] {
            assert_eq!(checked(link, LINK), Err(Refused::Scheme(scheme.into())), "{link}");
        }
    }

    #[test]
    fn what_is_not_an_absolute_url_is_refused() {
        for link in [
            "",
            "   ",
            "\\\\evil.example\\share\\run.exe",
            "//evil.example/share",
            "/docs/a",
            "docs/a",
            "#top",
        ] {
            assert_eq!(checked(link, LINK), Err(Refused::NotAbsolute), "{link:?}");
        }
    }

    #[test]
    fn a_popup_opens_web_pages_only() {
        assert!(checked("https://example.com", POPUP).is_ok());
        assert!(checked("http://example.com", POPUP).is_ok());
        assert_eq!(
            checked("mailto:a@example.com", POPUP),
            Err(Refused::Scheme("mailto".into()))
        );
        assert_eq!(
            checked("file:///etc/passwd", POPUP),
            Err(Refused::Scheme("file".into()))
        );
    }

    #[test]
    fn the_log_names_the_scheme_and_never_the_link() {
        let refused = checked("search-ms:query=secret-token", LINK).unwrap_err();
        assert_eq!(refused.to_string(), "scheme search-ms");
        let long = format!("{}:x", "a".repeat(100));
        assert_eq!(
            checked(&long, LINK).unwrap_err().to_string().len(),
            "scheme ".len() + 32
        );
    }

    #[test]
    fn a_right_click_or_long_press_does_not_open() {
        use gpui_kit::{MouseClickEvent, MouseDownEvent, MouseUpEvent, TouchClickEvent};
        let mouse = |button| {
            ClickEvent::Mouse(MouseClickEvent {
                down: MouseDownEvent {
                    button,
                    ..Default::default()
                },
                up: MouseUpEvent {
                    button,
                    ..Default::default()
                },
            })
        };
        assert!(opens(&mouse(MouseButton::Left)));
        assert!(opens(&mouse(MouseButton::Middle)));
        assert!(!opens(&mouse(MouseButton::Right)));
        assert!(opens(&ClickEvent::default()));
        assert!(!opens(&ClickEvent::Touch(TouchClickEvent {
            long_press: true,
            ..Default::default()
        })));
    }

    /// Code that does not live here: a file under `src/`, without its inline test module.
    fn app_code() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("src folder") {
                let path = entry.expect("src entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        files
            .into_iter()
            .filter(|path| {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
                name != "external_link.rs" && name != "tests.rs" && !name.ends_with("_tests.rs")
            })
            .map(|path| {
                let text = std::fs::read_to_string(&path).expect("source file");
                // An inline `#[cfg(test)] mod tests { .. }` ends the app's code in a file here.
                let lines: Vec<&str> = text.lines().collect();
                let end = lines
                    .windows(2)
                    .position(|pair| {
                        pair[0].trim() == "#[cfg(test)]"
                            && pair[1].trim_start().starts_with("mod ")
                            && pair[1].trim_end().ends_with('{')
                    })
                    .unwrap_or(lines.len());
                let shown = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
                (shown, lines[..end].join("\n"))
            })
            .collect()
    }

    #[test]
    fn every_link_to_the_os_goes_through_this_module() {
        let banned = [
            ".open_url(",
            "TextView::new(",
            "TextView::markdown(",
            "TextView::html(",
            "PopupMenuItem::link(",
        ];
        let mut found = Vec::new();
        for (file, code) in app_code() {
            for (ix, line) in code.lines().enumerate() {
                if banned.iter().any(|call| line.contains(call)) {
                    found.push(format!("src/{file}:{}: {}", ix + 1, line.trim()));
                }
            }
        }
        assert!(
            found.is_empty(),
            "open links with external_link::open and make text views with external_link::text_view:\n{}",
            found.join("\n")
        );
    }
}
