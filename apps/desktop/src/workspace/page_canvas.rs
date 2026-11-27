//! What a Nexus Design or Slides canvas tells the app, as it tells Nexus UI's page panel (`page-panel.tsx`).
//!
//! A canvas posts `{nexusDesign: 1, type}` to its parent frame (`canvas-runtime.ts` and `slides-runtime.ts` in
//! pages-host). Nexus UI frames the page, so it hears them. Here the page is the top document of its [`WebView`], so
//! it has no parent. [`STARTUP`] gives it one before the page's own scripts run: a `window.parent` whose
//! `postMessage` sends the four messages the app acts on to a DevTools binding. They come back as
//! `Runtime.bindingCalled` events, which [`parse`] reads. On Servo the page has no binding and posts nothing.
//!
//! A board's ⋯ posts `board`: Regenerate, Make 3 variants or Keep this variant. The page's own
//! scripts can post it too, so the page panel only stages it, and acts on a click in its own bar (`page_panel.rs`).
//!
//! The page's `window.nexus` bridge also posts to its parent. It still answers "unavailable" at once, as before.

use std::collections::HashSet;
use std::sync::Mutex;

use crate::web::Startup;

/// The function the page calls with a message, as JSON text.
const BINDING: &str = "__nexusPageMessage";

/// Runs first in each document of a Nexus page. Only the top document gets a parent: a frame inside the page keeps
/// its own. Only `fullscreen`, `escape`, `ready` and `board` go to the app, so the pointer's moves cost nothing.
const SCRIPT: &str = r#"(function () {
  if (window.parent !== window) return;
  var name = "__nexusPageMessage", wanted = { fullscreen: 1, escape: 1, ready: 1, board: 1 };
  var parent = {
    postMessage: function (data) {
      if (data && data.nexusDesign === 1) {
        var send = window[name];
        if (wanted[data.type] === 1 && typeof send === "function") {
          try { send(JSON.stringify(data)); } catch (error) {}
        }
        return;
      }
      if (data && data.nexus === 1) {
        var error = new Error("This works only when the page is open in Nexus.");
        error.code = "unavailable";
        throw error;
      }
    }
  };
  try { Object.defineProperty(window, "parent", { value: parent, configurable: true }); } catch (error) {}
})();"#;

pub(super) const STARTUP: Startup = Startup {
    source: SCRIPT,
    binding: Some(BINDING),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Design,
    Slides,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Message {
    /// The canvas's Full view button.
    FullView,
    /// Escape, with nothing on the canvas to clear.
    Escape,
    /// The canvas loaded: a design (artboards) or a deck (slides).
    Ready(Kind),
    /// A board's ⋯ asks for `ask` on `board`, its `data-id`.
    Board { ask: Ask, board: String },
}

/// What a board's ⋯ asks for: Gateway's `PageBoardAsk` (`@nexus/kit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ask {
    Regenerate,
    Variants,
    /// Keep a variant in place of its original. Here it goes to Nexus, as a `jsx` design's Keep does in Nexus UI.
    Keep,
}

impl Ask {
    /// Its name on the wire.
    pub(super) fn wire(self) -> &'static str {
        match self {
            Ask::Regenerate => "regenerate",
            Ask::Variants => "variants",
            Ask::Keep => "keep",
        }
    }

    /// The question the panel asks before it acts, and its button: Nexus UI's `boardConfirm`.
    pub(super) fn confirm(self, board: &str) -> (String, &'static str) {
        match self {
            Ask::Regenerate => (
                format!("Regenerate {board}? A new chat writes it again as the next version."),
                "Regenerate",
            ),
            Ask::Variants => (
                format!("Make 3 variants of {board}? A new chat adds them under it as the next version."),
                "Make 3 variants",
            ),
            Ask::Keep => (
                format!(
                    "Keep {board} in place of its original? A new chat writes the next version. The original stays in the versions."
                ),
                "Keep this variant",
            ),
        }
    }

    /// What the panel shows while Gateway opens the chat: Nexus UI's `askBoard`.
    pub(super) fn busy(self, board: &str) -> String {
        match self {
            Ask::Regenerate => format!("Asking Nexus to regenerate {board}…"),
            Ask::Variants => format!("Asking Nexus for 3 variants of {board}…"),
            Ask::Keep => format!("Asking Nexus to keep {board}…"),
        }
    }
}

/// A board's `data-id` as Gateway takes it (`board_of` in nexus-state; `BOARD_ID` in Nexus UI's `pages.ts`).
fn board_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
}

/// The canvas message in a DevTools event from the page, if it is one.
pub(super) fn parse(cdp: &str) -> Option<Message> {
    let event: serde_json::Value = serde_json::from_str(cdp).ok()?;
    if event["method"] != "Runtime.bindingCalled" || event["params"]["name"] != BINDING {
        return None;
    }
    let message: serde_json::Value = serde_json::from_str(event["params"]["payload"].as_str()?).ok()?;
    if message["nexusDesign"] != 1 {
        return None;
    }
    match message["type"].as_str()? {
        "fullscreen" => Some(Message::FullView),
        "escape" => Some(Message::Escape),
        // As Nexus UI's `pages.ts`: the list the canvas names says which it is.
        "ready" if message["artboards"].is_array() => Some(Message::Ready(Kind::Design)),
        "ready" if message["slides"].is_array() => Some(Message::Ready(Kind::Slides)),
        "board" => {
            let ask = match message["ask"].as_str()? {
                "regenerate" => Ask::Regenerate,
                "variants" => Ask::Variants,
                "keep" => Ask::Keep,
                _ => return None,
            };
            let board = message["board"].as_str().filter(|board| board_id(board))?;
            Some(Message::Board {
                ask,
                board: board.to_string(),
            })
        }
        _ => None,
    }
}

