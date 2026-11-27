//! The session's composer: the text being written, its cursor, and the prompts sent before (↑ and ↓ recall them).
//! Plain state, edited by the session's keys and tested without a terminal. The cursor is a byte index that always
//! sits on a character boundary.

#[derive(Debug, Clone, Default)]
pub struct Composer {
    text: String,
    cursor: usize,
    /// The prompts sent in this session, oldest first.
    history: Vec<String>,
    /// Which sent prompt ↑ shows now, and the text that was there before ↑ (↓ past the newest brings it back).
    recalled: Option<(usize, String)>,
}

impl Composer {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The text to send, trimmed; the composer is emptied and the prompt joins the history. `None` when blank.
    pub fn take(&mut self) -> Option<String> {
        let prompt = self.text.trim().to_string();
        if prompt.is_empty() {
            return None;
        }
        self.history.push(prompt.clone());
        self.clear();
        Some(prompt)
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.recalled = None;
    }

    /// Typed or pasted text. A paste's `\r\n` and `\r` become `\n`, and a tab becomes four spaces.
    pub fn insert(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }

    /// Put `text` in place of the bytes `start..end`, the cursor after it (an `@` mention chosen from its list).
    pub fn replace(&mut self, start: usize, end: usize, text: &str) {
        self.text.replace_range(start..end, text);
        self.cursor = start + text.len();
        self.recalled = None;
    }

    pub fn backspace(&mut self) {
        if let Some(previous) = self.previous_boundary(self.cursor) {
            self.text.replace_range(previous..self.cursor, "");
            self.cursor = previous;
        }
    }

    pub fn delete(&mut self) {
        if let Some(next) = self.next_boundary(self.cursor) {
            self.text.replace_range(self.cursor..next, "");
        }
    }

    pub fn left(&mut self) {
        if let Some(previous) = self.previous_boundary(self.cursor) {
            self.cursor = previous;
        }
    }

    pub fn right(&mut self) {
        if let Some(next) = self.next_boundary(self.cursor) {
            self.cursor = next;
        }
    }

    /// The start of the line the cursor is on.
    pub fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    /// The end of the line the cursor is on.
    pub fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    /// ⌃u: delete from the start of the line to the cursor.
    pub fn delete_to_line_start(&mut self) {
        let start = self.line_start(self.cursor);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// ⌃k: delete from the cursor to the end of the line.
    pub fn delete_to_line_end(&mut self) {
        let end = self.line_end(self.cursor);
        self.text.replace_range(self.cursor..end, "");
    }

    /// ⌃w: delete the word before the cursor and the spaces after it.
    pub fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches([' ', '\t']);
        let start = trimmed
            .rfind(|c: char| c.is_whitespace())
            .map(|at| at + trimmed[at..].chars().next().map_or(1, char::len_utf8))
            .unwrap_or(0);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// ↑: the line above, or on the first line, the prompt sent before.
    pub fn up(&mut self) {
        let start = self.line_start(self.cursor);
        if start == 0 {
            self.recall_older();
            return;
        }
        let column = self.text[start..self.cursor].chars().count();
        let above = self.line_start(start - 1);
        self.cursor = self.at_column(above, column);
    }

    /// ↓: the line below, or on the last line, the prompt sent after the one shown.
    pub fn down(&mut self) {
        let end = self.line_end(self.cursor);
        if end == self.text.len() {
            self.recall_newer();
            return;
        }
        let column = self.text[self.line_start(self.cursor)..self.cursor].chars().count();
        self.cursor = self.at_column(end + 1, column);
    }

    /// Whether ↑ brought back an earlier prompt that is shown now.
    pub fn recalling(&self) -> bool {
        self.recalled.is_some()
    }

    fn recall_older(&mut self) {
        let at = match &self.recalled {
            Some((at, _)) => at.checked_sub(1),
            None => self.history.len().checked_sub(1),
        };
        let Some(at) = at else { return };
        let draft = match self.recalled.take() {
            Some((_, draft)) => draft,
            None => std::mem::take(&mut self.text),
        };
        self.text = self.history[at].clone();
        self.cursor = self.text.len();
        self.recalled = Some((at, draft));
    }

    fn recall_newer(&mut self) {
        let Some((at, draft)) = self.recalled.take() else {
            return;
        };
        if at + 1 < self.history.len() {
            self.text = self.history[at + 1].clone();
            self.recalled = Some((at + 1, draft));
        } else {
            self.text = draft;
        }
        self.cursor = self.text.len();
    }

    fn previous_boundary(&self, at: usize) -> Option<usize> {
        self.text[..at].char_indices().next_back().map(|(index, _)| index)
    }

    fn next_boundary(&self, at: usize) -> Option<usize> {
        self.text[at..].chars().next().map(|c| at + c.len_utf8())
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |newline| newline + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |newline| at + newline)
    }

    /// The byte index `column` characters into the line that starts at `start`, or that line's end.
    fn at_column(&self, start: usize, column: usize) -> usize {
        let end = self.line_end(start);
        self.text[start..end]
            .char_indices()
            .nth(column)
            .map_or(end, |(index, _)| start + index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Composer {
        let mut composer = Composer::default();
        composer.insert(text);
        composer
    }

    #[test]
    fn edits_around_the_cursor() {
        let mut composer = typed("héllo");
        composer.left();
        composer.left();
        composer.backspace();
        assert_eq!((composer.text(), composer.cursor()), ("hélo", 3));
        composer.delete();
        assert_eq!(composer.text(), "héo");
        composer.home();
        composer.insert("¡");
        assert_eq!(composer.text(), "¡héo");
        composer.end();
        assert_eq!(composer.cursor(), composer.text().len());
    }

    #[test]
    fn paste_keeps_lines_and_expands_tabs() {
        let composer = typed("a\r\nb\rc\td");
        assert_eq!(composer.text(), "a\nb\nc    d");
    }

    #[test]
    fn line_keys_stay_on_their_line() {
        let mut composer = typed("first line\nsecond line");
        composer.delete_to_line_start();
        assert_eq!(composer.text(), "first line\n");
        composer.insert("two words  ");
        composer.delete_word();
        assert_eq!(composer.text(), "first line\ntwo ");
        composer.up();
        composer.delete_to_line_end();
        assert_eq!(composer.text(), "firs\ntwo ");
    }

    #[test]
    fn up_and_down_move_between_lines_by_column() {
        let mut composer = typed("abc\nde\nfghij");
        composer.up();
        assert_eq!(composer.cursor(), 6, "the end of `de`: it is shorter than column 5");
        composer.up();
        assert_eq!(composer.cursor(), 2);
        composer.down();
        composer.down();
        assert_eq!(composer.cursor(), 9);
    }

    #[test]
    fn take_sends_trimmed_text_and_remembers_it() {
        let mut composer = typed("  ");
        assert_eq!(composer.take(), None);
        composer.insert(" one ");
        assert_eq!(composer.take().as_deref(), Some("one"));
        assert!(composer.is_empty());
        composer.insert("two");
        composer.take();
        composer.insert("draft");
        composer.up();
        assert_eq!(composer.text(), "two");
        composer.up();
        assert_eq!(composer.text(), "one");
        composer.up();
        assert_eq!(composer.text(), "one", "nothing older");
        composer.down();
        composer.down();
        assert_eq!(composer.text(), "draft", "↓ past the newest brings the draft back");
    }
}
