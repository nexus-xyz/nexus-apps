//! A shell that runs one command and says how it ended: a setup fix's step (`crate::setup`, `Fix::RunInTerminal`),
//! such as `gh auth login --web`. The person sees it and answers its prompts. When it ends the terminal says so, Enter
//! starts a shell, and the view emits [`TerminalEvent::Exited`] once, with the exit code and the last line of output.

use std::sync::{Arc, Mutex};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use gpui_kit::{Context, EventEmitter, SharedString};
use portable_pty::CommandBuilder;

use super::{Terminal, exit_note};

/// How a watched command ended: unset while it runs, then its exit code (`None` when the system gave none).
pub(super) type Exit = Arc<Mutex<Option<Option<u32>>>>;

/// A watched command's state on its terminal.
#[derive(Default)]
pub(super) struct Once {
    /// The command the next start runs instead of a shell, once.
    pub(super) command: Option<String>,
    pub(super) exit: Exit,
    /// Whether anyone listens for the end: only a fix's terminal.
    pub(super) watched: bool,
    pub(super) reported: bool,
}

pub enum TerminalEvent {
    /// The watched command ended.
    Exited {
        code: Option<u32>,
        last_line: Option<String>,
    },
}

impl EventEmitter<TerminalEvent> for Terminal {}

/// The person's login shell running `line`, so their `PATH` and aliases apply, as when they type it.
pub(super) fn shell_command(line: &str) -> CommandBuilder {
    let shell = CommandBuilder::new_default_prog().get_shell();
    let mut command = CommandBuilder::new(shell);
    if cfg!(windows) {
        command.arg("/C");
    } else {
        command.arg("-lc");
    }
    command.arg(line);
    command
}

/// The last line of `rows` with text on it, trimmed.
fn last_text(rows: impl DoubleEndedIterator<Item = String>) -> Option<String> {
    rows.rev().map(|row| row.trim().to_string()).find(|row| !row.is_empty())
}

impl Terminal {
    /// A terminal in `dir` that runs `command` in the person's login shell, and reports its end.
    pub fn run_once(dir: std::path::PathBuf, command: &str, cx: &mut Context<Self>) -> Terminal {
        let mut terminal = Self::unstarted(dir, None, cx);
        terminal.once.command = Some(command.to_string());
        terminal.once.watched = true;
        terminal.start(cx);
        terminal
    }

    /// Report this terminal's command's end ([`TerminalEvent::Exited`]): a server shell made to run one command.
    pub fn watch_exit(&mut self) {
        self.once.watched = true;
    }

    /// The command ended since the last wake: say so on the terminal, and tell the listener once.
    pub(super) fn report_exit(&mut self, cx: &mut Context<Self>) {
        if !self.once.watched {
            return;
        }
        let Some(code) = self.once.exit.lock().ok().and_then(|exit| *exit) else {
            return;
        };
        // Every wake after the end keeps the note (a server's own note comes first).
        if self.notice.is_none() {
            self.notice = exit_note(code, true).map(SharedString::from);
        }
        if self.once.reported {
            return;
        }
        self.once.reported = true;
        let last_line = self.last_line();
        cx.emit(TerminalEvent::Exited { code, last_line });
    }

    /// The last line of output on screen with text on it.
    pub fn last_line(&self) -> Option<String> {
        let term = self.term.lock().ok()?;
        let grid = term.grid();
        let rows = (0..grid.screen_lines()).map(|row| {
            let line = &grid[Line(row as i32)];
            (0..grid.columns())
                .map(|column| line[Column(column)].c)
                .collect::<String>()
        });
        last_text(rows.collect::<Vec<_>>().into_iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_line_is_the_last_one_with_text() {
        let rows = ["$ gh auth login", "error: not a tty  ", "   ", ""].map(String::from);
        assert_eq!(last_text(rows.into_iter()).as_deref(), Some("error: not a tty"));
        assert_eq!(last_text(["", " "].map(String::from).into_iter()), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_watched_command_reports_its_exit_code() {
        use alacritty_terminal::term::test::TermSize;
        use alacritty_terminal::term::{Config, Term};
        let (input, _) = std::sync::mpsc::channel();
        let term = Arc::new(Mutex::new(Term::new(
            Config::default(),
            &TermSize::new(80, 24),
            super::super::Listener::new(input),
        )));
        let exit = Exit::default();
        let run = Some(("printf 'not signed in\\n'; exit 3", exit.clone()));
        let (_shell, _ended) = super::super::spawn(
            &std::env::temp_dir(),
            None,
            run,
            80,
            24,
            term,
            super::super::OutputWake::default(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        for _ in 0..200 {
            if exit.lock().is_ok_and(|exit| exit.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(*exit.lock().unwrap(), Some(Some(3)));
    }
}
