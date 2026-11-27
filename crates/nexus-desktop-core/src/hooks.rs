//! Hooks as the desktop app keeps and shows them: `settings.json`'s `hooks`, the managed `HooksEnabled`,
//! the [`Source`] the app that owns the store hands its workbench, and what Settings › Hooks shows. The rules (which
//! hook runs, once, how, with what variables) are nexus-workbench's `hooks`; the apps add no copy of them.
//!
//! Only the person's own settings.json sets a hook. Session sync, remote machines and Gateway never write one, and a
//! policy can only turn them all off.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use nexus_workbench::hooks::{
    DEFAULT_TIMEOUT_SECS, Hook, HookEvent, HookRun, Hooks, KEEP_RUNS, MAX_TIMEOUT_SECS, Source,
};

use crate::notify::Notifier;
use crate::policy::{Policy, key};
use crate::settings::{self, Settings};

/// The longest command Settings takes.
pub const MAX_COMMAND: usize = 4096;

/// `hooks` read one entry at a time: an entry that does not read (an unknown event, a wrong type) is dropped, so the
/// rest of settings.json still reads.
pub fn lenient<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Hook>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let entries = match value {
        serde_json::Value::Array(entries) => entries,
        _ => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value(entry).ok())
        .collect())
}

/// The hooks as Settings saves them: each command trimmed and not empty, each timeout between 1 and
/// [`MAX_TIMEOUT_SECS`] (the default left out).
pub fn checked(hooks: Vec<Hook>) -> Result<Vec<Hook>, String> {
    hooks
        .into_iter()
        .map(|hook| {
            let command = hook.command.trim().to_string();
            if command.is_empty() {
                return Err(format!("The {} hook has no command.", hook.event.label()));
            }
            if command.len() > MAX_COMMAND {
                return Err(format!("A hook's command is at most {MAX_COMMAND} characters."));
            }
            let timeout_secs = hook
                .timeout_secs
                .map(|secs| secs.clamp(1, MAX_TIMEOUT_SECS))
                .filter(|secs| *secs != DEFAULT_TIMEOUT_SECS);
            Ok(Hook {
                event: hook.event,
                command,
                timeout_secs,
            })
        })
        .collect()
}

impl Settings {
    /// Replaces the hooks with `hooks`, each checked first ([`checked`]): a refused list changes nothing.
    pub fn set_hooks(&mut self, hooks: Vec<Hook>) -> Result<(), String> {
        self.hooks = checked(hooks)?;
        Ok(())
    }
}

/// The hooks in effect: the person's, run only when the policy lets them.
pub fn in_effect(settings: &Settings, policy: &Policy) -> Hooks {
    Hooks {
        enabled: policy.hooks_enabled,
        list: settings.hooks.clone(),
    }
}

/// The workbench's [`Source`] in the app that owns the store, or in its background service: settings.json and the
/// policy, read again at each event (so a change in Settings applies at once), and the app's notifier for a failed run.
pub struct FromSettings {
    pub path: PathBuf,
    /// None in the background service, which shows no notifications.
    pub notifier: Option<Arc<dyn Notifier>>,
}

impl FromSettings {
    /// The person's settings.json in the app's config folder ([`settings::config_dir`]).
    pub fn in_config_dir(notifier: Option<Arc<dyn Notifier>>) -> Option<Arc<dyn Source>> {
        let path = settings::path(&settings::config_dir()?);
        Some(Arc::new(FromSettings { path, notifier }))
    }
}

impl Source for FromSettings {
    fn hooks(&self) -> Hooks {
        let settings = settings::load(&self.path);
        if settings.hooks.is_empty() {
            return Hooks::default();
        }
        in_effect(&settings, &settings::managed().0)
    }

    fn failed(&self, thread_id: &str, title: &str, body: &str) {
        // The notifier checks the person's notification setting, as for a finished turn.
        if let Some(notifier) = &self.notifier {
            notifier.show(None, thread_id, title, body);
        }
    }
}

/// The runs kept in the store in the data folder `data`, newest first: what Settings shows while the background
/// service runs the sessions. Empty when there is no store.
pub fn runs_in(data: &Path) -> Vec<HookRun> {
    nexus_workbench::reader::open_in(data)
        .ok()
        .flatten()
        .and_then(|store| store.hook_runs().ok())
        .unwrap_or_default()
}

/// One hook on Settings › Hooks, with its last runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookRow {
    pub id: String,
    #[serde(flatten)]
    pub hook: Hook,
    /// Newest first, at most [`KEEP_RUNS`].
    pub runs: Vec<HookRun>,
}

