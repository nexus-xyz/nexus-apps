//! Plain presentation and command owners cross primary-window closure; input/view entities do not.
use super::*;
use std::rc::Rc;

pub(in crate::workspace) struct RetainedChat {
    data: Option<Data>,
    pending: operations::Pending<operations::Completion>,
}
pub(in crate::workspace) struct RetainedSides {
    pub(in crate::workspace) chats: HashMap<String, RetainedChat>,
    pub(in crate::workspace) selected: HashMap<String, String>,
    pub(in crate::workspace) visible: bool,
    pub(in crate::workspace) creation: super::creation::Creation,
}
struct Data {
    summary: ThreadSummary,
    partial: bool,
    parent_title: String,
    source: crate::workspace::runs::Source,
    events: Vec<StoredEvent>,
    history: history::History,
    input: String,
    draft: crate::attachments::Draft,
    outbox: std::collections::VecDeque<operations::Outgoing>,
    uploading: Option<crate::attachments::Draft>,
    echo: Vec<echo::Echo>,
    replies: (Vec<crate::workspace::stacked_replies::Reply>, u64),
    revision: u64,
    rewrite: Option<(i64, String, String)>,
    brought_back: bool,
    problem: Option<String>,
    note: Option<String>,
    scroll: ListOffset,
    tail: bool,
    dock: f32,
    request: Option<String>,
    answers: Vec<(usize, String)>,
    reason: String,
    model: String,
    settings: Option<composer::Section>,
    working_since: Option<std::time::Instant>,
}
impl Drop for RetainedChat {
    fn drop(&mut self) {
        if self.data.is_some() {
            self.pending.revoke();
        }
    }
}
impl RetainedChat {
    /// A side chat just made. `sent`: the first message, which landed: it shows before the first read.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn created(
        summary: ThreadSummary,
        parent_title: String,
        source: crate::workspace::runs::Source,
        input: String,
        sent: Option<String>,
        problem: Option<String>,
        guard: Rc<dyn Fn() -> bool>,
        partial: bool,
    ) -> Self {
        let pending = operations::Pending::default();
        pending.guard(guard);
        Self {
            pending,
            data: Some(Data {
                summary,
                partial,
                parent_title,
                source,
                input,
                problem,
                events: vec![],
                history: Default::default(),
                draft: Default::default(),
                outbox: Default::default(),
                uploading: None,
                echo: sent.map(echo::Echo::landed).into_iter().collect(),
                replies: (vec![], 0),
                revision: 0,
                rewrite: None,
                brought_back: false,
                note: None,
                scroll: ListOffset::default(),
                tail: true,
                dock: 0.,
                request: None,
                answers: vec![],
                reason: String::new(),
                model: String::new(),
                settings: None,
                working_since: None,
            }),
        }
    }
    pub(in crate::workspace) fn preview(&self) -> (String, String, Vec<String>) {
        let data = self.data.as_ref().expect("retained side chat");
        let mut input = data.input.clone();
        let mut files: Vec<String> = data.draft.all().map(|file| file.name.clone()).collect();
        // A sent message left the composer; one the core refused goes back to it once the side chat is open again.
        self.pending.inspect(|completion| {
            if let Some(operations::Completion {
                effect: operations::Effect::Send { text, draft },
                result: Err(_),
            }) = completion
            {
                if let Some(value) = crate::workspace::uploads::restored(text, &input) {
                    input = value;
                }
                files = draft
                    .all()
                    .map(|file| file.name.clone())
                    .chain(files.drain(..))
                    .collect();
            }
        });
        (data.summary.title.clone(), input, files)
    }
    pub(in crate::workspace) fn parent(&self) -> Option<&str> {
        self.data.as_ref()?.summary.source_thread_id.as_deref()
    }
    pub(in crate::workspace) fn source(&self) -> &crate::workspace::runs::Source {
        &self.data.as_ref().expect("retained side chat").source
    }
    pub(in crate::workspace) fn valid(&self) -> bool {
        self.pending.valid()
    }
}
impl SideChat {
    pub(in crate::workspace) fn authority(&mut self, guard: Rc<dyn Fn() -> bool>) {
        self.pending.guard(guard);
    }
    pub(in crate::workspace) fn retire(&mut self, revoke: bool) {
        self.retired = true;
        self.poll = None;
        self.suspend_partials();
        self._completion = None;
        self.pending.detach();
        self._subscriptions.clear();
        if revoke {
            self.pending.revoke();
        }
    }
    pub(in crate::workspace) fn retain(&mut self, cx: &mut Context<Self>) -> RetainedChat {
        let questions = code::pending_approval(&self.blocks)
            .and_then(|block| match block {
                Block::Approval { input, .. } => Some(super::super::code_dock::questions_of(input)),
                _ => None,
            })
            .unwrap_or_default();
        let answers = self
            .answers
            .iter()
            .enumerate()
            .filter_map(|(index, input)| {
                questions
                    .get(index)
                    .filter(|question| !question.secret)
                    .map(|_| (index, input.read(cx).value().to_string()))
            })
            .collect();
        let data = Data {
            summary: self.summary.clone(),
            partial: false,
            parent_title: self.parent_title.clone(),
            source: self.runs.source(),
            events: std::mem::take(&mut self.events),
            history: std::mem::take(&mut self.history),
            input: self.input.read(cx).value().to_string(),
            draft: std::mem::take(&mut self.draft),
            outbox: std::mem::take(&mut self.outbox),
            uploading: self.uploading.take(),
            echo: std::mem::take(&mut self.echo),
            replies: self.replies.keep(),
            revision: self.draft_revision,
            rewrite: self
                .rewrite
                .take()
                .map(|rewrite| (rewrite.seq, rewrite.from, rewrite.input.read(cx).value().to_string())),
            brought_back: self.brought_back,
            problem: self.problem.take(),
            note: self.note.take(),
            scroll: self.scroll.list.logical_scroll_top(),
            tail: self.scroll.at_end(),
            dock: f32::from(self.dock_height.room()),
            request: self.request.clone(),
            answers,
            reason: self.permission_reason.read(cx).value().to_string(),
            model: self.model_input.read(cx).value().to_string(),
            settings: self.settings,
            working_since: self.working_since,
        };
        self.retire(false);
        RetainedChat {
            data: Some(data),
            pending: self.pending.clone(),
        }
    }
    pub(in crate::workspace) fn restore(
        mut saved: RetainedChat,
        runs: Runs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let data = saved.data.take().expect("retained side chat adopted once");
        let mut chat = Self::new(data.summary, data.parent_title, runs, window, cx);
        chat._completion = None;
        chat.pending = saved.pending.clone();
        chat.sending = chat.pending.occupied();
        chat.ready = false;
        chat.allow_partial_stream = data.partial;
        chat.events = data.events;
        chat.blocks = own_blocks(&chat.events, chat.summary.created_at);
        for (ix, block) in chat.blocks.iter().enumerate() {
            if let Block::Text(text) = block {
                chat.texts.insert(ix, cx.new(|cx| TextViewState::markdown(text, cx)));
            }
        }
        chat.history = data.history;
        chat.draft_text = data.input.clone();
        chat.input
            .update(cx, |input, cx| input.set_value(data.input, window, cx));
        chat.draft_revision = data.revision;
        chat.draft = data.draft;
        chat.outbox = data.outbox;
        // The chips follow an upload still running; a finished one's answer clears it (`apply_commands`).
        chat.uploading = data.uploading;
        chat.echo = data.echo;
        chat.replies.restore(data.replies);
        chat.brought_back = data.brought_back;
        chat.problem = data.problem;
        chat.note = data.note;
        chat.settings = data.settings;
        chat.working_since = data.working_since;
        chat.model_input
            .update(cx, |input, cx| input.set_value(data.model, window, cx));
        chat.dock_height = super::super::floating_dock::DockHeight::from_room(data.dock);
        chat.follow_request(window, cx);
        if chat.request == data.request {
            for (index, text) in data.answers {
                if let Some(input) = chat.answers.get(index) {
                    input.update(cx, |input, cx| input.set_value(text, window, cx));
                }
            }
            chat.permission_reason
                .update(cx, |input, cx| input.set_value(data.reason, window, cx));
        }
        if let Some((seq, from, edited)) = data.rewrite {
            let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6).submit_on_enter(false));
            input.update(cx, |input, cx| input.set_value(edited, window, cx));
            chat.rewrite = Some(composer::Rewrite { seq, from, input });
        }
        // The list takes the retained rows now, so the reader's row is there to go back to.
        chat.sync_history(
            crate::palette::Palette::for_choice(super::super::settings_view::saved_theme(), window.appearance()),
            cx,
        );
        if !data.tail {
            chat.scroll.list.scroll_to(data.scroll);
        }
        chat.watch_commands(window, cx);
        chat.watch_upload(window, cx);
        chat
    }
}
impl Drop for SideChat {
    fn drop(&mut self) {
        if !self.retired {
            self.pending.revoke();
        }
    }
}

