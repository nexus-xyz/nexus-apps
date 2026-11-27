//! Ephemeral watcher presence; never persisted. A leave applies to one window, not all of a person's windows.
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Peer {
    user_id: String,
    name: Option<String>,
}
#[derive(Deserialize)]
struct Presence {
    from: Peer,
    client: String,
    leave: bool,
}
#[derive(Default)]
pub(super) struct Watchers {
    peers: BTreeMap<String, (String, String, Instant)>,
}
impl Watchers {
    pub fn receive(&mut self, value: Value, own: Uuid) {
        let Ok(event) = serde_json::from_value::<Presence>(value) else {
            return;
        };
        if event.client == own.to_string() {
            return;
        }
        if event.leave {
            self.peers.remove(&event.client);
        } else {
            self.peers.insert(
                event.client,
                (
                    event.from.user_id,
                    event.from.name.unwrap_or_else(|| "A teammate".into()),
                    Instant::now(),
                ),
            );
        }
    }
    pub fn prune(&mut self) {
        self.peers.retain(|_, (_, _, at)| at.elapsed() < Duration::from_secs(9));
    }
    pub fn names(&self, self_id: &str) -> Vec<String> {
        let mut people = BTreeMap::new();
        for (id, name, at) in self.peers.values() {
            if id != self_id && at.elapsed() < Duration::from_secs(9) {
                people.insert(id.clone(), name.clone());
            }
        }
        people.into_values().collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn same_person_windows_are_deduplicated_and_expire() {
        let mut watchers = Watchers::default();
        let own = Uuid::new_v4();
        for client in ["a", "b"] {
            watchers.receive(
                json!({"from":{"userId":"member","name":"Fixture member"},"client":client,"leave":false}),
                own,
            );
        }
        assert_eq!(watchers.names("reader"), vec!["Fixture member"]);
        watchers.receive(
            json!({"from":{"userId":"member","name":"Fixture member"},"client":"a","leave":true}),
            own,
        );
        assert_eq!(watchers.names("reader").len(), 1);
        watchers.peers.get_mut("b").unwrap().2 = Instant::now() - Duration::from_secs(10);
        watchers.prune();
        assert!(watchers.names("reader").is_empty());
    }
    #[test]
    fn own_window_and_own_person_do_not_show_as_other_watchers() {
        let mut watchers = Watchers::default();
        let own = Uuid::new_v4();
        watchers.receive(
            json!({"from":{"userId":"reader","name":"Reader"},"client":own.to_string(),"leave":false}),
            own,
        );
        watchers.receive(
            json!({"from":{"userId":"reader","name":"Reader"},"client":"other-window","leave":false}),
            own,
        );
        assert!(watchers.names("reader").is_empty());
    }
}
