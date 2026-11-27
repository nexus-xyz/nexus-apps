//! A Nexus session marks the commits it makes. The run's environment adds a `commit-msg` hook through git
//! config (`GIT_CONFIG_PARAMETERS`, git's own `-c`), so nothing is written to the repository or to the person's
//! config. The hook ([`HOOK`], `commit_trailer.sh`) adds a `Co-authored-by` trailer that names the surface. GitHub keeps
//! the trailer when it squashes a pull request, so `git log` on main says which merged work came from a Nexus session.
//!
//! - Nexus is the only agent co-author: the hook removes the trailer Claude Code adds for Anthropic
//!   (`<noreply@anthropic.com>`). A person named as a co-author stays.
//! - Desktop includes every Nexus organization member, as [`desktop_eligible`] decides. CLIs use [`crate::staff`].
//! - Only repositories whose push remote is in nexus-xyz (its URL as git expands it, on any host name), and only new
//!   commits: not a merge, a rebase, a cherry-pick or a revert. The hook never fails a commit, and the repository's
//!   own hooks still run after it.
//! - Git 2.54 and later run hooks from config; older git ignores the setting and commits as before.
//! - A person turns it off with `git config --global hook.nexus-commit-trailer.enabled false`.
//! - Shared by Nexus for desktop and the Rust CLI; `commit-trailer.ts` is the TypeScript CLI's copy.

use std::collections::HashMap;

/// The variable git reads `-c` settings from, and passes on to every git it runs.
pub const VARIABLE: &str = "GIT_CONFIG_PARAMETERS";
/// The hook's name in git config: `hook.nexus-commit-trailer.event` and `.command`.
pub const NAME: &str = "nexus-commit-trailer";
/// The hook: a shell function that git runs with the trailer and the message file.
pub const HOOK: &str = include_str!("commit_trailer.sh");
/// What the agent is told in its system prompt, next to the page instructions. Without it, an agent that finds its own
/// co-author line gone tells the person and offers to put it back. A message from the hook is not enough: the agent
/// reads it as untrusted output. True with or without the hook, so a launcher may add it to every session.
pub const INSTRUCTIONS: &str = include_str!("commit_trailer.txt").trim_ascii_end();

/// Where the session runs, named in the trailer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Desktop,
    Cli,
}

impl Surface {
    /// The trailer. Only Nexus controls nexus.inc, so only Nexus can tie these addresses to a GitHub account;
    /// verified on one, they show its avatar on every commit that carries them.
    pub fn trailer(self) -> &'static str {
        match self {
            Surface::Desktop => "Co-authored-by: Nexus Desktop <desktop@nexus.inc>",
            Surface::Cli => "Co-authored-by: Nexus CLI <cli@nexus.inc>",
        }
    }
}

/// The hook's shell command: the function, then its call with the trailer. Git appends the message file.
pub fn command(surface: Surface) -> String {
    format!(
        "{}\nnexus_commit_trailer \"{}\"",
        HOOK.replace('\r', "").trim_end(),
        surface.trailer()
    )
}

/// `existing` (the run's `GIT_CONFIG_PARAMETERS`) with the hook added after it. A run inside a Nexus session keeps
/// the outer session's hook, since the work started there. The entries use git's older `'key=value'` quoting, which
/// every git reads; the hook holds no quote and no `!`, so it needs no escaping.
pub fn parameters(existing: Option<&str>, surface: Surface) -> String {
    let existing = existing.unwrap_or_default().trim();
    if existing.contains(&format!("'hook.{NAME}.")) {
        return existing.to_string();
    }
    let ours = format!(
        "'hook.{NAME}.event=commit-msg' 'hook.{NAME}.command={}'",
        command(surface)
    );
    if existing.is_empty() {
        ours
    } else {
        format!("{existing} {ours}")
    }
}

