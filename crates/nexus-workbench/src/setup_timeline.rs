//! Setup-script timeline rows, matching `timeline.ts` and `setup-row.tsx`.

use crate::event::SetupStatus;
pub const OUTPUT_MAX: usize = 64 * 1024;
pub const AFTER_FAILURE: &str = "The agent started anyway, so this worktree may be missing what the script sets up. Ask the agent to fix it or to run the script again.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupRow {
    pub id: String,
    pub command: String,
    pub status: SetupStatus,
    pub output: String,
    pub clipped: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
}
impl SetupRow {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.into(),
            command: String::new(),
            status: SetupStatus::Running,
            output: String::new(),
            clipped: false,
            exit_code: None,
            duration_ms: None,
        }
    }
    pub fn apply(
        &mut self,
        command: &str,
        status: SetupStatus,
        output: &str,
        exit_code: Option<i32>,
        duration_ms: Option<u64>,
    ) {
        self.command = command.into();
        self.status = status;
        self.exit_code = exit_code;
        self.duration_ms = duration_ms;
        self.output.push_str(output);
        let count = self.output.encode_utf16().count();
        if count > OUTPUT_MAX {
            self.clipped = true;
            let mut removed = 0;
            let mut end = 0;
            for (at, ch) in self.output.char_indices() {
                removed += ch.len_utf16();
                end = at + ch.len_utf8();
                if removed >= count - OUTPUT_MAX {
                    break;
                }
            }
            self.output.drain(..end);
        }
    }
    pub fn tag(&self) -> String {
        match self.status {
            SetupStatus::Running => "Running".into(),
            SetupStatus::Done => "Done".into(),
            SetupStatus::Stopped => "Stopped".into(),
            SetupStatus::TimedOut => "Timed out".into(),
            SetupStatus::Failed => self
                .exit_code
                .map_or("Failed".into(), |code| format!("Failed (exit {code})")),
        }
    }
    pub fn is_error(&self) -> bool {
        matches!(self.status, SetupStatus::Failed | SetupStatus::TimedOut)
    }
    pub fn problem(&self) -> Option<String> {
        let after = self
            .duration_ms
            .map(|ms| format!(" after {:.1} s", ms as f64 / 1000.))
            .unwrap_or_default();
        match self.status {
            SetupStatus::Stopped => Some(format!("You stopped it{after}. {AFTER_FAILURE}")),
            SetupStatus::TimedOut => Some(format!("Stopped{after}. {AFTER_FAILURE}")),
            SetupStatus::Failed => Some(AFTER_FAILURE.into()),
            _ => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn updates_accumulate_output_and_keep_the_final_status() {
        let mut row = SetupRow::new("setup_t");
        row.apply("npm ci", SetupStatus::Running, "added 12 packages\n", None, None);
        row.apply("npm ci", SetupStatus::Failed, "npm ERR! oops\n", Some(1), Some(4200));
        assert_eq!(row.output, "added 12 packages\nnpm ERR! oops\n");
        assert_eq!(row.tag(), "Failed (exit 1)");
        assert!(row.is_error());
        assert_eq!(row.duration_ms, Some(4200));
    }
    #[test]
    fn output_keeps_the_bounded_tail_on_unicode_boundaries() {
        let mut row = SetupRow::new("s");
        row.apply("x", SetupStatus::Running, &"x".repeat(OUTPUT_MAX), None, None);
        row.apply("x", SetupStatus::Done, "the end\n", Some(0), Some(1));
        assert!(row.clipped);
        assert_eq!(row.output.len(), OUTPUT_MAX);
        assert!(row.output.ends_with("the end\n"));
        row.apply("x", SetupStatus::Done, &"😀".repeat(OUTPUT_MAX), Some(0), Some(1));
        assert!(row.output.encode_utf16().count() <= OUTPUT_MAX);
    }
    #[test]
    fn stopped_is_a_plain_note_and_does_not_end_the_session() {
        let mut row = SetupRow::new("s");
        row.apply("x", SetupStatus::Stopped, "", None, Some(4200));
        assert_eq!(row.tag(), "Stopped");
        assert!(!row.is_error());
        assert!(
            row.problem()
                .expect("a stopped or timed-out script has a problem note")
                .starts_with("You stopped it after 4.2 s.")
        );
        row.status = SetupStatus::TimedOut;
        assert!(row.is_error());
        assert!(
            row.problem()
                .expect("a stopped or timed-out script has a problem note")
                .starts_with("Stopped after")
        );
        row.status = SetupStatus::Running;
        assert!(row.problem().is_none());
    }
    #[test]
    fn stopped_and_timed_out_notes_omit_unknown_duration() {
        let mut row = SetupRow::new("s");
        for (status, note) in [
            (SetupStatus::Stopped, "You stopped it."),
            (SetupStatus::TimedOut, "Stopped."),
        ] {
            row.apply("x", status, "", None, None);
            assert_eq!(row.problem(), Some(format!("{note} {AFTER_FAILURE}")));
        }
    }

    #[test]
    fn stopped_and_timed_out_notes_keep_known_zero_duration() {
        let mut row = SetupRow::new("s");
        for (status, note) in [
            (SetupStatus::Stopped, "You stopped it after 0.0 s."),
            (SetupStatus::TimedOut, "Stopped after 0.0 s."),
        ] {
            row.apply("x", status, "", None, Some(0));
            assert_eq!(row.problem(), Some(format!("{note} {AFTER_FAILURE}")));
        }
    }
}
