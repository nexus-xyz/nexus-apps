//! Desktop launch routing; prompt parsing and writing live in the shared desktop core.
pub use nexus_desktop_core::prompt_link::*;

/// A phone can hand this fixed route to Desktop. Setup still happens in the signed-in app.
pub(crate) fn is_agent_setup(url: &str) -> bool {
    url.split_once(':')
        .is_some_and(|(scheme, path)| scheme.eq_ignore_ascii_case("nexus") && path == "//settings/agents")
}

/// What reaches the running copy from a second launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arrival {
    /// A plain launch: bring the window forward.
    Show,
    /// A launch with [`NEW_WINDOW_ARG`]: open another window.
    NewWindow,
    Link(String),
}

/// The argument that asks the running copy for another window, as New Window does (`windows.rs`).
pub const NEW_WINDOW_ARG: &str = "--new-window";

/// The most links one launch hands over.
const MAX_WAITING: usize = 8;

/// What a later launch's arguments (`single_instance.rs`) bring: a new window for [`NEW_WINDOW_ARG`], then its links
/// (which open in that window), or a plain Show.
pub fn arrivals(args: &[String]) -> Vec<Arrival> {
    let new_window = args.iter().any(|arg| arg == NEW_WINDOW_ARG);
    let links: Vec<Arrival> = args
        .iter()
        .filter(|arg| accepted(arg))
        .take(MAX_WAITING)
        .map(|arg| Arrival::Link(arg.clone()))
        .collect();
    if new_window {
        std::iter::once(Arrival::NewWindow).chain(links).collect()
    } else if links.is_empty() {
        vec![Arrival::Show]
    } else {
        links
    }
}

/// The links this launch was started with, for the first window only: a window opened later does not open them again.
pub fn launch_links() -> Vec<String> {
    static TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if TAKEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return Vec::new();
    }
    from_args()
}

/// The links this launch was started with.
fn from_args() -> Vec<String> {
    std::env::args()
        .skip(1)
        .filter(|arg| accepted(arg))
        .take(MAX_WAITING)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_fixed_agent_settings_link_opens_setup() {
        assert!(is_agent_setup("nexus://settings/agents"));
        assert!(is_agent_setup("NEXUS://settings/agents"));
        for url in [
            "https://settings/agents",
            "nexus://settings/agents?install=codex",
            "nexus://settings/agents#sign-in",
            "nexus://user@settings/agents",
            "nexus://settings/agents/claude",
            "nexus://settings/%61gents",
        ] {
            assert!(!is_agent_setup(url), "{url}");
        }
    }

    /// `--new-window` on a second launch opens a window first; its links then go to that window, the front one.
    #[test]
    fn a_second_launch_asks_for_a_window_its_links_or_a_show() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(arrivals(&args(&["nexus"])), [Arrival::Show]);
        assert_eq!(arrivals(&args(&["nexus", NEW_WINDOW_ARG])), [Arrival::NewWindow]);
        let link = "nexus://new?prompt=a";
        assert_eq!(
            arrivals(&args(&["nexus", link, NEW_WINDOW_ARG])),
            [Arrival::NewWindow, Arrival::Link(link.into())]
        );
        assert_eq!(arrivals(&args(&["nexus", link])), [Arrival::Link(link.into())]);
    }
}
