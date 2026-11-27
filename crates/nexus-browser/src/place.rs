//! Where a Code session's browser shows pages: a session on this computer, one on a cloud machine, or one on a server.
//! A cloud session's browser opens the machine's ports, forwarded to this computer over SSH (`cloud_preview`). It keeps
//! them in the machine's own unsaved profile ([`crate::profile::cloud_scope`]). The agent does not drive it: the agent
//! runs on the machine, with its own browser. The person's browser sign-ins are not copied into it: the page is the
//! machine's. A server session's browser is the same, except that the local addresses its agent prints are the
//! server's: Nexus GPUI checks and opens them through an SSH forward ([`session_with_servers`]). Its agent drives it
//! too: the server's host passes the agent's browser calls to the app over the SSH connection. Nexus GPUI
//! (`side_pane.rs`) decides here.

use serde_json::Value;

/// The session a browser is for, once found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// On this computer.
    Local,
    /// On this cloud machine.
    Cloud(String),
    /// On this server: its `localhost` is reached through an SSH forward.
    Server(String),
}

/// Why a cloud session's browser refuses agent control.
pub const CLOUD_AGENT: &str = "The agent on a cloud machine uses the machine's own browser.";
/// Why a cloud session's browser refuses "Import sign-ins".
pub const CLOUD_SIGN_INS: &str = "Sign-ins from your browser are for sessions on this computer.";

/// The session a browser call names: one on this computer (`local`), else one on the cloud machine `machine` whose
/// relay carries it (`cloud_holds(machine)`: what the relay already knows, no call to the machine). Anything else is not
/// found: no machine, a server's session (a server has no browser yet), a session the machine's relay never carried,
/// or an id no host makes.
pub fn session(
    local: bool,
    thread: &str,
    machine: Option<&str>,
    cloud_holds: impl FnOnce(&str) -> bool,
) -> Result<Place, String> {
    session_with_servers(local, thread, machine, cloud_holds, |_| false)
}

/// [`session`], where a session on server `machine` (`is_server(machine)`) gets a browser too: Nexus GPUI's.
pub fn session_with_servers(
    local: bool,
    thread: &str,
    machine: Option<&str>,
    cloud_holds: impl FnOnce(&str) -> bool,
    is_server: impl FnOnce(&str) -> bool,
) -> Result<Place, String> {
    if local {
        return Ok(Place::Local);
    }
    let shaped = !thread.is_empty()
        && thread.len() <= 128
        && thread
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    match machine {
        Some(machine) if shaped && cloud_holds(machine) => Ok(Place::Cloud(machine.to_string())),
        Some(machine) if shaped && is_server(machine) => Ok(Place::Server(machine.to_string())),
        _ => Err("Session not found.".into()),
    }
}

impl Place {
    /// The machine, for a cloud or a server session.
    pub fn machine(&self) -> Option<&str> {
        match self {
            Place::Local => None,
            Place::Cloud(machine) | Place::Server(machine) => Some(machine),
        }
    }

    /// The server, for a server session: its dev servers open through an SSH forward.
    pub fn server(&self) -> Option<&str> {
        match self {
            Place::Server(machine) => Some(machine),
            _ => None,
        }
    }

    /// Whether "Import sign-ins" copies the person's browser cookies into it: only on this computer.
    pub fn imports_sign_ins(&self) -> bool {
        matches!(self, Place::Local)
    }

    /// Whether its browser checks and opens the local addresses the session prints: this computer's, or a server's
    /// through an SSH forward. A cloud machine's `localhost` is the machine's own: its pages come only from the Cloud
    /// panel's Preview.
    pub fn checks_servers(&self) -> bool {
        matches!(self, Place::Local | Place::Server(_))
    }

    /// Whether the session's agent drives its browser: on this computer, and on a server, whose host passes the
    /// agent's browser calls to the app. A cloud machine's agent uses the machine's own browser.
    pub fn agent_drives(&self) -> bool {
        matches!(self, Place::Local | Place::Server(_))
    }

