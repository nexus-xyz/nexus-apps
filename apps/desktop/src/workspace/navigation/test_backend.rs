//! Per-instance backend; all reads/writes and fingerprints are fixtures, never credentials or live calls.
use crate::backend::{Backend, Request, TextRequest};
use crate::dictation::Dictation;
use crate::gateway::{Event, Verb};
use futures::channel::oneshot;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
type MemberReply = oneshot::Sender<Result<Value, String>>;
/// An agent API call the probe took, and the sender a test answers it through.
pub(in crate::workspace) type AgentCall = (
    crate::agent::Call,
    futures::channel::mpsc::UnboundedSender<crate::agent::Chunk>,
);
#[derive(Clone)]
pub(in crate::workspace) struct BackendProbe {
    pub url: String,
    pub marker: Arc<Mutex<Option<String>>>,
    pub reads: Arc<Mutex<Vec<String>>>,
    pub calls: Arc<Mutex<Vec<&'static str>>>,
    pub opens: Arc<Mutex<Vec<String>>>,
    /// The pages and artifacts asked to open in the Workbench (`frame_*`, `chat_text`), as "page <id> v<N> <path>".
    pub frames: Arc<Mutex<Vec<String>>>,
    pub members: Arc<Mutex<Vec<(u64, MemberReply)>>>,
    pub hold_members: bool,
    pub member_count: usize,
    /// The agent API calls (`agent.rs`), in order, each with its open answer.
    pub agent_calls: Arc<Mutex<Vec<AgentCall>>>,
    /// `GET /v1/agents/profiles`'s answer; `{}` (no profiles) when None.
    pub profiles: Arc<Mutex<Option<Value>>>,
    sequence: Arc<AtomicU64>,
}
impl BackendProbe {
    pub fn new() -> Self {
        Self {
            url: "https://gateway.example.test".into(),
            marker: Arc::new(Mutex::new(Some("fixture-identity-a".into()))),
            reads: Arc::default(),
            calls: Arc::default(),
            opens: Arc::default(),
            frames: Arc::default(),
            members: Arc::default(),
            hold_members: false,
            member_count: 0,
            agent_calls: Arc::default(),
            profiles: Arc::default(),
            sequence: Arc::new(AtomicU64::new(1)),
        }
    }
    fn called(&self, name: &'static str) {
        self.calls.lock().expect("fixture backend calls").push(name);
    }

