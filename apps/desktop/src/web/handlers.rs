//! What CEF calls back into, in the web host process (`host.rs`): the app (command-line switches and the message
//! pump), and per page the client with its render handler (frames, popups and drags), display handler (title, address, cursor,
//! console), life-span handler (popups, close), request handler (a renderer crash, a session browser's address
//! rule, a document's single address, and a challenge page's handoff), download handler (a session browser or a document downloads nothing), load handler (loading, back and forward) and
//! DevTools observer (CDP replies). Each reports to the app as a protocol [`Event`]. CEF calls these on its UI thread,
//! which with the external message pump is the host's main thread.

use std::sync::{Arc, Mutex};

use cef::*;

/// CEF's native cursor handle, which `on_cursor_change` takes: its type differs on each system.
#[cfg(target_os = "linux")]
type CursorHandle = ::std::os::raw::c_ulong;
#[cfg(target_os = "macos")]
type CursorHandle = *mut u8;
#[cfg(target_os = "windows")]
type CursorHandle = cef::sys::HCURSOR;

use super::host::{self, PageState};
use super::protocol::{self, Event};

pub(super) type StateRef = Arc<Mutex<PageState>>;

wrap_app! {
    pub(super) struct NexusApp {
        ozone: Option<String>,
        gpu: bool,
    }

    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(Pump::new())
        }

        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(command_line) = command_line else {
                return;
            };
            // No first-run, crash-restore or keychain prompts: CEF is an engine inside the app, not a browser.
            command_line.append_switch(Some(&"no-first-run".into()));
            command_line.append_switch(Some(&"noerrdialogs".into()));
            command_line.append_switch(Some(&"hide-crash-restore-bubble".into()));
            command_line.append_switch(Some(&"use-mock-keychain".into()));
            command_line.append_switch(Some(&"password-store=basic".into()));
            // On macOS Chromium's notification bridge starts its "Helper (Alerts)" app at launch to check the
            // permission, and macOS then lists that helper in System Settings › Notifications. The app's own
            // notifications come from the main bundle (`src/notify.rs`), so Chromium's are off. CEF already put its
            // own features in the switch: add to them.
            if cfg!(target_os = "macos") {
                disable_feature(command_line, "SystemNotifications");
            }
            // Debug builds only, for finding a stutter inside Chromium, e.g. its own trace:
            // `NEXUS_WEB_CHROME_FLAGS="--trace-startup=cc,viz --trace-startup-file=/tmp/chrome.json
            // --trace-startup-duration=30"`. Only `--trace-*` switches: anything else (`--remote-debugging-port`,
            // `--disable-web-security`) would open the pages to other processes.
            #[cfg(debug_assertions)]
            for flag in std::env::var("NEXUS_WEB_CHROME_FLAGS").unwrap_or_default().split_whitespace() {
                let flag = flag.trim_start_matches('-');
                if !flag.starts_with("trace-") {
                    continue;
                }
                match flag.split_once('=') {
                    Some((name, value)) => command_line.append_switch_with_value(Some(&name.into()), Some(&value.into())),
                    None => command_line.append_switch(Some(&flag.into())),
                }
            }
            if let Some(ozone) = &self.ozone {
                command_line.append_switch_with_value(Some(&"ozone-platform".into()), Some(&ozone.as_str().into()));
            }
            // ANGLE on OpenGL paints shared frames as empty buffers; on Vulkan it fills them. ANGLE loads the
            // `libvulkan.so.1` beside libcef.so. `VulkanFromANGLE` gives the compositor ANGLE's Vulkan device:
            // without it, WebGL and worker canvases are missing from the frames.
            if self.gpu {
                command_line.append_switch_with_value(Some(&"use-angle".into()), Some(&"vulkan".into()));
                command_line.append_switch_with_value(
                    Some(&"enable-features".into()),
                    Some(&"Vulkan,VulkanFromANGLE,DefaultANGLEVulkan".into()),
                );
            }
        }
    }
}

wrap_browser_process_handler! {
    struct Pump {}

    impl BrowserProcessHandler {
        /// CEF asks for a turn of its message loop in `delay_ms` (now, when 0 or less). Called on any thread.
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            host::schedule_pump(delay_ms);
        }
    }
}

