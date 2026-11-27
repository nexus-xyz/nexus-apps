//! The organization's recovery key on this computer.
//! Under the `user_and_org` content policy, a runner locks each thread's content key for the organization's current
//! recovery key too, in the form Gateway's org copies take (`PUT /v1/threads/{threadId}/org-key`). Gateway serves that
//! key, and Gateway is not trusted for it: whoever registers the key reads every thread of the organization. So the
//! person confirms its fingerprint on this computer, against the one the organization publishes from its vault, and
//! the runner locks only for the key confirmed here.
//!
//! - **The pin.** A confirmation records the key's id and its X25519 key in the key store, one per organization. Only a
//!   confirmation writes it, never a sync pass. The fingerprint is computed here from the key Gateway serves
//!   ([`e2e::org_key_fingerprint`]); Gateway's own `fingerprint` field is never read.
//! - **No pin.** The runner syncs for the person's own devices and locks nothing for the organization's key. Gateway's
//!   coverage lists the person as not confirmed, so the organization's admins see it.
//! - **A changed key.** When Gateway serves another id or other bytes than the pin, the runner locks for neither, warns
//!   once, and rotates a content key it locked for the old one. The person confirms the new fingerprint to resume.
//! - **Each computer confirms.** As for the approver, the pin lives on the computer that locks.

use std::collections::BTreeSet;

use nexus_client::e2e;
use serde_json::Value;

use super::keystore::KeyStore;
use super::trust::key32;

/// Gateway mints each organization recovery key's id with this prefix. A content key's holders record its org copy by
/// that id.
pub(crate) const ORG_ID_PREFIX: &str = "ork_";
const INVALID_ANSWER: &str = "session sync: Gateway's organization recovery key is not in the expected form";

/// An organization's recovery key: its id and its X25519 public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrgKey {
    pub id: String,
    pub encryption: [u8; 32],
}

impl OrgKey {
    fn shown(&self) -> Shown {
        Shown {
            id: self.id.clone(),
            fingerprint: e2e::org_key_fingerprint(&self.encryption),
        }
    }
}

/// A key as a shell shows it: its id, and its fingerprint as this computer computes it (64 lower-case hex digits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub id: String,
    pub fingerprint: String,
}

/// Where the organization's recovery key stands on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgKeyState {
    /// The organization has registered no recovery key.
    None,
    /// The person confirmed this key here. Under `user_and_org`, this computer locks for it.
    Confirmed(Shown),
    /// Not confirmed here. This computer locks for it only after the person confirms its fingerprint.
    Unconfirmed(Shown),
    /// Gateway serves another key than the one confirmed here. This computer locks for neither until the person
    /// confirms the served one.
    Changed { confirmed: Shown, served: Shown },
}

/// What every shell that confirms the key says: the GPUI desktop, `nexus devices org-key` and `nexus-host pair`. One
/// text for one meaning, so a person reads the same words on each.
pub mod copy {
    /// The heading.
    pub const TITLE: &str = "Your org's recovery key";
    /// What the key is.
    pub const ABOUT: &str = "When your org chooses \"The person and the org\", your synced sessions are also locked for this key, so your org can read them.";
    /// Where and why to compare.
    pub const COMPARE: &str = "Compare this fingerprint with the one computed from your vault on your admin's own machine. Confirm only if they match.";
    pub const CONFIRMED: &str =
        "Confirmed on this computer. Your synced sessions are also locked for this key when your org chooses so.";
    pub const UNCONFIRMED: &str =
        "Not confirmed on this computer. Nothing is locked for this key until you confirm it.";
    pub const CHANGED: &str =
        "This key changed since you confirmed it. Nothing is locked for the new key until you confirm it.";
    pub const NO_KEY: &str = "Your org has no recovery key. There is nothing to confirm.";
    pub const MISMATCH: &str =
        "That fingerprint does not match the key Nexus serves now. Nothing was recorded. Tell your org's admin.";
}

