//! Whether the operating system asks apps to reduce motion, as a webview's `prefers-reduced-motion` reads it for the
//! Nexus app: macOS's Reduce motion, Windows' "Animation effects" off, and on Linux the desktop's animations off.
//!
//! On Linux it asks the settings portal for GNOME's `enable-animations` and KDE's `AnimationDurationFactor` (0 is
//! off), then GTK's own `gtk-enable-animations` in `gtk-3.0/settings.ini`, which KDE writes too. WebKitGTK, the Nexus
//! app's webview there, follows the same GTK setting.

/// Whether the system asks for less motion now. False when it can't tell.
pub fn reduces_motion() -> bool {
    platform::reduces_motion()
}

/// GTK's `gtk-enable-animations` in a `settings.ini`'s text: Some(true) when it turns them off.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn ini_reduces_motion(text: &str) -> Option<bool> {
    text.lines().rev().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "gtk-enable-animations").then(|| {
            let value = value.trim().to_ascii_lowercase();
            matches!(value.as_str(), "false" | "0" | "no")
        })
    })
}

#[cfg(target_os = "linux")]
mod platform {
    use gtk::gio;
    use gtk::glib::{ToVariant as _, Variant, VariantTy};

    /// One setting from the desktop portal (`org.freedesktop.portal.Settings.Read`), unwrapped. None when the portal
    /// or the setting is missing.
    fn portal(bus: &gio::DBusConnection, namespace: &str, key: &str) -> Option<Variant> {
        let answer = bus
            .call_sync(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Settings",
                "Read",
                Some(&(namespace, key).to_variant()),
                VariantTy::new("(v)").ok(),
                gio::DBusCallFlags::NONE,
                1000,
                gio::Cancellable::NONE,
            )
            .ok()?;
        // `Read` wraps the value in a second variant (`ReadOne` would not): take off every layer.
        let mut value = answer.child_value(0);
        while let Some(inner) = value.as_variant() {
            value = inner;
        }
        Some(value)
    }

    fn from_portal() -> Option<bool> {
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
        if let Some(on) = portal(&bus, "org.gnome.desktop.interface", "enable-animations").and_then(|v| v.get::<bool>())
        {
            return Some(!on);
        }
        let factor = portal(&bus, "org.kde.kdeglobals.KDE", "AnimationDurationFactor")?;
        factor
            .get::<f64>()
            .or_else(|| factor.str().and_then(|text| text.trim().parse().ok()))
            .map(|factor| factor <= 0.)
    }

    fn from_gtk_ini() -> Option<bool> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|dir| !dir.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config")))?;
        let text = std::fs::read_to_string(base.join("gtk-3.0/settings.ini")).ok()?;
        super::ini_reduces_motion(&text)
    }

    pub fn reduces_motion() -> bool {
        from_portal().or_else(from_gtk_ini).unwrap_or(false)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    pub fn reduces_motion() -> bool {
        objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }
}

#[cfg(windows)]
mod platform {
    /// `SPI_GETCLIENTAREAANIMATION`: whether Windows animates its controls ("Animation effects").
    const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SystemParametersInfoW(action: u32, param: u32, value: *mut core::ffi::c_void, ini: u32) -> i32;
    }

    pub fn reduces_motion() -> bool {
        let mut animate: i32 = 1;
        // SAFETY: the action writes one BOOL to the pointer, which lives for the call.
        let read = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animate).cast(), 0) };
        read != 0 && animate == 0
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    pub fn reduces_motion() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::ini_reduces_motion;
    use core::prelude::v1::test;

    #[test]
    fn reads_gtks_animation_setting() {
        assert_eq!(
            ini_reduces_motion("[Settings]\ngtk-enable-animations=false\n"),
            Some(true)
        );
        assert_eq!(
            ini_reduces_motion("[Settings]\ngtk-enable-animations = true\n"),
            Some(false)
        );
        assert_eq!(ini_reduces_motion("[Settings]\ngtk-theme-name=Breeze\n"), None);
    }
}
