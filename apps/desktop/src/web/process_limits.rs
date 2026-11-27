//! macOS memory: keep GPU work in the CEF host and skip speculative spare renderers.

use cef::{CommandLine, ImplCommandLine};

const SPARE: &str = "SpareRendererForSitePerProcess";

/// Runs in `on_before_command_line_processing`: a `disable-features` value that CEF or Chromium sets after it wins.
/// The merge is `handlers::with_feature`, tested on every system.
pub(super) fn apply(command_line: &mut CommandLine) {
    // Keep GPU rendering and WebGL enabled. A GPU failure can now stop all pages in the separate CEF host.
    command_line.append_switch(Some(&"in-process-gpu".into()));
    super::handlers::disable_feature(command_line, SPARE);
}
