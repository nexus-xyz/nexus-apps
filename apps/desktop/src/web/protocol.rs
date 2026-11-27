//! What the app and the web host process (`host.rs`) say to each other: one JSON object per line. The app writes
//! [`Command`]s to the host's stdin; the host writes [`Event`]s to its stdout. A page's frame does not go through the
//! pipe: the host writes its BGRA pixels into a memory file (`/dev/shm` on Linux), and the `frame` event names it. The
//! host writes the next frame only after the app acknowledges the last one ([`Command::Ack`]), so the app never reads
//! a frame while it is being written, and a slow app gets the newest frame instead of a queue.

use serde::{Deserialize, Serialize};

/// The host process is this binary, started with this argument.
pub(crate) const HOST_ARG: &str = "--nexus-web-host";

/// A website profile (`nexus_browser::profile::Profile`): its folder under the engine's cache (`browser-<id>`), and
/// whether it is saved there; one that is not lives in memory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WebProfile {
    pub folder: String,
    pub persistent: bool,
}

impl From<nexus_browser::profile::Profile> for WebProfile {
    fn from(profile: nexus_browser::profile::Profile) -> WebProfile {
        WebProfile {
            folder: profile.folder(),
            persistent: profile.persistent,
        }
    }
}

/// The frame rates CEF takes (`windowless_frame_rate`), frames a second.
pub(crate) const FRAME_RATES: std::ops::RangeInclusive<i32> = 1..=240;

