//! Suggested next prompts: after each answer, Gateway's `POST /v1/chat/sessions/{id}/suggest` asks the
//! model for the message the person is most likely to send next, as Claude Code does. The call reads the session's
//! cached prompt, so it costs a fraction of a turn, and nothing is stored. The session offers the text, dimmed, in the
//! empty composer (tab edits it, ⏎ sends it). `/suggestions off` or `NEXUS_NO_SUGGESTIONS=1` turns it off.
//!
//! Before it, each prompt carried an instruction and each answer ended with `<next>…</next>`, and Gateway stored both.
//! What is here takes them off again when an earlier session is shown.

const OPEN: &str = "<nexus-suggest>";
const CLOSE: &str = "</nexus-suggest>";
const NEXT: &str = "<next>";

/// A title Gateway took from a first message: what comes before the instruction, even when the title was cut short
/// inside it.
pub fn title_without_instruction(title: &str) -> &str {
    title.find(OPEN).map_or(title, |at| title[..at].trim_end())
}

/// A stored prompt without the instruction.
pub fn strip_instruction(content: &str) -> &str {
    match content.rfind(OPEN) {
        Some(at) if content.trim_end().ends_with(CLOSE) => content[..at].trim_end(),
        _ => content,
    }
}

/// Whether a stored prompt carried the instruction: an earlier session's, whose answer ends with a `<next>` line.
pub fn asked_for_next(prompt: &str) -> bool {
    strip_instruction(prompt).len() < prompt.len()
}

/// A stored answer without its `<next>` line, closed or cut off at its cap, when its prompt asked for one (`asked`,
/// [`asked_for_next`]). Any other answer stays whole, even one that mentions `<next>`.
pub fn strip_suggestion(answer: &str, asked: bool) -> &str {
    match answer.rfind(NEXT) {
        Some(at) if asked => answer[..at].trim_end(),
        _ => answer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_earlier_sessions_instruction_and_next_line_come_off() {
        let sent = "What is Nexus Gateway?\n\n<nexus-suggest>End with one line <next>…</next>.</nexus-suggest>";
        assert_eq!(strip_instruction(sent), "What is Nexus Gateway?");
        assert!(asked_for_next(sent));
        assert_eq!(
            strip_instruction("plain <nexus-suggest> mention"),
            "plain <nexus-suggest> mention"
        );
        assert!(!asked_for_next("plain <nexus-suggest> mention"));
        assert_eq!(
            strip_suggestion("It routes calls.\n<next>How do budgets work?</next>", true),
            "It routes calls."
        );
        // Cut off at its output cap, inside its last line.
        assert_eq!(
            strip_suggestion("It routes calls.\n<next>Show me an example", true),
            "It routes calls."
        );
        assert_eq!(strip_suggestion("a < b", true), "a < b");
        // A newer answer, whose prompt asked for nothing, stays whole.
        for answer in ["Wrap it in <next>…</next>", "The tag is <next>"] {
            assert_eq!(strip_suggestion(answer, false), answer);
        }
    }

    #[test]
    fn a_title_cut_inside_the_instruction_loses_it() {
        assert_eq!(
            title_without_instruction("Where is the budget? <nexus-suggest>End with one"),
            "Where is the budget?"
        );
        assert_eq!(title_without_instruction("Plans"), "Plans");
    }
}