wrap_render_handler! {
    pub(super) struct Render {
        id: u32,
        state: StateRef,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            let Some(rect) = rect else {
                return;
            };
            let state = self.state.lock().expect("page state");
            // CEF wants a non-empty view, even before the first layout.
            rect.width = state.size.0.max(1);
            rect.height = state.size.1.max(1);
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, screen_info: Option<&mut ScreenInfo>) -> ::std::os::raw::c_int {
            let Some(screen_info) = screen_info else {
                return 0;
            };
            screen_info.device_scale_factor = self.state.lock().expect("page state").scale;
            1
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            if buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let started = std::time::Instant::now();
            let len = width as usize * height as usize * 4;
            // SAFETY: CEF hands a BGRA buffer of `width * height` pixels, valid for this call.
            let pixels = unsafe { std::slice::from_raw_parts(buffer, len) };
            // A `<select>`'s open list paints apart from the page; the host draws it over the page (`popup.rs`).
            if type_ == PaintElementType::POPUP {
                self.state.lock().expect("page state").paint_popup(self.id, pixels, width as u32, height as u32);
                return;
            }
            if type_ != PaintElementType::VIEW {
                return;
            }
            // What changed since the last paint: only that is written for the app.
            let dirty: Vec<[i32; 4]> = dirty_rects
                .unwrap_or_default()
                .iter()
                .map(|rect| [rect.x, rect.y, rect.width, rect.height])
                .collect();
            self.state.lock().expect("page state").paint(self.id, pixels, width as u32, height as u32, &dirty);
            // CPU frames count in the trace's paints too, not only GPU frames.
            super::host::painted(started);
        }

        fn on_accelerated_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            info: Option<&AcceleratedPaintInfo>,
        ) {
            if type_ != PaintElementType::VIEW {
                return;
            }
            let started = std::time::Instant::now();
            // Only Linux asks for GPU frames (`frames/linux.rs`).
            #[cfg(target_os = "linux")]
            if let Some(info) = info {
                super::frames::paint(self.id, info);
            }
            super::host::painted(started);
            #[cfg(not(target_os = "linux"))]
            let _ = info;
        }

        fn on_popup_show(&self, browser: Option<&mut Browser>, show: ::std::os::raw::c_int) {
            self.state.lock().expect("page state").show_popup(self.id, show == 1);
            // The host keeps the page under the popup from its next paint: ask for one now.
            if show == 1
                && let Some(host) = browser.and_then(|browser| browser.host())
            {
                host.invalidate(PaintElementType::VIEW);
            }
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            if let Some(rect) = rect {
                self.state.lock().expect("page state").place_popup(self.id, [rect.x, rect.y, rect.width, rect.height]);
            }
        }

        /// A drag inside the page (an HTML drag and drop, a dragged link or text). The page has no window for the
        /// system's drag, so the host plays it on the page itself as the mouse moves (`page_drag.rs`).
        fn start_dragging(
            &self,
            _browser: Option<&mut Browser>,
            drag_data: Option<&mut DragData>,
            allowed_ops: DragOperationsMask,
            x: ::std::os::raw::c_int,
            y: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let Some(drag_data) = drag_data else {
                return 0;
            };
            let cursor = self.state.lock().expect("page state").cursor;
            super::page_drag::start(self.id, Clone::clone(&*drag_data), super::page_drag::bits(&allowed_ops), (x, y), cursor);
            1
        }

        fn update_drag_cursor(&self, _browser: Option<&mut Browser>, operation: DragOperationsMask) {
            let operation = super::page_drag::bits(&operation);
            super::page_drag::operation(self.id, operation);
            self.state.lock().expect("page state").set_cursor(self.id, drag_cursor(operation));
        }
    }
}

/// The cursor for the operation a drop would do now, as a browser shows it.
fn drag_cursor(operation: u32) -> protocol::Cursor {
    const COPY: u32 = 1;
    const LINK: u32 = 2;
    match operation {
        0 => protocol::Cursor::NotAllowed,
        op if op & COPY != 0 => protocol::Cursor::Copy,
        op if op & LINK != 0 => protocol::Cursor::Alias,
        _ => protocol::Cursor::Arrow,
    }
}