    pub fn member_ids(&self) -> Vec<u64> {
        self.members
            .lock()
            .expect("member reads")
            .iter()
            .map(|(id, _)| *id)
            .collect()
    }
    pub fn release_members(&self, id: u64, count: usize) -> bool {
        let mut pending = self.members.lock().expect("held member reads");
        let at = pending
            .iter()
            .position(|(asked, _)| *asked == id)
            .expect("exact requested member read");
        let (_, answer) = pending.remove(at);
        drop(pending);
        answer.send(Ok(members_body(count))).is_ok()
    }
}
impl Backend for BackendProbe {
    fn url(&self) -> &str {
        &self.url
    }
    fn marker(&self) -> Option<String> {
        self.marker.lock().expect("fixture marker").clone()
    }
    fn site_url(&self) -> String {
        "https://ui.example.test".into()
    }
    fn request(&self, _verb: Verb, path: String, _body: Option<Value>) -> Request {
        self.called("request");
        self.reads.lock().expect("fixture calls").push(path.clone());
        if path == "/v1/orgs/members" && self.hold_members {
            let (send, read) = oneshot::channel();
            let id = self.sequence.fetch_add(1, Ordering::SeqCst);
            self.members.lock().expect("member reads").push((id, send));
            return Box::pin(async move { read.await.map_err(|_| "fixture read lost".to_string())? });
        }
        let count = self.member_count;
        let profiles = self.profiles.lock().expect("fixture profiles").clone();
        Box::pin(async move {
            Ok(if path == "/v1/agents/profiles" {
                profiles.unwrap_or_else(|| json!({}))
            } else if path == "/v1/tenants/me" {
                json!({"tenant":{"id":"org-a","slug":"org-a","name":"Fixture organization","kind":"company"},"user":{"id":"user-a","personId":"person-a","name":"A","email":"a@example.test"},"role":"member","permissions":[]})
            } else if path == "/v1/orgs/members" {
                members_body(count)
            } else {
                json!({})
            })
        })
    }
    fn open_session(&self, id: String) -> u64 {
        self.called("open_session");
        self.opens.lock().expect("chat reads").push(id);
        self.sequence.fetch_add(1, Ordering::SeqCst)
    }
    fn text(&self, _path: String) -> TextRequest {
        self.called("text");
        Box::pin(async { Ok(String::new()) })
    }
    fn agent(&self, _base: Option<String>, call: crate::agent::Call) -> crate::agent::Stream {
        self.called("agent");
        let (answer, chunks) = futures::channel::mpsc::unbounded();
        self.agent_calls.lock().expect("agent calls").push((call, answer));
        crate::agent::Stream { chunks, _stop: None }
    }
    fn switched(&self, url: &str) -> (Arc<dyn Backend>, futures::channel::mpsc::UnboundedReceiver<Event>) {
        let mut next = self.clone();
        next.url = url.into();
        (Arc::new(next), futures::channel::mpsc::unbounded().1)
    }
    fn dictate(&self, _id: u64) -> Dictation {
        self.called("dictate");
        panic!("navigation fixture never starts dictation")
    }
    fn restore(&self) {
        self.called("restore");
    }
    fn window(&self) -> (Arc<dyn Backend>, futures::channel::mpsc::UnboundedReceiver<Event>) {
        (Arc::new(self.clone()), futures::channel::mpsc::unbounded().1)
    }
    fn sign_in(&self) {
        self.called("sign_in");
    }
    fn cancel_sign_in(&self) {
        self.called("cancel_sign_in");
    }
    fn sign_in_with_key(&self, _key: String) {
        self.called("sign_in_with_key");
    }
    fn switch_org(&self, _id: String) {
        self.called("switch_org");
    }
    fn sign_out(&self) {
        self.called("sign_out");
    }
    fn load_account(&self) {
        self.called("load_account");
    }
    fn load_org_settings(&self) {
        self.called("load_org_settings");
    }
    fn load_inbox(&self) {
        self.called("load_inbox");
    }
    fn load_notifications(&self, _request: crate::inbox::PageRequest) {
        self.called("load_notifications");
    }
    fn mark_notification(&self, _id: String, _mark: crate::inbox::Mark) {
        self.called("mark_notification");
    }
    fn read_all_notifications(&self, _known: Vec<String>) {
        self.called("read_all_notifications");
    }
    fn docs_index(&self) {
        self.called("docs_index");
    }
    fn docs_page(&self, _path: String) {
        self.called("docs_page");
    }
    fn list_sessions(&self) {
        self.called("list_sessions");
    }
    fn list_models(&self) {
        self.called("list_models");
    }
    fn load_pays(&self) {
        self.called("load_pays");
    }
    fn list_providers(&self) {
        self.called("list_providers");
    }
    fn suggest(&self, _session: String) {
        self.called("suggest");
    }
    fn ask(
        &self,
        _turn: u64,
        _session: Option<String>,
        _prompt: String,
        _model: Option<String>,
        _attachments: crate::attachments::Draft,
        _choices: crate::gateway::ChatChoices,
    ) {
        self.called("ask");
    }
    fn stop(&self, _turn: u64) {
        self.called("stop");
    }
    fn rewind(&self, _ask: u64, _session: String, _message: String, _text: String, _attachments: Vec<String>) {
        self.called("rewind");
    }
    fn search(&self, _seq: u64, _query: String) {
        self.called("search");
    }
    fn load_library(&self, _org: Option<String>) {
        self.called("load_library");
    }
    fn frame_artifact(&self, id: String, path: String) {
        self.called("frame_artifact");
        self.frames.lock().expect("frames").push(format!("share {id} {path}"));
    }
    fn frame_page(&self, id: String, path: String) {
        self.called("frame_page");
        self.frames
            .lock()
            .expect("frames")
            .push(format!("page {id} latest {path}"));
    }
    fn frame_page_version(&self, id: String, version: u32, path: String) {
        self.called("frame_page_version");
        self.frames
            .lock()
            .expect("frames")
            .push(format!("page {id} v{version} {path}"));
    }
    fn chat_text(&self, id: String, _title: String, path: String) {
        self.called("chat_text");
        self.frames.lock().expect("frames").push(format!("chat {id} {path}"));
    }
    fn load_notification_settings(&self) {
        self.called("load_notification_settings");
    }
    fn set_notification_setting(&self, _group: String, _channel: String, _on: bool) {
        self.called("set_notification_setting");
    }
    fn list_tokens(&self) {
        self.called("list_tokens");
    }
    fn create_token(&self, _label: String) {
        self.called("create_token");
    }
    fn revoke_token(&self, _id: String) {
        self.called("revoke_token");
    }
}

fn members_body(count: usize) -> Value {
    let members=(0..count).map(|n|json!({"userId":format!("user-{n}"),"name":format!("Member {n}"),"email":format!("person-{n}@example.test"),"role":"member","spentUsd":"0.00"})).collect::<Vec<_>>();
    json!({"members":members})
}
