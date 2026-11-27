//! Actual per-instance Backend event pipes; no live requests or credentials.
use crate::backend::{Backend, Request, TextRequest};
use crate::dictation::Dictation;
use crate::gateway::{Event, Verb};
use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
#[derive(Clone)]
pub(in crate::workspace) struct Ask {
    pub side: u64,
    pub turn: u64,
    pub session: Option<String>,
    pub prompt: String,
    pub model: Option<String>,
    pub choices: crate::gateway::ChatChoices,
    pub attachments: crate::attachments::Draft,
}
#[derive(Default)]
struct Data {
    asks: Vec<Ask>,
    reads: Vec<(String, u64)>,
    revoke_on_ask: bool,
    revoke_on_factory: bool,
    marker_checks_until_revoke: usize,
    stops: Vec<(u64, u64)>,
    events: HashMap<u64, UnboundedSender<Event>>,
}
#[derive(Clone)]
pub(in crate::workspace) struct Probe {
    pub url: String,
    pub marker: Arc<Mutex<Option<String>>>,
    pub epoch: Arc<AtomicU64>,
    side: u64,
    pinned: Option<u64>,
    data: Arc<Mutex<Data>>,
    sequence: Arc<AtomicU64>,
}
impl Probe {
    pub fn new() -> Self {
        Self {
            url: "https://gateway.example.test".into(),
            marker: Arc::new(Mutex::new(Some("fixture-identity-a".into()))),
            epoch: Arc::new(AtomicU64::new(1)),
            side: 0,
            pinned: None,
            data: Arc::default(),
            sequence: Arc::new(AtomicU64::new(1)),
        }
    }
    pub fn revoke_next_factory(&self) {
        self.data.lock().expect("fixture data").revoke_on_factory = true;
    }
    pub fn revoke_after_pinned_checks(&self, n: usize) {
        self.data.lock().expect("fixture data").marker_checks_until_revoke = n;
    }
    pub fn revoke_next_dispatch(&self) {
        self.data.lock().expect("fixture data").revoke_on_ask = true;
    }
    pub fn reads(&self) -> Vec<(String, u64)> {
        self.data.lock().expect("fixture data").reads.clone()
    }
    pub fn asks(&self) -> Vec<Ask> {
        self.data.lock().expect("fixture data").asks.clone()
    }
    pub fn stops(&self) -> Vec<(u64, u64)> {
        self.data.lock().expect("fixture data").stops.clone()
    }
    pub fn emit(&self, side: u64, event: Event) {
        let sender = self
            .data
            .lock()
            .expect("fixture data")
            .events
            .get(&side)
            .expect("requested execution side")
            .clone();
        sender.unbounded_send(event).expect("live owner receiver");
    }
    pub fn reply(&self, ask: &Ask, event: Event) {
        self.emit(
            ask.side,
            Event::ReadSnapshot {
                epoch: self.epoch.clone(),
                expected: self.epoch.load(Ordering::SeqCst),
                revision: None,
                event: Box::new(event),
            },
        );
    }
}
impl Backend for Probe {
    fn url(&self) -> &str {
        &self.url
    }
    fn marker(&self) -> Option<String> {
        if self
            .pinned
            .is_some_and(|epoch| epoch != self.epoch.load(Ordering::SeqCst))
        {
            return None;
        }
        let value = self.marker.lock().expect("fixture marker").clone();
        let revoke = if self.pinned.is_some() {
            let mut data = self.data.lock().expect("fixture data");
            if data.marker_checks_until_revoke > 0 {
                data.marker_checks_until_revoke -= 1;
                data.marker_checks_until_revoke == 0
            } else {
                false
            }
        } else {
            false
        };
        if revoke {
            self.epoch.fetch_add(1, Ordering::SeqCst);
        }
        value
    }
    fn site_url(&self) -> String {
        "https://ui.example.test".into()
    }
    fn request(&self, _verb: Verb, _path: String, _body: Option<Value>) -> Request {
        Box::pin(async { Ok(json!({})) })
    }
    fn open_session(&self, id: String) -> u64 {
        let read = self.sequence.fetch_add(1, Ordering::SeqCst);
        self.data.lock().expect("fixture data").reads.push((id, read));
        read
    }
    fn text(&self, _path: String) -> TextRequest {
        Box::pin(async { Ok(String::new()) })
    }
    fn agent(&self, _base: Option<String>, _call: crate::agent::Call) -> crate::agent::Stream {
        crate::agent::Stream {
            chunks: futures::channel::mpsc::unbounded().1,
            _stop: None,
        }
    }
    fn switched(&self, url: &str) -> (Arc<dyn Backend>, futures::channel::mpsc::UnboundedReceiver<Event>) {
        let mut next = self.clone();
        next.url = url.into();
        (Arc::new(next), futures::channel::mpsc::unbounded().1)
    }
    fn dictate(&self, _id: u64) -> Dictation {
        panic!("chat execution fixture never starts dictation")
    }
    fn restore(&self) {}
    fn window(&self) -> (Arc<dyn Backend>, futures::channel::mpsc::UnboundedReceiver<Event>) {
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        let mut next = self.clone();
        next.side = self.sequence.fetch_add(1, Ordering::SeqCst);
        self.data.lock().expect("fixture data").events.insert(next.side, sender);
        (Arc::new(next), receiver)
    }
    fn execution_window(
        &self,
        authority: &crate::backend::ExecutionAuthority,
    ) -> Option<(Arc<dyn Backend>, futures::channel::mpsc::UnboundedReceiver<Event>)> {
        let revoke = {
            let mut data = self.data.lock().expect("fixture data");
            std::mem::take(&mut data.revoke_on_factory)
        };
        if revoke {
            self.epoch.fetch_add(2, Ordering::SeqCst);
        }
        if !authority.matches(&self.epoch) {
            return None;
        }
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        let mut next = self.clone();
        next.side = self.sequence.fetch_add(1, Ordering::SeqCst);
        next.pinned = Some(self.epoch.load(Ordering::SeqCst));
        self.data.lock().expect("fixture data").events.insert(next.side, sender);
        Some((Arc::new(next), receiver))
    }
    fn sign_in(&self) {}
    fn cancel_sign_in(&self) {}
    fn sign_in_with_key(&self, _key: String) {}
    fn switch_org(&self, _id: String) {}
    fn sign_out(&self) {}
    fn load_account(&self) {}
    fn load_org_settings(&self) {}
    fn load_inbox(&self) {}
    fn load_notifications(&self, _request: crate::inbox::PageRequest) {}
    fn mark_notification(&self, _id: String, _mark: crate::inbox::Mark) {}
    fn read_all_notifications(&self, _known: Vec<String>) {}
    fn docs_index(&self) {}
    fn docs_page(&self, _path: String) {}
    fn list_sessions(&self) {}
    fn list_models(&self) {}
    fn load_pays(&self) {}
    fn list_providers(&self) {}
    fn suggest(&self, _session: String) {}
    fn ask(
        &self,
        turn: u64,
        session: Option<String>,
        prompt: String,
        model: Option<String>,
        attachments: crate::attachments::Draft,
        choices: crate::gateway::ChatChoices,
    ) {
        assert!(self.pinned.is_some(), "execution uses its dedicated pinned factory");
        let revoke = {
            let mut data = self.data.lock().expect("fixture data");
            std::mem::take(&mut data.revoke_on_ask)
        };
        if revoke {
            self.epoch.fetch_add(1, Ordering::SeqCst);
        }
        if self.marker().is_none() {
            return;
        }
        self.data.lock().expect("fixture data").asks.push(Ask {
            side: self.side,
            turn,
            session,
            prompt,
            model,
            choices,
            attachments,
        });
    }
    fn stop(&self, turn: u64) {
        self.data.lock().expect("fixture data").stops.push((self.side, turn));
    }
    fn rewind(&self, _ask: u64, _session: String, _message: String, _text: String, _attachments: Vec<String>) {}
    fn search(&self, _seq: u64, _query: String) {}
    fn load_library(&self, _org: Option<String>) {}
    fn frame_artifact(&self, _id: String, _path: String) {}
    fn frame_page(&self, _id: String, _path: String) {}
    fn frame_page_version(&self, _id: String, _version: u32, _path: String) {}
    fn chat_text(&self, _id: String, _title: String, _path: String) {}
    fn load_notification_settings(&self) {}
    fn set_notification_setting(&self, _group: String, _channel: String, _on: bool) {}
    fn list_tokens(&self) {}
    fn create_token(&self, _label: String) {}
    fn revoke_token(&self, _id: String) {}
}