/// The cursor for the one Chromium shows. Kinds the app's windows lack get the arrow.
fn cursor_of(kind: CursorType) -> protocol::Cursor {
    use protocol::Cursor;
    match kind {
        CursorType::HAND => Cursor::Hand,
        CursorType::IBEAM => Cursor::Text,
        CursorType::VERTICALTEXT => Cursor::VerticalText,
        CursorType::CROSS | CursorType::CELL => Cursor::Crosshair,
        CursorType::GRAB | CursorType::MOVE => Cursor::Grab,
        CursorType::GRABBING => Cursor::Grabbing,
        CursorType::NOTALLOWED | CursorType::NODROP | CursorType::DND_NONE => Cursor::NotAllowed,
        CursorType::COPY | CursorType::DND_COPY => Cursor::Copy,
        CursorType::ALIAS | CursorType::DND_LINK => Cursor::Alias,
        CursorType::CONTEXTMENU => Cursor::ContextMenu,
        CursorType::EASTRESIZE => Cursor::ResizeEast,
        CursorType::WESTRESIZE => Cursor::ResizeWest,
        CursorType::EASTWESTRESIZE => Cursor::ResizeEastWest,
        CursorType::NORTHRESIZE => Cursor::ResizeNorth,
        CursorType::SOUTHRESIZE => Cursor::ResizeSouth,
        CursorType::NORTHSOUTHRESIZE => Cursor::ResizeNorthSouth,
        CursorType::NORTHWESTRESIZE | CursorType::SOUTHEASTRESIZE | CursorType::NORTHWESTSOUTHEASTRESIZE => {
            Cursor::ResizeNorthWestSouthEast
        }
        CursorType::NORTHEASTRESIZE | CursorType::SOUTHWESTRESIZE | CursorType::NORTHEASTSOUTHWESTRESIZE => {
            Cursor::ResizeNorthEastSouthWest
        }
        CursorType::COLUMNRESIZE => Cursor::ResizeColumn,
        CursorType::ROWRESIZE => Cursor::ResizeRow,
        _ => Cursor::Arrow,
    }
}

wrap_display_handler! {
    pub(super) struct Display {
        id: u32,
        state: StateRef,
    }

    impl DisplayHandler {
        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.is_some_and(|frame| frame.is_main() == 1)
                && let Some(url) = url
            {
                host::emit(&Event::Address { id: self.id, url: url.to_string() });
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let title = title.map(|title| title.to_string()).unwrap_or_default();
            host::emit(&Event::Title { id: self.id, title });
        }

        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: CursorHandle,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> ::std::os::raw::c_int {
            self.state.lock().expect("page state").set_cursor(self.id, cursor_of(type_));
            // Handled: CEF has no window of its own to set it on.
            1
        }

        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            _source: Option<&CefString>,
            _line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            if std::env::var_os("NEXUS_WEB_CONSOLE").is_some()
                && let Some(message) = message
            {
                eprintln!("web console: {message}");
            }
            0
        }
    }
}