/// What Settings › Hooks shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HooksPage {
    pub hooks: Vec<HookRow>,
    /// False: the organization turned hooks off.
    pub enabled: bool,
    /// An administrator forced `HooksEnabled`.
    pub managed: bool,
}

pub fn page(settings: &Settings, policy: &Policy, runs: &[HookRun]) -> HooksPage {
    let hooks = settings
        .hooks
        .iter()
        .map(|hook| {
            let id = hook.id();
            HookRow {
                runs: runs.iter().filter(|run| run.hook == id).cloned().collect(),
                id,
                hook: hook.clone(),
            }
        })
        .collect();
    HooksPage {
        hooks,
        enabled: policy.hooks_enabled,
        managed: policy.is_forced(key::HOOKS_ENABLED),
    }
}

/// What Settings says when the policy turned hooks off.
pub const OFF_BY_POLICY: &str = "Set by your organization: hooks are off.";

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(event: HookEvent, command: &str, timeout_secs: Option<u64>) -> Hook {
        Hook {
            event,
            command: command.into(),
            timeout_secs,
        }
    }

    #[test]
    fn settings_round_trip_and_a_bad_entry_does_not_lose_the_file() {
        let text = r#"{"gatewayUrl":"https://g.example","hooks":[
            {"event":"turnFinished","command":"say done"},
            {"event":"teleported","command":"x"},
            {"event":"checkFailed","command":"notify","timeoutSecs":30}]}"#;
        let settings: Settings = serde_json::from_str(text).expect("settings");
        assert_eq!(settings.gateway_url.as_deref(), Some("https://g.example"));
        assert_eq!(
            settings.hooks,
            vec![
                hook(HookEvent::TurnFinished, "say done", None),
                hook(HookEvent::CheckFailed, "notify", Some(30)),
            ]
        );
        let again: Settings = serde_json::from_str(&serde_json::to_string(&settings).expect("json")).expect("again");
        assert_eq!(again, settings);
        // No hooks: the key is left out.
        assert!(
            !serde_json::to_string(&Settings::default())
                .expect("json")
                .contains("hooks")
        );
        let odd: Settings = serde_json::from_str(r#"{"hooks":"nope","org":"acme"}"#).expect("odd");
        assert_eq!((odd.hooks.len(), odd.org.as_deref()), (0, Some("acme")));
    }

    #[test]
    fn set_hooks_trims_clamps_and_refuses_an_empty_command() {
        let mut settings = Settings::default();
        settings
            .set_hooks(vec![
                hook(HookEvent::Settled, "  make deploy  ", Some(9000)),
                hook(HookEvent::Archived, "x", Some(DEFAULT_TIMEOUT_SECS)),
            ])
            .expect("set");
        assert_eq!(
            settings.hooks,
            vec![
                hook(HookEvent::Settled, "make deploy", Some(MAX_TIMEOUT_SECS)),
                hook(HookEvent::Archived, "x", None),
            ]
        );
        let refused = settings.set_hooks(vec![hook(HookEvent::Snoozed, " ", None)]);
        assert!(refused.is_err());
        assert_eq!(settings.hooks.len(), 2, "a refused list changes nothing");
    }

    #[test]
    fn the_policy_turns_every_hook_off_and_the_page_says_so() {
        let settings = Settings {
            hooks: vec![hook(HookEvent::NeedsYou, "say help", None)],
            ..Settings::default()
        };
        assert_eq!(
            in_effect(&settings, &Policy::default())
                .matching(HookEvent::NeedsYou)
                .len(),
            1
        );
        let off = Policy {
            hooks_enabled: false,
            forced: vec![key::HOOKS_ENABLED.into()],
            ..Policy::default()
        };
        assert!(in_effect(&settings, &off).matching(HookEvent::NeedsYou).is_empty());
        let id = settings.hooks[0].id();
        let runs = vec![
            HookRun {
                hook: id.clone(),
                event: "needsYou".into(),
                thread_id: "t".into(),
                started_at: 5,
                exit_code: Some(1),
                timed_out: false,
                output: "no".into(),
            },
            HookRun {
                hook: "other".into(),
                event: "needsYou".into(),
                thread_id: "t".into(),
                started_at: 4,
                exit_code: Some(0),
                timed_out: false,
                output: String::new(),
            },
        ];
        let page = page(&settings, &off, &runs);
        assert!(!page.enabled && page.managed);
        assert_eq!(page.hooks[0].id, id);
        assert_eq!(page.hooks[0].runs.len(), 1);
        let json = serde_json::to_value(&page).expect("json");
        assert_eq!(json["hooks"][0]["event"], "needsYou");
        assert_eq!(json["hooks"][0]["runs"][0]["exitCode"], 1);
    }
}
