//! The tier of the person's own Claude or ChatGPT plan, as `POST /v1/agents/sessions` takes it in `plan` (the
//! collection contract). The app learns the agent's raw plan name when Settings probes the plans
//! ([`crate::plans`]) and when a session starts (Claude Code's `initialize` answer, Codex's `account/read`). It keeps
//! the latest tier per agent and login folder in memory, and adds it to the next session it opens. Only the tier
//! leaves the computer: never the raw name, the account or the email. A login the app has not read yet sends no
//! `plan`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use crate::event::SessionAgent;
use crate::harness::harness;

/// The latest tier per agent and login folder (None: the agent's own login).
type Tiers = Mutex<HashMap<(&'static str, Option<PathBuf>), &'static str>>;

fn known_tiers() -> &'static Tiers {
    static KNOWN: OnceLock<Tiers> = OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The contract's tier for an agent's raw plan name: Claude Code's `subscriptionType` (`max`, `Claude Max`,
/// `team`) or Codex's `planType` (`plus`, `business`, `edu`). A name outside the lists is `unknown`. Nexus's own
/// agent has no plan.
pub fn tier(agent: SessionAgent, raw: &str) -> Option<&'static str> {
    let lower = raw.trim().to_ascii_lowercase();
    // "Claude Max", "max_20x", "max-5x": the first word after the product's name.
    let word = lower
        .strip_prefix("claude")
        .unwrap_or(&lower)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|word| !word.is_empty())
        .unwrap_or_default();
    harness(agent).plan_tier(word)
}

/// Keeps the tier of `raw` for the login in `folder` (None: the agent's own login). An empty name changes nothing.
pub fn remember(agent: SessionAgent, folder: Option<&Path>, raw: Option<&str>) {
    let Some(tier) = raw
        .filter(|raw| !raw.trim().is_empty())
        .and_then(|raw| tier(agent, raw))
    else {
        return;
    };
    let mut known = known_tiers().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    known.insert((agent.as_str(), folder.map(Path::to_path_buf)), tier);
}

/// Forgets the tier of the login in `folder`: a read that worked found no plan (signed out, or on an API key).
pub fn forget(agent: SessionAgent, folder: Option<&Path>) {
    let mut known = known_tiers().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    known.remove(&(agent.as_str(), folder.map(Path::to_path_buf)));
}

/// The tier last read for the login in `folder`, if any.
pub fn known(agent: SessionAgent, folder: Option<&Path>) -> Option<&'static str> {
    let known = known_tiers().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    known.get(&(agent.as_str(), folder.map(Path::to_path_buf))).copied()
}

/// Adds `plan` to a session's open body when the tier is known; leaves it out otherwise (the Gateway refuses a
/// `plan` that is not a string).
pub fn add_to(body: &mut Value, agent: SessionAgent, folder: Option<&Path>) {
    if let (Some(tier), Some(fields)) = (known(agent, folder), body.as_object_mut()) {
        fields.insert("plan".into(), Value::from(tier));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_claude_plans_to_the_contracts_list() {
        for (raw, tier_name) in [
            ("pro", "pro"),
            ("max", "max"),
            ("Claude Max", "max"),
            ("max_20x", "max"),
            ("Claude Team", "team"),
            ("enterprise", "enterprise"),
            ("free", "free"),
            ("plus", "unknown"),
            ("something new", "unknown"),
        ] {
            assert_eq!(tier(SessionAgent::Claude, raw), Some(tier_name), "{raw}");
        }
    }

    #[test]
    fn maps_codex_plans_to_the_contracts_list() {
        for (raw, tier_name) in [
            ("free", "free"),
            ("plus", "plus"),
            ("pro", "pro"),
            ("team", "team"),
            ("business", "team"),
            ("enterprise", "enterprise"),
            ("edu", "enterprise"),
            ("max", "unknown"),
        ] {
            assert_eq!(tier(SessionAgent::Codex, raw), Some(tier_name), "{raw}");
        }
        assert_eq!(tier(SessionAgent::Nexus, "pro"), None, "Nexus's agent has no plan");
    }

    #[test]
    fn a_session_carries_the_tier_of_its_own_login_only() {
        // A folder of this test's own, so the process-wide memory is not shared with another test.
        let own = PathBuf::from("/synthetic/plan-tier-test/own");
        let other = PathBuf::from("/synthetic/plan-tier-test/other");
        remember(SessionAgent::Codex, Some(&own), Some("business"));
        remember(SessionAgent::Codex, Some(&own), Some(""));
        let mut body = json!({"agent": "codex", "via": "subscription"});
        add_to(&mut body, SessionAgent::Codex, Some(&own));
        assert_eq!(body["plan"], "team", "the tier, never the raw name");
        let mut unknown = json!({"agent": "codex"});
        add_to(&mut unknown, SessionAgent::Codex, Some(&other));
        assert!(unknown.get("plan").is_none(), "a login not read yet sends no plan");
        add_to(&mut unknown, SessionAgent::Claude, Some(&own));
        assert!(unknown.get("plan").is_none(), "another agent's login is another plan");
        forget(SessionAgent::Codex, Some(&own));
        let mut after = json!({"agent": "codex"});
        add_to(&mut after, SessionAgent::Codex, Some(&own));
        assert!(after.get("plan").is_none(), "a login that lost its plan sends none");
    }
}
