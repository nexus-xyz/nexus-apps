//! Nexus's voice. On macOS it is the system's speech synthesizer (AppKit's `NSSpeechSynthesizer`, the Mac's spoken
//! content voice), as the web uses the browser's and the phone its own: Gateway has no text-to-speech, and none is added.
//! It runs on the main thread, where AppKit wants it. The pane polls whether it still speaks; it has no word events here,
//! so the pane pulses the level as the web does for a voice that sends none.
//!
//! Voice mode is built for macOS only. Elsewhere the voice says nothing, and the composer shows no voice button.

/// What the pane needs of a voice.
pub trait Speaker {
    /// Start saying `text`; false when it could not start.
    fn speak(&mut self, text: &str) -> bool;
    /// Stop at once.
    fn stop(&mut self);
    /// Whether it still speaks.
    fn speaking(&self) -> bool;
}

/// Whether this build has voice mode.
pub const BUILT: bool = cfg!(target_os = "macos");

/// The system's voice.
pub fn system() -> Box<dyn Speaker> {
    Box::new(SystemVoice::default())
}

// AVSpeechSynthesizer is AVFAudio's, a framework this app does not link yet; NSSpeechSynthesizer is AppKit's and says
// the same system voices. Its deprecation is only that pointer.
#[cfg(target_os = "macos")]
#[derive(Default)]
#[allow(deprecated)]
struct SystemVoice {
    synthesizer: Option<objc2::rc::Retained<objc2_app_kit::NSSpeechSynthesizer>>,
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
impl Speaker for SystemVoice {
    fn speak(&mut self, text: &str) -> bool {
        let synthesizer = self
            .synthesizer
            .get_or_insert_with(objc2_app_kit::NSSpeechSynthesizer::new);
        synthesizer.stopSpeaking();
        synthesizer.startSpeakingString(&objc2_foundation::NSString::from_str(text))
    }

    fn stop(&mut self) {
        if let Some(synthesizer) = &self.synthesizer {
            synthesizer.stopSpeaking();
        }
    }

    fn speaking(&self) -> bool {
        self.synthesizer
            .as_ref()
            .is_some_and(|synthesizer| synthesizer.isSpeaking())
    }
}

#[cfg(not(target_os = "macos"))]
#[derive(Default)]
struct SystemVoice;

#[cfg(not(target_os = "macos"))]
impl Speaker for SystemVoice {
    fn speak(&mut self, _: &str) -> bool {
        false
    }

    fn stop(&mut self) {}

    fn speaking(&self) -> bool {
        false
    }
}