wrap_life_span_handler! {
    pub(super) struct LifeSpan {
        id: u32,
        // The session whose browser tab this is, if it is one.
        session: Option<String>,
        // The page's state, when the engine creates the page later (a popup, or a page in a website profile):
        // `on_after_created` hands it to the main loop.
        adopt: Option<StateRef>,
        // A session browser's popup: it counts against the session's limit.
        popup: bool,
        // The page's own state, for a popup's size and scale.
        state: StateRef,
        // A read-only document: it opens no window, not even in the person's browser.
        document: bool,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let url = target_url.map(|url| url.to_string()).unwrap_or_default();
            if self.document {
                return 1;
            }
            let Some(session) = &self.session else {
                // A link that opens a new window goes to the person's browser instead.
                if !url.is_empty() {
                    host::emit(&Event::Popup { id: self.id, url });
                }
                return 1;
            };
            // A session browser's popup (a sign-in, say) is a real page with its opener:
            // a web address or a blank page, at most MAX_TABS per session.
            let url = if url.is_empty() { "about:blank".to_string() } else { url };
            let (Some(window_info), Some(client)) = (window_info, client) else {
                return 1;
            };
            if !nexus_browser::popup_allowed(&url) || !host::take_popup(session) {
                return 1;
            }
            let id = host::popup_id();
            let scale = self.state.lock().expect("page state").scale;
            let state: StateRef = Arc::new(Mutex::new(host::PageState::new(
                id,
                POPUP_SIZE,
                scale,
                Some(session.clone()),
            )));
            window_info.windowless_rendering_enabled = 1;
            // CPU frames: the app draws a popup through the memory file.
            window_info.shared_texture_enabled = 0;
            *client = Some(page_client(id, &state, true, true));
            host::emit(&Event::PopupOpened { id: self.id, popup: id, url, user_gesture: user_gesture != 0 });
            0
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let (Some(state), Some(browser)) = (&self.adopt, browser) {
                host::adopt(self.id, browser.clone(), state.clone());
            }
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            if self.popup
                && let Some(session) = &self.session
            {
                host::drop_popup(session);
            }
            host::closed(self.id);
            host::emit(&Event::Closed { id: self.id });
            host::browser_closed();
        }
    }
}

wrap_request_handler! {
    pub(super) struct Request {
        id: u32,
        // A session's browser tab: it goes only to web addresses.
        browser: bool,
        // A read-only document's first address: the only one it goes to (`Command::Open`'s `document`). None on
        // other pages.
        document: Option<Arc<Mutex<Option<String>>>>,
        // The address the page hands its result over by (`Command::Open`'s `handoff`). None on other pages.
        handoff: Option<String>,
    }

    impl RequestHandler {
        /// A session browser's frames go only to web addresses and blank pages:
        /// never a file, a script or the app's own pages.
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut cef::Request>,
            _user_gesture: ::std::os::raw::c_int,
            _is_redirect: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let url = request.map(|request| CefString::from(&request.url()).to_string()).unwrap_or_default();
            // Guest chat's challenge page hands its token over by going to its handoff address: the app reads the
            // fragment, and the address never loads.
            if let Some(handoff) = &self.handoff
                && frame.as_ref().is_some_and(|frame| frame.is_main() == 1)
                && hands_off(handoff, &url)
            {
                host::emit(&Event::Handoff { id: self.id, url });
                return 1;
            }
            if let Some(first) = &self.document {
                let main = frame.is_some_and(|frame| frame.is_main() == 1);
                let mut first = first.lock().expect("document address");
                return (!stays(&mut first, main, &url)) as ::std::os::raw::c_int;
            }
            if !self.browser {
                return 0;
            }
            (!browsable(&url)) as ::std::os::raw::c_int
        }

        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            _status: TerminationStatus,
            _error_code: ::std::os::raw::c_int,
            _error_string: Option<&CefString>,
        ) {
            host::emit(&Event::Crashed { id: self.id });
        }
    }
}

wrap_load_handler! {
    pub(super) struct Load {
        id: u32,
    }

    impl LoadHandler {
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: ::std::os::raw::c_int) {
            if frame.is_some_and(|frame| frame.is_main() == 1) && (400..=599).contains(&http_status_code) {
                host::emit(&Event::LoadFailed { id: self.id, status: Some(http_status_code as u16) });
            }
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            error_code: Errorcode,
            _error_text: Option<&CefString>,
            _failed_url: Option<&CefString>,
        ) {
            if error_code != Errorcode::ABORTED && frame.is_some_and(|frame| frame.is_main() == 1) {
                host::emit(&Event::LoadFailed { id: self.id, status: None });
            }
        }

        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            host::emit(&Event::Loading {
                id: self.id,
                loading: is_loading == 1,
                back: can_go_back == 1,
                forward: can_go_forward == 1,
            });
        }
    }
}

wrap_download_handler! {
    pub(super) struct Download {}

    impl DownloadHandler {
        /// A session browser's page downloads nothing:
        /// no file lands on the computer from a website.
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            0
        }
    }
}

