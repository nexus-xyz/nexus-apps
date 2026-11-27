//! Native-only permission ownership. Public requests keep their real session IDs.
use super::{Answer, Guard};
use crate::windows::retention::Identity;
use nexus_browser::access::Request;
use std::{collections::HashMap, sync::Mutex};

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Owner {
    Isolated(String),
    Local {
        origin: super::LocalOrigin,
        identity: Identity,
        session: String,
    },
}
struct Pending {
    owner: Owner,
    requester: String,
    request: Request,
    valid: Guard,
    answer: Option<Answer>,
}
#[derive(Default)]
pub(crate) struct Requests(Mutex<HashMap<String, Pending>>);
impl Requests {
    pub(super) fn ask(
        &self,
        owner: &Owner,
        requester: &str,
        session: &str,
        url: &str,
        tab: Option<String>,
        valid: Guard,
    ) -> Result<Request, String> {
        if !valid() {
            return Err(super::INVALID.into());
        }
        self.prune_invalid();
        if !valid() {
            return Err(super::INVALID.into());
        }
        let mut open = self.0.lock().map_err(|_| "Browser requests unavailable.")?;
        if open.values().any(|pending| &pending.owner == owner) {
            return Err("This session already waits for browser access. Finish that request first.".into());
        }
        if open.len() >= 4 {
            return Err("Other sessions wait for browser access. Try again after one is answered.".into());
        }
        let request = Request {
            id: format!("browser-{}", uuid::Uuid::new_v4()),
            thread_id: session.into(),
            url: url.into(),
            tab,
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };
        open.insert(
            request.id.clone(),
            Pending {
                owner: owner.clone(),
                requester: requester.into(),
                request: request.clone(),
                valid,
                answer: None,
            },
        );
        Ok(request)
    }
    pub(crate) fn of(&self, owner: &Owner) -> Option<Request> {
        self.prune_invalid();
        let open = self.0.lock().ok()?;
        open.values()
            .find(|pending| &pending.owner == owner)
            .map(|pending| pending.request.clone())
    }
    /// A matching view can answer, but only the originating Browser applies the grant.
    pub(super) fn answer(&self, owner: &Owner, requester: &str, id: &str, answer: Answer) -> bool {
        self.prune_invalid();
        let Ok(mut open) = self.0.lock() else { return false };
        let Some(pending) = open.get_mut(id).filter(|pending| &pending.owner == owner) else {
            return false;
        };
        if matches!(answer, Answer::Decline) {
            open.remove(id);
            return false;
        }
        if pending.requester == requester {
            pending.answer = None;
            true
        } else {
            pending.answer = Some(answer);
            false
        }
    }
    pub(super) fn take_answer(&self, owner: &Owner, requester: &str, id: &str) -> Option<Answer> {
        self.prune_invalid();
        let mut open = self.0.lock().ok()?;
        open.get_mut(id)
            .filter(|pending| &pending.owner == owner && pending.requester == requester)?
            .answer
            .take()
    }
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.prune_invalid();
        let Ok(open) = self.0.lock() else { return false };
        open.contains_key(id)
    }
    pub(crate) fn remove(&self, id: &str) {
        if let Ok(mut open) = self.0.lock() {
            open.remove(id);
        }
    }
    /// Guard callbacks may lock producer state. Never evaluate them under the registry mutex.
    pub(crate) fn prune_invalid(&self) {
        let guards = match self.0.lock() {
            Ok(open) => open
                .iter()
                .map(|(id, pending)| (id.clone(), pending.valid.clone()))
                .collect::<Vec<_>>(),
            Err(_) => return,
        };
        let invalid = guards
            .into_iter()
            .filter_map(|(id, valid)| (!valid()).then_some(id))
            .collect::<Vec<_>>();
        if let Ok(mut open) = self.0.lock() {
            for id in invalid {
                open.remove(&id);
            } // IDs are unique and their guard is immutable.
        }
    }
    #[cfg(test)]
    pub(crate) fn fixture_contains_unchecked(&self, id: &str) -> bool {
        self.0.lock().expect("fixture registry").contains_key(id)
    }
    #[cfg(test)]
    pub(crate) fn fixture_pending(&self, valid: Guard) -> Request {
        let owner = Owner::Isolated(uuid::Uuid::new_v4().to_string());
        self.ask(
            &owner,
            "producer-fixture",
            "real-session",
            "https://permission.invalid/",
            None,
            valid,
        )
        .expect("producer permission")
    }
    pub(super) fn retire(&self, requester: &str) {
        if let Ok(mut open) = self.0.lock() {
            open.retain(|_, pending| pending.requester != requester);
        }
    }
}
pub(super) struct Lease(pub String);
impl Drop for Lease {
    fn drop(&mut self) {
        super::REQUESTS.remove(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::Requests;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn producer_guards_run_without_the_permission_registry_lock() {
        let requests = Arc::new(Requests::default());
        let live = Arc::new(AtomicBool::new(true));
        let valid = {
            let requests = Arc::downgrade(&requests);
            let live = live.clone();
            Arc::new(move || {
                let requests = requests.upgrade().expect("registry still alive");
                assert!(
                    requests.0.try_lock().is_ok(),
                    "producer callback must not run under registry lock"
                );
                live.load(Ordering::SeqCst)
            })
        };
        let request = requests.fixture_pending(valid);
        assert!(requests.fixture_contains_unchecked(&request.id));
        live.store(false, Ordering::SeqCst);
        requests.prune_invalid();
        assert!(!requests.fixture_contains_unchecked(&request.id));
    }
}
