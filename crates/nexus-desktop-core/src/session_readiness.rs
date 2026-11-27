//! Session start rules.
use nexus_client::local_agents::{LocalAgent, Login};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Budget {
    #[default]
    Unknown,
    Remaining,
    Exhausted {
        resets_at: Option<String>,
    },
}

impl Budget {
    pub fn read(value: &Value) -> Self {
        let amount = value
            .get("remainingUsdDecimal")
            .filter(|value| !value.is_null())
            .or_else(|| value.get("remainingUsd"));
        let text = match amount {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Number(number)) => number.to_string(),
            _ => return Self::Unknown,
        };
        match nonpositive(&text) {
            None => Self::Unknown,
            Some(false) => Self::Remaining,
            Some(true) => Self::Exhausted {
                resets_at: value.get("resetsAt").and_then(Value::as_str).map(str::to_owned),
            },
        }
    }

    pub fn exhausted(&self) -> bool {
        matches!(self, Self::Exhausted { .. })
    }

    /// Subscription-backed turns do not spend the organization Gateway budget.
    pub fn blocks_send(&self, gateway: bool) -> bool {
        gateway && self.exhausted()
    }

    pub fn line(&self, organization: &str, reset_day: Option<&str>) -> Option<String> {
        if !self.exhausted() {
            return None;
        }
        let org = if organization.is_empty() {
            "Your organization"
        } else {
            organization
        };
        Some(format!(
            "{org}'s budget is used up. Calls resume {}when an owner raises it.",
            reset_day.map_or(String::new(), |day| format!("{day} or "))
        ))
    }
}

// Inspect decimal digits directly: tiny positive decimal strings must not round to zero.
fn nonpositive(value: &str) -> Option<bool> {
    let value = value.trim();
    let negative = value.starts_with('-');
    let unsigned = value.strip_prefix(['-', '+']).unwrap_or(value);
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, None), |(m, e)| (m, Some(e)));
    if let Some(exponent) = exponent {
        let digits = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    }
    let mut digits = 0;
    let mut dots = 0;
    let mut zero = true;
    for byte in mantissa.bytes() {
        if byte == b'.' {
            dots += 1;
        } else if byte.is_ascii_digit() {
            digits += 1;
            zero &= byte == b'0';
        } else {
            return None;
        }
    }
    (digits > 0 && dots <= 1).then_some(zero || negative)
}

/// A plan login error does not invent a blocker; the host can report its own launch failure.
pub fn plan_login_blocks(
    needed: bool,
    pending: bool,
    failed: bool,
    another_claude: bool,
    local: Option<&LocalAgent>,
) -> bool {
    needed
        && !failed
        && !another_claude
        && (pending || local.is_some_and(|local| matches!(local.login, Some(Login::ApiKey | Login::SignedOut))))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    Install,
    Offline,
    CloudPlan,
    PlanLogin,
    NoKey,
    Budget,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Facts {
    pub agent_missing: bool,
    pub offline: bool,
    pub cloud_plan: bool,
    pub login_blocks: bool,
    pub login_installed: bool,
    pub no_key: bool,
    pub budget: bool,
}

pub fn blockers(facts: Facts) -> Vec<Blocker> {
    let mut out = Vec::new();
    if facts.agent_missing {
        out.push(Blocker::Install);
    }
    if facts.offline {
        out.push(Blocker::Offline);
    }
    if facts.cloud_plan {
        out.push(Blocker::CloudPlan);
    }
    if facts.login_blocks && facts.login_installed {
        out.push(Blocker::PlanLogin);
    }
    if facts.no_key {
        out.push(Blocker::NoKey);
    }
    if facts.budget {
        out.push(Blocker::Budget);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn budgets_keep_unknown_distinct_and_preserve_tiny_positive_decimals() {
        assert_eq!(Budget::read(&json!({})), Budget::Unknown);
        assert_eq!(Budget::read(&json!({"remainingUsd":null})), Budget::Unknown);
        assert_eq!(Budget::read(&json!({"remainingUsd":"invalid"})), Budget::Unknown);
        assert_eq!(
            Budget::read(&json!({"remainingUsdDecimal":"0.00000000000000000000000000001"})),
            Budget::Remaining
        );
        assert_eq!(Budget::read(&json!({"remainingUsd":1})), Budget::Remaining);
        for amount in [json!(0), json!("0.0000"), json!("-1.5")] {
            assert!(Budget::read(&json!({"remainingUsd":amount})).exhausted());
        }
        assert_eq!(Budget::Unknown.line("Acme", None), None);
        assert_eq!(
            Budget::Exhausted { resets_at: None }
                .line("Acme", Some("Nov 1"))
                .as_deref(),
            Some("Acme's budget is used up. Calls resume Nov 1 or when an owner raises it.")
        );
    }

    #[test]
    fn pending_and_signed_out_logins_block_but_failed_checks_and_other_claude_accounts_do_not() {
        let local = LocalAgent {
            agent: "claude".into(),
            installed: true,
            path: None,
            version: None,
            login: Some(Login::SignedOut),
            plan: None,
        };
        assert!(plan_login_blocks(true, true, false, false, None));
        assert!(plan_login_blocks(true, false, false, false, Some(&local)));
        assert!(!plan_login_blocks(true, false, true, false, Some(&local)));
        assert!(!plan_login_blocks(true, false, false, true, Some(&local)));
        assert!(!plan_login_blocks(false, true, false, false, Some(&local)));
        assert!(!plan_login_blocks(true, false, false, false, None));
    }

    #[test]
    fn sends_block_only_for_known_exhausted_gateway_budgets() {
        for budget in [
            Budget::Unknown,
            Budget::Remaining,
            Budget::Exhausted { resets_at: None },
        ] {
            assert!(!budget.blocks_send(false));
            assert_eq!(budget.blocks_send(true), budget.exhausted());
        }
    }

    #[test]
    fn blockers_come_in_their_fixed_order() {
        let facts = Facts {
            agent_missing: true,
            offline: true,
            cloud_plan: true,
            login_blocks: true,
            login_installed: true,
            no_key: true,
            budget: true,
        };
        assert_eq!(
            blockers(facts),
            [
                Blocker::Install,
                Blocker::Offline,
                Blocker::CloudPlan,
                Blocker::PlanLogin,
                Blocker::NoKey,
                Blocker::Budget
            ]
        );
        assert!(blockers(Facts::default()).is_empty());
        assert!(
            blockers(Facts {
                login_blocks: true,
                ..Facts::default()
            })
            .is_empty()
        );
    }
}
