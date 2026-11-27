//! `NEXUS_LAYOUT_DUMP=<file>`: every window's last frame, written to `<file>` as JSON a quarter second after it was
//! painted (the boxes, text lines and icons it drew, with their sizes, colors and, in debug builds, the `file:line`
//! that built them). Every painted string is recorded verbatim (chat messages, file paths, token prefixes on
//! Settings). Do not point this at a real profile, and do not attach a dump from one to an issue. For the UI polish
//! tools in `polish/`, which measure it against `polish/rules.toml`. Unset, this does nothing and GPUI records nothing
//! (`vendor/gpui-pre/src/layout_dump.rs`).

use std::time::Duration;

use gpui_kit::App;

pub fn start(cx: &mut App) {
    let Some(path) = std::env::var_os("NEXUS_LAYOUT_DUMP").filter(|path| !path.is_empty()) else {
        return;
    };
    gpui_kit::layout_dump::enable(path.into());
    cx.spawn(async |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            if let Err(error) = gpui_kit::layout_dump::flush() {
                eprintln!("nexus-desktop-gpui: layout dump: {error}");
            }
        }
    })
    .detach();
}
