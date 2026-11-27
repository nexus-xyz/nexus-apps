//! Why Gateway refused a connector call for missing setup, from its error code: the person has no account for the
//! connector, their account expired, the organization has not turned it on for them, or this Gateway does not offer it
//! (`nexus-integrations`, `connectors/connections.rs` and `connectors/mod.rs`). A view shows the setup for these, and a
//! load failure (network, 5xx) for anything else.

/// What a view needs before a connector call can answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gap {
    /// `connection_required`: no account for it yet. Whoever holds [`CONNECT_OWN`] connects their own.
    NotConnected,
    /// `connection_expired`: the service refused the account's credential, so its holder connects it again.
    Expired,
    /// `connector_disabled`: the organization has not made it available to the person. [`MANAGE`] turns it on.
    Off,
    /// `connector_not_offered`, or connectors off on this Gateway (`not_configured`): nobody in the app can fix it.
    NotOffered,
}

/// The permission that connects one's own account (`POST /v1/connections/{connector}`).
pub const CONNECT_OWN: &str = "connections.manage_own";

/// The permission that decides who may use a connector (`PUT /v1/policies/connectors/{name}`).
pub const MANAGE: &str = "policy.manage";

/// The gap a Gateway error code names, if it names one.
pub fn of_code(code: &str) -> Option<Gap> {
    match code {
        "connection_required" => Some(Gap::NotConnected),
        "connection_expired" => Some(Gap::Expired),
        "connector_disabled" => Some(Gap::Off),
        "connector_not_offered" | "not_configured" => Some(Gap::NotOffered),
        _ => None,
    }
}

/// The gap behind a connector call's refusal, or None for a failure (network, 5xx, anything else).
pub fn classify(error: &nexus_client::Error) -> Option<Gap> {
    match error {
        nexus_client::Error::Core(error) => of_code(error.code.as_deref()?),
        nexus_client::Error::Transport(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_client::error::{CoreError, Transport};

    fn refusal(status: u16, code: &str) -> nexus_client::Error {
        CoreError {
            status,
            message: "refused".into(),
            code: Some(code.into()),
            interval: None,
            details: None,
        }
        .into()
    }

    #[test]
    fn a_missing_connection_is_setup_not_a_failure() {
        assert_eq!(classify(&refusal(403, "connection_required")), Some(Gap::NotConnected));
        assert_eq!(classify(&refusal(403, "connection_expired")), Some(Gap::Expired));
        assert_eq!(classify(&refusal(403, "connector_disabled")), Some(Gap::Off));
        assert_eq!(classify(&refusal(403, "connector_not_offered")), Some(Gap::NotOffered));
        assert_eq!(classify(&refusal(503, "not_configured")), Some(Gap::NotOffered));
    }

    #[test]
    fn a_real_load_error_stays_a_failure() {
        assert_eq!(classify(&refusal(502, "upstream_error")), None);
        assert_eq!(classify(&refusal(429, "rate_limited")), None);
        assert_eq!(classify(&Transport::Timeout.into()), None);
        assert_eq!(classify(&Transport::Other("offline".into()).into()), None);
    }
}
