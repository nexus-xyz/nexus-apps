//! The composer's pending picks, matching `workbench/pending.ts`.
use crate::event::{Effort, Pending, PermissionMode};

pub const APPLIES_LATER: &str = "Applies at the agent's next step";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Values {
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub mode: PermissionMode,
    pub waiting_model: bool,
    pub waiting_effort: bool,
    pub waiting_mode: bool,
}

impl Values {
    pub fn with_pending(
        model: Option<&str>,
        effort: Option<Effort>,
        mode: PermissionMode,
        pending: Option<&Pending>,
    ) -> Self {
        Self {
            model: pending
                .and_then(|p| p.model.clone())
                .unwrap_or_else(|| model.map(str::to_owned)),
            effort: pending.and_then(|p| p.effort).unwrap_or(effort),
            mode: pending.and_then(|p| p.permission_mode).unwrap_or(mode),
            waiting_model: pending.is_some_and(|p| p.model.is_some()),
            waiting_effort: pending.is_some_and(|p| p.effort.is_some()),
            waiting_mode: pending.is_some_and(|p| p.permission_mode.is_some()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(pending: Option<&Pending>) -> Values {
        Values::with_pending(Some("sonnet"), None, PermissionMode::Default, pending)
    }

    #[test]
    fn saved_values_show_when_no_change_waits() {
        assert_eq!(
            shown(None),
            Values {
                model: Some("sonnet".into()),
                effort: None,
                mode: PermissionMode::Default,
                waiting_model: false,
                waiting_effort: false,
                waiting_mode: false,
            }
        );
    }

    #[test]
    fn each_pending_value_overrides_its_saved_value_until_the_host_settles_it() {
        let pending = Pending {
            permission_mode: Some(PermissionMode::Plan),
            effort: Some(Some(Effort::High)),
            model: None,
            fast: None,
        };
        let result = shown(Some(&pending));
        assert_eq!(result.model.as_deref(), Some("sonnet"));
        assert_eq!(result.effort, Some(Effort::High));
        assert_eq!(result.mode, PermissionMode::Plan);
        assert!(!result.waiting_model);
        assert!(result.waiting_effort && result.waiting_mode);
        assert_eq!(shown(None).mode, PermissionMode::Default);
    }

    #[test]
    fn explicit_default_resets_still_wait_after_a_remote_round_trip() {
        let pending: Pending =
            serde_json::from_str(r#"{"model":null,"effort":null}"#).expect("pending null choices fixture");
        assert_eq!(pending.model, Some(None));
        assert_eq!(pending.effort, Some(None));
        let result = Values::with_pending(
            Some("sonnet"),
            Some(Effort::High),
            PermissionMode::Default,
            Some(&pending),
        );
        assert_eq!(result.model, None);
        assert_eq!(result.effort, None);
        assert!(result.waiting_model && result.waiting_effort);
        let absent: Pending = serde_json::from_str("{}").expect("empty pending fixture");
        assert_eq!(shown(Some(&absent)), shown(None));
    }
}
