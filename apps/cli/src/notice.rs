//! The update notice (`notice.ts`): at most once a day, after a command has printed its output, one line on
//! stderr when the Gateway publishes a newer CLI. Gateway is asked at most once every 24 hours with a short timeout; the
//! answer is cached in `update-check.json` next to the CLI config. Never with `--json`, when stdout is not a
//! terminal, in CI, or with `NEXUS_NO_UPDATE_CHECK=1`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::config::{create_private_dir, write_private};
use crate::json::{pretty, s};
use crate::style::{iso, parse_ms};
use crate::update::fetch_published;

pub const CHECK_EVERY_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

pub fn update_cache_path(config_path: &Path) -> PathBuf {
    config_path.parent().unwrap_or(Path::new(".")).join("update-check.json")
}

/// Whether this run must stay quiet: machine output, no terminal, CI, opted out, or `nexus update` itself.
pub fn notice_suppressed(argv: &[String], env: &dyn Fn(&str) -> Option<String>, stdout_is_tty: bool) -> bool {
    if matches!(env("NEXUS_NO_UPDATE_CHECK").as_deref(), Some("1") | Some("true")) {
        return true;
    }
    if env("CI").is_some_and(|ci| !ci.is_empty() && ci != "0" && ci != "false") {
        return true;
    }
    if !stdout_is_tty || argv.iter().any(|arg| arg == "--json") {
        return true;
    }
    argv.iter()
        .find(|arg| !arg.starts_with('-'))
        .is_some_and(|command| command == "update")
}

/// The notice line to print, if any; refreshes the cache when a check is due. Never fails.
pub fn update_notice(
    cache_path: &Path,
    core_url: &str,
    current: &str,
    now: f64,
    fetch: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let cache: Map<String, Value> = std::fs::read_to_string(cache_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let cached = Value::Object(cache.clone());
    let same_core = s(&cached, "coreUrl") == Some(core_url);
    let checked = s(&cached, "checkedAt").and_then(parse_ms);
    let due = checked.is_none() || !same_core || checked.is_some_and(|at| now - at >= CHECK_EVERY_MS);
    let mut next = cache.clone();
    if due {
        // A failed check counts as a check: an offline laptop is not asked again for a day.
        let latest = fetch(core_url).or_else(|| {
            if same_core {
                s(&cached, "latest").map(String::from)
            } else {
                None
            }
        });
        if !same_core {
            next = Map::new();
        }
        next.insert("checkedAt".into(), json!(iso(now)));
        next.insert("coreUrl".into(), json!(core_url));
        if let Some(latest) = latest {
            next.insert("latest".into(), json!(latest));
        }
    }
    let mut line = None;
    let next_value = Value::Object(next.clone());
    let notified_recently = s(&next_value, "notifiedAt")
        .and_then(parse_ms)
        .is_some_and(|at| now - at < CHECK_EVERY_MS);
    if let Some(latest) = s(&next_value, "latest")
        && nexus_client::base::version::compare_versions(latest, current).is_some_and(|o| o.is_gt())
        && !notified_recently
    {
        line = Some(format!(
            "nexus {latest} is available (you have {current}). Run `nexus update`."
        ));
        next.insert("notifiedAt".into(), json!(iso(now)));
    }
    if next != cache {
        if let Some(dir) = cache_path.parent() {
            let _ = create_private_dir(dir);
        }
        let _ = write_private(cache_path, format!("{}\n", pretty(&Value::Object(next))).as_bytes());
    }
    line
}

/// The version the Gateway publishes for this platform (a Gateway that serves only the JavaScript bundle publishes none),
/// asked with a short timeout.
pub fn published_version(core_url: &str) -> Option<String> {
    fetch_published(core_url, Duration::from_millis(1500))
        .ok()
        .filter(|published| published.target.as_deref() == Some(crate::update::target().as_str()))
        .map(|published| published.version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_once_a_day_and_stays_quiet_otherwise() {
        let env = |_: &str| None;
        assert!(notice_suppressed(&["spend".into(), "--json".into()], &env, true));
        assert!(notice_suppressed(&["update".into()], &env, true));
        assert!(notice_suppressed(&["spend".into()], &env, false));
        assert!(!notice_suppressed(&["spend".into()], &env, true));
        assert!(notice_suppressed(
            &["spend".into()],
            &|name| (name == "CI").then(|| "true".into()),
            true
        ));

        let dir = std::env::temp_dir().join(format!("nexus-notice-{}", uuid::Uuid::new_v4()));
        let cache = dir.join("update-check.json");
        let now = parse_ms("2026-10-18T12:00:00Z").expect("time");
        let newer = |_: &str| Some("0.3.0".to_string());
        assert_eq!(
            update_notice(&cache, "http://core.test", "0.2.0", now, &newer).as_deref(),
            Some("nexus 0.3.0 is available (you have 0.2.0). Run `nexus update`.")
        );
        // Asked and shown at most once a day.
        let never = |_: &str| -> Option<String> { panic!("not due") };
        assert_eq!(
            update_notice(&cache, "http://core.test", "0.2.0", now + 1000.0, &never),
            None
        );
        assert!(update_notice(&cache, "http://core.test", "0.2.0", now + CHECK_EVERY_MS, &newer).is_some());
        assert_eq!(
            update_notice(
                &cache,
                "http://core.test",
                "0.3.0+abc",
                now + 3.0 * CHECK_EVERY_MS,
                &newer
            ),
            None
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
