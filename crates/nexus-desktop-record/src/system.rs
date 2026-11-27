//! What else a recording notes and where it goes, on macOS: the app in front (`NSWorkspace`), the address of a
//! browser's front tab (AppleScript through `osascript`; macOS asks the person once per browser whether Nexus may read
//! it, and a refusal only leaves the address out), and the clipboard (`NSPasteboard`: the contact sheet as an image and
//! the text that points an agent at the folder, on one item).
//!
//! Unsafe: reading AppKit's exported pasteboard type names.
#![allow(unsafe_code)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use objc2_app_kit::{NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString, NSWorkspace};
use objc2_foundation::{NSData, NSString};

#[derive(Debug, Clone, PartialEq)]
pub struct Front {
    pub name: String,
    pub bundle_id: Option<String>,
}

pub fn frontmost() -> Option<Front> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    Some(Front {
        name: app.localizedName().map(|name| name.to_string()).unwrap_or_default(),
        bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
    })
}

/// The AppleScript that reads a browser's front tab, for the browsers that have one (Firefox has none).
fn tab_script(bundle_id: &str) -> Option<String> {
    let chromium = [
        "com.google.Chrome",
        "com.google.Chrome.canary",
        "com.brave.Browser",
        "com.microsoft.edgemac",
        "com.vivaldi.Vivaldi",
        "org.chromium.Chromium",
        "company.thebrowser.Browser",
    ];
    if chromium.contains(&bundle_id) {
        Some(format!(
            "tell application id \"{bundle_id}\" to get URL of active tab of front window"
        ))
    } else if bundle_id == "com.apple.Safari" || bundle_id == "com.apple.SafariTechnologyPreview" {
        Some(format!(
            "tell application id \"{bundle_id}\" to get URL of front document"
        ))
    } else {
        None
    }
}

pub fn is_browser(bundle_id: Option<&str>) -> bool {
    bundle_id.and_then(tab_script).is_some()
}

/// The front tab's address, or `None` (not a browser, no window, or not allowed). Gives up after two seconds.
pub fn browser_url(bundle_id: &str) -> Option<String> {
    let script = tab_script(bundle_id)?;
    let mut child = Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && url.contains("://")).then_some(url)
}

/// Put the contact sheet (`png`) and `text` on the clipboard. Main thread.
pub fn copy(png: &[u8], text: &str) {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: AppKit's exported type names.
    let (png_type, string_type) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeString) };
    if !png.is_empty() {
        pasteboard.setData_forType(Some(&NSData::with_bytes(png)), png_type);
    }
    pasteboard.setString_forType(&NSString::from_str(text), string_type);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsers_with_a_tab_script() {
        assert!(is_browser(Some("com.apple.Safari")));
        assert!(is_browser(Some("company.thebrowser.Browser")));
        assert!(!is_browser(Some("org.mozilla.firefox")));
        assert!(!is_browser(None));
        assert_eq!(
            tab_script("com.brave.Browser").as_deref(),
            Some("tell application id \"com.brave.Browser\" to get URL of active tab of front window")
        );
    }
}
