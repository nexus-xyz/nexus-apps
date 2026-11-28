//! The two sign-ins a new cloud machine gets from this computer (Settings, Cloud): a Claude setup token and a GitHub
//! token. The secrets are in the keychain, under [`CLAUDE_ACCOUNT`] and [`GITHUB_ACCOUNT`]; `settings.json`'s `cloud`
//! keeps only a hint of each and when it was saved ([`CloudAccountSetting`]). Nexus GPUI's Settings, Cloud, saves them
//! through these.

use crate::settings::{CloudAccountSetting, Settings};

/// The keychain account of the Claude setup token.
pub const CLAUDE_ACCOUNT: &str = "cloud-claude";

/// The keychain account of the person's GitHub token for the machines.
pub const GITHUB_ACCOUNT: &str = "cloud-github";

/// A hint of a secret for the webview: its first `head` characters and its last four, never enough to use.
pub fn hint(secret: &str, head: usize) -> String {
    let chars: Vec<char> = secret.trim().chars().collect();
    if chars.len() <= head + 4 {
        return "…".into();
    }
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - 4..].iter().collect();
    format!("{start}…{end}")
}

/// Whether `value` looks like what `claude setup-token` prints.
pub fn looks_like_setup_token(value: &str) -> bool {
    let Some(rest) = value.trim().strip_prefix("sk-ant-oat") else {
        return false;
    };
    let mut chars = rest.chars();
    let two_digits =
        chars.next().is_some_and(|c| c.is_ascii_digit()) && chars.next().is_some_and(|c| c.is_ascii_digit());
    let rest = chars.as_str();
    two_digits
        && rest.starts_with('-')
        && rest[1..].len() >= 20
        && rest[1..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Why a pasted value is not saved as the Claude setup token.
pub const NOT_A_SETUP_TOKEN: &str = "That doesn't look like a setup token. Run `claude setup-token` and paste what it \
                                     prints (it starts with sk-ant-oat).";

/// Change `settings.cloud`, and drop it once nothing is left in it but its owner.
pub fn change_account(settings: &mut Settings, change: impl FnOnce(&mut CloudAccountSetting)) {
    let mut account = settings.cloud.take().unwrap_or_default();
    change(&mut account);
    let empty = CloudAccountSetting {
        owner: None,
        ..account.clone()
    } == CloudAccountSetting::default();
    settings.cloud = (!empty).then_some(account);
}

/// A Claude setup token was saved at `at` (unix ms): keep its hint, never the token.
pub fn claude_saved(account: &mut CloudAccountSetting, token: &str, at: i64) {
    account.claude_hint = Some(hint(token, 10));
    account.claude_saved_at = Some(at);
    account.claude_cleared = false;
    account.claude_on_gateway = false;
}

/// The Claude setup token was removed here: the next connect removes it from the machines too. `claude_on_gateway`
/// stays until Gateway confirms its removal.
pub fn claude_cleared(account: &mut CloudAccountSetting) {
    account.claude_hint = None;
    account.claude_saved_at = None;
    account.claude_cleared = true;
}

/// The GitHub token was saved at `at` (unix ms), for `user`.
pub fn github_saved(account: &mut CloudAccountSetting, user: String, at: i64) {
    account.github_user = Some(user);
    account.github_saved_at = Some(at);
}

/// The GitHub token was forgotten here. Machines that have it keep their `gh` sign-in.
pub fn github_cleared(account: &mut CloudAccountSetting) {
    account.github_user = None;
    account.github_saved_at = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_never_show_the_secret() {
        assert_eq!(
            hint("sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123", 10),
            "sk-ant-oat…0123"
        );
        assert_eq!(hint("short", 10), "…");
    }

    #[test]
    fn setup_tokens_are_recognised() {
        assert!(looks_like_setup_token("sk-ant-oat01-abcdefghijklmnopqrstuvwxyz_-0123"));
        assert!(looks_like_setup_token("  sk-ant-oat02-ABCDEFGHIJKLMNOPQRSTUVWX \n"));
        assert!(!looks_like_setup_token("sk-ant-api03-abcdefghijklmnopqrstuvwxyz"));
        assert!(!looks_like_setup_token("sk-ant-oat01-short"));
        assert!(!looks_like_setup_token("sk-ant-oatxx-abcdefghijklmnopqrstuvwxyz"));
        assert!(!looks_like_setup_token("sk-ant-oat01-abcdefghijklmnopqrst uvwxyz"));
    }

    #[test]
    fn the_account_keeps_hints_only_and_goes_when_empty() {
        let token = "sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123";
        let mut settings = Settings::default();
        change_account(&mut settings, |account| claude_saved(account, token, 7));
        let saved = settings.cloud.clone().expect("the account");
        assert_eq!(saved.claude_hint.as_deref(), Some("sk-ant-oat…0123"));
        assert_eq!(saved.claude_saved_at, Some(7));
        assert!(!serde_json::to_string(&settings).expect("json").contains(token));
        change_account(&mut settings, claude_cleared);
        assert!(settings.cloud.as_ref().is_some_and(|account| account.claude_cleared));
        change_account(&mut settings, |account| github_saved(account, "octo".into(), 9));
        change_account(&mut settings, |account| account.owner = Some("tag".into()));
        change_account(&mut settings, github_cleared);
        assert!(settings.cloud.as_ref().is_some_and(|account| account.owner.is_some()));
        change_account(&mut settings, |account| account.claude_cleared = false);
        assert_eq!(settings.cloud, None, "an owner of nothing is not kept");
    }
}
