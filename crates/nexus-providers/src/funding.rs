//! Who pays for a model's calls, and how much of a plan is left.

/// How much of a plan's limits is left for one model, every window that applies to it folded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    Left,
    UsedUp,
    Unknown,
}

impl Room {
    pub fn as_str(self) -> &'static str {
        match self {
            Room::Left => "left",
            Room::UsedUp => "usedUp",
            Room::Unknown => "unknown",
        }
    }
}

/// An index into one launch's plan accounts, on the wire only (`"account": 0`). It is never stored, and never an email
/// or a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccountHandle(pub u32);

/// Who pays for a model's calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Funding {
    /// The person's own plan account.
    Plan { account: AccountHandle },
    /// The person's own API key on this computer.
    LocalKey,
    /// A model server the person runs or chose, with no per-call price.
    OwnServer,
    /// The organization's key or Nexus credit, through Nexus Gateway.
    Gateway,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_text() {
        assert_eq!(Room::Left.as_str(), "left");
        assert_eq!(Room::UsedUp.as_str(), "usedUp");
        assert_eq!(Room::Unknown.as_str(), "unknown");
    }
}
