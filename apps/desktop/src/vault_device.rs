//! The Assistant's vault on this device, as the Passwords view uses it: the Nexus vault in the macOS
//! Keychain, the person's own 1Password through `op`, Touch ID, and the index sent to the agent API.
//!
//! Every method here blocks (the keychain, `op`, the Touch ID sheet), so the view calls them on the background
//! executor. None of them returns a secret except [`Device::reveal`], and that needs Touch ID first. None logs.
//! The rules and their tests are in `nexus_desktop_core::{vault_local, vault_import, vault_op}`.
//!
//! macOS only: elsewhere [`Device::native`] is false, nothing touches a keychain, and the view says the vault is not
//! connected here.

use std::sync::Arc;

use nexus_desktop_core::agent_vault::{Grant, ManagerKind, Outcome};
use nexus_desktop_core::vault_import;
use nexus_desktop_core::vault_local::{
    ApprovedUse, Auth, Authenticator, Authorized, Built, Edit, Filled, ImportReport, LocalVault, LoginMeta, NewLogin,
    Secret, build_index, fill_login, line,
};
use nexus_desktop_core::vault_op::{self, OpState, Run};

/// What the view shows of the device's vault: metadata only.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub nexus: Vec<LoginMeta>,
    pub op_state: OpState,
    pub op: Vec<LoginMeta>,
    pub op_vault: Option<String>,
}

pub struct Device {
    pub vault: LocalVault,
    pub auth: Arc<dyn Authenticator>,
    pub op: Option<Arc<dyn Run>>,
    /// This computer's name, for the index.
    pub name: String,
    /// Whether this system has the Keychain, Touch ID and `op` at all.
    pub native: bool,
}

#[cfg(target_os = "macos")]
fn computer_name() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this Mac".into())
}

impl Device {
    /// This system's vault: the Keychain, Touch ID and the `op` found here. In tests, a fresh in-memory one.
    pub fn system() -> Arc<Device> {
        #[cfg(test)]
        {
            if let Some(device) = tests::NEXT.with(|next| next.borrow_mut().take()) {
                return device;
            }
            return Arc::new(tests::device(Arc::new(tests::Approve), None));
        }
        #[cfg(all(target_os = "macos", not(test)))]
        {
            Arc::new(Device {
                vault: LocalVault::os(),
                auth: Arc::new(crate::touch_id::TouchId),
                op: nexus_desktop_core::vault_op::OsOp::find().map(|op| Arc::new(op) as Arc<dyn Run>),
                name: computer_name(),
                native: true,
            })
        }
        #[cfg(all(not(target_os = "macos"), not(test)))]
        {
            Arc::new(Device {
                vault: LocalVault::new(Arc::new(NoStore)),
                auth: Arc::new(crate::touch_id::TouchId),
                op: None,
                name: "this computer".into(),
                native: false,
            })
        }
    }

    fn op_run(&self) -> Option<&dyn Run> {
        self.op.as_deref()
    }

    /// Read the device's vault. 1Password is detected (cheap, no prompt) and its items come from the last Refresh.
    pub fn load(&self) -> Result<Loaded, String> {
        if !self.native {
            return Ok(Loaded {
                nexus: Vec::new(),
                op_state: OpState::NotInstalled,
                op: Vec::new(),
                op_vault: None,
            });
        }
        let nexus = self.vault.list().map_err(|error| error.to_string())?;
        let op_state = vault_op::detect(self.op_run());
        let (op, op_vault) = if matches!(op_state, OpState::Ready { .. }) {
            self.vault.op_cache().map_err(|error| error.to_string())?
        } else {
            (Vec::new(), None)
        };
        Ok(Loaded {
            nexus,
            op_state,
            op,
            op_vault,
        })
    }

    /// Refresh 1Password's items: `op` lists them (1Password asks for Touch ID in its own app), and their metadata is
    /// kept. Never a password.
    pub fn refresh_op(&self) -> Result<Loaded, String> {
        let state = vault_op::detect(self.op_run());
        if let (OpState::Ready { .. }, Some(run)) = (&state, self.op_run()) {
            let listed = vault_op::list_items(run)?;
            self.vault
                .set_op_cache(&listed.logins, listed.vault.as_deref())
                .map_err(|error| error.to_string())?;
            let note = format!("Refreshed {} logins from 1Password", listed.logins.len());
            self.vault
                .log(line(
                    note,
                    Some("1Password"),
                    None,
                    "Names and sites only",
                    Outcome::Done,
                ))
                .ok();
        }
        self.load()
    }

    /// The index this device sends now, without a secret. `Err` says why nothing may be sent.
    pub fn index(&self, op_ready: bool) -> Result<Built, String> {
        if !self.native {
            return Err("The vault is on the Mac only.".into());
        }
        let snapshot = self
            .vault
            .snapshot(&self.name, op_ready)
            .map_err(|error| error.to_string())?;
        let secrets = self.vault.secrets().map_err(|error| error.to_string())?;
        build_index(&snapshot, &secrets).map_err(|refusal| format!("The index was not sent: {refusal:?}."))
    }

