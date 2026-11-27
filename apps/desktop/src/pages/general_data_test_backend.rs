//! Deterministic Backend fixture: every pending answer can be completed in an explicit order.
use crate::backend::{Backend, DetailedRequest, Request, TextRequest};
use crate::dictation::Dictation;
use crate::gateway::Verb;
use futures::channel::oneshot;
use serde_json::Value;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Pending {
    pub verb: Verb,
    pub path: String,
    pub body: Option<Value>,
    pub answer: oneshot::Sender<nexus_client::Result<Value>>,
    pub consumed: Arc<AtomicBool>,
}

#[derive(Default)]
pub(super) struct BackendFixture {
    pub marker: Mutex<Option<String>>,
    pub pending: Mutex<Vec<Pending>>,
}
impl BackendFixture {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            marker: Mutex::new(Some("account-a".into())),
            pending: Mutex::default(),
        })
    }
    pub fn take(&self, path: &str) -> Pending {
        let mut pending = self.pending.lock().unwrap();
        let index = pending
            .iter()
            .position(|request| request.path == path)
            .unwrap_or_else(|| panic!("No pending request for {path}"));
        pending.remove(index)
    }
}
#[allow(unused_variables)] // Other Backend operations must never run in this focused fixture.
impl Backend for BackendFixture {
    fn url(&self) -> &str {
        "https://gateway.example.test"
    }
    fn marker(&self) -> Option<String> {
        self.marker.lock().unwrap().clone()
    }
    fn site_url(&self) -> String {
        "https://app.example.test".into()
    }
    fn request(&self, verb: Verb, path: String, body: Option<Value>) -> Request {
        let answer = self.request_typed(verb, path, body);
        Box::pin(async move { answer.await.map_err(|error| error.to_string()) })
    }
    fn request_typed(&self, verb: Verb, path: String, body: Option<Value>) -> DetailedRequest {
        let (answer, result) = oneshot::channel();
        let consumed = Arc::new(AtomicBool::new(false));
        self.pending.lock().unwrap().push(Pending {
            verb,
            path,
            body,
            answer,
            consumed: consumed.clone(),
        });
        Box::pin(async move {
            let value = result.await.expect("fixture request answered");
            consumed.store(true, Ordering::SeqCst);
            value
        })
    }
    fn text(&self, path: String) -> TextRequest {
        panic!("Unexpected Data fixture call: text")
    }
    fn agent(&self, base: Option<String>, call: crate::agent::Call) -> crate::agent::Stream {
        panic!("Unexpected Data fixture call: agent")
    }
    fn restore(&self) {
        panic!("Unexpected Data fixture call: restore")
    }
    fn switched(
        &self,
        url: &str,
    ) -> (
        std::sync::Arc<dyn Backend>,
        futures::channel::mpsc::UnboundedReceiver<crate::gateway::Event>,
    ) {
        panic!("Unexpected Data fixture call: switched")
    }
    fn window(
        &self,
    ) -> (
        std::sync::Arc<dyn Backend>,
        futures::channel::mpsc::UnboundedReceiver<crate::gateway::Event>,
    ) {
        panic!("Unexpected Data fixture call: window")
    }
    fn sign_in(&self) {
        panic!("Unexpected Data fixture call: sign_in")
    }
    fn cancel_sign_in(&self) {
        panic!("Unexpected Data fixture call: cancel_sign_in")
    }
    fn sign_in_with_key(&self, key: String) {
        panic!("Unexpected Data fixture call: sign_in_with_key")
    }
    fn switch_org(&self, id: String) {
        panic!("Unexpected Data fixture call: switch_org")
    }
    fn sign_out(&self) {
        panic!("Unexpected Data fixture call: sign_out")
    }
    fn load_account(&self) {
        panic!("Unexpected Data fixture call: load_account")
    }
    fn load_org_settings(&self) {
        panic!("Unexpected Data fixture call: load_org_settings")
    }
    fn load_inbox(&self) {
        panic!("Unexpected Data fixture call: load_inbox")
    }
    fn load_notifications(&self, request: crate::inbox::PageRequest) {
        panic!("Unexpected Data fixture call: load_notifications")
    }
    fn mark_notification(&self, id: String, mark: crate::inbox::Mark) {
        panic!("Unexpected Data fixture call: mark_notification")
    }
    fn read_all_notifications(&self, known: Vec<String>) {
        panic!("Unexpected Data fixture call: read_all_notifications")
    }
    fn docs_index(&self) {
        panic!("Unexpected Data fixture call: docs_index")
    }
    fn docs_page(&self, path: String) {
        panic!("Unexpected Data fixture call: docs_page")
    }
    fn list_sessions(&self) {
        panic!("Unexpected Data fixture call: list_sessions")
    }
    fn open_session(&self, id: String) -> u64 {
        panic!("Unexpected Data fixture call: open_session")
    }
    fn list_models(&self) {
        panic!("Unexpected Data fixture call: list_models")
    }
    fn load_pays(&self) {
        panic!("Unexpected Data fixture call: load_pays")
    }
    fn list_providers(&self) {
        panic!("Unexpected Data fixture call: list_providers")
    }
    fn suggest(&self, session: String) {
        panic!("Unexpected Data fixture call: suggest")
    }
    fn ask(
        &self,
        turn: u64,
        session: Option<String>,
        prompt: String,
        model: Option<String>,
        attachments: crate::attachments::Draft,
        choices: crate::gateway::ChatChoices,
    ) {
        panic!("Unexpected Data fixture call: ask")
    }
    fn stop(&self, turn: u64) {
        panic!("Unexpected Data fixture call: stop")
    }
    fn rewind(&self, ask: u64, session: String, message: String, text: String, attachments: Vec<String>) {
        panic!("Unexpected Data fixture call: rewind")
    }
    fn dictate(&self, id: u64) -> Dictation {
        panic!("Unexpected Data fixture call: dictate")
    }
    fn search(&self, seq: u64, query: String) {
        panic!("Unexpected Data fixture call: search")
    }
    fn load_library(&self, org: Option<String>) {
        panic!("Unexpected Data fixture call: load_library")
    }
    fn frame_artifact(&self, id: String, path: String) {
        panic!("Unexpected Data fixture call: frame_artifact")
    }
    fn frame_page(&self, id: String, path: String) {
        panic!("Unexpected Data fixture call: frame_page")
    }
    fn frame_page_version(&self, id: String, version: u32, path: String) {
        panic!("Unexpected Data fixture call: frame_page_version")
    }
    fn chat_text(&self, id: String, title: String, path: String) {
        panic!("Unexpected Data fixture call: chat_text")
    }
    fn load_notification_settings(&self) {
        panic!("Unexpected Data fixture call: load_notification_settings")
    }
    fn set_notification_setting(&self, group: String, channel: String, on: bool) {
        panic!("Unexpected Data fixture call: set_notification_setting")
    }
    fn list_tokens(&self) {
        panic!("Unexpected Data fixture call: list_tokens")
    }
    fn create_token(&self, label: String) {
        panic!("Unexpected Data fixture call: create_token")
    }
    fn revoke_token(&self, id: String) {
        panic!("Unexpected Data fixture call: revoke_token")
    }
}
