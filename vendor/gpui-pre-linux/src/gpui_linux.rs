#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

pub use linux::{current_platform, linux_platform};
// Nexus: see `vendor/README.md`.
pub use linux::{set_fallback_appearance, window_blur_available};