    pub fn add(&self, login: NewLogin) -> Result<(), String> {
        let added = self.vault.add(login).map_err(|error| error.to_string())?;
        self.vault
            .log(line(
                format!("Added the {} login", added.name),
                Some("Nexus vault"),
                None,
                "Saved on this device",
                Outcome::Done,
            ))
            .ok();
        Ok(())
    }

    pub fn edit(&self, id: &str, edit: Edit) -> Result<(), String> {
        let edited = self.vault.edit(id, edit).map_err(|error| error.to_string())?;
        self.vault
            .log(line(
                format!("Changed the {} login", edited.name),
                Some("Nexus vault"),
                None,
                "Saved on this device",
                Outcome::Done,
            ))
            .ok();
        Ok(())
    }

    pub fn delete(&self, id: &str, name: &str) -> Result<(), String> {
        self.vault.delete(id).map_err(|error| error.to_string())?;
        self.vault
            .log(line(
                format!("Deleted the {name} login"),
                Some("Nexus vault"),
                None,
                "Gone from this device",
                Outcome::Done,
            ))
            .ok();
        Ok(())
    }

    /// Delete every Nexus login, and the keychain item with them.
    pub fn delete_all(&self) -> Result<(), String> {
        self.vault.delete_all().map_err(|error| error.to_string())
    }

    /// Read a CSV the person picked and add its logins. The file's text is dropped when this returns.
    pub fn import(&self, bytes: Vec<u8>) -> Result<(vault_import::Format, ImportReport, usize), String> {
        let parsed = vault_import::parse_bytes(&bytes).map_err(|error| error.to_string())?;
        drop(bytes);
        let (format, skipped) = (parsed.format, parsed.skipped);
        let report = self.vault.import(parsed.logins).map_err(|error| error.to_string())?;
        self.vault
            .log(line(
                format!("Imported {} logins from {}", report.added, format.name()),
                Some("Nexus vault"),
                None,
                "Saved on this device",
                Outcome::Done,
            ))
            .ok();
        Ok((format, report, skipped))
    }

    /// Set what Nexus may do with a login of either source.
    pub fn grant(&self, id: &str, grant: Option<&Grant>) -> Result<(), String> {
        self.vault.set_grant(id, grant).map_err(|error| error.to_string())
    }

    /// Ask for Touch ID. Declined and unavailable come back as the sentence the view shows.
    pub fn confirm(&self, reason: &str) -> Result<Authorized, String> {
        match self.auth.ask(reason) {
            Auth::Approved => Authorized::from(&Auth::Approved).ok_or_else(|| "Not approved.".to_string()),
            Auth::Declined => Err("Not approved.".into()),
            Auth::Unavailable(why) => Err(why),
        }
    }

    /// Show a Nexus login's password: Touch ID first, then the keychain.
    pub fn reveal(&self, id: &str, name: &str) -> Result<Secret, String> {
        let authorized = self.confirm(&format!("show the password of your {name} login"))?;
        let secret = self.vault.secret(&authorized, id).map_err(|error| error.to_string())?;
        self.vault
            .log(line(
                format!("Showed the {name} password"),
                Some("Nexus vault"),
                Some("Touch ID"),
                "Shown on this screen",
                Outcome::Done,
            ))
            .ok();
        Ok(secret)
    }

    /// An approved use ends at "ready on this device": the grant is kept, the use is noted, and [`fill_login`] is the
    /// seam a browser filler takes later. Nothing is typed anywhere and no password is read.
    pub fn record_approved(&self, used: &ApprovedUse, name: &str, until: Option<&str>) -> Result<Filled, String> {
        if let Some(until) = until {
            self.vault
                .set_grant(&used.login, Some(&Grant::Until(until.to_string())))
                .map_err(|error| error.to_string())?;
        }
        self.vault.used(&used.login).map_err(|error| error.to_string())?;
        let source = match used.source {
            ManagerKind::OnePassword => "1Password",
            _ => "Nexus vault",
        };
        self.vault
            .log(line(
                format!("Approved the {name} login on {}", used.site),
                Some(source),
                Some("Touch ID"),
                "Ready on this device",
                Outcome::Approved,
            ))
            .map_err(|error| error.to_string())?;
        Ok(fill_login(used, &|| None))
    }
}

#[cfg(all(not(target_os = "macos"), not(test)))]
struct NoStore;