impl Workspace {
    pub(in crate::workspace) fn retain_side_chats(&mut self, cx: &mut Context<Self>) -> RetainedSides {
        self.side.side_chat_retired = true;
        self.side.side_chat_creation_watch = None;
        self.side.side_chat_creation.detach();
        let mut chats = std::mem::take(&mut self.side.side_chat_waiting);
        for (id, (chat, subscription)) in std::mem::take(&mut self.side.side_chats) {
            drop(subscription);
            chats.insert(id, chat.update(cx, |chat, cx| chat.retain(cx)));
        }
        RetainedSides {
            chats,
            selected: std::mem::take(&mut self.side.side_chat_for),
            visible: self.side.side_chat_visible,
            creation: std::mem::take(&mut self.side.side_chat_creation),
        }
    }
    pub(in crate::workspace) fn restore_side_chats(
        &mut self,
        saved: RetainedSides,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.side.side_chat_retired = false;
        self.side.side_chat_waiting = saved.chats;
        self.side.side_chat_for = saved.selected;
        self.side.side_chat_visible = saved.visible;
        self.side.side_chat_creation = saved.creation;
        self.watch_side_chat_creation(window, cx);
        self.adopt_side_chats(window, cx);
    }
    pub(super) fn adopt_side_chats(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.side.side_chat_retired {
            return;
        }
        let mut waiting = std::mem::take(&mut self.side.side_chat_waiting);
        for (id, saved) in waiting.drain() {
            if !saved.valid() {
                self.side.side_chat_for.retain(|_, side| side != &id);
                continue;
            }
            let Some(parent) = saved.parent().map(str::to_string) else {
                continue;
            };
            let runs = self
                .side_chat_runs(&parent)
                .filter(|runs| runs.source().eq(saved.source()));
            let Some(runs) = runs else {
                self.side.side_chat_error =
                    Some("Reconnect to the side chat's original machine or take over its store to resume.".into());
                self.side.side_chat_waiting.insert(id, saved);
                continue;
            };
            let chat = cx.new(|cx| SideChat::restore(saved, runs, window, cx));
            self.bind_side_chat(parent, id, chat.clone(), window, cx);
            chat.update(cx, |chat, cx| chat.apply_commands(window, cx));
        }
    }
}
