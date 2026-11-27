//! The person's own settings that live on Gateway, read from its JSON: notification channels
//! (`GET /v1/notifications/settings`) and API keys (`GET /v1/auth/tokens`). Settings of this computer (the theme) are in
//! `paths.rs`.

use serde_json::Value;

use crate::model::text;

/// One group's setting on one channel (`kit/src/notifications.ts`).
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSetting {
    /// `my_sessions`, `usage_and_billing`, `requests_and_approvals`, `automations_and_integrations`,
    /// `account_and_security`.
    pub group: String,
    /// `inbox`, `email` or `push`.
    pub channel: String,
    pub on: bool,
    /// Inbox is always on; a required kind can't be turned off.
    pub locked: bool,
    /// Whether the channel exists for this person (push needs a device).
    pub available: bool,
}

impl ChannelSetting {
    pub fn from_json(setting: &Value) -> Option<ChannelSetting> {
        let flag = |name: &str| setting.get(name).and_then(Value::as_bool).unwrap_or(false);
        Some(ChannelSetting {
            group: text(setting, "group").filter(|group| !group.is_empty())?,
            channel: text(setting, "channel").filter(|channel| !channel.is_empty())?,
            on: text(setting, "setting").as_deref() == Some("on"),
            locked: flag("locked") || flag("required"),
            available: setting.get("available").and_then(Value::as_bool).unwrap_or(true),
        })
    }
}

/// One notification request at a time. Keep the confirmed snapshot until Gateway accepts a change.
#[derive(Default)]
pub struct ChannelPreferences {
    pub channels: Option<Result<Vec<ChannelSetting>, String>>,
    pub note: Option<String>,
    pub pending: bool,
}

impl ChannelPreferences {
    pub fn begin_read(&mut self) -> bool {
        if self.pending {
            return false;
        }
        self.pending = true;
        self.note = None;
        true
    }

    pub fn begin_change(&mut self, group: &str, channel: &str) -> Option<bool> {
        if self.pending {
            return None;
        }
        let setting = self.channels.as_ref()?.as_ref().ok()?.iter().find(|setting| {
            setting.group == group && setting.channel == channel && !setting.locked && setting.available
        })?;
        let on = !setting.on;
        self.pending = true;
        self.note = None;
        Some(on)
    }

    /// A failed request ends here. Retrying is explicit, so an offline Gateway cannot cause a reload loop.
    pub fn finish(&mut self, answer: Result<Vec<ChannelSetting>, String>) {
        self.pending = false;
        match answer {
            Ok(channels) => {
                self.channels = Some(Ok(channels));
                self.note = None;
            }
            Err(error) if matches!(self.channels, Some(Ok(_))) => self.note = Some(error),
            Err(error) => self.channels = Some(Err(error)),
        }
    }
}

/// The settings of `GET` or `PUT /v1/notifications/settings` (a PUT answers every setting).
pub fn channel_settings(response: &Value) -> Vec<ChannelSetting> {
    response
        .get("settings")
        .and_then(Value::as_array)
        .map(|settings| settings.iter().filter_map(ChannelSetting::from_json).collect())
        .unwrap_or_default()
}

/// The notification groups in Nexus UI's order, with their names (`inbox.ts`'s `GROUP_LABELS`).
pub const GROUPS: [(&str, &str); 5] = [
    ("my_sessions", "My sessions"),
    ("usage_and_billing", "Usage and billing"),
    ("requests_and_approvals", "Requests and approvals"),
    ("automations_and_integrations", "Automations and integrations"),
    ("account_and_security", "Account and security"),
];

/// A personal API key or sign-in (`TokenSummary`). The key itself comes back once, when it is made.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiToken {
    pub id: String,
    /// `api`, `cli` or `ui`.
    pub kind: String,
    /// The key's first characters, to tell keys apart.
    pub prefix: String,
    pub label: Option<String>,
    pub created_at: String,
    pub expires_at: Option<String>,
    /// The token this app signs in with.
    pub current: bool,
}

