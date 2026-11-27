//! Cite selected answer text in a plain composer, matching `workbench/cite.ts`.

pub const CITE_MAX: usize = 8_000;

pub fn quote_block(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n").replace('\u{a0}', " ");
    let lines: Vec<&str> = normalized.split('\n').map(str::trim_end).collect();
    let Some(first) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return String::new();
    };
    let last = lines.iter().rposition(|line| !line.trim().is_empty()).unwrap_or(first);
    let mut out = Vec::new();
    let mut blank = false;
    for line in &lines[first..=last] {
        if line.trim().is_empty() {
            if !blank {
                out.push(">".into());
            }
            blank = true;
        } else {
            out.push(format!("> {line}"));
            blank = false;
        }
    }
    out.join("\n")
}

/// None leaves the draft and its attachments untouched.
pub fn with_quote(draft: &str, selection: &str) -> Option<String> {
    if selection.encode_utf16().count() > CITE_MAX {
        return None;
    }
    let quote = quote_block(selection);
    if quote.is_empty() {
        return None;
    }
    let before = draft.trim_end();
    Some(format!(
        "{}{quote}\n\n",
        if before.is_empty() {
            String::new()
        } else {
            format!("{before}\n\n")
        }
    ))
}

/// A window selection must belong entirely to one finished answer.
pub fn eligible<'a>(parts: impl IntoIterator<Item = &'a str>, whole: &str) -> Option<String> {
    let mut parts = parts.into_iter().filter(|text| !text.trim().is_empty());
    let first = parts.next()?;
    if parts.next().is_some() || first.trim() != whole.trim() {
        return None;
    }
    Some(first.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_each_line_and_preserves_only_one_paragraph_gap() {
        assert_eq!(quote_block("first line\nsecond line"), "> first line\n> second line");
        assert_eq!(
            quote_block("\n\n  \nparagraph one\n\n\n\nparagraph two  \n\n"),
            "> paragraph one\n>\n> paragraph two"
        );
        assert_eq!(quote_block("a\u{a0}b\r\nc"), "> a b\n> c");
        assert_eq!(quote_block(" \n \n"), "");
    }
    #[test]
    fn joins_after_the_draft_with_room_for_a_new_paragraph() {
        assert_eq!(
            with_quote("", "the bridge drops chunks").as_deref(),
            Some("> the bridge drops chunks\n\n")
        );
        assert_eq!(
            with_quote("Look at this:  \n", "x").as_deref(),
            Some("Look at this:\n\n> x\n\n")
        );
        assert!(with_quote("kept", "  ").is_none());
    }
    #[test]
    fn caps_the_selection_in_utf16_units() {
        assert!(with_quote("", &"x".repeat(CITE_MAX)).is_some());
        assert!(with_quote("", &"x".repeat(CITE_MAX + 1)).is_none());
        assert!(with_quote("", &"😀".repeat(4001)).is_none());
    }
    #[test]
    fn rejects_multiple_answers_and_selection_outside_the_answer() {
        assert_eq!(eligible(["answer"], "answer"), Some("answer".into()));
        assert!(eligible(["first", "second"], "first\nsecond").is_none());
        assert!(eligible(["answer"], "question\nanswer").is_none());
        assert!(eligible(["  "], " ").is_none());
    }
}
