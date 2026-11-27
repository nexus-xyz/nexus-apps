//! The workbench's notice: a short line at the foot of the window that outlives the panel that raised it, with one
//! action at most (Undo after an archive). One at a time: a newer one replaces the older, and an action runs only while
//! its notice is still the one on screen. Nexus GPUI draws from [`Notices`].

use std::time::Duration;

/// How long a notice stays.
pub const NOTICE: Duration = Duration::from_millis(12_000);
/// How long Undo stays offered after an archive.
pub const UNDO: Duration = Duration::from_millis(8_000);
/// How long the question about unpushed commits stays: it is a question, so longer than Undo.
pub const UNPUSHED: Duration = Duration::from_millis(30_000);

/// What the notice says once the sessions called `titles` are archived (`archivedText`).
pub fn archived_text<S: AsRef<str>>(titles: &[S]) -> String {
    match titles {
        [only] => format!("Archived \u{201c}{}\u{201d}", only.as_ref()),
        _ => format!("Archived {} sessions", titles.len()),
    }
}

/// What the notice says when Undo could not bring a session back.
pub fn restore_failed_text(error: &str) -> String {
    format!("Couldn\u{2019}t restore: {error}")
}

/// A notice on screen: its id, its line, and its action's label and what it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice<A> {
    pub id: u64,
    pub text: String,
    pub action: Option<(String, A)>,
}

/// The one notice on screen, if any. `A` is what its action does, which the app runs.
#[derive(Debug)]
pub struct Notices<A> {
    current: Option<Notice<A>>,
    next: u64,
}

impl<A> Default for Notices<A> {
    fn default() -> Self {
        Self { current: None, next: 0 }
    }
}

impl<A> Notices<A> {
    /// Shows `text` in place of any notice on screen, with `action`'s button when given. Answers its id: the app takes
    /// it away with [`Notices::dismiss`] once its time ([`NOTICE`] unless the caller picks another) is up.
    pub fn show(&mut self, text: impl Into<String>, action: Option<(String, A)>) -> u64 {
        self.next += 1;
        self.current = Some(Notice {
            id: self.next,
            text: text.into(),
            action,
        });
        self.next
    }

    /// The notice on screen.
    pub fn current(&self) -> Option<&Notice<A>> {
        self.current.as_ref()
    }

    /// Presses notice `id`'s action, if it is still the one on screen, and takes the notice away. Answers what the
    /// action does, for the app to run; nothing when the notice was replaced or is gone, so it runs once at most.
    pub fn run_action(&mut self, id: u64) -> Option<A> {
        if self.current.as_ref()?.id != id {
            return None;
        }
        self.current.take()?.action.map(|(_, action)| action)
    }

    /// Takes the notice away; with `id`, only if it is still that one (a replaced notice's timer leaves the newer
    /// one alone). Answers whether one went.
    pub fn dismiss(&mut self, id: Option<u64>) -> bool {
        match (&self.current, id) {
            (Some(notice), Some(id)) if notice.id != id => false,
            (Some(_), _) => {
                self.current = None;
                true
            }
            (None, _) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn undo(run: &'static str) -> Option<(String, &'static str)> {
        Some(("Undo".to_string(), run))
    }

    #[test]
    fn names_one_session_counts_several() {
        assert_eq!(
            archived_text(&["Fix the build"]),
            "Archived \u{201c}Fix the build\u{201d}"
        );
        assert_eq!(archived_text(&["a", "b"]), "Archived 2 sessions");
        assert_eq!(
            restore_failed_text("The core kept it."),
            "Couldn\u{2019}t restore: The core kept it."
        );
    }

    #[test]
    fn a_newer_notice_replaces_it_and_the_older_ones_timer_leaves_it() {
        let mut notices = Notices::<&str>::default();
        let first = notices.show("first", None);
        assert_eq!(notices.current().map(|notice| notice.text.as_str()), Some("first"));
        notices.show("second", None);
        // The first notice's time is up: the second stays.
        assert!(!notices.dismiss(Some(first)));
        assert_eq!(notices.current().map(|notice| notice.text.as_str()), Some("second"));
        let second = notices.current().expect("the second notice").id;
        assert!(notices.dismiss(Some(second)));
        assert!(notices.current().is_none());
    }

    #[test]
    fn dismisses_only_the_notice_it_was_asked_about() {
        let mut notices = Notices::<&str>::default();
        let old = notices.show("one", None);
        notices.show("two", None);
        notices.dismiss(Some(old));
        assert_eq!(notices.current().map(|notice| notice.text.as_str()), Some("two"));
        notices.dismiss(None);
        assert!(notices.current().is_none());
    }

    #[test]
    fn runs_its_action_once_and_only_while_it_is_still_the_one_on_screen() {
        let mut notices = Notices::default();
        let id = notices.show("Archived", undo("restore"));
        assert_eq!(
            notices
                .current()
                .and_then(|notice| notice.action.as_ref())
                .map(|(label, _)| label.as_str()),
            Some("Undo")
        );
        assert_eq!(notices.run_action(id), Some("restore"));
        assert_eq!(notices.run_action(id), None);
        assert!(notices.current().is_none());
        let id = notices.show("Archived", undo("restore"));
        assert!(notices.dismiss(Some(id)));
        assert_eq!(notices.run_action(id), None);
    }

    #[test]
    fn does_not_run_a_replaced_notices_action_nor_the_newer_ones() {
        let mut notices = Notices::default();
        let a = notices.show("A", undo("a"));
        notices.show("B", undo("b"));
        assert_eq!(notices.run_action(a), None);
        assert_eq!(notices.current().map(|notice| notice.text.as_str()), Some("B"));
    }
}
