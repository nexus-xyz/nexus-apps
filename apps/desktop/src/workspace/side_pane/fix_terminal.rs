//! A setup fix's command in the Workbench (`crate::setup`, `Fix::RunInTerminal`): it opens the pane on its terminal and
//! runs the command where the person sees it and answers its prompts, then reports its end. A started session's
//! Commands run it as the person's own command; any other folder (or a server session) gets a new terminal tab.

use gpui_kit::*;

use super::{Tool, Workspace};
use crate::setup::TerminalStep;
use crate::side::terminal::Terminal;

/// Where the step runs, and how its end comes back.
pub(in crate::workspace) enum FixTerminal {
    /// A Workbench shell: its `TerminalEvent::Exited`.
    Shell(Entity<Terminal>),
    /// The session's Commands: `SharedTerminalsEvent::FixEnded` with the step's token.
    Shared,
    /// The session's Commands in another window: the end is not seen here.
    Elsewhere,
    /// Nothing here can run it (Commands busy, or read-only).
    Refused,
}

impl Workspace {
    /// Open the Workbench on its terminal and run `step` there; `token` names it in the session's Commands.
    pub(in crate::workspace) fn start_fix_terminal(
        &mut self,
        step: &TerminalStep,
        token: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FixTerminal {
        self.reveal_manual_workbench();
        self.sync_side(window, cx);
        if let Some(commands) = self.ensure_shared_terminal(window, cx) {
            self.side.open = true;
            self.side.tool = Tool::Terminal;
            let (command, name) = (step.command.clone(), step.name.clone());
            let sent = commands.update(cx, |commands, cx| commands.run_watched(command, name, token, cx));
            cx.notify();
            return if sent {
                FixTerminal::Shared
            } else {
                FixTerminal::Refused
            };
        }
        let (command, name) = (step.command.clone(), step.name.clone());
        if self.route_commands_action(window, cx, move |commands, cx| commands.run(command, name, cx)) {
            return FixTerminal::Elsewhere;
        }
        let Some(set) = self.ensure_shell_set(window, cx) else {
            return FixTerminal::Refused;
        };
        self.side.open = true;
        self.side.tool = Tool::Terminal;
        let shell = set.update(cx, |set, cx| {
            let shell = set.run_watched(&step.command, step.name.as_deref(), step.cwd.clone(), cx);
            set.focus(window, cx);
            shell
        });
        cx.notify();
        FixTerminal::Shell(shell)
    }
}
