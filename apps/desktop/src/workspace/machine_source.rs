//! An explicit machine registry keeps fixtures separate from live app connections.
use crate::machines::Machines;
use std::sync::Arc;

pub(super) struct MachineSource {
    pub machines: Arc<Machines>,
    pub restore_sign_in: bool,
}
impl MachineSource {
    pub fn configured() -> Self {
        Self {
            machines: crate::machines::get(),
            restore_sign_in: true,
        }
    }
    #[cfg(test)]
    pub fn isolated() -> Self {
        Self {
            machines: Machines::new(
                Arc::new(|_| panic!("isolated workspace fixtures must not connect to remote machines")),
                nexus_remote::machine::Backoff::default(),
                crate::code_run::runtime(),
            ),
            restore_sign_in: false,
        }
    }
}