/// Popups' ids start here: the host names them, and the app's own ids count up from 1, never this far.
pub(crate) const POPUP_IDS: u32 = 1 << 30;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub(crate) enum Command {
    /// Open a page, `width` by `height` logical pixels at `scale`. Ids are the app's, never reused while it runs.
    Open {
        id: u32,
        url: String,
        width: i32,
        height: i32,
        scale: f32,
        frame_rate: i32,
        /// Paint into GPU buffers and pass them over the frame socket (`frames/linux.rs`), not into the memory file.
        #[serde(default)]
        gpu: bool,
        /// Paint a frame only when asked ([`Command::BeginFrame`]), at the screen's pace, not on Chromium's own timer.
        #[serde(default)]
        paced: bool,
        /// A Code session's browser tab, named by its session: it opens only web addresses, and its sign-in popups
        /// open as pages of their own ([`Event::PopupOpened`]), at most `nexus_browser::MAX_TABS` per session. Other
        /// pages (artifacts, Pages) send a new window's link to the person's browser ([`Event::Popup`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        browser: Option<String>,
        /// A session browser's website profile: its sign-in's, apart from every other sign-in's and from the app's own
        /// pages, which keep the default profile.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<WebProfile>,
        /// A read-only document (the Files tool's HTML or video preview): no JavaScript, no popups, no downloads,
        /// and it never leaves its first address.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        document: bool,
        /// The address a page hands its result over by (guest chat's challenge page, `crate::guest`): a main-frame
        /// navigation to it, with any fragment, is cancelled and reported as [`Event::Handoff`]. None on other pages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handoff: Option<String>,
        /// No engine background: what the page leaves unpainted is see-through, so the app's surface shows under it
        /// (an answer's math, `rich_text`). Other pages are opaque white until they paint, as a browser tab.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        clear: bool,
    },
    /// Go to `url` in the page, which keeps its history.
    Navigate {
        id: u32,
        url: String,
    },
    /// Back or forward in the page's history (`step` -1 or 1), or the page loaded again (0).
    History {
        id: u32,
        step: i32,
    },
    Close {
        id: u32,
    },
    Resize {
        id: u32,
        width: i32,
        height: i32,
        scale: f32,
    },
    /// The page is off screen (a hidden or minimised window): Chromium stops painting and animating it.
    Hidden {
        id: u32,
        hidden: bool,
    },
    Focus {
        id: u32,
        focused: bool,
    },
    Mouse {
        id: u32,
        x: i32,
        y: i32,
        /// Chromium's event flags (`keys::flags`), the held buttons included.
        modifiers: u32,
        action: MouseAction,
    },
    /// A wheel or touchpad scroll, in pixels.
    Wheel {
        id: u32,
        x: i32,
        y: i32,
        modifiers: u32,
        dx: i32,
        dy: i32,
    },
    Key {
        id: u32,
        key: KeyInput,
    },
    /// Files dragged from the system over the page (`file_drag.rs`): the page sees them as an HTML drag and drop.
    FileDrag {
        id: u32,
        x: i32,
        y: i32,
        modifiers: u32,
        step: FileDragStep,
    },
    /// The app has read the page's last frame; the host may write the next.
    Ack {
        id: u32,
    },
    /// The app is drawing a frame: a paced page paints its next one now, for the app's next frame.
    BeginFrame {
        id: u32,
    },
    /// The page paints at most `fps` frames a second from now on: the window moved to a display with another refresh
    /// rate, or the display's rate changed. `Open`'s `frame_rate` is the first.
    FrameRate {
        id: u32,
        fps: i32,
    },
    /// A Chrome DevTools Protocol message for the page, as JSON text (`{"id":1,"method":"Page.navigate",...}`).
    Cdp {
        id: u32,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum MouseAction {
    Move,
    Leave,
    Down { button: Button, clicks: i32 },
    Up { button: Button, clicks: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Button {
    Left,
    Middle,
    Right,
}

/// One of Chromium's key events: `RawDown` with the key's Windows virtual-key code, a `Char` per UTF-16 unit typed,
/// then `Up` (`keys.rs`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct KeyInput {
    pub kind: KeyKind,
    pub modifiers: u32,
    pub code: i32,
    pub character: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeyKind {
    RawDown,
    Char,
    Up,
}

/// A step of a system file drag over the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum FileDragStep {
    /// The files came over the page.
    Enter {
        paths: Vec<String>,
    },
    Over,
    Leave,
    Drop,
}

/// The cursor a page asks for: CSS's cursors that the app's windows can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Cursor {
    #[default]
    Arrow,
    Hand,
    Text,
    VerticalText,
    Crosshair,
    Grab,
    Grabbing,
    NotAllowed,
    Copy,
    Alias,
    ContextMenu,
    ResizeEast,
    ResizeWest,
    ResizeEastWest,
    ResizeNorth,
    ResizeSouth,
    ResizeNorthSouth,
    /// `nwse-resize`: up-left and down-right.
    ResizeNorthWestSouthEast,
    /// `nesw-resize`: up-right and down-left.
    ResizeNorthEastSouthWest,
    ResizeColumn,
    ResizeRow,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum Event {
    /// CEF started, or could not (`ok: false`, and the host exits).
    Ready {
        ok: bool,
    },
    /// The page could not be created.
    Failed {
        id: u32,
    },
    /// A new frame, `width` by `height` device pixels, BGRA, in the file at `path` (`cpu.rs`). The app reads it, then
    /// sends [`Command::Ack`].
    Frame {
        id: u32,
        path: String,
        width: u32,
        height: u32,
        /// The page's frame number, from 1.
        #[serde(default)]
        seq: u64,
        /// The rects (`x, y, width, height`, device pixels) that changed since the frame before; empty: all of it.
        #[serde(default)]
        dirty: Vec<[u32; 4]>,
    },
    Title {
        id: u32,
        title: String,
    },
    Cursor {
        id: u32,
        cursor: Cursor,
    },
    /// The page asked to open a link in a new window.
    Popup {
        id: u32,
        url: String,
    },
    /// The page's renderer process ended (a crash, or out of memory): the page shows nothing until it opens again.
    Crashed {
        id: u32,
    },
    /// The page's main frame is at `url` now.
    Address {
        id: u32,
        url: String,
    },
    /// The page started or stopped loading, and whether its history has a page behind or ahead.
    Loading {
        id: u32,
        loading: bool,
        back: bool,
        forward: bool,
    },
    /// The main document failed; no URL or native error text crosses the host boundary.
    LoadFailed {
        id: u32,
        status: Option<u16>,
    },
    /// A session browser's page opened a popup (a sign-in, say): page `popup`, at `url`, which the app shows in a
    /// window of its own. Its id is the host's, from [`POPUP_IDS`].
    PopupOpened {
        id: u32,
        popup: u32,
        url: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        user_gesture: bool,
    },
    /// The page closed itself (a popup's `window.close()`).
    Closed {
        id: u32,
    },
    /// A DevTools Protocol message from the page: a reply to a [`Command::Cdp`], or an event it subscribed to.
    Cdp {
        id: u32,
        message: String,
    },
    /// The page's main frame was going to its `handoff` address ([`Command::Open`]), at `url` with its fragment. The
    /// host cancelled that navigation.
    Handoff {
        id: u32,
        url: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_load_failure_crosses_the_host_boundary_without_url_or_native_error() {
        for status in [None, Some(403)] {
            let line = serde_json::to_string(&Event::LoadFailed { id: 7, status }).unwrap();
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(value["event"], "load_failed");
            assert_eq!(value.as_object().unwrap().len(), 3, "{line}");
            assert!(!line.contains("url") && !line.contains("error"), "{line}");
        }
    }

    #[test]
    fn commands_round_trip_as_tagged_json() {
        let command = Command::Mouse {
            id: 3,
            x: 10,
            y: 20,
            modifiers: 16,
            action: MouseAction::Down {
                button: Button::Left,
                clicks: 2,
            },
        };
        let line = serde_json::to_string(&command).unwrap();
        assert!(line.contains(r#""cmd":"mouse""#), "{line}");
        assert!(line.contains(r#""kind":"down""#), "{line}");
        let Command::Mouse { action, .. } = serde_json::from_str(&line).unwrap() else {
            panic!("not a mouse command: {line}");
        };
        assert!(matches!(
            action,
            MouseAction::Down {
                button: Button::Left,
                clicks: 2
            }
        ));
    }

    #[test]
    fn a_page_without_a_session_reads_as_before() {
        let line = r#"{"cmd":"open","id":1,"url":"https://x.test","width":1,"height":1,"scale":1.0,"frame_rate":60}"#;
        let Command::Open { browser, gpu, .. } = serde_json::from_str(line).unwrap() else {
            panic!("not an open command");
        };
        assert_eq!(browser, None);
        assert!(!gpu);
        let Command::Open { handoff, clear, .. } = serde_json::from_str(line).unwrap() else {
            panic!("not an open command");
        };
        assert_eq!(handoff, None);
        assert!(!clear, "a page is opaque unless it asks to be clear");
        let popup = serde_json::to_string(&Event::PopupOpened {
            id: 1,
            popup: POPUP_IDS,
            url: "about:blank".into(),
            user_gesture: false,
        })
        .unwrap();
        assert_eq!(
            popup,
            r#"{"event":"popup_opened","id":1,"popup":1073741824,"url":"about:blank"}"#
        );
    }

    #[test]
    fn popup_focus_keeps_the_native_user_gesture_across_the_host_boundary() {
        let old = r#"{"event":"popup_opened","id":1,"popup":2,"url":"about:blank"}"#;
        assert!(matches!(
            serde_json::from_str::<Event>(old).unwrap(),
            Event::PopupOpened {
                user_gesture: false,
                ..
            }
        ));
        let native = Event::PopupOpened {
            id: 1,
            popup: 2,
            url: "about:blank".into(),
            user_gesture: true,
        };
        let encoded = serde_json::to_string(&native).unwrap();
        assert!(matches!(
            serde_json::from_str::<Event>(&encoded).unwrap(),
            Event::PopupOpened { user_gesture: true, .. }
        ));
    }

    #[test]
    fn a_handoff_crosses_with_its_fragment() {
        let line = serde_json::to_string(&Event::Handoff {
            id: 5,
            url: "https://nexus.test/app/guest/challenge/token#abc".into(),
        })
        .unwrap();
        assert_eq!(
            line,
            r#"{"event":"handoff","id":5,"url":"https://nexus.test/app/guest/challenge/token#abc"}"#
        );
    }

    #[test]
    fn a_frame_rate_is_one_tagged_line() {
        let line = serde_json::to_string(&Command::FrameRate { id: 3, fps: 120 }).unwrap();
        assert_eq!(line, r#"{"cmd":"frame_rate","id":3,"fps":120}"#);
        assert!(matches!(
            serde_json::from_str(&line).unwrap(),
            Command::FrameRate { id: 3, fps: 120 }
        ));
    }

    #[test]
    fn events_round_trip_as_tagged_json() {
        let line = serde_json::to_string(&Event::Cursor {
            id: 1,
            cursor: Cursor::Hand,
        })
        .unwrap();
        assert_eq!(line, r#"{"event":"cursor","id":1,"cursor":"hand"}"#);
        assert!(matches!(
            serde_json::from_str(&line).unwrap(),
            Event::Cursor {
                cursor: Cursor::Hand,
                ..
            }
        ));
    }

    #[test]
    fn a_file_drag_is_one_tagged_line() {
        let enter = Command::FileDrag {
            id: 4,
            x: 10,
            y: 20,
            modifiers: 0,
            step: FileDragStep::Enter {
                paths: vec!["/tmp/a.png".into()],
            },
        };
        let line = serde_json::to_string(&enter).unwrap();
        assert_eq!(
            line,
            r#"{"cmd":"file_drag","id":4,"x":10,"y":20,"modifiers":0,"step":{"kind":"enter","paths":["/tmp/a.png"]}}"#
        );
        let Command::FileDrag { step, .. } = serde_json::from_str(&line).unwrap() else {
            panic!("not a file drag: {line}");
        };
        assert_eq!(
            step,
            FileDragStep::Enter {
                paths: vec!["/tmp/a.png".into()]
            }
        );
        let drop = r#"{"cmd":"file_drag","id":4,"x":1,"y":2,"modifiers":0,"step":{"kind":"drop"}}"#;
        assert!(matches!(
            serde_json::from_str(drop).unwrap(),
            Command::FileDrag {
                step: FileDragStep::Drop,
                ..
            }
        ));
    }

    #[test]
    fn the_new_cursors_cross_as_snake_case_and_the_old_ones_still_read() {
        let line = serde_json::to_string(&Event::Cursor {
            id: 1,
            cursor: Cursor::ResizeNorthWestSouthEast,
        })
        .unwrap();
        assert_eq!(
            line,
            r#"{"event":"cursor","id":1,"cursor":"resize_north_west_south_east"}"#
        );
        for (name, cursor) in [("arrow", Cursor::Arrow), ("hand", Cursor::Hand), ("text", Cursor::Text)] {
            let old = format!(r#"{{"event":"cursor","id":1,"cursor":"{name}"}}"#);
            assert!(matches!(serde_json::from_str(&old).unwrap(), Event::Cursor { cursor: got, .. } if got == cursor));
        }
    }

    #[test]
    fn a_frame_without_a_number_or_rects_is_the_whole_frame() {
        let old = r#"{"event":"frame","id":2,"path":"/dev/shm/f","width":4,"height":3}"#;
        let Event::Frame { seq, dirty, .. } = serde_json::from_str(old).unwrap() else {
            panic!("not a frame: {old}");
        };
        assert_eq!((seq, dirty), (0, Vec::new()));
        let line = serde_json::to_string(&Event::Frame {
            id: 2,
            path: "/nxw1-2-0".into(),
            width: 4,
            height: 3,
            seq: 7,
            dirty: vec![[1, 0, 2, 1]],
        })
        .unwrap();
        assert_eq!(
            line,
            r#"{"event":"frame","id":2,"path":"/nxw1-2-0","width":4,"height":3,"seq":7,"dirty":[[1,0,2,1]]}"#
        );
        let Event::Frame { seq, dirty, .. } = serde_json::from_str(&line).unwrap() else {
            panic!("not a frame: {line}");
        };
        assert_eq!((seq, dirty), (7, vec![[1, 0, 2, 1]]));
    }
}
