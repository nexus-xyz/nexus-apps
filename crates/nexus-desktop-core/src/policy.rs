//! What an administrator decides for Nexus on this computer (README.md, "Managed configuration"), from three places,
//! merged into one [`Policy`]:
//!
//! - **macOS managed preferences**, domain `xyz.nexus.mac` (a configuration profile from Jamf, Kandji or Intune; the
//!   sample is `xyz.nexus.mac.mobileconfig`), read through CFPreferences, which layers a profile's forced values over
//!   the person's own (`defaults write xyz.nexus.mac …`). The keys are the ones the retired Swift app read.
//! - **Windows Group Policy**, `HKLM\Software\Policies\Nexus` (then `HKCU\…`), the same keys as registry values
//!   (`REG_SZ` for text, `REG_DWORD` 0/1 for switches). Every value there is forced: only administrators write it.
//! - **The managed JSON file** every native client reads (`nexus_client::config::ManagedConfig`): `url`, `org`,
//!   `uiUrl`, `lockUrl`.
//!
//! For the Gateway, Nexus UI and the organization, a forced preference wins, then the JSON file, then a preference the
//! person set. Switches apply whoever set them; a forced one is shown as managed and cannot be changed in the app.
//! No secret is ever read from any of them: people sign in as themselves.

use nexus_client::config::ManagedConfig;
use serde::Serialize;

/// The macOS preference domain (the Swift app's bundle identifier, kept so existing profiles keep working).
pub const MAC_DOMAIN: &str = "xyz.nexus.mac";
/// The Windows policy key, under HKLM (then HKCU).
pub const WINDOWS_KEY: &str = r"Software\Policies\Nexus";

/// The keys, as configuration profiles and Group Policy name them.
pub mod key {
    /// Text: the organization's Nexus Gateway. Forced, people cannot change it.
    pub const GATEWAY_URL: &str = "GatewayURL";
    /// Text: Nexus UI's base (`https://…/app`), for "Open in Nexus UI".
    pub const NEXUS_UI_URL: &str = "NexusUIURL";
    /// Text: the organization (slug or id) the app opens; after sign-in the app switches to it.
    pub const ORGANIZATION: &str = "Organization";
    /// Switch (default on): whether people may switch to another organization.
    pub const ALLOW_ORGANIZATION_SWITCHING: &str = "AllowOrganizationSwitching";
    /// Switch (default on): keep the local cache on disk; off, nothing but the keychain entry is stored.
    pub const ALLOW_LOCAL_CACHE: &str = "AllowLocalCache";
    /// Switch (default on): the global shortcut that opens the panel.
    pub const GLOBAL_SHORTCUT_ENABLED: &str = "GlobalShortcutEnabled";
    /// Switch: open at login. Forced, the app sets it at launch and the setting is locked.
    pub const LAUNCH_AT_LOGIN: &str = "LaunchAtLogin";
    /// Switch (default on): screen recordings for agents (macOS, crates/nexus-desktop-record). The person starts every recording and
    /// it stays on the computer; off, there is no shortcut, menu item or button for it.
    pub const SCREEN_CAPTURE_ENABLED: &str = "ScreenCaptureEnabled";
    /// Switch (default on): the hooks people set in Settings › Hooks (a command run on a Code session event). Off, none
    /// runs. A policy never adds a hook.
    pub const HOOKS_ENABLED: &str = "HooksEnabled";

    pub const ALL: [&str; 9] = [
        GATEWAY_URL,
        NEXUS_UI_URL,
        ORGANIZATION,
        ALLOW_ORGANIZATION_SWITCHING,
        ALLOW_LOCAL_CACHE,
        GLOBAL_SHORTCUT_ENABLED,
        LAUNCH_AT_LOGIN,
        SCREEN_CAPTURE_ENABLED,
        HOOKS_ENABLED,
    ];
}

/// One preference's value, as a property list or the registry holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    Bool(bool),
    Number(i64),
}

/// Where preferences come from: the OS (macOS, Windows), or a map in tests.
pub trait Source {
    fn value(&self, key: &str) -> Option<Value>;
    /// True when an administrator set the key (a configuration profile, Group Policy): people cannot change it.
    fn is_forced(&self, key: &str) -> bool;
    /// Where these come from, for the settings screen and support.
    fn describe(&self) -> String;
}

