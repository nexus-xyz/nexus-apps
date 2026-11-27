//! The mode table: each agent's own settings for each Nexus mode. The expected values are written by hand, never
//! derived from the mapping functions, so a changed mapping fails here.

use super::*;
use crate::event::PermissionMode as M;

/// Every run the table covers: both Sandbox switch values, and every answer agy's last run gave about `auto`.
fn runs() -> Vec<RunFacts> {
    let mut runs = Vec::new();
    for sandbox in [false, true] {
        for auto_offered in [None, Some(false), Some(true)] {
            runs.push(RunFacts { sandbox, auto_offered });
        }
    }
    runs
}

fn settings(list: &[(&'static str, &str)]) -> ModeSettings {
    ModeSettings(list.iter().map(|(name, value)| (*name, value.to_string())).collect())
}

#[test]
fn every_agents_mode_settings() {
    // Claude Code: `--permission-mode` and `set_permission_mode` (harness/claude.rs `permission_mode`).
    let claude = [
        (M::Default, "default"),
        (M::AcceptEdits, "acceptEdits"),
        (M::Plan, "plan"),
        (M::Auto, "auto"),
        (M::BypassPermissions, "bypassPermissions"),
    ];
    // Codex: (approvalPolicy, sandbox, approvalsReviewer, collaborationMode) (harness/codex.rs `policy`, `reviewer`,
    // `collaboration_mode`). Accept edits asks before commands, as Ask first does.
    let codex = [
        (M::Default, ["untrusted", "workspace-write", "user", "default"]),
        (M::AcceptEdits, ["untrusted", "workspace-write", "user", "default"]),
        (M::Plan, ["on-request", "read-only", "user", "plan"]),
        (M::Auto, ["on-request", "workspace-write", "auto_review", "default"]),
        (M::BypassPermissions, ["never", "danger-full-access", "user", "default"]),
    ];
    // Nexus's own agent: the `approvals` level (harness/nexus.rs `approval_level`). Plan is read-only.
    let nexus = [
        (M::Default, "ask"),
        (M::AcceptEdits, "auto-edit"),
        (M::Plan, "read-only"),
        (M::Auto, "auto"),
        (M::BypassPermissions, "auto"),
    ];
    // Antigravity: (mode, what agy's last run said of `auto`) -> (ACP `modeId`, `toolPermission`, `--sandbox` with the
    // switch off) (harness/antigravity.rs `tool_permission`, `sandboxed`, `acp_mode_id`). Plan passes no
    // `toolPermission`. Full access never runs agy's `auto`. The switch adds `--sandbox` in every mode.
    let agy = |mode: M, auto: Option<bool>| -> (&str, Option<&str>, bool) {
        match (mode, auto) {
            (M::Default, _) => ("default", Some("request-review"), false),
            (M::AcceptEdits, _) => ("accept-edits", Some("request-review"), false),
            (M::Plan, _) => ("plan", None, false),
            (M::Auto, Some(true)) => ("auto", None, false),
            (M::Auto, Some(false)) => ("accept-edits", Some("proceed-in-sandbox"), true),
            (M::Auto, None) => ("auto", Some("proceed-in-sandbox"), true),
            (M::BypassPermissions, _) => ("accept-edits", Some("always-proceed"), false),
        }
    };
    for run in runs() {
        let case = format!("{run:?}");
        for (mode, value) in claude {
            assert_eq!(
                harness(SessionAgent::Claude).mode_settings(mode, &run),
                settings(&[("--permission-mode", value)]),
                "{mode:?} {case}"
            );
        }
        for (mode, [approval, sandbox, reviewer, collaboration]) in codex {
            assert_eq!(
                harness(SessionAgent::Codex).mode_settings(mode, &run),
                settings(&[
                    ("approvalPolicy", approval),
                    ("sandbox", sandbox),
                    ("approvalsReviewer", reviewer),
                    ("collaborationMode", collaboration),
                ]),
                "{mode:?} {case}"
            );
        }
        for (mode, level) in nexus {
            assert_eq!(
                harness(SessionAgent::Nexus).mode_settings(mode, &run),
                settings(&[("approvals", level)]),
                "{mode:?} {case}"
            );
        }
        #[cfg(feature = "cursor")]
        for (mode, expected) in [(PermissionMode::AcceptEdits, "agent"), (PermissionMode::Plan, "plan")] {
            assert_eq!(
                harness(SessionAgent::Cursor).mode_settings(mode, &run),
                settings(&[("modeId", expected)])
            );
        }
        #[cfg(feature = "opencode")]
        for mode in EVERY_MODE {
            assert_eq!(harness(SessionAgent::Opencode).mode_settings(mode, &run), settings(&[]));
        }
        for mode in EVERY_MODE {
            let (mode_id, permission, forced) = agy(mode, run.auto_offered);
            let mut expected = vec![("modeId", mode_id)];
            if let Some(permission) = permission {
                expected.push(("toolPermission", permission));
            }
            if forced || run.sandbox {
                expected.push(("--sandbox", "on"));
            }
            assert_eq!(
                harness(SessionAgent::Antigravity).mode_settings(mode, &run),
                settings(&expected),
                "{mode:?} {case}"
            );
            // The same flags reach agy's command line.
            let line = crate::harness::antigravity::args(mode, run.sandbox, run.auto_offered);
            assert_eq!(
                line.iter().any(|arg| arg == "--sandbox"),
                forced || run.sandbox,
                "{mode:?} {case}"
            );
            let passed = permission.map(|permission| format!(r#"{{"toolPermission":"{permission}"}}"#));
            assert_eq!(
                line.iter().find(|arg| arg.contains("toolPermission")),
                passed.as_ref(),
                "{mode:?} {case}"
            );
        }
    }
}

/// Each default is one of the modes the agent offers.
#[test]
fn every_agent_has_a_supported_default_mode() {
    assert_eq!(
        EVERY_MODE.map(|mode| mode.as_str()),
        ["default", "acceptEdits", "plan", "auto", "bypassPermissions"]
    );
    for agent in SessionAgent::ALL {
        let harness = harness(agent);
        let expected: &[PermissionMode] = match agent {
            #[cfg(feature = "cursor")]
            SessionAgent::Cursor => &[PermissionMode::AcceptEdits, PermissionMode::Plan],
            #[cfg(feature = "opencode")]
            SessionAgent::Opencode => &[PermissionMode::Default],
            _ => &EVERY_MODE,
        };
        assert_eq!(harness.offered_modes(), expected, "{agent:?}");
        assert!(expected.contains(&harness.default_mode()), "{agent:?}");
    }
}

/// Claude Code's command line carries its mode.
#[test]
fn claude_code_starts_in_its_mode() {
    for mode in EVERY_MODE {
        let args = crate::claude::args(mode, None, None, "s1", false, None, None, None);
        let at = args.iter().position(|arg| arg == "--permission-mode");
        assert_eq!(
            at.and_then(|at| args.get(at + 1)).map(String::as_str),
            Some(crate::harness::claude::permission_mode(mode)),
            "{mode:?}"
        );
    }
}