    /// Whether the browser does `action` with `args`. A cloud session's browser
    /// shows pages, but refuses agent control and the person's sign-ins. A server session's refuses the sign-ins only.
    pub fn allows(&self, action: &str, args: &Value) -> Result<(), String> {
        if self.imports_sign_ins() {
            return Ok(());
        }
        if !self.agent_drives()
            && matches!(action, "agentControl" | "authorizeAll")
            && args["enabled"].as_bool() == Some(true)
        {
            return Err(CLOUD_AGENT.into());
        }
        if matches!(action, "cookieSources" | "importCookies") {
            return Err(CLOUD_SIGN_INS.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_local_session_or_one_a_cloud_machines_relay_carries_gets_a_browser() {
        let cloud_carries = |machine: &str| machine == "m_cloud";
        let thread = "6f1c2a9e-0b7d-4c55-9a51-3f2b8f0c1d2e";
        assert_eq!(session(true, thread, None, |_| false), Ok(Place::Local));
        assert_eq!(
            session(false, thread, Some("m_cloud"), cloud_carries),
            Ok(Place::Cloud("m_cloud".into()))
        );
        // An unknown session with no machine, or naming a server, or a cloud machine whose relay never carried it.
        assert!(session(false, thread, None, |_| true).is_err());
        assert!(session(false, thread, Some("m_server"), cloud_carries).is_err());
        assert!(session(false, thread, Some("m_cloud"), |_| false).is_err());
        // An id no host makes is refused before the relay is asked.
        for bad in ["", "../x", "a b", "chat:main:acme", &"a".repeat(129)] {
            assert!(
                session(false, bad, Some("m_cloud"), |_| panic!("asked for {bad:?}")).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_cloud_sessions_browser_shows_pages_without_agent_control_or_local_sign_ins() {
        let cloud = Place::Cloud("m_cloud".into());
        assert_eq!(cloud.machine(), Some("m_cloud"));
        assert!(!cloud.imports_sign_ins());
        assert!(!cloud.checks_servers());
        for action in ["open", "info", "reload", "back", "screenshot", "pick", "tabs", "close"] {
            assert_eq!(cloud.allows(action, &json!({})), Ok(()), "{action}");
        }
        for action in ["agentControl", "authorizeAll"] {
            assert_eq!(
                cloud.allows(action, &json!({ "enabled": true })),
                Err(CLOUD_AGENT.into())
            );
            assert_eq!(cloud.allows(action, &json!({ "enabled": false })), Ok(()));
        }
        for action in ["cookieSources", "importCookies"] {
            assert_eq!(cloud.allows(action, &json!({})), Err(CLOUD_SIGN_INS.into()));
        }
    }

    #[test]
    fn a_server_session_gets_a_browser_that_checks_the_servers_addresses_only_where_servers_count() {
        let thread = "6f1c2a9e-0b7d-4c55-9a51-3f2b8f0c1d2e";
        let is_server = |machine: &str| machine == "m_server";
        assert_eq!(
            session_with_servers(false, thread, Some("m_server"), |_| false, is_server),
            Ok(Place::Server("m_server".into()))
        );
        assert_eq!(
            session_with_servers(true, thread, None, |_| false, |_| panic!("asked")),
            Ok(Place::Local),
            "a local session never asks"
        );
        assert!(session_with_servers(false, "../x", Some("m_server"), |_| false, is_server).is_err());
        assert!(session_with_servers(false, thread, Some("m_other"), |_| false, is_server).is_err());
        let server = Place::Server("m_server".into());
        assert_eq!(server.machine(), Some("m_server"));
        assert_eq!(server.server(), Some("m_server"));
        assert_eq!(Place::Cloud("m".into()).server(), None);
        assert_eq!(Place::Local.server(), None);
        assert!(server.checks_servers());
        assert!(!server.imports_sign_ins());
    }

    #[test]
    fn a_server_sessions_agent_drives_its_browser_but_gets_no_local_sign_ins() {
        let server = Place::Server("m_server".into());
        assert!(server.agent_drives());
        for action in ["agentControl", "authorizeAll"] {
            assert_eq!(server.allows(action, &json!({ "enabled": true })), Ok(()), "{action}");
        }
        for action in ["cookieSources", "importCookies"] {
            assert_eq!(server.allows(action, &json!({})), Err(CLOUD_SIGN_INS.into()));
        }
        assert!(Place::Local.agent_drives());
        assert!(!Place::Cloud("m_cloud".into()).agent_drives());
    }

    #[test]
    fn a_local_sessions_browser_does_everything() {
        let local = Place::Local;
        assert_eq!(local.machine(), None);
        assert!(local.imports_sign_ins() && local.checks_servers());
        for action in ["agentControl", "authorizeAll", "cookieSources", "importCookies", "open"] {
            assert_eq!(local.allows(action, &json!({ "enabled": true })), Ok(()), "{action}");
        }
    }
}
