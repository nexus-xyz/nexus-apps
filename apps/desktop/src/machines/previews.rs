//! A server's dev servers, reached from this computer for its sessions' Browser (`side/browser/server_preview.rs`):
//! one `ssh -N -L` forward per port (`nexus_remote::forward`), on the server's own SSH destination. The forwards
//! close when the server is removed or gets another address, when its connection is lost or closed, when the app
//! signs in to another Gateway, and when the app quits.

use std::sync::Arc;

use nexus_remote::forward::Forwards;
use nexus_remote::ssh::Target;

use super::{Machines, Reach};

impl Machines {
    /// Whether `machine` is a server reached over SSH, not a cloud machine.
    pub fn is_server(&self, machine: &str) -> bool {
        self.server_target(machine).is_some()
    }

    /// Server `machine`'s name, as saved.
    pub fn server_name(&self, machine: &str) -> Option<String> {
        self.with_server(machine, |server| server.setting.name.clone())
    }

    fn server_target(&self, machine: &str) -> Option<Target> {
        self.with_server(machine, |server| match server.reach {
            Reach::Ssh(_) => Some(Target::Ssh {
                destination: server.setting.ssh.trim().to_string(),
            }),
            Reach::Cloud => None,
        })
        .flatten()
    }

    /// The port on this computer that reaches `port` on server `machine`: a live forward's, else a new one. Run it on
    /// the app's Tokio runtime: it starts `ssh` and waits until the forward accepts.
    pub async fn preview(&self, machine: &str, port: u16) -> Result<u16, String> {
        let target = self
            .server_target(machine)
            .ok_or_else(|| "This server is no longer in your servers.".to_string())?;
        let forwards = {
            let mut previews = self
                .previews
                .lock()
                .map_err(|_| "The previews are unavailable.".to_string())?;
            Arc::clone(previews.entry(machine.to_string()).or_default())
        };
        forwards.get(&target, port).await
    }

    /// Closes the forwards to server `machine`, or to every server.
    pub(super) fn close_previews(&self, machine: Option<&str>) {
        let closed: Vec<Arc<Forwards>> = match self.previews.lock() {
            Ok(mut previews) => match machine {
                Some(machine) => previews.remove(machine).into_iter().collect(),
                None => previews.drain().map(|(_, forwards)| forwards).collect(),
            },
            Err(_) => return,
        };
        // Outside the lock: a forward in the middle of starting still holds its own copy.
        for forwards in closed {
            forwards.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machines::tests::{Fake, machines, settings};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_a_server_gets_previews_and_they_close_with_it() {
        let machines = machines(&Fake::new(nexus_workbench::api::PROTOCOL));
        machines.sync(&settings(&[("m1", "me@box")]));
        assert_eq!(machines.server_name("m1").as_deref(), Some("m1"));
        assert!(machines.is_server("m1"));
        assert!(!machines.is_server("m2"));
        assert_eq!(
            machines.server_target("m1"),
            Some(Target::Ssh {
                destination: "me@box".into()
            })
        );
        if let Ok(mut previews) = machines.previews.lock() {
            previews.insert("m1".into(), Arc::default());
        }
        machines.sync(&settings(&[]));
        assert!(machines.previews.lock().is_ok_and(|previews| previews.is_empty()));
        assert!(!machines.is_server("m1"));
        machines.shutdown();
    }
}
