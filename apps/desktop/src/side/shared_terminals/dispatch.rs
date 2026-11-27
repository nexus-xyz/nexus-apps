//! Where a Commands pane's actions go: a session on this computer through its session runner, a server's session
//! through that server's host (`host_terminal`, `Machines::terminal`).
use super::Guard;
use crate::code_run::{Answer, CodeRunner};
use crate::machines::Machines;
use serde_json::Value;
use std::rc::Rc;
use std::sync::Arc;

/// One terminal action: the session, the operation, its arguments, and the proof that the pane still owns them.
pub(crate) type Dispatch = Rc<dyn Fn(String, String, Value, Guard) -> Answer<Value>>;

/// This computer's sessions: the session runner checks the proof when the action runs.
pub(crate) fn runner(runner: Rc<CodeRunner>) -> Dispatch {
    Rc::new(move |thread, op, args, valid| runner.terminal_guarded(thread, op, args, valid))
}

/// A session on server `machine`: the proof is checked before the action leaves this computer.
pub(crate) fn server(machines: Arc<Machines>, machine: String) -> Dispatch {
    Rc::new(move |thread, op, args, valid| {
        if !valid() {
            let (sender, receiver) = futures::channel::oneshot::channel();
            let _ = sender.send(Err("The Commands owner is no longer current.".into()));
            return receiver;
        }
        machines.terminal(&machine, &thread, &op, args)
    })
}