/// The merged decision, for the core and (serialised) for the webview's settings.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    /// The Gateway, Nexus UI and organization, merged with the JSON file.
    #[serde(skip)]
    pub managed: Option<ManagedConfig>,
    pub organization: Option<String>,
    pub allow_organization_switching: bool,
    pub allow_local_cache: bool,
    pub global_shortcut_enabled: bool,
    /// Set by an administrator: the app applies it at launch.
    pub launch_at_login: Option<bool>,
    pub screen_capture_enabled: bool,
    pub hooks_enabled: bool,
    /// The keys an administrator forced (shown as "Managed by your organization").
    pub forced: Vec<String>,
    /// Where the preferences were read (domain, registry key), and the JSON file's path.
    pub sources: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            managed: None,
            organization: None,
            allow_organization_switching: true,
            allow_local_cache: true,
            global_shortcut_enabled: true,
            launch_at_login: None,
            screen_capture_enabled: true,
            hooks_enabled: true,
            forced: Vec::new(),
            sources: Vec::new(),
        }
    }
}

fn text(source: &dyn Source, key: &str) -> Option<String> {
    match source.value(key)? {
        Value::Text(text) => Some(text.trim().to_string()).filter(|text| !text.is_empty()),
        _ => None,
    }
}

fn switch(source: &dyn Source, key: &str) -> Option<bool> {
    match source.value(key)? {
        Value::Bool(value) => Some(value),
        Value::Number(value) => Some(value != 0),
        Value::Text(text) => match text.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" => Some(false),
            _ => None,
        },
    }
}

fn url(text: String) -> Option<String> {
    let text = text.trim_end_matches('/').to_string();
    if text.starts_with("https://") || text.starts_with("http://") {
        Some(text)
    } else if !text.contains("://") && text.contains('.') {
        Some(format!("https://{text}"))
    } else {
        None
    }
}

impl Policy {
    /// Merge the OS preferences (`source`) with the JSON file (`file`, at `file_path`).
    pub fn merge(source: &dyn Source, file: Option<ManagedConfig>, file_path: &str) -> Policy {
        let forced = |key: &str| source.is_forced(key);
        let mut managed = file.clone().unwrap_or_default();
        let pick = |key: &str, from_file: Option<String>, is_url: bool| -> Option<String> {
            let value = text(source, key).and_then(|value| if is_url { url(value) } else { Some(value) });
            match value {
                Some(value) if forced(key) => Some(value),
                value => from_file.or(value),
            }
        };
        managed.url = pick(key::GATEWAY_URL, managed.url.clone(), true);
        managed.ui_url = pick(key::NEXUS_UI_URL, managed.ui_url.clone(), true);
        managed.org = pick(key::ORGANIZATION, managed.org.clone(), false);
        managed.lock_url = managed.lock_url || (forced(key::GATEWAY_URL) && text(source, key::GATEWAY_URL).is_some());
        let any = managed.url.is_some() || managed.ui_url.is_some() || managed.org.is_some() || file.is_some();
        let mut sources = vec![source.describe()];
        if file.is_some() {
            sources.push(file_path.to_string());
        }
        Policy {
            organization: managed.org.clone(),
            managed: any.then_some(managed),
            allow_organization_switching: switch(source, key::ALLOW_ORGANIZATION_SWITCHING).unwrap_or(true),
            allow_local_cache: switch(source, key::ALLOW_LOCAL_CACHE).unwrap_or(true),
            global_shortcut_enabled: switch(source, key::GLOBAL_SHORTCUT_ENABLED).unwrap_or(true),
            launch_at_login: switch(source, key::LAUNCH_AT_LOGIN).filter(|_| forced(key::LAUNCH_AT_LOGIN)),
            screen_capture_enabled: switch(source, key::SCREEN_CAPTURE_ENABLED).unwrap_or(true),
            hooks_enabled: switch(source, key::HOOKS_ENABLED).unwrap_or(true),
            forced: key::ALL
                .iter()
                .filter(|key| forced(key) && source.value(key).is_some())
                .map(|key| key.to_string())
                .collect(),
            sources,
        }
    }

    /// Whether the person may move to `org` (its id and slug): always when switching is allowed; otherwise only the
    /// pinned organization.
    pub fn may_switch_to(&self, id: &str, slug: Option<&str>) -> bool {
        if self.allow_organization_switching {
            return true;
        }
        self.organization
            .as_deref()
            .is_some_and(|pinned| pinned == id || slug == Some(pinned))
    }

    pub fn is_forced(&self, key: &str) -> bool {
        self.forced.iter().any(|forced| forced == key)
    }
}