/// A fingerprint in groups of four, as Nexus UI shows it, so two people can read it to each other.
pub fn grouped(fingerprint: &str) -> String {
    fingerprint
        .as_bytes()
        .chunks(4)
        .map(|group| String::from_utf8_lossy(group).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Why a confirmation was not recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmError {
    /// Gateway's answer is not in the expected form.
    InvalidAnswer,
    /// The organization has no recovery key.
    NoKey,
    /// The fingerprint the person gave is not the one of the key Gateway serves now.
    Mismatch,
    /// The key store failed.
    Store,
}

impl ConfirmError {
    pub fn message(self) -> &'static str {
        match self {
            ConfirmError::InvalidAnswer => "Nexus's answer about your org's recovery key is not in the expected form.",
            ConfirmError::NoKey => copy::NO_KEY,
            ConfirmError::Mismatch => copy::MISMATCH,
            ConfirmError::Store => "The session sync key store failed.",
        }
    }
}

impl std::fmt::Display for ConfirmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ConfirmError {}

/// The key in `GET /v1/orgs/recovery-key`'s answer, or none when the organization has not registered one.
pub(crate) fn served(answer: &Value) -> Result<Option<OrgKey>, &'static str> {
    let key = match answer.get("key") {
        Some(Value::Null) => return Ok(None),
        Some(key) => key,
        None => return Err(INVALID_ANSWER),
    };
    let id = key.get("id").and_then(Value::as_str).ok_or(INVALID_ANSWER)?;
    let fits = id.len() <= 128
        && id.len() > ORG_ID_PREFIX.len()
        && id.starts_with(ORG_ID_PREFIX)
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if !fits {
        return Err(INVALID_ANSWER);
    }
    let encryption = key
        .get("encryptionKey")
        .and_then(Value::as_str)
        .and_then(key32)
        .ok_or(INVALID_ANSWER)?;
    Ok(Some(OrgKey {
        id: id.to_string(),
        encryption,
    }))
}

/// What a runner locks for under the content policy: nothing for the organization (`Off`), not yet (`Waiting`: no
/// key, no pin, or a changed key), or the pinned key Gateway still serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OrgLock {
    Off,
    Waiting,
    Pinned(OrgKey),
}

impl OrgLock {
    /// Under `user_and_org`: the served key when it is the pinned one, same id and same bytes.
    pub(crate) fn of(pinned: Option<&OrgKey>, served: Option<&OrgKey>) -> OrgLock {
        match (pinned, served) {
            (Some(pinned), Some(served)) if pinned == served => OrgLock::Pinned(served.clone()),
            _ => OrgLock::Waiting,
        }
    }

    /// Whether a content key locked for `holders` stays in use. A key locked for an organization key other than the
    /// pinned one rotates: after a replacement, a swap by Gateway, or a policy change back to `user`.
    pub(crate) fn keeps(&self, holders: &BTreeSet<String>) -> bool {
        let mut org_ids = holders.iter().filter(|holder| holder.starts_with(ORG_ID_PREFIX));
        match self {
            OrgLock::Off | OrgLock::Waiting => org_ids.next().is_none(),
            OrgLock::Pinned(key) => org_ids.all(|id| *id == key.id),
        }
    }
}

fn normalized(fingerprint: &str) -> String {
    fingerprint
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':' && *c != '-')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Where the organization's key stands for `owner` ([`super::approver::owner`]) in organization `tenant`, from
/// `GET /v1/orgs/recovery-key`'s answer. It reads the key store and writes nothing.
pub fn state(keys: &KeyStore, owner: &str, tenant: &str, answer: &Value) -> Result<OrgKeyState, ConfirmError> {
    let Some(served) = served(answer).map_err(|_| ConfirmError::InvalidAnswer)? else {
        return Ok(OrgKeyState::None);
    };
    let pinned = keys.pinned_org_key(owner, tenant).map_err(|_| ConfirmError::Store)?;
    Ok(match pinned {
        Some(pinned) if pinned == served => OrgKeyState::Confirmed(served.shown()),
        Some(pinned) => OrgKeyState::Changed {
            confirmed: pinned.shown(),
            served: served.shown(),
        },
        None => OrgKeyState::Unconfirmed(served.shown()),
    })
}

