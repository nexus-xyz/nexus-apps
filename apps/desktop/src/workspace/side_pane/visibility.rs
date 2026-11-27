//! Each session keeps its docked Workbench choice while Nexus stays open.
use super::Scope;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Visibility {
    current: Option<(bool, Scope)>,
    saved: HashMap<(bool, Scope), bool>,
}

impl Visibility {
    /// Save the departing screen. An unseen session starts closed; New session has its own choice.
    /// The first binding preserves explicit actions and primary-window restoration.
    pub(super) fn switch(&mut self, scope: &Scope, home: bool, docked_open: bool) -> Option<bool> {
        let next = (home, if home { Scope::Home } else { scope.clone() });
        if self.current.as_ref() == Some(&next) {
            return None;
        }
        let previous = self.current.replace(next.clone())?;
        self.saved.insert(previous, docked_open);
        Some(self.saved.get(&next).copied().unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_and_chats_restore_independent_choices_and_default_closed() {
        let mut visibility = Visibility::default();
        let one = Scope::Session("one".into());
        let two = Scope::Session("two".into());
        let chat = Scope::Chat("one".into());
        assert_eq!(visibility.switch(&one, false, false), None);
        assert_eq!(visibility.switch(&two, false, true), Some(false));
        assert_eq!(visibility.switch(&one, false, false), Some(true));
        assert_eq!(visibility.switch(&chat, false, false), Some(false));
        assert_eq!(visibility.switch(&two, false, true), Some(false));
        assert_eq!(visibility.switch(&chat, false, false), Some(true));
        assert_eq!(visibility.switch(&one, false, true), Some(false));
    }

    #[test]
    fn new_session_keeps_its_choice_across_projects_without_opening_the_started_session() {
        let mut visibility = Visibility::default();
        let project = Scope::Project("project".into());
        let session = Scope::Session("started".into());
        assert_eq!(visibility.switch(&project, true, false), None);
        assert_eq!(visibility.switch(&Scope::Home, true, true), None);
        assert_eq!(visibility.switch(&session, false, true), Some(false));
        assert_eq!(visibility.switch(&project, true, false), Some(true));
    }

    #[test]
    fn narrow_drawers_do_not_overwrite_each_sessions_docked_choice() {
        let mut visibility = Visibility::default();
        let mut layout = nexus_desktop_core::pane_layout::Visibility::default();
        let one = Scope::Session("one".into());
        let two = Scope::Session("two".into());
        visibility.switch(&one, false, false);
        assert!(!layout.resize(900., true));
        let next = visibility.switch(&two, false, layout.docked_choice(false)).unwrap();
        assert!(!layout.switch_context(next));
        assert!(!layout.resize(1200., true));
        let next = visibility.switch(&one, false, layout.docked_choice(false)).unwrap();
        assert!(layout.switch_context(next));
    }
}