wrap_dev_tools_message_observer! {
    pub(super) struct DevTools {
        id: u32,
    }

    impl DevToolsMessageObserver {
        /// Every DevTools message for the page, replies and events alike, goes to the app as it came.
        fn on_dev_tools_message(&self, _browser: Option<&mut Browser>, message: Option<&[u8]>) -> ::std::os::raw::c_int {
            if let Some(message) = message {
                host::emit(&Event::Cdp {
                    id: self.id,
                    message: String::from_utf8_lossy(message).into_owned(),
                });
            }
            // Handled: CEF does not split it into a method result or an event too.
            1
        }
    }
}

wrap_client! {
    pub(super) struct PageClient {
        render: RenderHandler,
        display: DisplayHandler,
        life_span: LifeSpanHandler,
        request: RequestHandler,
        load: LoadHandler,
        download: Option<DownloadHandler>,
    }

    impl Client {
        fn download_handler(&self) -> Option<DownloadHandler> {
            self.download.clone()
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }

        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.request.clone())
        }
    }
}

/// Whether a session browser's frame may go to `url`: a web address or a blank page (`nexus_browser`'s rule), or
/// Chromium's own error page for an address that did not load. Refusing that page would leave the tab loading.
fn browsable(url: &str) -> bool {
    nexus_browser::popup_allowed(url) || url == "chrome-error://chromewebdata/"
}

/// Whether `url` is the `handoff` address, with or without a fragment.
fn hands_off(handoff: &str, url: &str) -> bool {
    url.split_once('#').map_or(url, |(address, _)| address) == handoff
}

/// Whether a read-only document goes to `url`: its first address in the main frame, which it keeps, and that address
/// again (a reload). Never another address, nor a frame inside it.
fn stays(first: &mut Option<String>, main: bool, url: &str) -> bool {
    if !main {
        return false;
    }
    match first {
        Some(first) => first == url,
        None => {
            *first = Some(url.to_string());
            true
        }
    }
}

/// A session browser's popup opens at this size, in logical pixels, as the Nexus app's "Browser sign-in" window.
pub(super) const POPUP_SIZE: (i32, i32) = (520, 680);

/// The client for page `id`, its handlers reading and writing `state`.
pub(super) fn client(id: u32, state: &StateRef) -> Client {
    page_client(id, state, false, false)
}

/// The client for a page the engine creates later (`browser_host_create_browser`): `on_after_created` hands it to the
/// main loop.
pub(super) fn later_client(id: u32, state: &StateRef) -> Client {
    page_client(id, state, false, true)
}

/// The client for page `id`; `popup` when the engine creates it as a session browser's popup.
fn page_client(id: u32, state: &StateRef, popup: bool, adopt: bool) -> Client {
    let (session, document, handoff) = {
        let state = state.lock().expect("page state");
        (state.browser.clone(), state.document, state.handoff.clone())
    };
    PageClient::new(
        Render::new(id, state.clone()),
        Display::new(id, state.clone()),
        LifeSpan::new(
            id,
            session.clone(),
            adopt.then(|| state.clone()),
            popup,
            state.clone(),
            document,
        ),
        Request::new(
            id,
            session.is_some(),
            document.then(|| Arc::new(Mutex::new(None))),
            handoff,
        ),
        Load::new(id),
        (session.is_some() || document).then(Download::new),
    )
}

/// The observer that sends page `id`'s DevTools messages to the app.
pub(super) fn dev_tools(id: u32) -> DevToolsMessageObserver {
    DevTools::new(id)
}

/// Adds `feature` to the command line's `--disable-features`, beside the features already there.
fn disable_feature(command_line: &mut CommandLine, feature: &str) {
    let name: CefString = "disable-features".into();
    let current = CefString::from(&command_line.switch_value(Some(&name))).to_string();
    command_line.remove_switch(Some(&name));
    let features = with_feature(&current, feature);
    command_line.append_switch_with_value(Some(&name), Some(&features.as_str().into()));
}

