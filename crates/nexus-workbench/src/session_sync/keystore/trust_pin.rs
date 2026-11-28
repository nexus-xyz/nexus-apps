//! The device trust this computer is pinned to, per person on a Gateway. Gateway's `trust: "gateway"` makes every
//! device it lists a key holder and turns Remote control on, so a computer that relied on approvals (it verified a
//! device or confirmed an approver) moves to it only when the person opts in here ([`KeyStore::trust_gateway`]).
//! Gateway's answer alone cannot move it. A computer that only read approvals relied on nothing, so it follows Gateway.
//! Going back to approvals needs no opt-in, and a later Gateway trust needs a new one.
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::{KeyStore, check};
use crate::session_sync::DeviceTrust;

pub(super) const SCHEMA: &str = "
create table if not exists sync_trust_pins (
  owner text primary key,
  mode text not null check (mode in ('approvals', 'gateway'))
);
";

fn text(trust: DeviceTrust) -> &'static str {
    match trust {
        DeviceTrust::Approvals => "approvals",
        DeviceTrust::Gateway => "gateway",
    }
}

impl KeyStore {
    /// The trust this computer follows for `owner` when Gateway's device list names `offered`, pinned for the next read.
    /// Approvals is always followed. Gateway is followed when the pin is Gateway, or when this computer remembers no
    /// approval: a new computer, or one whose approvals pin only echoed Gateway. Otherwise it stays on approvals until
    /// the person opts in.
    pub fn settle_trust(&self, owner: &str, offered: DeviceTrust) -> Result<DeviceTrust, &'static str> {
        // A computer from before the pin existed that verified a device or confirmed an approver relied on approvals.
        let memory = self.memory(owner)?;
        let relied_on_approvals = !memory.verified.is_empty() || !memory.anchors.is_empty();
        let mut db = self.lock();
        // Read then write: immediate, so the app and the background service never interleave (see `add_holders`).
        let tx = check(db.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let pin: Option<String> = check(
            tx.query_row("select mode from sync_trust_pins where owner = ?1", [owner], |row| {
                row.get(0)
            })
            .optional(),
        )?;
        let followed = match (offered, pin.as_deref()) {
            (DeviceTrust::Approvals, _) => DeviceTrust::Approvals,
            (DeviceTrust::Gateway, Some("gateway")) => DeviceTrust::Gateway,
            (DeviceTrust::Gateway, _) if !relied_on_approvals => DeviceTrust::Gateway,
            (DeviceTrust::Gateway, _) => DeviceTrust::Approvals,
        };
        check(tx.execute(
            "insert into sync_trust_pins (owner, mode) values (?1, ?2) on conflict (owner) do update set mode = ?2",
            params![owner, text(followed)],
        ))?;
        check(tx.commit())?;
        Ok(followed)
    }

    /// The person chose, on this computer, to trust Gateway's device list for `owner`.
    pub fn trust_gateway(&self, owner: &str) -> Result<(), &'static str> {
        check(self.lock().execute(
            "insert into sync_trust_pins (owner, mode) values (?1, 'gateway') on conflict (owner) do update set mode = 'gateway'",
            params![owner],
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_sync::trust::Memory;

    const OWNER: &str = "https://gw.test|user_ada";

    /// A memory that relied on approvals: this computer verified a device under them.
    fn relied(keys: &KeyStore, owner: &str) {
        let mut memory = Memory::default();
        let device = nexus_client::e2e::DeviceKey::generate();
        memory.verified.insert(
            "dev_phone".into(),
            crate::session_sync::trust::Keys {
                encryption: device.encryption_key(),
                signing: Some(device.signing_key()),
            },
        );
        keys.remember(owner, &memory).expect("remembered");
    }

    #[test]
    fn a_computer_that_only_read_approvals_follows_gateway_trust_with_no_opt_in() {
        // Gateway said approvals, and this computer pinned it, but nothing was ever approved or verified here: nothing
        // relied on approvals, so Gateway's later trust needs no opt-in.
        let keys = KeyStore::in_memory().expect("store");
        assert_eq!(
            keys.settle_trust(OWNER, DeviceTrust::Approvals),
            Ok(DeviceTrust::Approvals)
        );
        assert_eq!(keys.settle_trust(OWNER, DeviceTrust::Gateway), Ok(DeviceTrust::Gateway));
    }

    #[test]
    fn gateway_s_answer_alone_never_moves_a_computer_off_approvals() {
        let keys = KeyStore::in_memory().expect("store");
        relied(&keys, OWNER);
        assert_eq!(
            keys.settle_trust(OWNER, DeviceTrust::Approvals),
            Ok(DeviceTrust::Approvals)
        );
        // Gateway now says it vouches for every device: this computer stays on approvals, read after read.
        assert_eq!(
            keys.settle_trust(OWNER, DeviceTrust::Gateway),
            Ok(DeviceTrust::Approvals)
        );
        assert_eq!(
            keys.settle_trust(OWNER, DeviceTrust::Gateway),
            Ok(DeviceTrust::Approvals)
        );
        // The person opts in here.
        keys.trust_gateway(OWNER).expect("opted in");
        assert_eq!(keys.settle_trust(OWNER, DeviceTrust::Gateway), Ok(DeviceTrust::Gateway));
        // Back to approvals needs nothing, and Gateway trust again needs a new opt-in.
        assert_eq!(
            keys.settle_trust(OWNER, DeviceTrust::Approvals),
            Ok(DeviceTrust::Approvals)
        );
        assert_eq!(
            keys.settle_trust(OWNER, DeviceTrust::Gateway),
            Ok(DeviceTrust::Approvals)
        );
        // Another person on this computer has a pin of their own.
        assert_eq!(
            keys.settle_trust("https://gw.test|user_bob", DeviceTrust::Gateway),
            Ok(DeviceTrust::Gateway)
        );
    }

    #[test]
    fn a_new_computer_follows_gateway_trust_and_one_that_relied_on_approvals_does_not() {
        let keys = KeyStore::in_memory().expect("store");
        assert_eq!(keys.settle_trust(OWNER, DeviceTrust::Gateway), Ok(DeviceTrust::Gateway));
        assert_eq!(keys.settle_trust(OWNER, DeviceTrust::Gateway), Ok(DeviceTrust::Gateway));
        // A computer from before the pin, which verified a device under approvals.
        let older = "https://gw.test|user_cy";
        relied(&keys, older);
        assert_eq!(
            keys.settle_trust(older, DeviceTrust::Gateway),
            Ok(DeviceTrust::Approvals)
        );
    }
}