/// Records that the person confirmed the key Gateway serves now. `fingerprint` is the one they compared with the
/// organization's vault, in any case, with spaces, colons or hyphens. It is recorded only when it is this computer's
/// fingerprint of the served key; a later confirmation replaces it.
pub fn confirm(
    keys: &KeyStore,
    owner: &str,
    tenant: &str,
    answer: &Value,
    fingerprint: &str,
) -> Result<Shown, ConfirmError> {
    let served = served(answer)
        .map_err(|_| ConfirmError::InvalidAnswer)?
        .ok_or(ConfirmError::NoKey)?;
    let shown = served.shown();
    if normalized(fingerprint) != shown.fingerprint {
        return Err(ConfirmError::Mismatch);
    }
    keys.pin_org_key(owner, tenant, &served)
        .map_err(|_| ConfirmError::Store)?;
    Ok(shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn answer(id: &str, byte: u8) -> Value {
        json!({ "key": {
            "id": id,
            "encryptionKey": e2e::b64u(&[byte; 32]),
            // Gateway's own fingerprint is never read: a wrong one changes nothing.
            "fingerprint": "00",
            "registeredBy": "user_admin",
            "createdAt": "2026-11-18T10:00:00Z",
        }})
    }

    #[test]
    fn a_legacy_gateway_screen_confirmation_needs_confirmation_from_the_vault() {
        let path = std::env::temp_dir().join(format!("nexus-org-pin-migration-{}.sqlite3", uuid::Uuid::new_v4()));
        let keys = KeyStore::open(&path).expect("store");
        let db = rusqlite::Connection::open(&path).expect("legacy database");
        db.execute_batch("create table sync_org_keys (owner text, tenant_id text, key_id text, encryption blob, confirmed_at integer)").expect("legacy table");
        db.execute(
            "insert into sync_org_keys values (?1, ?2, ?3, ?4, 0)",
            rusqlite::params!["owner", "tenant", "ork_legacy", &[1u8; 32][..]],
        )
        .expect("legacy pin");
        assert!(matches!(
            state(&keys, "owner", "tenant", &answer("ork_legacy", 1)).expect("state"),
            OrgKeyState::Unconfirmed(_)
        ));
        drop(db);
        drop(keys);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_key_is_confirmed_only_by_the_fingerprint_this_computer_computes() {
        let keys = KeyStore::in_memory().expect("store");
        let served = answer("ork_one", 1);
        let print = e2e::org_key_fingerprint(&[1; 32]);
        let shown = Shown {
            id: "ork_one".into(),
            fingerprint: print.clone(),
        };
        assert_eq!(
            state(&keys, "o", "ten_a", &served),
            Ok(OrgKeyState::Unconfirmed(shown.clone()))
        );
        // Gateway's fingerprint field, or another key's, is refused, and nothing is recorded.
        for wrong in ["00", e2e::org_key_fingerprint(&[2; 32]).as_str()] {
            assert_eq!(
                confirm(&keys, "o", "ten_a", &served, wrong),
                Err(ConfirmError::Mismatch)
            );
        }
        assert_eq!(keys.pinned_org_key("o", "ten_a"), Ok(None));
        // As the person reads it from the vault: upper case, in groups.
        let grouped = print
            .to_uppercase()
            .as_bytes()
            .chunks(4)
            .map(|chunk| std::str::from_utf8(chunk).expect("ascii"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(confirm(&keys, "o", "ten_a", &served, &grouped), Ok(shown.clone()));
        assert_eq!(state(&keys, "o", "ten_a", &served), Ok(OrgKeyState::Confirmed(shown)));
        // Another organization, or another person, has confirmed nothing.
        assert!(matches!(
            state(&keys, "o", "ten_b", &served),
            Ok(OrgKeyState::Unconfirmed(_))
        ));
        assert!(matches!(
            state(&keys, "p", "ten_a", &served),
            Ok(OrgKeyState::Unconfirmed(_))
        ));
        assert_eq!(
            state(&keys, "o", "ten_a", &json!({ "key": null })),
            Ok(OrgKeyState::None)
        );
        assert_eq!(
            confirm(&keys, "o", "ten_a", &json!({ "key": null }), &print),
            Err(ConfirmError::NoKey)
        );
    }

    #[test]
    fn a_key_gateway_swapped_is_a_changed_key_until_the_person_confirms_it() {
        let keys = KeyStore::in_memory().expect("store");
        confirm(
            &keys,
            "o",
            "ten_a",
            &answer("ork_one", 1),
            &e2e::org_key_fingerprint(&[1; 32]),
        )
        .expect("confirmed");
        // The same id with other bytes, and another id with the same bytes, are both changes.
        let pinned = Shown {
            id: "ork_one".into(),
            fingerprint: e2e::org_key_fingerprint(&[1; 32]),
        };
        for (swapped, served) in [
            (answer("ork_one", 9), ("ork_one", 9)),
            (answer("ork_two", 1), ("ork_two", 1)),
        ] {
            assert_eq!(
                state(&keys, "o", "ten_a", &swapped),
                Ok(OrgKeyState::Changed {
                    confirmed: pinned.clone(),
                    served: Shown {
                        id: served.0.into(),
                        fingerprint: e2e::org_key_fingerprint(&[served.1; 32]),
                    },
                })
            );
        }
        // The organization replaced its key, and the person confirms the new one: it replaces the pin.
        let replaced = answer("ork_two", 2);
        confirm(&keys, "o", "ten_a", &replaced, &e2e::org_key_fingerprint(&[2; 32])).expect("confirmed again");
        assert!(matches!(
            state(&keys, "o", "ten_a", &replaced),
            Ok(OrgKeyState::Confirmed(Shown { id, .. })) if id == "ork_two"
        ));
    }

    #[test]
    fn the_runner_locks_only_for_the_pinned_key_and_rotates_away_from_any_other() {
        let one = OrgKey {
            id: "ork_one".into(),
            encryption: [1; 32],
        };
        let mut swapped = one.clone();
        swapped.encryption = [9; 32];
        let two = OrgKey {
            id: "ork_two".into(),
            encryption: [2; 32],
        };
        assert_eq!(OrgLock::of(Some(&one), Some(&one)), OrgLock::Pinned(one.clone()));
        // A missing pin, a swapped key, another id, or no key at all: nothing is locked for the organization.
        assert_eq!(OrgLock::of(None, Some(&one)), OrgLock::Waiting);
        assert_eq!(OrgLock::of(Some(&one), Some(&swapped)), OrgLock::Waiting);
        assert_eq!(OrgLock::of(Some(&one), Some(&two)), OrgLock::Waiting);
        assert_eq!(OrgLock::of(Some(&one), None), OrgLock::Waiting);
        let holders = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<BTreeSet<_>>();
        let own = holders(&["dev_a", "dev_b"]);
        let locked = holders(&["dev_a", "ork_one"]);
        let pinned = OrgLock::Pinned(one);
        assert!(pinned.keeps(&own), "the org copy is added to a key in use");
        assert!(pinned.keeps(&locked));
        assert!(!pinned.keeps(&holders(&["dev_a", "ork_old", "ork_one"])));
        // Back to `user`, or waiting for a confirmation: a key the org key holds rotates, and a key it does not stays.
        for lock in [OrgLock::Off, OrgLock::Waiting] {
            assert!(lock.keeps(&own));
            assert!(!lock.keeps(&locked));
        }
    }

    #[test]
    fn a_fingerprint_reads_in_sixteen_groups_of_four() {
        let print = e2e::org_key_fingerprint(&[0; 32]);
        let shown = grouped(&print);
        assert_eq!(shown.split(' ').count(), 16);
        assert!(shown.starts_with("e10e 3354 "));
        assert_eq!(normalized(&shown), print);
    }

    #[test]
    fn gateways_answer_must_name_an_org_key_id_and_a_32_byte_key() {
        assert_eq!(served(&json!({ "key": null })), Ok(None));
        assert!(served(&json!({})).is_err());
        assert!(served(&answer("ork_one", 1)).expect("served").is_some());
        for id in ["dev_one", "ork_", "ork:one", "ork_one/../x"] {
            assert!(served(&answer(id, 1)).is_err(), "{id}");
        }
        let mut short = answer("ork_one", 1);
        short["key"]["encryptionKey"] = json!(e2e::b64u(&[1; 31]));
        assert!(served(&short).is_err());
    }
}
