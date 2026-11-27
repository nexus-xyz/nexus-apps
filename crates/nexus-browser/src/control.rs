//! Who drives a browser tab: the person, or the agent while the tab allows it. "Allow agent control", "Take over" and
//! "Stop" each end the agent's action in progress (a new control epoch), so its result is dropped. Both apps keep each
//! tab's state their own way and decide by these rules.

/// The actions an agent may run while a tab is loading: those that move the page.
pub fn runs_while_loading(action: &str) -> bool {
    matches!(
        action,
        "browser_open" | "browser_back" | "browser_forward" | "browser_reload"
    )
}

/// Whether an agent's tool `action` may start on a tab: it allows agent control, it is not loading (unless the action
/// moves the page), and no other action runs on it.
pub fn may_begin(allowed: bool, loading: bool, busy: bool, action: &str) -> Result<(), String> {
    if !allowed {
        return Err("Open this session's Browser tab and enable agent control first.".into());
    }
    if loading && !runs_while_loading(action) {
        return Err("The preview is loading. Wait before reading or interacting with it.".into());
    }
    if busy {
        return Err("The browser is busy. Wait for the current action.".into());
    }
    Ok(())
}

/// The control bar's words: who has the browser now.
pub fn line(agent: &str, allowed: bool, activity: Option<&str>) -> String {
    if !allowed {
        return "You have the browser".into();
    }
    if activity.is_some() {
        format!("{agent} is using the browser")
    } else {
        format!("{agent} can use the browser")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_only_allows_navigation_and_one_action_at_a_time() {
        assert!(may_begin(false, false, false, "browser_read").is_err());
        assert!(may_begin(true, true, false, "browser_read").is_err());
        assert!(may_begin(true, true, false, "browser_open").is_ok());
        assert!(may_begin(true, false, true, "browser_read").is_err());
        assert!(may_begin(true, false, false, "browser_click").is_ok());
    }

    #[test]
    fn the_control_bar_says_who_has_the_browser() {
        assert_eq!(line("Claude Code", false, None), "You have the browser");
        assert_eq!(line("Claude Code", true, None), "Claude Code can use the browser");
        assert_eq!(
            line("Claude Code", true, Some("browser_read")),
            "Claude Code is using the browser"
        );
    }
}