/// A `--disable-features` list with `feature` in it, once.
fn with_feature(list: &str, feature: &str) -> String {
    let mut features: Vec<&str> = list.split(',').map(str::trim).filter(|name| !name.is_empty()).collect();
    if !features.contains(&feature) {
        features.push(feature);
    }
    features.join(",")
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn system_notifications_join_the_features_that_cef_disabled() {
        assert_eq!(with_feature("", "SystemNotifications"), "SystemNotifications");
        assert_eq!(
            with_feature("AperitifHelpers,GlicActorUi", "SystemNotifications"),
            "AperitifHelpers,GlicActorUi,SystemNotifications"
        );
        assert_eq!(
            with_feature("SystemNotifications,LensOverlay", "SystemNotifications"),
            "SystemNotifications,LensOverlay"
        );
    }

    #[test]
    fn a_session_browser_goes_to_web_addresses_and_its_error_page_only() {
        for url in [
            "https://example.com/",
            "http://localhost:3000/",
            "about:blank",
            "chrome-error://chromewebdata/",
        ] {
            assert!(browsable(url), "{url}");
        }
        for url in [
            "file:///etc/passwd",
            "chrome://settings/",
            "chrome-error://other/",
            "javascript:alert(1)",
        ] {
            assert!(!browsable(url), "{url}");
        }
    }

    #[test]
    fn only_the_handoff_address_is_handed_off() {
        let handoff = "https://nexus.test/app/guest/challenge/token";
        assert!(hands_off(handoff, "https://nexus.test/app/guest/challenge/token#abc"));
        assert!(hands_off(
            handoff,
            "https://nexus.test/app/guest/challenge/token#error=300030"
        ));
        assert!(hands_off(handoff, handoff));
        assert!(!hands_off(handoff, "https://nexus.test/app/guest/challenge"));
        assert!(!hands_off(
            handoff,
            "https://nexus.test/app/guest/challenge/token/x#abc"
        ));
        assert!(!hands_off(handoff, "https://other.test/app/guest/challenge/token#abc"));
    }

    #[test]
    fn a_document_stays_at_its_first_address() {
        let mut first = None;
        assert!(!stays(&mut first, false, "https://example.com/"), "no frame inside it");
        assert!(stays(&mut first, true, "data:text/html;base64,PGI+"));
        assert!(stays(&mut first, true, "data:text/html;base64,PGI+"), "a reload");
        assert!(!stays(&mut first, true, "https://example.com/"));
        assert!(!stays(&mut first, true, "file:///etc/passwd"));
    }

    #[test]
    fn the_page_s_cursors_reach_the_window() {
        use protocol::Cursor;
        for (kind, cursor) in [
            (CursorType::POINTER, Cursor::Arrow),
            (CursorType::HAND, Cursor::Hand),
            (CursorType::IBEAM, Cursor::Text),
            (CursorType::GRAB, Cursor::Grab),
            (CursorType::GRABBING, Cursor::Grabbing),
            (CursorType::MOVE, Cursor::Grab),
            (CursorType::CROSS, Cursor::Crosshair),
            (CursorType::NOTALLOWED, Cursor::NotAllowed),
            (CursorType::EASTWESTRESIZE, Cursor::ResizeEastWest),
            (CursorType::COLUMNRESIZE, Cursor::ResizeColumn),
            (CursorType::NORTHWESTSOUTHEASTRESIZE, Cursor::ResizeNorthWestSouthEast),
            (CursorType::SOUTHWESTRESIZE, Cursor::ResizeNorthEastSouthWest),
            // The app's windows have no wait or zoom cursor.
            (CursorType::WAIT, Cursor::Arrow),
            (CursorType::ZOOMIN, Cursor::Arrow),
        ] {
            assert_eq!(cursor_of(kind), cursor, "{kind:?}");
        }
    }

    #[test]
    fn a_drag_shows_what_a_drop_would_do() {
        use protocol::Cursor;
        assert_eq!(drag_cursor(0), Cursor::NotAllowed);
        assert_eq!(drag_cursor(1), Cursor::Copy);
        assert_eq!(drag_cursor(2), Cursor::Alias);
        assert_eq!(drag_cursor(16), Cursor::Arrow);
    }
}