impl ApiToken {
    pub fn from_json(token: &Value) -> Option<ApiToken> {
        Some(ApiToken {
            id: text(token, "id").filter(|id| !id.is_empty())?,
            kind: text(token, "kind").unwrap_or_else(|| "api".into()),
            prefix: text(token, "prefix").unwrap_or_default(),
            label: text(token, "label").filter(|label| !label.trim().is_empty()),
            created_at: text(token, "createdAt").unwrap_or_default(),
            expires_at: text(token, "expiresAt"),
            current: token.get("current").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// What the row calls it: its label, else what kind of sign-in it is.
    pub fn title(&self) -> String {
        self.label.clone().unwrap_or_else(|| match self.kind.as_str() {
            "cli" => "Nexus CLI sign-in".into(),
            "ui" => "App sign-in".into(),
            _ => "API key".into(),
        })
    }
}

pub fn tokens(response: &Value) -> Vec<ApiToken> {
    response
        .get("tokens")
        .and_then(Value::as_array)
        .map(|tokens| tokens.iter().filter_map(ApiToken::from_json).collect())
        .unwrap_or_default()
}

/// A key just made: its summary, and the key itself, shown once.
#[derive(Debug, Clone, PartialEq)]
pub struct NewToken {
    pub token: ApiToken,
    pub secret: String,
}

impl NewToken {
    pub fn from_json(response: &Value) -> Option<NewToken> {
        Some(NewToken {
            token: ApiToken::from_json(response)?,
            secret: text(response, "token").filter(|secret| !secret.is_empty())?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn preferences() -> ChannelPreferences {
        let mut state = ChannelPreferences::default();
        assert!(state.begin_read());
        state.finish(Ok(vec![ChannelSetting {
            group: "my_sessions".into(),
            channel: "email".into(),
            on: false,
            locked: false,
            available: true,
        }]));
        state
    }

    #[test]
    fn notification_changes_wait_for_confirmation_and_serialize_requests() {
        let mut state = preferences();
        assert_eq!(state.begin_change("my_sessions", "email"), Some(true));
        assert!(!state.channels.as_ref().unwrap().as_ref().unwrap()[0].on);
        assert_eq!(state.begin_change("my_sessions", "email"), None);
        assert!(!state.begin_read());
        let mut confirmed = state.channels.as_ref().unwrap().as_ref().unwrap().clone();
        confirmed[0].on = true;
        state.finish(Ok(confirmed));
        assert_eq!(state.begin_change("my_sessions", "email"), Some(false));
    }

    #[test]
    fn failed_notification_save_preserves_confirmed_values_and_allows_explicit_retry() {
        let mut state = preferences();
        state.begin_change("my_sessions", "email");
        state.finish(Err("Offline".into()));
        assert!(!state.pending);
        assert_eq!(state.note.as_deref(), Some("Offline"));
        assert!(!state.channels.as_ref().unwrap().as_ref().unwrap()[0].on);
        assert!(state.begin_read());
        state.finish(Err("Still offline".into()));
        assert!(!state.pending);
        assert_eq!(state.note.as_deref(), Some("Still offline"));
        assert!(state.begin_read());
        state.finish(Ok(vec![]));
        assert!(state.note.is_none());
    }

    #[test]
    fn notification_controls_refuse_locked_unavailable_and_unknown_entries() {
        let mut state = preferences();
        assert_eq!(state.begin_change("unknown", "email"), None);
        let channels = state.channels.as_mut().unwrap().as_mut().unwrap();
        channels[0].locked = true;
        assert_eq!(state.begin_change("my_sessions", "email"), None);
        let channels = state.channels.as_mut().unwrap().as_mut().unwrap();
        channels[0].locked = false;
        channels[0].available = false;
        assert_eq!(state.begin_change("my_sessions", "email"), None);
        assert!(!state.pending);
    }

    #[test]
    fn initial_notification_failure_is_retryable() {
        let mut state = ChannelPreferences::default();
        assert_eq!(state.begin_change("my_sessions", "email"), None);
        assert!(state.begin_read());
        state.finish(Err("Offline".into()));
        assert!(matches!(state.channels, Some(Err(_))));
        assert!(state.begin_read());
        state.finish(Ok(vec![]));
        assert!(matches!(state.channels, Some(Ok(_))));
    }

    #[test]
    fn reads_channel_settings() {
        let settings = channel_settings(&json!({ "settings": [
            { "group": "my_sessions", "channel": "inbox", "setting": "on", "default": "on", "locked": true, "required": false, "available": true },
            { "group": "my_sessions", "channel": "email", "setting": "off", "default": "on", "locked": false, "required": false, "available": true },
            { "channel": "push" }
        ]}));
        assert_eq!(settings.len(), 2);
        assert!(settings[0].on && settings[0].locked);
        assert!(!settings[1].on && !settings[1].locked);
    }

    #[test]
    fn reads_a_new_key_once() {
        let made = NewToken::from_json(&json!({
            "id": "tok_1", "userId": "u", "kind": "api", "prefix": "nx_ab", "label": "CI", "createdAt": "2026-11-05T10:00:00Z",
            "token": "nx_abcdef"
        }))
        .expect("a key");
        assert_eq!(made.token.title(), "CI");
        assert_eq!(made.secret, "nx_abcdef");
        assert!(NewToken::from_json(&json!({ "id": "tok_1" })).is_none());
    }
}
