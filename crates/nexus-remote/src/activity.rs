//! Which calls are the person using a machine, and which only read in the background. The desktop app and a cloud
//! machine's host share these lists:
//!
//! - The app does not wake a sleeping cloud machine for a [`NO_WAKE`] or [`READS`] call: it answers "asleep" instead
//!   ([`wakes`]). Only acting on a session (sending a message, starting one, git, a terminal) wakes it.
//! - The host does not count a [`background`] call as the person interacting (`interactedAt`), so the sidebar and the
//!   relay's own reads do not keep a machine awake.

/// Calls that only read what the Code screen shows in the background (the sidebar, the new-session sheet): they get
/// "asleep" from a sleeping machine instead of waking it. Anything else (opening or starting a session, sending,
/// git) is the person using the machine, and wakes it first.
pub const NO_WAKE: [&str; 10] = [
    "wb_overview",
    "wb_agent_models",
    "wb_schedules",
    "wb_pull_requests",
    "wb_session_start_preference",
    "wb_project_clone_progress",
    "wb_cloud_setup",
    "wb_mention_prs",
    "wb_connection",
    "wb_plans",
];

/// Reads the person makes by opening a session to look at it: its log and what its side panes show. They do not wake
/// a sleeping machine either: people often open a session only to read it, so "anything else" above means a call in
/// neither list ([`wakes`]). Unlike [`NO_WAKE`], they still count as the person using an awake machine (all but the
/// log's catch-up, `wb_thread_events`).
pub const READS: [&str; 16] = [
    "wb_thread_page",
    "wb_thread_events",
    "wb_thread_stream",
    "wb_git_state",
    "wb_changed_turns",
    "wb_changes",
    "wb_turn_files",
    "wb_diff",
    "wb_diff_targets",
    "wb_worktrees",
    "wb_task_output",
    "wb_files",
    "wb_file_read",
    "wb_file_preview",
    "wb_file_media",
    "wb_project_scripts",
];

/// Whether `call` wakes a sleeping cloud machine: anything but a [`NO_WAKE`] or [`READS`] read, or a shell's own
/// upkeep ([`nexus_workbench::shell_pty::QUIET`]: a sleeping machine's shells are gone anyway).
pub fn wakes(call: &str) -> bool {
    !NO_WAKE.contains(&call) && !READS.contains(&call) && !nexus_workbench::shell_pty::QUIET.contains(&call)
}

/// Whether `call` is a read the app makes by itself, not the person using the machine: the [`NO_WAKE`] calls, the
/// host's own calls (`host_status`, which the relay reads on every connect), the relay's catch-up
/// (`wb_thread_events`), and a shell's reads, resizes and close. Every other call counts as interaction, terminal
/// input too, and opening a shell.
pub fn background(call: &str) -> bool {
    NO_WAKE.contains(&call)
        || call.starts_with("host_")
        || call == "wb_thread_events"
        || call == "wb_thread_stream"
        || nexus_workbench::shell_pty::QUIET.contains(&call)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_a_session_does_not_wake_its_machine_but_acting_does() {
        for call in NO_WAKE.iter().chain(&READS) {
            assert!(!wakes(call), "{call}");
        }
        for call in [
            "wb_send",
            "wb_send_files",
            "wb_thread_create",
            "wb_approve",
            "wb_commit",
            "term_write",
        ] {
            assert!(wakes(call), "{call}");
        }
        // Reading a session on an awake machine still keeps it awake.
        assert!(!background("wb_thread_page"));
    }

    #[test]
    fn opening_a_session_on_a_sleeping_machine_wakes_it_by_no_read() {
        // Everything an open session loads: its log, side panes and the project's commands (`wb_project_scripts`,
        // read by the top bar and the shortcuts sheet).
        for call in [
            "wb_thread_page",
            "wb_thread_events",
            "wb_git_state",
            "wb_changes",
            "wb_files",
            "wb_project_scripts",
        ] {
            assert!(READS.contains(&call), "{call}");
            assert!(!wakes(call), "{call}");
        }
        // The composer's plan usage poll is the app's own read: it never wakes, and it is not interaction.
        assert!(!wakes("wb_plans"));
        assert!(background("wb_plans"));
        // Saving commands, and opening or updating the pull request (commit, push), are acting.
        assert!(wakes("wb_project_scripts_set"));
        assert!(wakes("wb_pull_request"));
    }

    #[test]
    fn only_the_apps_own_reads_are_background() {
        for call in NO_WAKE {
            assert!(background(call), "{call}");
        }
        assert!(background("host_status"));
        assert!(background("wb_thread_events"));
        assert!(background("wb_thread_stream"));
        assert!(!wakes("wb_thread_stream"));
        for call in [
            "wb_send",
            "wb_thread_page",
            "wb_approve",
            "wb_git_state",
            "wb_commit",
            "term_write",
        ] {
            assert!(!background(call), "{call}");
        }
    }

    /// A shell on screen polls its output: that keeps no machine awake and wakes none. Typing, or opening one, is
    /// the person using the machine.
    #[test]
    fn a_shells_reads_are_background_and_its_input_is_not() {
        use nexus_workbench::shell_pty::{CLOSE, INPUT, OPEN, READ, RESIZE};
        for call in [READ, RESIZE, CLOSE] {
            assert!(background(call), "{call}");
            assert!(!wakes(call), "{call}");
        }
        for call in [INPUT, OPEN] {
            assert!(!background(call), "{call}");
            assert!(wakes(call), "{call}");
        }
    }
}
