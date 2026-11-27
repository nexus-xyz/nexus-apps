//! Per-App registry setup uses the real close callback, without startup/socket/keychain/runner work.
use super::{Open, Primary, Registry, Shell};
use crate::{backend::Backend, workspace::Workspace};
use futures::channel::mpsc::unbounded;
use gpui_kit::*;
use std::sync::Arc;

pub(crate) fn register(
    gateway: Arc<dyn Backend>,
    window: AnyWindowHandle,
    workspace: Entity<Workspace>,
    panel: bool,
    cx: &mut App,
) {
    if !cx.has_global::<Shell>() {
        cx.set_global(Shell {
            windows: Vec::new(),
            order: Registry::default(),
            primary: Primary::default(),
            quitting: false,
            _identity_events: None,
            gateway,
            events: None,
            runner: None,
            _code_events: None,
            panel: None,
            panel_state: Default::default(),
        });
        super::install_close_handler(cx);
        let machines = crate::machines::Machines::new(
            Arc::new(|_| panic!("fixture must not start a machine connection")),
            nexus_remote::machine::Backoff::default(),
            crate::code_run::runtime(),
        );
        super::install_quit_handler(machines, cx);
    }
    let (arrive, _) = unbounded();
    let open = Open {
        window,
        workspace,
        arrive,
    };
    let shell = cx.global_mut::<Shell>();
    if panel {
        shell.panel = Some(open);
    } else {
        shell.order.opened(window.window_id());
        shell.primary.opened(window.window_id());
        shell.windows.push(open);
    }
}

/// Supply the production app-owned reader with a real channel; never restore credentials/start services.
pub(crate) fn watch_identity(
    gateway: Arc<dyn Backend>,
    cx: &mut App,
) -> futures::channel::mpsc::UnboundedSender<crate::gateway::Event> {
    let (send, events) = unbounded();
    let reader = super::identity_reader(gateway, events, cx);
    cx.global_mut::<Shell>()._identity_events = Some(reader);
    send
}
