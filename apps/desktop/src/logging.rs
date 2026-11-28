//! The app's log lines on stderr, at the level `NEXUS_LOG` sets, as the Gateway's. GPUI and most of the
//! crates under it log through `log`, so a small `log` logger here shows their lines too, with no new dependency.
//!
//! `NEXUS_LOG` takes the Gateway's filter: a default level and `target=level` directives, comma-separated, such as
//! `info,nexus_desktop_gpui=debug`. A target matches every target that starts with it, and the longest match wins.
//! Unset, it falls back to `RUST_LOG`, then to [`DEFAULT`]. A value that is not a filter gives [`DEFAULT`] and a
//! warning. Each line is the time, the level, the target and the message, as the Gateway's text lines.

use std::io::Write as _;
use std::sync::OnceLock;
use std::time::Instant;

use log::{LevelFilter, Log, Metadata, Record};

/// `NEXUS_LOG` and `RUST_LOG` unset, or not a filter: warnings and errors from every crate, and the app's own start.
pub const DEFAULT: &str = "warn,nexus_desktop_gpui=info";

/// When the logger was installed, the process start for the start's lines.
static START: OnceLock<Instant> = OnceLock::new();

/// A parsed filter: the level for targets no directive names, and each directive's target and level.
#[derive(Debug, PartialEq)]
struct Filter {
    default: LevelFilter,
    targets: Vec<(String, LevelFilter)>,
}

impl Filter {
    /// The filter in `value`, or `None` when it has no directive or a directive is not one.
    fn parse(value: &str) -> Option<Filter> {
        // As the Gateway's: with only target directives, other targets are off.
        let mut filter = Filter {
            default: LevelFilter::Off,
            targets: Vec::new(),
        };
        let mut any = false;
        for directive in value
            .split(',')
            .map(str::trim)
            .filter(|directive| !directive.is_empty())
        {
            any = true;
            match directive.split_once('=') {
                Some((target, level)) => {
                    let target = target.trim();
                    if !is_target(target) {
                        return None;
                    }
                    filter.targets.push((target.to_string(), level.trim().parse().ok()?));
                }
                None => match directive.parse() {
                    Ok(level) => filter.default = level,
                    // A target alone takes every level, as the Gateway's.
                    Err(_) if is_target(directive) => filter.targets.push((directive.to_string(), LevelFilter::Trace)),
                    Err(_) => return None,
                },
            }
        }
        any.then_some(filter)
    }

    fn level(&self, target: &str) -> LevelFilter {
        self.targets
            .iter()
            .filter(|(prefix, _)| target.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .map_or(self.default, |(_, level)| *level)
    }

    fn max(&self) -> LevelFilter {
        self.targets
            .iter()
            .map(|(_, level)| *level)
            .fold(self.default, Ord::max)
    }
}

/// A Rust target: a crate or module path. Span filters (`[span]`) and other syntax are not ones.
fn is_target(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':' || c == '-')
}

/// The filter from `NEXUS_LOG`, else `RUST_LOG`, else [`DEFAULT`], and a warning for a `NEXUS_LOG` that is not one.
fn choose(nexus: Option<&str>, rust: Option<&str>) -> (Filter, Option<String>) {
    fn written(value: Option<&str>) -> Option<&str> {
        value.filter(|value| value.split(',').any(|part| !part.trim().is_empty()))
    }
    let default = || Filter::parse(DEFAULT).expect("the default is a filter");
    if let Some(value) = written(nexus) {
        return match Filter::parse(value) {
            Some(filter) => (filter, None),
            None => {
                let shown: String = value.chars().take(80).collect();
                (
                    default(),
                    Some(format!("NEXUS_LOG {shown:?} is not a log filter; using {DEFAULT}")),
                )
            }
        };
    }
    (written(rust).and_then(Filter::parse).unwrap_or_else(default), None)
}