/// Whether desktop marks this sign-in's commits. Every Nexus organization member qualifies, regardless of role,
/// email domain or staff marker. Existing staff eligibility remains supported, including personal organizations.
/// Checked at each launch or workbench commit; an unavailable Gateway does not establish eligibility.
pub async fn desktop_eligible(client: &crate::Client) -> bool {
    client
        .get("/v1/tenants/me")
        .await
        .is_ok_and(|me| desktop_eligible_identity(&me))
}

fn desktop_eligible_identity(me: &serde_json::Value) -> bool {
    crate::staff::is_employee(me) || me.pointer("/tenant/slug").and_then(serde_json::Value::as_str) == Some("nexus-xyz")
}

/// Adds the hook to an agent's environment.
pub fn mark(env: &mut HashMap<String, String>, surface: Surface) {
    let value = parameters(env.get(VARIABLE).map(String::as_str), surface);
    env.insert(VARIABLE.into(), value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_nexus_org_member_is_eligible_without_staff_or_email_requirements() {
        use serde_json::json;
        for role in [json!("owner"), json!("admin"), json!("member"), json!(null)] {
            for user in [json!({"email": "contractor@example.com"}), json!({}), json!(null)] {
                let me = json!({"tenant": {"slug": "nexus-xyz"}, "role": role, "user": user});
                assert!(desktop_eligible_identity(&me), "{me}");
            }
        }
        assert!(desktop_eligible_identity(&json!({"tenant": {"slug": "nexus-xyz"}})));
    }

    #[test]
    fn desktop_eligibility_follows_the_current_org_without_granting_staff_privileges() {
        use serde_json::json;
        let mut me = json!({"tenant": {"slug": "nexus-xyz"}, "user": {"email": "contractor@example.com"}});
        assert!(desktop_eligible_identity(&me));
        assert!(!crate::staff::is_employee(&me));
        for slug in ["personal-person", "acme", "nexus-xyz-other", "NEXUS-XYZ", ""] {
            me["tenant"] = json!({"slug": slug, "name": "Nexus", "mark": "staff"});
            assert!(!desktop_eligible_identity(&me), "{me}");
        }
        for user in [json!({"mark": "staff"}), json!({"email": "Person@Nexus.xyz"})] {
            me["user"] = user;
            assert!(desktop_eligible_identity(&me), "existing eligibility stays: {me}");
        }
        assert!(!desktop_eligible_identity(&json!({})));
    }

    #[test]
    fn the_hook_travels_quoted_without_escaping() {
        for surface in [Surface::Desktop, Surface::Cli] {
            let command = command(surface);
            assert!(!command.contains('\''), "a single quote would end the quoted value");
            assert!(!command.contains('!'), "git escapes ! inside a quoted value");
            assert!(!command.contains('\r'));
            assert!(command.ends_with(&format!("nexus_commit_trailer \"{}\"", surface.trailer())));
        }
    }

    #[test]
    fn the_hook_follows_what_the_run_already_sets() {
        let ours = parameters(None, Surface::Cli);
        assert!(ours.starts_with("'hook.nexus-commit-trailer.event=commit-msg' 'hook.nexus-commit-trailer.command="));
        assert_eq!(parameters(Some("  "), Surface::Cli), ours);
        let theirs = "'user.name'='Someone'";
        assert_eq!(parameters(Some(theirs), Surface::Cli), format!("{theirs} {ours}"));
    }

    #[test]
    fn a_session_inside_a_session_keeps_the_outer_hook() {
        let outer = parameters(None, Surface::Desktop);
        assert_eq!(parameters(Some(&outer), Surface::Cli), outer);
        // git's newer quoting, as `git -c` writes it, counts too.
        let newer = "'hook.nexus-commit-trailer.event'='commit-msg'";
        assert_eq!(parameters(Some(newer), Surface::Cli), newer);
        let mut env = HashMap::from([(VARIABLE.to_string(), outer.clone())]);
        mark(&mut env, Surface::Cli);
        assert_eq!(env[VARIABLE], outer);
    }
}
