//! The Assistant's two soft sounds, as Messages plays them: one when the person sends, one when Nexus's reply lands.
//! They play only while the window is in front, never with the app's "Assistant sounds" off (`paths::assistant_sounds`)
//! and never under less motion, which asks for a calmer app.
//!
//! On macOS they are the system's own "Pop" and "Tink" at a low volume, through `NSSound`, so nothing is bundled. Other
//! systems have no sound here yet.

/// What happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    Send,
    Receive,
}

impl Cue {
    /// The macOS system sound for it.
    pub fn system_name(self) -> &'static str {
        match self {
            Cue::Send => "Pop",
            Cue::Receive => "Tink",
        }
    }
}

/// How loud, from 0 to 1: under the system's alert volume, so it reads as a tap, not an alert.
#[cfg_attr(any(test, not(target_os = "macos")), allow(dead_code))]
const VOLUME: f32 = 0.25;

/// Whether a cue may play: the setting is on, motion is not reduced, and the window is in front.
pub fn should_play(on: bool, reduced_motion: bool, focused: bool) -> bool {
    on && !reduced_motion && focused
}

/// Play `cue` for `window`, when [`should_play`] allows it. Reads the setting from this build's settings file.
pub fn play(cue: Cue, window: &gpui_kit::Window, cx: &gpui_kit::App) {
    if !should_play(
        crate::paths::assistant_sounds(),
        cx.reduce_motion(),
        window.is_window_active(),
    ) {
        return;
    }
    platform::play(cue);
}

#[cfg(all(target_os = "macos", not(test)))]
mod platform {
    use objc2_app_kit::NSSound;
    use objc2_foundation::NSString;

    pub fn play(cue: super::Cue) {
        let Some(sound) = NSSound::soundNamed(&NSString::from_str(cue.system_name())) else {
            eprintln!("nexus-desktop-gpui: the system sound {} is missing", cue.system_name());
            return;
        };
        // A sound still playing from the last cue does not start again: `play` says false, and nothing is lost.
        sound.setVolume(super::VOLUME);
        sound.play();
    }
}

#[cfg(any(not(target_os = "macos"), test))]
mod platform {
    pub fn play(_cue: super::Cue) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cue_plays_only_on_with_full_motion_in_a_focused_window() {
        assert!(should_play(true, false, true));
        assert!(!should_play(false, false, true), "muted");
        assert!(!should_play(true, true, true), "less motion");
        assert!(!should_play(true, false, false), "the window is behind another");
    }

    #[test]
    fn the_cues_are_the_systems_soft_sounds() {
        assert_eq!(Cue::Send.system_name(), "Pop");
        assert_eq!(Cue::Receive.system_name(), "Tink");
    }
}
