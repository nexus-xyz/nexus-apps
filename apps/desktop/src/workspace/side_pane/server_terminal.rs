//! A server session's Terminal tab: its shells run on its server (`side/terminal/server_shell.rs`) when the server's
//! host opens them (`nexus_workbench::shell_pty::FEATURE`). An older host keeps the note that there is none
//! (`side::server::NO_TERMINAL`).

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{AppContext as _, Context, Entity, Window};

use super::{Scope, SidePane};
use crate::side::terminal::{MachineShells, ServerTarget, ShellHost};
use crate::side::terminals::TerminalSet;
use crate::workspace::Workspace;

/// Where server `machine`'s shells go: None while its host opens none (an older host, or not connected).
pub(super) type ServerShells = fn(&str) -> Option<Arc<dyn ShellHost>>;
pub(super) type ServerSetFactory = fn(PathBuf, ServerTarget, &mut Context<TerminalSet>) -> TerminalSet;

/// The app's servers: a connected host that names the feature in its hello.
pub(super) fn machine_shells(machine: &str) -> Option<Arc<dyn ShellHost>> {
    let machines = crate::machines::get();
    if !machines.has_feature(machine, nexus_workbench::shell_pty::FEATURE) {
        return None;
    }
    Some(Arc::new(MachineShells {
        machines,
        machine: machine.to_string(),
    }))
}

/// The name a server session's (or project's) terminals are kept under: not a folder here.
fn server_key(machine: &str, id: &str) -> PathBuf {
    PathBuf::from(format!("nexus-server://{machine}/{id}"))
}

impl SidePane {
    /// The name the scope's terminals are kept under: the folder here, or the server and the session there.
    pub(super) fn terminal_key(&self) -> PathBuf {
        match (&self.server, &self.scope) {
            (Some(machine), Some(Scope::Session(id) | Scope::Project(id))) => server_key(machine, id),
            _ => self.dir.clone(),
        }
    }
}

impl Workspace {
    /// The server session's terminals, made with one shell there when there are none yet; None when its host opens
    /// no shells.
    pub(super) fn ensure_server_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalSet>> {
        if let Some(set) = self.side.terminal() {
            return Some(set.clone());
        }
        let machine = self.side.server.clone()?;
        let (thread, project) = match &self.side.scope {
            Some(Scope::Session(id)) => (Some(id.clone()), None),
            Some(Scope::Project(id)) => (None, Some(id.clone())),
            _ => return None,
        };
        let host = (self.side.server_shells)(&machine)?;
        let target = ServerTarget {
            host,
            thread,
            project,
            command: None,
        };
        let key = self.side.terminal_key();
        let factory = self.side.server_set_factory;
        let set = cx.new(|cx| factory(key.clone(), target, cx));
        let subscription = cx.subscribe_in(&set, window, Self::on_terminal_set_event);
        self.side.terminals.insert(key, (set.clone(), subscription));
        Some(set)
    }
}

#[cfg(test)]
#[path = "server_terminal_tests.rs"]
mod tests;
