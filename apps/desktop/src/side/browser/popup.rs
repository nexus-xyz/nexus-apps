//! A session browser's popup (a sign-in, say) in a window of its own: a real page with its opener, so OAuth's `postMessage`, cookies and
//! `window.close()` work. "Browser sign-in" until its page has a title, then "host · title". It closes when its page
//! calls `window.close()`, or with the tab that opened it.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::web::{Opened, POPUP_SIZE, WebEvent, WebView};

/// The window's title before its page has one.
pub const TITLE: &str = "Browser sign-in";

/// A popup window's title: the page's host and title.
pub fn title(address: Option<&str>, title: &str) -> String {
    let host = address
        .and_then(|address| url::Url::parse(address).ok())
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default();
    format!("{host} · {title}")
}

pub struct Popup {
    page: Entity<WebView>,
    _page: Subscription,
}

impl Popup {
    /// Open `opened` in a new window.
    pub fn open(opened: Opened, focus: bool, cx: &mut App) -> Option<AnyWindowHandle> {
        let options = WindowOptions {
            focus,
            window_bounds: Some(WindowBounds::centered(size(px(POPUP_SIZE.0), px(POPUP_SIZE.1)), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some(TITLE.into()),
                ..Default::default()
            }),
            app_id: Some(crate::app_id::get()),
            ..Default::default()
        };
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| {
                let page = cx.new(|cx| WebView::adopt(opened, window, cx));
                let subscription =
                    cx.subscribe_in(&page, window, |_, page, event: &WebEvent, window, cx| match event {
                        WebEvent::TitleChanged(text) => {
                            window.set_window_title(&title(page.read(cx).address().as_deref(), text));
                        }
                        WebEvent::Closed => window.remove_window(),
                        _ => {}
                    });
                Popup {
                    page,
                    _page: subscription,
                }
            })
        });
        match opened {
            Ok((window, _)) => Some(window),
            Err(error) => {
                eprintln!("nexus-desktop-gpui: a browser popup did not open: {error}");
                None
            }
        }
    }
}

impl Render for Popup {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.page.clone())
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn the_title_names_the_site() {
        assert_eq!(
            title(Some("https://accounts.example.com/authorize?x=1"), "Sign in"),
            "accounts.example.com · Sign in"
        );
        assert_eq!(title(None, "Sign in"), " · Sign in");
    }
}