/// This computer's preferences: CFPreferences on macOS, the policy registry key on Windows, none elsewhere.
pub fn system() -> Box<dyn Source> {
    #[cfg(target_os = "macos")]
    {
        Box::new(mac::Preferences::new(MAC_DOMAIN))
    }
    #[cfg(windows)]
    {
        Box::new(registry::Registry)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Box::new(None_)
    }
}

/// No OS preferences (Linux: the JSON file is the managed config).
#[allow(dead_code)]
struct None_;

impl Source for None_ {
    fn value(&self, _key: &str) -> Option<Value> {
        None
    }
    fn is_forced(&self, _key: &str) -> bool {
        false
    }
    fn describe(&self) -> String {
        "no OS preferences on this platform".into()
    }
}

#[cfg(target_os = "macos")]
mod mac {
    //! CFPreferences: `CFPreferencesCopyAppValue` returns the value from the highest layer that has it (a forced
    //! managed value, then the person's, then any-host and global ones); `CFPreferencesAppValueIsForced` says whether a
    //! configuration profile forces it. The only unsafe code in the app: two FFI calls with owned CoreFoundation
    //! strings, the returned value taken under the create rule (we own the +1 reference, released on drop).
    #![allow(unsafe_code)]

    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::number::CFNumber;
    use core_foundation::propertylist::CFPropertyList;
    use core_foundation::string::CFString;
    use core_foundation_sys::preferences::{CFPreferencesAppValueIsForced, CFPreferencesCopyAppValue};

    use super::{Source, Value};

    pub struct Preferences {
        domain: CFString,
        name: String,
    }

    impl Preferences {
        pub fn new(domain: &str) -> Preferences {
            Preferences {
                domain: CFString::new(domain),
                name: domain.to_string(),
            }
        }
    }

    impl Source for Preferences {
        fn value(&self, key: &str) -> Option<Value> {
            let key = CFString::new(key);
            // SAFETY: both arguments are valid CFStrings that outlive the call; the result is null or a new reference
            // (the "Copy" rule), which `wrap_under_create_rule` takes ownership of.
            let raw =
                unsafe { CFPreferencesCopyAppValue(key.as_concrete_TypeRef(), self.domain.as_concrete_TypeRef()) };
            if raw.is_null() {
                return None;
            }
            // SAFETY: non-null and owned by us (see above).
            let value = unsafe { CFPropertyList::wrap_under_create_rule(raw) };
            if let Some(text) = value.downcast::<CFString>() {
                return Some(Value::Text(text.to_string()));
            }
            if let Some(flag) = value.downcast::<CFBoolean>() {
                return Some(Value::Bool(flag.into()));
            }
            value
                .downcast::<CFNumber>()
                .and_then(|number| number.to_i64())
                .map(Value::Number)
        }

        fn is_forced(&self, key: &str) -> bool {
            let key = CFString::new(key);
            // SAFETY: both arguments are valid CFStrings that outlive the call; the result is a plain Boolean.
            unsafe { CFPreferencesAppValueIsForced(key.as_concrete_TypeRef(), self.domain.as_concrete_TypeRef()) != 0 }
        }

        fn describe(&self) -> String {
            format!("macOS preferences, domain {}", self.name)
        }
    }
}