#[cfg(all(not(target_os = "macos"), not(test)))]
impl nexus_desktop_core::own_keychain::Store for NoStore {
    fn read(&self, _: &str, _: &str) -> Result<Option<String>, keyring::Error> {
        Ok(None)
    }
    fn write(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
        Err("The vault is on the Mac only.".into())
    }
    fn delete(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// The words for Touch ID's system sheet when Nexus asks to use a login: the person reads them there.
pub fn use_reason(name: &str, site: &str) -> String {
    format!("let Nexus use your {name} login on {site}")
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use nexus_desktop_core::own_keychain::Store;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Mutex;

    thread_local! {
        /// The device the next page takes, so a test supplies its own.
        pub static NEXT: RefCell<Option<Arc<Device>>> = const { RefCell::new(None) };
    }

    #[derive(Default)]
    pub struct Mem(pub Mutex<HashMap<String, String>>);

    impl Store for Mem {
        fn read(&self, service: &str, account: &str) -> Result<Option<String>, keyring::Error> {
            Ok(self.0.lock().unwrap().get(&format!("{service}/{account}")).cloned())
        }
        fn write(&self, service: &str, account: &str, secret: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(format!("{service}/{account}"), secret.into());
            Ok(())
        }
        fn delete(&self, service: &str, account: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(&format!("{service}/{account}"));
            Ok(())
        }
    }

    #[test]
    fn a_missing_entry_reads_as_none() {
        assert!(Mem::default().read("vault", "missing").unwrap().is_none());
    }

    pub struct Approve;
    impl Authenticator for Approve {
        fn ask(&self, _: &str) -> Auth {
            Auth::Approved
        }
    }

    /// Declines, and counts the asks.
    #[derive(Default)]
    pub struct Decline(pub Mutex<usize>);
    impl Authenticator for Decline {
        fn ask(&self, _: &str) -> Auth {
            *self.0.lock().unwrap() += 1;
            Auth::Declined
        }
    }

    pub fn device(auth: Arc<dyn Authenticator>, op: Option<Arc<dyn Run>>) -> Device {
        Device {
            vault: LocalVault::new(Arc::new(Mem::default())),
            auth,
            op,
            name: "Maya’s MacBook Pro".into(),
            native: true,
        }
    }

    fn login(name: &str, site: &str, password: &str) -> NewLogin {
        NewLogin {
            name: name.into(),
            site: site.into(),
            username: "maya".into(),
            password: Secret::new(password),
        }
    }

    #[test]
    fn a_declined_touch_id_shows_no_password_and_changes_nothing() {
        let decline = Arc::new(Decline::default());
        let device = device(decline.clone(), None);
        device.add(login("Delta", "delta.com", "Pw-Never-Shown-1")).unwrap();
        let id = device.vault.list().unwrap()[0].id.clone();
        assert_eq!(device.reveal(&id, "Delta").unwrap_err(), "Not approved.");
        assert_eq!(*decline.0.lock().unwrap(), 1);
        assert!(
            !device
                .vault
                .activity()
                .unwrap()
                .iter()
                .any(|line| line.what.contains("Showed"))
        );
    }

    #[test]
    fn an_approved_touch_id_shows_the_password_once_and_notes_it() {
        let device = device(Arc::new(Approve), None);
        device.add(login("Delta", "delta.com", "Pw-Shown-Once-1")).unwrap();
        let id = device.vault.list().unwrap()[0].id.clone();
        assert_eq!(device.reveal(&id, "Delta").unwrap().expose(), "Pw-Shown-Once-1");
        let activity = device.vault.activity().unwrap();
        assert!(activity[0].what.contains("Showed the Delta password"));
        assert!(
            !activity
                .iter()
                .any(|line| line.what.contains("Pw-Shown") || line.result.contains("Pw-Shown"))
        );
    }

    #[test]
    fn an_approved_use_ends_ready_on_this_device_with_no_password_read() {
        let device = device(Arc::new(Approve), None);
        device.add(login("Delta", "delta.com", "Pw-Not-Read-1")).unwrap();
        let id = device.vault.list().unwrap()[0].id.clone();
        let used = ApprovedUse {
            login: id.clone(),
            source: ManagerKind::Nexus,
            site: "delta.com".into(),
            task: "Check in".into(),
        };
        let until = "2026-12-01T00:00:00Z";
        assert_eq!(
            device.record_approved(&used, "Delta", Some(until)).unwrap(),
            Filled::ReadyOnDevice
        );
        let (grant, used_at) = device.vault.state_of(&id).unwrap();
        assert_eq!(grant, Some(Grant::Until(until.into())));
        assert!(used_at.is_some());
        let activity = device.vault.activity().unwrap();
        assert_eq!(activity[0].result, "Ready on this device");
        assert!(!activity.iter().any(|line| line.what.contains("Pw-Not-Read")));
    }

    #[test]
    fn the_index_is_built_from_metadata_and_holds_no_password() {
        let device = device(Arc::new(Approve), None);
        device.add(login("Delta", "delta.com", "Pw-Index-Secret-1")).unwrap();
        device.add(login("Bank", "bank.example", "Pw-Index-Secret-2")).unwrap();
        let built = device.index(false).unwrap();
        assert!(!built.body.contains("Pw-Index-Secret"));
        assert!(nexus_desktop_core::agent_vault::Request::index(built.body).is_some());
    }

    #[test]
    fn a_system_without_the_mac_pieces_is_not_connected() {
        let mut device = device(Arc::new(Approve), None);
        device.native = false;
        let loaded = device.load().unwrap();
        assert!(loaded.nexus.is_empty() && loaded.op.is_empty());
        assert!(device.index(true).is_err());
    }
}