struct Logger(Filter);

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.0.level(metadata.target())
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:>5} {}: {}\n",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ"),
            record.level(),
            record.target(),
            record.args()
        );
        // One write per line, so threads' lines do not mix. A closed stderr (a Windows GUI build) drops it.
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// First thing in `main`, in every process this binary runs: the logger, at the level the environment sets.
pub fn install() {
    START.get_or_init(Instant::now);
    let env = |name: &str| std::env::var(name).ok();
    let (filter, warning) = choose(env("NEXUS_LOG").as_deref(), env("RUST_LOG").as_deref());
    let max = filter.max();
    if log::set_boxed_logger(Box::new(Logger(filter))).is_ok() {
        log::set_max_level(max);
    }
    if let Some(warning) = warning {
        log::warn!("{warning}");
    }
}

/// Milliseconds since [`install`], for the start's lines.
pub fn since_start_ms() -> u128 {
    START.get().map_or(0, |start| start.elapsed().as_millis())
}

/// An address for a log line, without a user name or password in it.
pub fn shown_url(text: &str) -> String {
    match url::Url::parse(text) {
        Ok(mut url) if !url.username().is_empty() || url.password().is_some() => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        Ok(_) => text.to_string(),
        Err(_) => "an address that does not parse".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_shows_warnings_and_the_apps_start() {
        let (filter, warning) = choose(None, None);
        assert!(warning.is_none());
        assert_eq!(filter.level("gpui_pre::window"), LevelFilter::Warn);
        assert_eq!(filter.level("nexus_desktop_gpui"), LevelFilter::Info);
        assert_eq!(filter.level("nexus_desktop_gpui::gateway"), LevelFilter::Info);
        assert_eq!(filter.max(), LevelFilter::Info);
    }

    #[test]
    fn nexus_log_wins_over_rust_log() {
        let (filter, _) = choose(Some("debug"), Some("error"));
        assert_eq!(filter.level("rustls"), LevelFilter::Debug);
        let (filter, _) = choose(None, Some("error"));
        assert_eq!(filter.level("rustls"), LevelFilter::Error);
        // Blank, or commas only, counts as unset, as the Gateway's.
        let (filter, warning) = choose(Some(" , "), Some("trace"));
        assert!(warning.is_none());
        assert_eq!(filter.level("rustls"), LevelFilter::Trace);
    }

    #[test]
    fn the_longest_target_wins_and_other_targets_are_off_without_a_level() {
        let filter = Filter::parse("info,nexus_desktop_gpui=debug,nexus_desktop_gpui::web=error").unwrap();
        assert_eq!(filter.level("nexus_desktop_gpui::gateway"), LevelFilter::Debug);
        assert_eq!(filter.level("nexus_desktop_gpui::web::client"), LevelFilter::Error);
        assert_eq!(filter.level("gpui_pre"), LevelFilter::Info);
        let only = Filter::parse("nexus_desktop_gpui=debug").unwrap();
        assert_eq!(only.level("gpui_pre"), LevelFilter::Off);
        let bare = Filter::parse("nexus_desktop_gpui").unwrap();
        assert_eq!(bare.level("nexus_desktop_gpui"), LevelFilter::Trace);
        assert_eq!(Filter::parse("OFF").unwrap().max(), LevelFilter::Off);
    }

    #[test]
    fn a_value_that_is_not_a_filter_gives_the_default_and_a_warning() {
        for value in ["info,[[[", "nexus_desktop_gpui=loud", "=info", "[span]=debug"] {
            let (filter, warning) = choose(Some(value), Some("trace"));
            assert_eq!(filter, Filter::parse(DEFAULT).unwrap(), "{value}");
            assert!(warning.is_some_and(|warning| warning.contains("NEXUS_LOG")), "{value}");
        }
        // A `RUST_LOG` that is not one is another tool's: the default, quietly.
        assert_eq!(choose(None, Some("info,[[[")), (Filter::parse(DEFAULT).unwrap(), None));
    }

    #[test]
    fn a_logged_address_has_no_credentials() {
        assert_eq!(shown_url("https://gateway.nexus.inc"), "https://gateway.nexus.inc");
        assert_eq!(shown_url("http://me:secret@localhost:8080/"), "http://localhost:8080/");
        assert!(!shown_url("not a url secret").contains("secret"));
    }
}
