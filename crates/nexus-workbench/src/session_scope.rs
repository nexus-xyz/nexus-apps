//! Local and account ledger keys as the desktop and service use them.

const LOCAL: &str = "|";

/// A ledger key from an older or account-scoped caller. Preserve its text exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonLocalScope<S = String>(S);

impl<S: AsRef<str>> NonLocalScope<S> {
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

/// A session ledger key. Local uses the legacy key `"|"`, and Unscoped uses the empty key.
/// A ledger key does not grant permission to launch or view a session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SessionScope<S = String> {
    Local,
    #[default]
    Unscoped,
    Other(NonLocalScope<S>),
}

impl<S: AsRef<str>> SessionScope<S> {
    pub fn from_text(text: S) -> Self {
        match text.as_ref() {
            LOCAL => Self::Local,
            "" => Self::Unscoped,
            _ => Self::Other(NonLocalScope(text)),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Local => LOCAL,
            Self::Unscoped => "",
            Self::Other(text) => text.as_str(),
        }
    }
}

impl SessionScope {
    pub fn local_text() -> &'static str {
        LOCAL
    }

    pub fn into_text(self) -> String {
        match self {
            Self::Local => LOCAL.into(),
            Self::Unscoped => String::new(),
            Self::Other(NonLocalScope(text)) => text,
        }
    }

    /// Classify a borrowed row or request without allocating its key.
    pub fn decode(text: &str) -> SessionScope<&str> {
        SessionScope::from_text(text)
    }
}

impl From<String> for SessionScope {
    fn from(text: String) -> Self {
        Self::from_text(text)
    }
}

impl From<&str> for SessionScope {
    fn from(text: &str) -> Self {
        match Self::decode(text) {
            SessionScope::Local => Self::Local,
            SessionScope::Unscoped => Self::Unscoped,
            SessionScope::Other(NonLocalScope(text)) => Self::Other(NonLocalScope(text.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_keys_round_trip_without_normalization() {
        assert_eq!(SessionScope::from("|"), SessionScope::Local);
        assert_eq!(SessionScope::from(""), SessionScope::Unscoped);
        for text in ["gateway|org", "gateway|", "|org", "old opaque scope", " | ", "||"] {
            let scope = SessionScope::from(text);
            assert!(matches!(scope, SessionScope::Other(_)), "{text:?}");
            let borrowed = SessionScope::decode(text);
            assert!(!borrowed.is_local());
            assert_eq!(borrowed.as_str(), text);
            assert_eq!(scope.as_str(), text);
            assert_eq!(scope.into_text(), text);
        }
        assert_eq!(SessionScope::Local.into_text(), "|");
        assert_eq!(SessionScope::local_text(), "|");
        assert_eq!(SessionScope::<String>::Unscoped.as_str(), "");
        assert_eq!(SessionScope::Unscoped.into_text(), "");
    }
}
