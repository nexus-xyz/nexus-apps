//! New-session Workbench rules.

pub const HOME_TOOLS: [&str; 2] = ["browser", "terminal"];

pub fn home_key(project: Option<&str>) -> String {
    format!("home:{}", project.unwrap_or_default())
}

pub fn home_project(key: &str) -> Option<&str> {
    key.strip_prefix("home:").filter(|project| !project.is_empty())
}

pub fn home_browser(window: &str, organization: &str) -> String {
    format!("home:{window}:{organization}")
}

pub fn home_tab(tab: &str) -> &'static str {
    if tab == "browser" { "browser" } else { "terminal" }
}

#[derive(Default)]
pub struct Visibility {
    pub home: bool,
    session_open: bool,
    home_open: bool,
}

impl Visibility {
    pub fn switch(&mut self, home: bool, shown: bool) -> bool {
        if home == self.home {
            return shown;
        }
        self.home = home;
        if home {
            self.session_open = shown;
            self.home_open
        } else {
            self.home_open = shown;
            self.session_open
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_terminals_by_project_and_browser_by_window_and_organization() {
        assert_eq!(home_key(Some("p_1")), "home:p_1");
        assert_eq!(home_key(None), "home:");
        assert_eq!(home_project(&home_key(Some("p_1"))), Some("p_1"));
        assert_eq!(home_project(&home_key(None)), None);
        assert_eq!(home_browser("main", "acme"), "home:main:acme");
        assert!(home_key(None).starts_with("home:"));
        assert!(!"t_6f1c2a9e".starts_with("home:"));
    }

    #[test]
    fn offers_browser_and_terminal_and_defaults_session_tools_to_terminal() {
        assert_eq!(HOME_TOOLS, ["browser", "terminal"]);
        assert_eq!(home_tab("browser"), "browser");
        for tab in ["terminal", "tasks", "changes", "files", "artifact", "side"] {
            assert_eq!(home_tab(tab), "terminal");
        }
    }

    #[test]
    fn home_and_session_visibility_stay_independent() {
        let mut state = Visibility::default();
        assert!(!state.switch(true, true));
        assert!(state.switch(false, false));
        assert!(!state.switch(true, false));
        assert!(!state.switch(false, true));
        assert!(state.switch(true, false));
    }
}