/// The pages that opened in full view, once each: a new version or a reload does not put back a full view the person
/// left. Nexus UI's `OPENED_FULL`.
#[derive(Default)]
pub(super) struct OpenedFull(HashSet<String>);

impl OpenedFull {
    /// Whether page `key` opens in full view now: the first time it is ready, and only if the person opened it.
    pub(super) fn first(&mut self, key: &str, auto: bool) -> bool {
        !auto && self.0.insert(key.to_string())
    }
}

/// [`OpenedFull`] for this run of the app, across its windows.
pub(super) fn first_full_view(key: &str, auto: bool) -> bool {
    static OPENED: Mutex<Option<OpenedFull>> = Mutex::new(None);
    let Ok(mut opened) = OPENED.lock() else {
        return false;
    };
    opened.get_or_insert_with(OpenedFull::default).first(key, auto)
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn called(name: &str, payload: serde_json::Value) -> String {
        serde_json::json!({
            "method": "Runtime.bindingCalled",
            "params": { "name": name, "payload": payload.to_string(), "executionContextId": 1 },
        })
        .to_string()
    }

    #[test]
    fn reads_the_canvas_messages_the_app_acts_on() {
        let full = called(BINDING, serde_json::json!({ "nexusDesign": 1, "type": "fullscreen" }));
        assert_eq!(parse(&full), Some(Message::FullView));
        let escape = called(BINDING, serde_json::json!({ "nexusDesign": 1, "type": "escape" }));
        assert_eq!(parse(&escape), Some(Message::Escape));
        let design = serde_json::json!({ "nexusDesign": 1, "type": "ready", "artboards": ["Home"], "source": "ab" });
        assert_eq!(parse(&called(BINDING, design)), Some(Message::Ready(Kind::Design)));
        let deck = serde_json::json!({ "nexusDesign": 1, "type": "ready", "slides": ["Title"], "notes": [""] });
        assert_eq!(parse(&called(BINDING, deck)), Some(Message::Ready(Kind::Slides)));
    }

    #[test]
    fn reads_a_board_ask_and_refuses_a_malformed_one() {
        let board = |ask: &str, board: serde_json::Value| {
            parse(&called(
                BINDING,
                serde_json::json!({ "nexusDesign": 1, "type": "board", "ask": ask, "board": board }),
            ))
        };
        for (ask, want) in [
            ("regenerate", Ask::Regenerate),
            ("variants", Ask::Variants),
            ("keep", Ask::Keep),
        ] {
            assert_eq!(
                board(ask, "web:home.v2_b-1".into()),
                Some(Message::Board {
                    ask: want,
                    board: "web:home.v2_b-1".into()
                })
            );
        }
        // An ask Gateway does not take, and boards that are not a `data-id` it takes.
        assert_eq!(board("delete", "L1".into()), None);
        assert_eq!(board("variants", "".into()), None);
        assert_eq!(board("variants", "a b".into()), None);
        assert_eq!(board("variants", "L1\"><img>".into()), None);
        assert_eq!(board("variants", "x".repeat(65).into()), None);
        assert_eq!(board("variants", 1.into()), None);
        // The script forwards it.
        assert!(SCRIPT.contains("board: 1"));
    }

    #[test]
    fn ignores_everything_else() {
        // Another binding, another event, a reply, and text that is not JSON.
        let other = called("other", serde_json::json!({ "nexusDesign": 1, "type": "fullscreen" }));
        assert_eq!(parse(&other), None);
        assert_eq!(parse(r#"{"method":"Page.loadEventFired","params":{}}"#), None);
        assert_eq!(parse(r#"{"id":4,"result":{}}"#), None);
        assert_eq!(parse("not json"), None);
        // Not the canvas's, a message the app does not act on, and a ready that names no boards or slides.
        let plain = called(BINDING, serde_json::json!({ "type": "fullscreen" }));
        assert_eq!(parse(&plain), None);
        let pointer = called(
            BINDING,
            serde_json::json!({ "nexusDesign": 1, "type": "pointer", "at": [1, 2] }),
        );
        assert_eq!(parse(&pointer), None);
        let bare = called(BINDING, serde_json::json!({ "nexusDesign": 1, "type": "ready" }));
        assert_eq!(parse(&bare), None);
        let event = serde_json::json!({
            "method": "Runtime.bindingCalled",
            "params": { "name": BINDING, "payload": "{" },
        });
        assert_eq!(parse(&event.to_string()), None);
    }

    #[test]
    fn a_page_opens_in_full_view_once_and_only_when_the_person_opened_it() {
        let mut opened = OpenedFull::default();
        // The app opened it beside a running chat: not now, and that does not use up its once.
        assert!(!opened.first("/app/acme/pages/p1", true));
        assert!(opened.first("/app/acme/pages/p1", false));
        // A reload or a new version is ready again: it stays as the person left it.
        assert!(!opened.first("/app/acme/pages/p1", false));
        assert!(opened.first("/app/acme/pages/p2", false));
    }

    #[test]
    fn the_script_names_the_binding() {
        assert!(SCRIPT.contains(BINDING));
        assert_eq!(STARTUP.binding, Some(BINDING));
    }
}