#[cfg(windows)]
mod registry {
    //! Group Policy's registry key. Values under `HKLM\Software\Policies\Nexus` (machine) win over
    //! `HKCU\Software\Policies\Nexus` (user); both are policy (forced), as only administrators can write them.
    //! Not yet run on Windows (CI builds it; README.md, "What's left").

    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};

    use super::{Source, Value, WINDOWS_KEY};

    pub struct Registry;

    fn read(hive: winreg::HKEY, key: &str) -> Option<Value> {
        let policy = RegKey::predef(hive)
            .open_subkey_with_flags(WINDOWS_KEY, KEY_READ)
            .ok()?;
        if let Ok(number) = policy.get_value::<u32, _>(key) {
            return Some(Value::Number(i64::from(number)));
        }
        policy.get_value::<String, _>(key).ok().map(Value::Text)
    }

    impl Source for Registry {
        fn value(&self, key: &str) -> Option<Value> {
            read(HKEY_LOCAL_MACHINE, key).or_else(|| read(HKEY_CURRENT_USER, key))
        }

        fn is_forced(&self, key: &str) -> bool {
            self.value(key).is_some()
        }

        fn describe(&self) -> String {
            format!(r"Group Policy, HKLM\{WINDOWS_KEY}")
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    /// A map of preferences, some forced.
    #[derive(Default)]
    pub struct Map {
        pub values: HashMap<String, Value>,
        pub forced: HashSet<String>,
    }

    impl Map {
        pub fn with(mut self, key: &str, value: Value, forced: bool) -> Map {
            self.values.insert(key.into(), value);
            if forced {
                self.forced.insert(key.into());
            }
            self
        }
    }

    impl Source for Map {
        fn value(&self, key: &str) -> Option<Value> {
            self.values.get(key).cloned()
        }
        fn is_forced(&self, key: &str) -> bool {
            self.forced.contains(key)
        }
        fn describe(&self) -> String {
            "test".into()
        }
    }

    #[test]
    fn defaults_without_anything() {
        let policy = Policy::merge(&Map::default(), None, "/m.json");
        assert_eq!(policy.managed, None);
        assert!(policy.allow_organization_switching);
        assert!(policy.allow_local_cache && policy.global_shortcut_enabled && policy.screen_capture_enabled);
        assert!(policy.hooks_enabled);
        assert_eq!(policy.launch_at_login, None);
        assert!(policy.forced.is_empty());
    }

    #[test]
    fn a_forced_gateway_wins_and_locks() {
        let file = ManagedConfig {
            url: Some("https://from-file.example".into()),
            org: Some("file-org".into()),
            ..ManagedConfig::default()
        };
        let source = Map::default()
            .with(key::GATEWAY_URL, Value::Text("nexus.acme.example/".into()), true)
            .with(key::ORGANIZATION, Value::Text("acme".into()), false);
        let policy = Policy::merge(&source, Some(file), "/m.json");
        let managed = policy.managed.clone().expect("managed");
        assert_eq!(managed.url.as_deref(), Some("https://nexus.acme.example"));
        assert!(managed.lock_url);
        // The person's own Organization loses to the file's.
        assert_eq!(policy.organization.as_deref(), Some("file-org"));
        assert_eq!(policy.forced, vec![key::GATEWAY_URL.to_string()]);
        assert_eq!(policy.sources, vec!["test".to_string(), "/m.json".to_string()]);
    }

    #[test]
    fn a_persons_preference_is_a_default_not_a_lock() {
        let source = Map::default().with(key::GATEWAY_URL, Value::Text("http://127.0.0.1:4741".into()), false);
        let policy = Policy::merge(&source, None, "/m.json");
        let managed = policy.managed.expect("managed");
        assert_eq!(managed.url.as_deref(), Some("http://127.0.0.1:4741"));
        assert!(!managed.lock_url);
    }

    #[test]
    fn switches_from_bools_numbers_and_text() {
        let source = Map::default()
            .with(key::ALLOW_LOCAL_CACHE, Value::Number(0), true)
            .with(key::GLOBAL_SHORTCUT_ENABLED, Value::Text("no".into()), false)
            .with(key::LAUNCH_AT_LOGIN, Value::Bool(true), false)
            .with(key::SCREEN_CAPTURE_ENABLED, Value::Bool(false), true)
            .with(key::HOOKS_ENABLED, Value::Number(0), true);
        let policy = Policy::merge(&source, None, "/m.json");
        assert!(!policy.screen_capture_enabled);
        assert!(!policy.hooks_enabled && policy.is_forced(key::HOOKS_ENABLED));
        assert!(policy.is_forced(key::SCREEN_CAPTURE_ENABLED));
        assert!(!policy.allow_local_cache);
        assert!(!policy.global_shortcut_enabled);
        // Launch at login is applied only when forced; otherwise it is the person's choice.
        assert_eq!(policy.launch_at_login, None);
        let forced = Map::default().with(key::LAUNCH_AT_LOGIN, Value::Number(1), true);
        assert_eq!(Policy::merge(&forced, None, "").launch_at_login, Some(true));
    }

    #[test]
    fn switching_limited_to_the_pinned_organization() {
        let source = Map::default()
            .with(key::ORGANIZATION, Value::Text("acme".into()), true)
            .with(key::ALLOW_ORGANIZATION_SWITCHING, Value::Bool(false), true);
        let policy = Policy::merge(&source, None, "");
        assert!(policy.may_switch_to("t_1", Some("acme")));
        assert!(policy.may_switch_to("acme", None));
        assert!(!policy.may_switch_to("t_2", Some("personal-ada")));
        assert!(Policy::default().may_switch_to("t_2", None));
    }
}
