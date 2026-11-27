//! `/btw`'s overlay, as Claude Code shows a side question: in place of the composer, the question, the
//! newest earlier ones dimmed above it, and the answer as it streams. Space, Enter or Esc dismiss it; `x` clears the
//! earlier ones; `/btw` alone opens the last again. Nothing of it joins the conversation.

use super::{Answer, App, Command, Key, Mode};

/// The earlier side questions that go with the next one, and those shown.
const REPLAYED: usize = 20;
const SHOWN: usize = 5;

/// A side question and its answer so far.
#[derive(Debug, Clone, Default)]
pub struct Btw {
    pub question: String,
    pub answer: String,
    pub done: bool,
    pub error: Option<String>,
}

impl App {
    /// `/btw <question>`, or `/btw` alone to open the last answer again.
    pub(super) fn btw(&mut self, question: String) -> Option<Command> {
        if question.is_empty() {
            if self.btw.is_some() {
                self.btw_open = true;
            } else {
                self.notice(
                    "Ask a side question with /btw <question>: it answers from this conversation, without tools.",
                );
            }
            return None;
        }
        if self.btw.as_ref().is_some_and(|btw| !btw.done) {
            self.notice("A side question is still being answered: wait for it, then ask the next.");
            return None;
        }
        let earlier: Vec<(String, String)> = self.btw_log.iter().rev().take(REPLAYED).rev().cloned().collect();
        self.btw = Some(Btw {
            question: question.clone(),
            ..Btw::default()
        });
        self.btw_open = true;
        Some(Command::Btw {
            question,
            agent: self.mode == Mode::Agent,
            session: self.session.clone(),
            model: self.requested.clone(),
            earlier,
        })
    }

    /// A key while the overlay is open.
    pub(super) fn btw_key(&mut self, key: &Key) -> Option<Command> {
        match key {
            Key::Char(' ') | Key::Enter | Key::Esc => self.btw_open = false,
            Key::Char('x') => self.btw_log.clear(),
            _ => {}
        }
        None
    }

    pub(super) fn btw_delta(&mut self, text: &str) {
        if let Some(btw) = &mut self.btw {
            btw.answer.push_str(text);
        }
    }

    pub(super) fn btw_done(&mut self, result: Result<f64, String>) {
        let Some(btw) = &mut self.btw else { return };
        btw.done = true;
        match result {
            Ok(cost) => {
                self.spent += cost;
                self.btw_log.push((btw.question.clone(), btw.answer.clone()));
            }
            Err(error) => btw.error = Some(error),
        }
    }

    /// The overlay's lines.
    pub(super) fn btw_lines(&self, width: usize) -> Vec<String> {
        let style = &self.theme.style;
        let Some(btw) = &self.btw else { return Vec::new() };
        let fit = |line: String| crate::style::truncate(&line, width.saturating_sub(1), style.g.ellipsis);
        let rule = if style.unicode { "─" } else { "-" };
        let mut lines = vec![style.accent(&rule.repeat(width.saturating_sub(1)))];
        // The newest earlier ones, dimmed, the current one excluded.
        let earlier: Vec<&(String, String)> = self
            .btw_log
            .iter()
            .filter(|(question, _)| !(btw.done && *question == btw.question))
            .collect();
        let older = earlier.len().saturating_sub(SHOWN);
        if older > 0 {
            lines.push(fit(format!(
                "  {}",
                style.dim(&format!("{older} earlier side questions"))
            )));
        }
        for (question, _) in earlier.iter().skip(older) {
            lines.push(fit(format!("  {}", style.dim(&format!("/btw {question}")))));
        }
        lines.push(fit(format!("  {} {}", style.accent("/btw"), style.bold(&btw.question))));
        lines.push(String::new());
        if let Some(error) = &btw.error {
            lines.push(fit(format!("  {}", style.red(error))));
        } else if btw.answer.trim().is_empty() {
            lines.push(format!("  {}", style.dim("Answering…")));
        } else {
            let mut answer = Answer::default();
            answer.push(&btw.answer);
            let (shown, _) = answer.take(&self.theme, width, true);
            lines.extend(shown);
        }
        lines.push(String::new());
        let keys = if btw.done {
            "Space, Enter or Esc to dismiss · x to clear earlier side questions · not added to the conversation"
        } else {
            "Esc to dismiss (it keeps answering; /btw opens it again)"
        };
        lines.push(fit(format!("  {}", style.dim(keys))));
        lines
    }
}
