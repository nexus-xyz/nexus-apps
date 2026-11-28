//! A Code session, read from the Nexus app's event log: what the person asked, the agent's text, its thinking and
//! one line per tool call. When this build runs the sessions (`code_run.rs`), the dock under it (`code_dock.rs`) sends
//! to the agent, queues behind a running turn, stops it, and answers what the agent asks: a question, a plan or a
//! request to use a tool. While the Nexus app runs them, the session is read-only here and follows the store, polled.

use crate::workspace::glass::FloatingGround;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::tooltip::Tooltip;
use nexus_workbench::api::UpdateArgs;
use nexus_workbench::event::{Decision, PermissionMode, TurnStatus};
use nexus_workbench::queued_delivery::Delivery;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::answer_links::AnswerFiles;
use super::code_dock::{Dock, PLAN_TOOL, QUESTION_TOOL, plan_of, plan_title};
use super::code_pages::AutoPage;
use super::message_text::MessageText;
use super::minimap::{Minimap, PREVIEW, Row, first_line};
use super::{COPIED_FOR, Place, SettingsSection, View, Workspace, glyph, transcript};
use crate::code::{self, AgentEnd, AgentRow, Block, CodeSession, MOVED_ON, Status, Transcript};
use crate::code_run::NewSession;
use crate::motion;
use crate::pages::PageId;
use crate::palette::Palette;
use nexus_workbench::crew_fold::AgentStatus;

/// How often the working line's elapsed time moves.
pub(super) const WORKING_TICK: Duration = Duration::from_secs(1);

/// How far past the visible part the transcript lays out blocks, so a scroll does not show them pop in.
const OVERDRAW: Pixels = px(800.);

/// How near the top, in blocks, the reader scrolls before the page before loads (`timeline-view.tsx` counts pixels).
const OLDER_MARGIN: usize = 10;

/// A session sent from New session that the runner is still making (`new-session.tsx`'s launch): the view opens at
/// once under a draft id, shows the message, and takes the session's id when it is made.
pub(super) struct Pending {
    pub new: NewSession,
    /// Preview handles pass to the stored message when its event arrives.
    pub images: std::sync::Arc<Vec<crate::code_media::ToolImage>>,
    /// Why the session could not be made; the view offers Try again and Edit.
    pub failed: Option<SharedString>,
    /// Set when the session implements another one's plan: once it is made, the planning session is told to stop.
    pub moved_from: Option<MovedFrom>,
    /// A cloud session (`cloud_view.rs`): its machine comes up first, under the message.
    pub cloud: Option<crate::cloud::CloudStart>,
}

/// The session whose plan went to a new session ("Implement in a new session", `plan-card.tsx`'s
/// `implementElsewhere`): its id, the plan's request, and the new session's title.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MovedFrom {
    pub thread: String,
    pub request: String,
    pub title: String,
}

/// The rewind a person asked for on one of their messages, waiting on their confirmation or on the store.
pub(super) struct Rewinding {
    /// The message's event.
    pub seq: i64,
    pub busy: bool,
    pub error: Option<SharedString>,
}

/// A message being edited in place (`user-message.tsx`'s editor): sending rewinds to before it and sends the new text.
pub(super) struct Editing {
    /// The message's event.
    pub seq: i64,
    pub input: Entity<TextareaState>,
    pub images: Vec<nexus_workbench::event::Image>,
    pub busy: bool,
    pub error: Option<SharedString>,
    _subscription: Subscription,
}

/// Where "Restore to here"'s confirmation is (`revert.ts`'s `Step`): counting the files, ready, restoring, or failed.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum RestoreStep {
    Counting,
    Ready(u32),
    Restoring(u32),
    Failed(SharedString),
}

/// "Restore to here" asked on the turn ending at event `turn`, its confirmation showing under the turn's line.
pub(super) struct Restoring {
    pub turn: i64,
    pub step: RestoreStep,
    /// Which opening this is: a count from an earlier one, answering late, is not this one's.
    pub ask: u64,
}

/// The open Code session.
pub(super) struct CodeView {
    pub id: String,
    pub start_card: super::session_start::StartCard,
    pub blocks: Vec<Block>,
    /// The event each block starts at (`code::Transcript::starts`).
    pub starts: Vec<i64>,
    pub setup_scrolls: std::collections::HashMap<String, ScrollHandle>,
    pub queue: nexus_workbench::queued_delivery::Queue,
    /// When the agent read each message (`code::Transcript::reads`).
    pub reads: std::collections::BTreeMap<i64, i64>,
    pub cite: super::cite_selection::CiteState,
    pub context_estimate: nexus_workbench::context_breakdown::Estimate,
    /// A message search hit to show once the transcript holds it (`code_jump.rs`).
    pub jump: Option<super::code_jump::Jump>,
    /// A Markdown view for each `Text` block, by the block's index; kept while the text is unchanged.
    pub texts: Vec<Option<Entity<TextViewState>>>,
    /// The last event read, to read the transcript again only when it moved.
    pub last_seq: i64,
    /// The first event shown: the transcript reads from it to the end, so new events never move the blocks above.
    /// It belongs to this view: opening a session builds a new view (`Workspace::open_code`), which starts from the
    /// latest page ([`crate::code::EVENTS`] events). The older pages read before are dropped, with the minimap's drag.
    pub start: Option<i64>,
    /// Older events exist before `start`.
    pub more: bool,
    /// The reader scrolled near the top: the next read takes the page before `start` too.
    pub older: bool,
    /// The `start` the page before was last asked for. A server's page lands after its read, so this asks once.
    pub asked_before: Option<i64>,
    pub loaded: bool,
    pub navigation_fresh: bool,
    /// Dropping the view cancels the callback of its background transcript read.
    pub reading: Option<Task<()>>,
    /// A change arrived during the read: read once more after it completes.
    pub read_again: bool,
    pub commands: Option<Vec<String>>,
    pub error: Option<SharedString>,
    /// What the dock under the session keeps: its text fields, a question's picks, the open menu.
    pub dock: Dock,
    /// Set while the session is being made, until its first events are read.
    pub pending: Option<Pending>,
    /// Since when the agent works (or the session starts), for the working line's elapsed time.
    pub working_since: Option<Instant>,
    /// The transcript as a list that lays out only the blocks on screen: one item per block, then the tail (the
    /// draft, the working line, a note). A long session drew every block on each frame, the caret's blink included.
    /// It follows the end while the end shows.
    pub list: ListState,
    /// The message just copied (its event), whose Copy shows a check for a moment.
    pub copied: Option<i64>,
    /// The turn a fork is being made from: its Fork waits, so a second click makes no second fork.
    pub forking: Option<i64>,
    hovered_message: Option<i64>,
    /// The row under the pointer and the turn it belongs to: a turn's footer actions show while any row of its answer
    /// is hovered, not only the footer.
    hovered_turn: Option<(usize, i64)>,
    message_actions: RefCell<HashMap<i64, super::message_actions::Motion>>,
    copied_reset: Option<Task<()>>,
    /// The message the person asked to rewind to; its confirmation shows under it.
    pub rewinding: Option<Rewinding>,
    /// The message being edited in place.
    pub editing: Option<Editing>,
    /// The turn "Restore to here" was asked on.
    pub restoring: Option<Restoring>,
    pub agent_switch: Option<super::agent_switch::PendingSwitch>,
    /// Failed tool rows opened to show their error, by call.
    pub open_errors: HashSet<String>,
    /// Sub-agent rows the person opened (true) or folded (false), by call. The rest start collapsed.
    pub open_agents: HashMap<String, bool>,
    /// Each sub-agent in the blocks, by call: where its row and its end are, and whether it runs.
    pub agents: HashMap<String, AgentAt>,
    /// What each running sub-agent is still writing (`stream.rs`), by call: drawn at the end of its open body.
    pub agent_drafts: Vec<(String, String)>,
    /// Tool rows opened to what they did (`tool_body.rs`), by call.
    pub open_tools: HashSet<String>,
    /// Opened rows showing their whole output ("Show all N lines").
    pub all_output: HashSet<String>,
    /// Commands being moved to the background, by call id.
    pub moving: HashSet<String>,
    /// The last row action that failed (Move to background, open in the editor): its call id and why.
    pub row_error: Option<(String, String)>,
    /// The answer the agent is still writing (`stream.rs`), drawn after the transcript until its text event lands.
    pub streamed: Option<(String, Entity<TextViewState>)>,
    /// Opens the pages the session writes, as it writes them (`code_pages.rs`).
    pub auto_page: AutoPage,
    /// The files of each ended turn, for the row of links at the top of its answer (`answer_links.rs`).
    pub answer_files: AnswerFiles,
    /// The Browser that opens on its own when the agent starts a server (`auto_preview.rs`).
    pub auto_browser: super::auto_preview::AutoBrowser,
    /// The minimap's hover and drag (`minimap.rs`): a drag ends with the session it scrolls.
    pub minimap: super::minimap::MinimapState,
    /// How many blocks older pages put above the ones first shown: what keeps find's match in place.
    pub shifted: usize,
    /// Find in this session (`code_find.rs`).
    pub find: super::code_find::CodeFind,
    /// The images and videos its answers link to, by path, once asked for (`code_media.rs`).
    pub media: std::collections::HashMap<String, crate::code_media::Media>,
    /// Sent images keep their handles while GPUI loads and draws them.
    pub user_images: super::code_media::SentImages,
    /// How each block draws in its run of work (`code_folds.rs`), laid out again as the blocks or the folds change.
    pub folds: Vec<super::code_folds::Place>,
    /// The cards and folds the person opened (true) or closed (false), by key (`code_folds::Head::key`).
    pub fold_open: std::collections::HashMap<super::code_folds::Key, bool>,
    /// The cards whose earlier steps the person asked to see.
    pub fold_all: HashSet<isize>,
    /// The thoughts opened to their text, by the same keys.
    pub thoughts_open: HashSet<isize>,
    /// What the turn still running has cost so far: the working line's "$0.84 so far".
    pub turn_cost: Option<f64>,
    /// How long the session's prompt cache stays warm: the meter's "Cache warm · 4:12 left" (`code_meter.rs`).
    pub cache: nexus_workbench::prompt_cache::CacheClock,
    /// A turn ended interrupted since the person's last message, and what its crew still runs (`interrupt_bars.rs`).
    pub interrupted: bool,
    pub crew: nexus_workbench::crew_fold::CrewCounts,
    /// The last event read when Continue was clicked: the Continue row hides until the transcript moves on.
    pub continued_at: Option<i64>,
    /// Up and Down's walk through the messages sent in this session (`prompt_history.rs`).
    pub recall: Option<super::prompt_history::Walk>,
}

impl CodeView {
    pub fn new(id: String, window: &mut Window, cx: &mut Context<Workspace>) -> CodeView {
        let list = transcript_list();
        list.set_scroll_handler(cx.listener(|this, _, window, cx| this.cite_scrolled(window, cx)));
        CodeView {
            id,
            start_card: super::session_start::StartCard::new(cx),
            blocks: Vec::new(),
            texts: Vec::new(),
            starts: Vec::new(),
            setup_scrolls: Default::default(),
            queue: Default::default(),
            reads: Default::default(),
            cite: super::cite_selection::CiteState::new(cx.focus_handle()),
            context_estimate: Default::default(),
            jump: None,
            last_seq: -1,
            start: None,
            more: false,
            older: false,
            asked_before: None,
            loaded: false,
            navigation_fresh: false,
            reading: None,
            read_again: false,
            commands: None,
            error: None,
            dock: Dock::new(window, cx),
            pending: None,
            working_since: None,
            list,
            copied: None,
            forking: None,
            hovered_message: None,
            hovered_turn: None,
            message_actions: Default::default(),
            copied_reset: None,
            rewinding: None,
            editing: None,
            restoring: None,
            agent_switch: None,
            open_errors: HashSet::new(),
            open_agents: HashMap::new(),
            agents: HashMap::new(),
            agent_drafts: Vec::new(),
            open_tools: HashSet::new(),
            all_output: HashSet::new(),
            moving: HashSet::new(),
            row_error: None,
            streamed: None,
            auto_page: AutoPage::new(false),
            answer_files: AnswerFiles::default(),
            auto_browser: super::auto_preview::AutoBrowser::new(false),
            minimap: Default::default(),
            shifted: 0,
            find: Default::default(),
            media: Default::default(),
            user_images: Default::default(),
            folds: Vec::new(),
            fold_open: Default::default(),
            fold_all: HashSet::new(),
            thoughts_open: HashSet::new(),
            turn_cost: None,
            cache: Default::default(),
            interrupted: false,
            crew: Default::default(),
            continued_at: None,
            recall: None,
        }
    }

    /// The view of a session not made yet, under a draft id, with its first message showing.
    pub fn pending(new: NewSession, window: &mut Window, cx: &mut Context<Workspace>) -> CodeView {
        static DRAFTS: AtomicU64 = AtomicU64::new(0);
        let draft = DRAFTS.fetch_add(1, Ordering::Relaxed);
        let mut view = CodeView::new(format!("draft-{draft}"), window, cx);
        view.pending = Some(Pending {
            images: std::sync::Arc::new(crate::code_media::tool_images(crate::attachments::session_images(
                &new.attachments,
            ))),
            new,
            failed: None,
            moved_from: None,
            cloud: None,
        });
        view.working_since = Some(Instant::now());
        // A new session: every page it writes is new.
        view.auto_page = AutoPage::new(true);
        view
    }

    /// Whether the session is not made yet: its id is a draft's and the store has nothing to read.
    pub fn is_draft(&self) -> bool {
        self.pending.is_some() && self.id.starts_with("draft-")
    }

    /// Take what the agent is still writing (`stream.rs`): None once its text landed. True when it changed.
    pub fn stream(&mut self, text: Option<String>, cx: &mut App) -> bool {
        let changed = self.take_stream(text, cx);
        if changed {
            // The tail holds the streamed answer: it is measured again.
            let tail = self.blocks.len();
            self.list.remeasure_items(tail..tail + 1);
        }
        changed
    }

    fn take_stream(&mut self, text: Option<String>, cx: &mut App) -> bool {
        match (text, &self.streamed) {
            (None, None) => false,
            (None, Some(_)) => {
                self.streamed = None;
                true
            }
            (Some(text), Some((shown, _))) if *shown == text => false,
            (Some(text), Some((_, view))) => {
                view.update(cx, |state, cx| state.set_text(&text, cx));
                if let Some((shown, _)) = &mut self.streamed {
                    *shown = text;
                }
                true
            }
            (Some(text), None) => {
                let view = cx.new(|cx| TextViewState::markdown(&text, cx));
                self.streamed = Some((text, view));
                true
            }
        }
    }

    /// Take the session's blocks as last read. Text views of blocks that did not change are kept, so a growing
    /// session does not re-lay out its whole history.
    pub fn set(&mut self, read: Transcript, cx: &mut App) {
        let _span = crate::watchdog::span("CodeView::set");
        let Transcript {
            queue,
            reads,
            context_estimate,
            blocks,
            starts,
            last: last_seq,
            start,
            more,
            turn_cost,
            cache,
            interrupted,
            crew,
            ..
        } = read;
        self.setup_scrolls.retain(|id, _| {
            blocks
                .iter()
                .any(|block| matches!(block, Block::Setup(row) if &row.id == id))
        });
        for block in &blocks {
            if let Block::Setup(row) = block {
                let scroll = self.setup_scrolls.entry(row.id.clone()).or_default();
                if row.status == nexus_workbench::event::SetupStatus::Running {
                    scroll.scroll_to_bottom();
                }
            }
        }
        self.queue = queue;
        self.reads = reads;
        self.interrupted = interrupted;
        self.crew = crew;
        self.context_estimate = context_estimate;
        // An older page landed above: its blocks go in before the ones shown. The list moves its anchor past them,
        // which keeps the reader on the block they were reading.
        let earlier = matches!((self.start, start), (Some(shown), Some(read)) if read < shown);
        let above = if earlier {
            older_blocks(&self.blocks, &blocks)
        } else {
            0
        };
        if above > 0 {
            self.texts.splice(0..0, std::iter::repeat_n(None, above));
            self.blocks.splice(0..0, blocks[..above].iter().cloned());
            self.list.splice(0..0, above);
            self.shifted += above;
        }
        let texts = retain_texts(&self.blocks, &mut self.texts, &blocks, cx);
        // Only the blocks that changed are measured again; the tail always is. A splice puts the reader at the top of
        // the first block it replaces, so it takes only the blocks added or removed. A change above the reader, such as
        // a sub-agent's step under its row, leaves them on the line they were reading.
        let mut same = common_prefix(&self.blocks, &blocks);
        // The block where an older page joins can take in its events: when only it changed, only it is measured again.
        if above > 0
            && same == above
            && self.blocks.get(above + 1).is_some()
            && self.blocks.get(above + 1) == blocks.get(above + 1)
        {
            self.list.remeasure_items(above..above + 1);
            same = above + 1 + common_prefix(&self.blocks[above + 1..], &blocks[above + 1..]);
        }
        let kept = common_suffix(&self.blocks[same..], &blocks[same..]);
        let (old_end, new_end) = (self.blocks.len() - kept, blocks.len() - kept);
        let in_place = (old_end - same).min(new_end - same);
        self.list.remeasure_items(same..same + in_place);
        self.list.splice(same + in_place..old_end, new_end - same - in_place);
        self.list.remeasure_items(blocks.len()..blocks.len() + 1);
        // The message whose answer changed shows a new row of links (`answer_links.rs`): measured again too.
        let row = super::answer_links::row_of(&blocks, same);
        if row < same {
            self.list.remeasure_items(row..row + 1);
        }
        self.start = start;
        self.more = more;
        self.dock.permissions.follow(&blocks);
        let pending = self.dock.permissions.current(&blocks);
        self.dock.follow(pending, cx);
        // The session's own first message takes the draft's place.
        if let Some(seq) = blocks.iter().find_map(|block| match block {
            Block::User { seq, from: None, .. } => Some(*seq),
            _ => None,
        }) && let Some(pending) = self.pending.take()
        {
            self.user_images.keep(seq, pending.images);
        }
        self.agents = agents_at(&blocks);
        self.blocks = blocks;
        self.starts = starts;
        self.texts = texts;
        self.turn_cost = turn_cost;
        self.cache = cache;
        if above > 0 {
            // The places from the last layout sit `above` blocks lower now.
            self.folds.splice(0..0, std::iter::repeat_n(Default::default(), above));
        }
        self.relayout();
        self.last_seq = last_seq;
        self.loaded = true;
        self.error = None;
    }
}

/// Where a sub-agent sits in the blocks: its row's index, its end's, and whether it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentAt {
    pub row: usize,
    pub end: usize,
    pub running: bool,
}

/// Every sub-agent in `blocks`, by its call.
fn agents_at(blocks: &[Block]) -> HashMap<String, AgentAt> {
    let mut agents: HashMap<String, AgentAt> = HashMap::new();
    for (ix, block) in blocks.iter().enumerate() {
        match block.inner() {
            Block::Agent(row) => {
                agents.insert(
                    row.call_id.clone(),
                    AgentAt {
                        row: ix,
                        end: ix,
                        running: row.running(),
                    },
                );
            }
            Block::AgentEnd(end) => {
                if let Some(at) = agents.get_mut(&end.call_id) {
                    at.end = ix;
                }
            }
            _ => {}
        }
    }
    agents
}

impl CodeView {
    /// Whether sub-agent `call`'s row is open: as the person left it, else collapsed (`AgentRow`'s `open ?? false`).
    pub fn agent_open(&self, call: &str) -> bool {
        self.open_agents.get(call).copied().unwrap_or(false)
    }

    /// Whether `block` shows: every sub-agent row above it is open.
    pub fn shows(&self, block: &Block) -> bool {
        block.path().iter().all(|call| self.agent_open(call))
    }

    /// Opens or folds sub-agent `call`'s row. Its row and everything under it are measured again.
    pub fn toggle_agent(&mut self, call: &str) {
        let open = !self.agent_open(call);
        self.open_agents.insert(call.to_string(), open);
        if let Some(at) = self.agents.get(call) {
            self.list.splice(at.row..at.end + 1, at.end + 1 - at.row);
        }
    }

    /// Take what the running sub-agents are still writing. True when it changed; their ends are measured again.
    pub fn stream_agents(&mut self, drafts: Vec<(String, String)>) -> bool {
        if drafts == self.agent_drafts {
            return false;
        }
        let old = std::mem::replace(&mut self.agent_drafts, drafts);
        let changed: HashSet<&str> = old
            .iter()
            .chain(&self.agent_drafts)
            .filter(|draft| !old.contains(draft) || !self.agent_drafts.contains(draft))
            .map(|(call, _)| call.as_str())
            .collect();
        for call in changed {
            if let Some(at) = self.agents.get(call) {
                self.list.remeasure_items(at.end..at.end + 1);
            }
        }
        true
    }

    /// What sub-agent `call` is still writing.
    pub fn agent_draft(&self, call: &str) -> Option<&str> {
        self.agent_drafts
            .iter()
            .find(|(of, _)| of == call)
            .map(|(_, text)| text.as_str())
    }
}

/// How many blocks at the start are as they were.
fn common_prefix(old: &[Block], new: &[Block]) -> usize {
    old.iter().zip(new).take_while(|(old, new)| old == new).count()
}

/// How many blocks at the end are as they were: they keep their heights.
fn common_suffix(old: &[Block], new: &[Block]) -> usize {
    old.iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(old, new)| old == new)
        .count()
}

/// How many blocks an older page put above the ones shown (`old`), now read as `new`. The first block shown can take
/// in older events, and the last can grow, so the blocks between them find where `old` sits in `new`.
fn older_blocks(old: &[Block], new: &[Block]) -> usize {
    let added = new.len().saturating_sub(old.len());
    let window = old.len().saturating_sub(2).min(4);
    if window == 0 {
        return added;
    }
    let inner = &old[1..1 + window];
    // The match nearest where it would be with nothing appended, in case a block repeats.
    (0..new.len())
        .filter(|&at| new.get(at + 1..at + 1 + window) == Some(inner))
        .min_by_key(|&at| at.abs_diff(added))
        .unwrap_or(added)
}

/// Keep parsed Markdown only for rows the virtual list has requested.
fn retain_texts(
    previous: &[Block],
    cached: &mut [Option<Entity<TextViewState>>],
    blocks: &[Block],
    cx: &mut App,
) -> Vec<Option<Entity<TextViewState>>> {
    let mut texts = Vec::with_capacity(blocks.len());
    for (ix, block) in blocks.iter().enumerate() {
        let Block::Text(markdown) = block.inner() else {
            texts.push(None);
            continue;
        };
        let kept = match (previous.get(ix).map(Block::inner), cached.get_mut(ix)) {
            (Some(Block::Text(old)), Some(view @ Some(_))) if old == markdown => view.take(),
            (Some(Block::Text(_)), Some(Some(view))) => {
                let view = view.clone();
                view.update(cx, |state, cx| state.set_text(markdown, cx));
                Some(view)
            }
            _ => None,
        };
        // Unseen answers stay unparsed until the virtual list requests their row.
        texts.push(kept);
    }
    texts
}

impl Workspace {
    /// Whether the open Code session's agent is working now: the composer shows Stop, and Esc stops it.
    pub(super) fn code_running(&self) -> bool {
        let View::Code(view) = &self.view else {
            return false;
        };
        self.runner.is_some()
            && self
                .projects
                .iter()
                .flat_map(|project| &project.sessions)
                .any(|session| session.id == view.id && session.status == Status::Running)
    }

    /// The messages sent into the running turn that the agent has not read yet, oldest first: they wait on the
    /// composer, "On queue", and join the transcript where the agent reads them. Only Claude Code and Codex say when
    /// they read a message, so a Nexus agent's stay in the transcript, as does a message a stopped session never read.
    pub(super) fn sent_on_queue(&self) -> &[nexus_workbench::queued_delivery::Message] {
        let View::Code(view) = &self.view else {
            return &[];
        };
        // A coding agent's session confirms; Nexus's own agent is no coding agent.
        let confirms = self
            .code_session()
            .is_some_and(|session| session.agent.coding().is_some());
        if confirms && self.code_running() {
            &view.queue.sending
        } else {
            &[]
        }
    }

    /// Message `seq` waits on the composer ([`Self::sent_on_queue`]), not in the transcript.
    fn waits_on_composer(&self, seq: i64) -> bool {
        self.sent_on_queue().iter().any(|message| message.seq == seq)
    }

    /// Send the composer's text to the open Code session, or start a new one with it. `queued`: while the session's turn
    /// runs, the message waits for it to end (a chip in the dock) instead of joining it.
    pub(super) fn submit_code(&mut self, prompt: String, queued: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_local()
            && matches!(self.view, View::NewCode)
            && let Some(id) = self.code_form.provider.clone()
            && let Some(runner) = self.runner.as_ref()
            && self.local_code_blocker().is_none()
            && !self.start_blocked()
        {
            let source = runner.source_guard();
            let signed_out = self.gateway.signed_out_guard();
            self.sync_draft(window, cx);
            let agent = self.code_form.agent;
            let project = self.code_form.project.clone();
            let model = self.code_form.model.clone();
            let text = self.composer.read(cx).value();
            let prepared = self.compose_code_prompt(&prompt);
            let attachment_ids: Vec<_> = self.draft.all().map(|item| item.id).collect();
            let server_model = self.code_form.selected_server_model().map(str::to_owned);
            let validation = crate::pages::local_providers::validate_for_launch(
                id.clone(),
                agent,
                server_model.clone(),
                move || source() && signed_out(),
                cx,
            );
            self.code_form.provider_validation = Some(cx.spawn_in(window, async move |this, cx| {
                let result = validation.await;
                if let Err(error) = this.update_in(cx, |this, window, cx| {
                    this.code_form.provider_validation = None;
                    this.sync_draft(window, cx);
                    if this.is_local()
                        && matches!(this.view, View::NewCode)
                        && this.code_form.provider.as_ref() == Some(&id)
                        && this.code_form.agent == agent
                        && this.code_form.project == project
                        && this.code_form.model == model
                        && this.code_form.selected_server_model() == server_model.as_deref()
                        && this.composer.read(cx).value() == text
                        && this.compose_code_prompt(&prompt) == prepared
                        && this.draft.all().map(|item| item.id).eq(attachment_ids)
                    {
                        match result {
                            Ok(()) => this.submit_code_checked(prompt, queued, window, cx),
                            Err(error) => {
                                this.code_form.require_provider_reselection(error);
                                this.load_local_provider_choices(cx);
                            }
                        }
                    }
                    cx.notify();
                }) {
                    eprintln!("nexus-desktop-gpui: provider validation closed: {error}");
                }
            }));
            cx.notify();
            return;
        }
        self.submit_code_checked(prompt, queued, window, cx);
    }

    fn compose_code_prompt(&self, text: &str) -> String {
        let prompt = super::stacked_replies::message(self.pending_replies(), text);
        nexus_workbench::diff_comment::prompt_text(self.pending_diff_comments(), &prompt)
    }

    fn submit_code_checked(&mut self, prompt: String, queued: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_handoff_blocker().is_some() || (self.is_local() && self.local_code_blocker().is_some()) {
            return;
        }
        self.sync_draft(window, cx);
        let submitted = self.draft_submission(cx);
        // Stacked replies go first, each passage quoted over its reply, then the composer's text (`stacked_replies.rs`).
        let prompt = self.compose_code_prompt(&prompt);
        if prompt.is_empty() && self.draft.is_empty() {
            return;
        }
        if self.attaching > 0 {
            return;
        }
        // A used-up budget holds the message: its line says why, and it sends once the budget is raised.
        if self.hold_for_limit(cx) {
            cx.notify();
            return;
        }
        // One send of files in pieces at a time, as the Nexus app's composer (`uploads.rs`).
        if self.uploading() {
            self.composer_note = Some("Files are still sending. Wait, or cancel them.".into());
            cx.notify();
            return;
        }
        if matches!(self.view, View::NewCode) && self.start_blocked() {
            // Plans off hold the message: their setup line says what to do, and it sends once the plan is picked.
            if self.hold_for_plans(cx) {
                cx.notify();
                return;
            }
            // A missing key holds the message: its setup line says what to do, and it sends once the key lands.
            if !self.hold_for_key(cx) {
                self.composer_note = Some("Resolve the session setup item above before sending.".into());
            }
            cx.notify();
            return;
        }
        self.note_setup_start();
        // A new cloud session starts on a machine of its own (`cloud_view.rs`).
        if matches!(self.view, View::NewCode) && self.cloud_ui.on {
            if self.code_form.starting.is_some() {
                self.composer_note = Some("The last session is still starting.".into());
                cx.notify();
                return;
            }
            self.cancel_dictation();
            if self.start_cloud(prompt, window, cx) {
                self.diff_comments.remove(super::diff_comments::NEW_CODE_DRAFT);
                self.save_diff_comments();
                self.composer_note = None;
                self.composer.update(cx, |state, cx| state.set_value("", window, cx));
                self.composer_empty = true;
                self.draft_submitted(submitted, window, cx);
            }
            cx.notify();
            return;
        }
        // A cloud session still starting: the message waits behind the first (`cloud_view.rs`).
        if self.starting_cloud().is_some() {
            if self.starting_cloud().is_some_and(|start| start.waiting.is_full()) {
                self.composer_note = Some(super::cloud_view::waiting_full_note().into());
                cx.notify();
                return;
            }
            self.cancel_dictation();
            self.send_while_starting(prompt);
            if let View::Code(view) = &self.view {
                let id = view.id.clone();
                self.replies_sent(&id);
            }
            self.composer_note = None;
            self.composer.update(cx, |state, cx| state.set_value("", window, cx));
            self.composer_empty = true;
            self.draft_submitted(submitted, window, cx);
            cx.notify();
            return;
        }
        // A server's session or project runs there; this computer's need this build's runner.
        let target = match &self.view {
            View::Code(view) => Some(view.id.clone()),
            View::NewCode => self.code_form.project.clone(),
            _ => None,
        };
        let runs = match &target {
            Some(id) => self.runs(id).is_some(),
            None => self.runner.is_some(),
        };
        if !runs {
            self.refuse_read_only();
            cx.notify();
            return;
        }
        if matches!(self.auth, super::Auth::SignedIn)
            && self.code_session().is_some_and(|session| session.saved_provider)
        {
            self.composer_note = Some("Sign out to continue on this provider".into());
            cx.notify();
            return;
        }
        let new = match &self.view {
            View::Code(view) if view.pending.is_some() => {
                self.composer_note = Some("The session is still starting.".into());
                cx.notify();
                return;
            }
            View::Code(view) => {
                let id = view.id.clone();
                if let Some(runs) = self.runs(&id) {
                    // To a sleeping cloud machine, the send wakes it and waits: the session shows as working.
                    self.sending_to_cloud(&id);
                    let mut draft = std::mem::take(&mut self.draft);
                    // Referenced artifacts go as a block in the text: the agent reads them with the `nexus` CLI.
                    let prompt = draft.take_artifacts(&prompt, self.gateway.url());
                    self.begin_upload(&mut draft, &prompt, window, cx);
                    runs.send(id.clone(), prompt, draft, queued);
                    self.diff_comments.remove(&id);
                    self.save_diff_comments();
                    self.replies_sent(&id);
                }
                None
            }
            View::NewCode => {
                if self.code_form.starting.is_some() {
                    self.composer_note = Some("The last session is still starting.".into());
                    cx.notify();
                    return;
                }
                let Some(project_id) = self.code_form.project.clone() else {
                    // The project menu opens as the way on, and the note links to it (`place_links.rs`).
                    self.open_project_menu(window, cx);
                    self.composer_note = Some(super::place_links::NO_PROJECT.into());
                    cx.notify();
                    return;
                };
                if self.code_form.review.on && self.code_form.review.number.is_none() {
                    self.composer_note = Some("Choose the pull request to review, under Review a pull request.".into());
                    cx.notify();
                    return;
                }
                let mut attachments = std::mem::take(&mut self.draft);
                let prompt = attachments.take_artifacts(&prompt, self.gateway.url());
                self.begin_upload(&mut attachments, &prompt, window, cx);
                Some(self.new_session(project_id, prompt, attachments))
            }
            _ => return,
        };
        self.cancel_dictation();
        if let Some(new) = new {
            self.replace_diff_comments(Vec::new());
            self.start_pending(new, submitted.as_ref(), window, cx);
        }
        self.upload_view();
        self.composer_note = None;
        self.composer.update(cx, |state, cx| state.set_value("", window, cx));
        self.composer_empty = true;
        self.draft_submitted(submitted, window, cx);
        cx.notify();
    }

    /// Open the session view at once for a new session, its message showing, and have the runner make it.
    fn start_pending(
        &mut self,
        new: NewSession,
        submitted: Option<&super::drafts::Submission>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(runs) = self.runs(&new.project_id) else {
            return;
        };
        self.begin_launch_motion(cx);
        let title = new.text.lines().next().unwrap_or("Code session").to_string();
        let view = CodeView::pending(new.clone(), window, cx);
        if self.is_local() {
            let provider_start = submitted.filter(|_| new.provider.is_some()).map(|submission| {
                super::local_mode::PendingProviderStart {
                    project: new.project_id.clone(),
                    submission: submission.clone(),
                }
            });
            self.local_mode.starts.insert(view.id.clone(), provider_start);
        }
        self.code_form.starting = Some(view.id.clone());
        // The organization's API for one session: the next one asks again.
        self.code_form.api_once = false;
        runs.create(view.id.clone(), new);
        let launch_motion = self.launch_motion.clone();
        self.reset_chat();
        self.launch_motion = launch_motion;
        self.title = title.into();
        self.view = View::Code(view);
        self.scroll.scroll_to_bottom();
    }

    /// Make the session that failed to start again, from the same choices and message.
    pub(super) fn retry_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A cloud session starts again with the same request id: the same machine, from the step that failed. Its
        // line can show a failed step before its start has stopped: then the start goes on as it is.
        if self.code_form.starting.is_some() && self.starting_cloud().is_some() {
            return;
        }
        let cloud = match &mut self.view {
            View::Code(view) => view.pending.as_mut().and_then(|pending| {
                let start = pending.cloud.clone()?;
                pending.failed = None;
                Some((view.id.clone(), start))
            }),
            _ => None,
        };
        if let Some((draft, start)) = cloud {
            if let View::Code(view) = &mut self.view {
                view.working_since = Some(Instant::now());
            }
            self.run_cloud_start(draft, start, window, cx);
            cx.notify();
            return;
        }
        let project = match &self.view {
            View::Code(view) => view.pending.as_ref().map(|pending| pending.new.project_id.clone()),
            _ => None,
        };
        let Some(runs) = project.and_then(|project| self.runs(&project)) else {
            return;
        };
        let local = self.is_local();
        let View::Code(view) = &mut self.view else {
            return;
        };
        let Some(pending) = &mut view.pending else {
            return;
        };
        if local {
            self.local_mode.starts.insert(view.id.clone(), None);
        }
        pending.failed = None;
        view.working_since = Some(Instant::now());
        self.code_form.starting = Some(view.id.clone());
        runs.create(view.id.clone(), pending.new.clone());
        cx.notify();
    }

    /// Back to New session with the message that failed to start in the composer, to change it. A cloud session's
    /// messages that waited behind it come back too, after it.
    pub(super) fn edit_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else {
            return;
        };
        let Some(mut pending) = view.pending.take() else {
            return;
        };
        self.open_new_code_form(window, cx);
        if let Some(start) = &pending.cloud {
            for message in start.waiting.take_all() {
                pending.new.text = [pending.new.text.as_str(), message.text.as_str()]
                    .into_iter()
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                pending.new.attachments.images.extend(message.attachments.images);
                pending.new.attachments.files.extend(message.attachments.files);
            }
            start.waiting.end();
            // Back on Cloud, with the same repository.
            self.cloud_ui.on = true;
            self.cloud_ui.repo = Some(crate::cloud::CloudRepo {
                name: start.repo.clone(),
                url: start.repo.clone(),
                private: false,
                pushed_at: None,
            });
        } else {
            self.code_form.project = Some(pending.new.project_id);
        }
        // The text goes back into its project's draft, not the one open_new_code showed.
        self.sync_draft(window, cx);
        self.draft = pending.new.attachments;
        let text = pending.new.text;
        self.composer
            .update(cx, |state, cx| state.set_value(text.clone(), window, cx));
        self.composer_empty = text.trim().is_empty();
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// The session was made: the draft view takes its id in place, so nothing reopens.
    pub(super) fn pending_created(&mut self, draft: &str, id: &str) -> bool {
        let View::Code(view) = &mut self.view else {
            return false;
        };
        if !view.is_draft() || view.id != draft {
            return false;
        }
        view.id = id.to_string();
        // A session started here: its first output counts, as the Nexus app's launched session.
        view.auto_browser = super::auto_preview::AutoBrowser::new(true);
        view.navigation_fresh = true;
        let moved = view.pending.as_mut().and_then(|pending| pending.moved_from.take());
        if let (Some(moved), Some(runner)) = (moved, &self.runner) {
            for call in moved_on(id, moved) {
                match call {
                    MovedOnCall::Title(args) => runner.update_session(args),
                    MovedOnCall::Stop {
                        thread,
                        request,
                        message,
                    } => runner.approve(thread, request, Decision::Deny, Some(message), None, None),
                }
            }
        }
        self.visit(Place::Code(id.to_string()));
        true
    }

    /// The session could not be made: the draft view says why, if it is open.
    pub(super) fn pending_failed(&mut self, draft: &str, error: &str) -> bool {
        let View::Code(view) = &mut self.view else {
            return false;
        };
        if view.id != draft {
            return false;
        }
        let Some(pending) = &mut view.pending else {
            return false;
        };
        pending.failed = Some(error.to_string().into());
        view.working_since = None;
        true
    }

    /// Read the open session's page before the first event shown, once for each first event.
    pub(super) fn read_older_code(&mut self, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        if !view.more || view.asked_before == view.start {
            return;
        }
        view.asked_before = view.start;
        view.older = true;
        self.read_code_transcript(cx);
    }

    /// Keep the working line's start: set while the session starts or its agent works, cleared once it stops.
    pub(super) fn sync_working(&mut self) {
        self.working_times.retain(|id| {
            self.projects
                .iter()
                .flat_map(|project| &project.sessions)
                .any(|session| session.id == id && matches!(session.status, Status::Running | Status::NeedsYou))
        });
        let running = self.code_running();
        let delivering = matches!(&self.view, View::Code(view) if self.cloud_delivering(&view.id));
        let View::Code(view) = &mut self.view else {
            return;
        };
        let starting = view.pending.as_ref().is_some_and(|pending| pending.failed.is_none());
        if running && !view.is_draft() {
            let previous_turn = view
                .blocks
                .iter()
                .rev()
                .find_map(|block| match block {
                    Block::Turn { seq, .. } => Some(*seq),
                    _ => None,
                })
                .or_else(|| (view.loaded && !view.more).then_some(0));
            view.working_since = Some(self.working_times.since(
                &view.id,
                previous_turn,
                view.working_since.unwrap_or_else(Instant::now),
                Instant::now(),
            ));
        } else if starting || delivering {
            view.working_since.get_or_insert_with(Instant::now);
        } else {
            view.working_since = None;
        }
    }

    /// Why the Code sessions are read-only here, as text: for a form or a panel. Over the composer, the read-only setup
    /// line says it with its fix (`read_only.rs`).
    pub(super) fn read_only_note(&self) -> SharedString {
        self.owner
            .note()
            .map(SharedString::from)
            .unwrap_or_else(|| "Code sessions open here once you're signed in to an organization.".into())
    }

    pub(super) fn render_code(&self, view: &CodeView, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.code_error.is_some()
            && self.code.is_none()
            && !view.is_draft()
            && crate::machines::get().machine_of(&view.id).is_none()
        {
            return self.render_code_store_error(palette, cx).into_any_element();
        }
        let connecting = self.servers.iter().any(|server| {
            matches!(
                server.status,
                nexus_remote::machine::Status::Connecting { .. } | nexus_remote::machine::Status::Updating
            )
        });
        if missing_session(
            self.code_overview_loaded && self.code_error.is_none(),
            self.session_by_id(&view.id).is_some(),
            view.is_draft(),
            // A cloud session is known by its machine even before its log is read (its machine may sleep).
            connecting || self.machine_coming(view) || self.cloud_machine_of_session(&view.id).is_some(),
        ) {
            let offline = self
                .servers
                .iter()
                .any(|server| !matches!(server.status, nexus_remote::machine::Status::Connected { .. }));
            return v_flex().flex_1().size_full().items_center().justify_center().gap(px(16.))
                .child(div().text_size(px(24.)).text_color(palette.foreground).child("This session isn't here"))
                .child(div().max_w(px(460.)).text_center().text_size(px(14.)).text_color(palette.muted).child(if offline {
                    "A server is offline right now, and this session may be on it. It will show up here once the server is back."
                } else {
                    "It belongs to another organization, it ended when Nexus quit, or it is on a server you removed."
                }))
                .child(div().id("missing-session-new").cursor_pointer().text_size(px(14.)).child("Start a new session")
                    .on_click(cx.listener(|this, _, window, cx| this.open_new_code(window, cx))))
                .into_any_element();
        }
        // Scrolled up from the end: the list stopped following it.
        let away = view.loaded && !view.list.is_following_tail();
        // Near the top of a session with older events: read the page before (`timeline-view.tsx`'s `useOlderPages`).
        if away && view.more && view.asked_before != view.start && view.list.logical_scroll_top().item_ix < OLDER_MARGIN
        {
            let this = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = this.update(cx, |this, cx| this.read_older_code(cx));
            });
        }
        v_flex()
            .relative()
            // The home leaving under a session just started (`launch_motion.rs`).
            .children(self.render_launch_exit(palette, cx))
            .child(view.dock.tray_layout.measure(false, cx))
            .capture_any_mouse_down(cx.listener(|this, event, window, cx| this.cite_pointer_down(event, window, cx)))
            .capture_any_mouse_up(cx.listener(|this, event, window, cx| this.cite_pointer_up(event, window, cx)))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.cite_pointer_up(event, window, cx)),
            )
            .capture_key_down(cx.listener(|this, event, window, cx| this.cite_key(event, window, cx)))
            .child(self.render_cite(palette, cx))
            // The reply box over a passage, while it is open (`stacked_replies.rs`).
            .children(self.render_reply_box(palette, cx))
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .h_full()
            // The transcript is a cached view of its own (`pane.rs`): a caret blink in the composer leaves it be.
            .child(self.render_transcript_pane(super::pane::Region::CodeTranscript))
            // Reserve the whole dock below the transcript, including workbench notices.
            .child(
                div()
                    .flex_shrink_0()
                    .w_full()
                    .max_w(px(self.composer_width()))
                    .mx_auto()
                    .px(px(24.))
                    .pb(px(28.))
                    .child(self.render_code_dock(view, palette, cx)),
            )
            .when(
                view.dock.permissions.reviewing && !view.dock.permissions.pending(&view.blocks).is_empty(),
                |area| {
                    area.child(
                        div()
                            .absolute()
                            .inset_0()
                            .bg(crate::frost::cover(&palette))
                            .p(px(12.))
                            .children(self.render_permission_queue(
                                view,
                                palette,
                                view.dock.tray_layout.pane_height() - 24.,
                                cx,
                            )),
                    )
                },
            )
            .into_any_element()
    }

    /// A session's transcript: its list, the fade under the title, the minimap, the find bar and the jump to the
    /// latest. Drawn in its own pane (`pane.rs`).
    pub(super) fn render_code_scroll(&self, view: &CodeView, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        // Scrolled up from the end: the list stopped following it.
        let away = view.loaded && !view.list.is_following_tail();
        let state = view.list.clone();
        div()
            .relative()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .child(
                list(
                    view.list.clone(),
                    cx.processor(|this: &mut Self, ix: usize, window, cx| {
                        let palette = Palette::for_choice(this.theme, window.appearance());
                        this.render_code_item(ix, palette, window, cx)
                    }),
                )
                .size_full(),
            )
            .child(transcript::top_fade(palette))
            .child(self.render_code_minimap(view, palette, cx))
            .when(view.find.open, |area| {
                area.child(self.render_code_find_bar(view, palette, cx))
            })
            .when_some(motion::presence("code-jump", away, cx), |area, phase| {
                area.child(transcript::over_dock(px(0.), render_jump(state, phase, palette, cx)))
            })
            .into_any_element()
    }

    /// The minimap beside the transcript (`minimap.rs`): a tick per message and per answer.
    fn render_code_minimap(&self, view: &CodeView, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = minimap_rows(&view.blocks);
        let map = Minimap {
            id: "code-minimap",
            list: view.list.clone(),
            rows: rows.clone(),
            end: view.blocks.len(),
            column: self.column_width(),
            state: &view.minimap,
            slot: code_minimap,
        };
        self.render_minimap(map, &|row| minimap_preview(&view.blocks, &rows, row), palette, cx)
    }

    /// One item of the transcript's list, in the centered column: a block, or the tail after the last one.
    fn render_code_item(
        &mut self,
        ix: usize,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let View::Code(view) = &mut self.view else {
            return div().into_any_element();
        };
        if let Some(Block::Text(markdown)) = view.blocks.get(ix).map(Block::inner)
            && let Some(text) = view.texts.get_mut(ix)
            && text.is_none()
        {
            let entity = cx.new(|cx| TextViewState::markdown(markdown, cx));
            view.cite
                .observers
                .push(cx.observe_in(&entity, window, |this, _, window, cx| this.refresh_cite(window, cx)));
            *text = Some(entity);
        }
        self.mark_code_text(ix, cx);
        let media = self.code_item_media(ix, palette, cx);
        let images = self.code_user_images(ix, palette, cx);
        let View::Code(view) = &self.view else { unreachable!() };
        let entry = view.blocks.get(ix);
        // A folded sub-agent hides all its nested rows, including work-fold headers.
        if entry.is_some_and(|block| !view.shows(block)) {
            return div().into_any_element();
        }
        let agent_depth = entry.map_or(0, |block| block.path().len());
        let width = self.column_width();
        let make_column = || v_flex().w_full().max_w(px(width)).mx_auto().px(px(24.));
        // Its run's card header and fold row, then its own row unless its card or fold is closed (`code_folds.rs`).
        let place = view.folds.get(ix).cloned().unwrap_or_default();
        let lead = self.fold_lead(&place, palette, cx);
        let View::Code(view) = &self.view else { unreachable!() };
        let lead = (!lead.is_empty()).then(|| make_column().pt(px(if ix == 0 { 28. } else { 14. })).children(lead));
        if place.hidden {
            return match lead {
                Some(lead) => div().w_full().child(lead).into_any_element(),
                None => div().into_any_element(),
            };
        }
        // Under a header the row follows closely; in an open card or fold it sits in.
        let first = ix == 0 && lead.is_none();
        // Steps in an open card stack on its rail with no gap between them.
        let rail = place.depth > 0;
        let top = px(match (lead.is_some(), first) {
            (true, _) => 4.,
            (false, true) => 28.,
            (false, false) if rail => 0.,
            (false, false) if agent_depth > 0 => 4.,
            (false, false) => 14.,
        });
        let indent = super::code_folds::INDENT;
        let column = if rail {
            // The rail: a 1px line 6.5 in from the head's chevron, the rows 15 past it.
            let depth = agent_depth + usize::from(place.depth) - 1;
            v_flex()
                .w_full()
                .ml(px(indent * depth as f32 + 6.5))
                .pl(px(15.))
                .border_l_1()
                .border_color(palette.border)
        } else {
            match agent_depth {
                0 => make_column(),
                depth => make_column().pl(px(24. + indent * depth as f32)),
            }
        };
        let item = match view.blocks.get(ix).map(Block::inner) {
            Some(Block::Setup(row)) => column
                .pt(px(if first { 28. } else { 14. }))
                .child(self.render_setup_row(ix, row, palette, cx)),
            // Plumbing (the agent loading a tool) takes no row, and no gap.
            Some(Block::Tool { name, .. }) if hidden_tool(name) => return div().into_any_element(),
            Some(Block::Agent(row)) => {
                column
                    .pt(top)
                    .child(self.render_agent(ix, row, view.agent_open(&row.call_id), palette, cx))
            }
            Some(Block::AgentEnd(end)) => {
                column
                    .pt(px(4.))
                    .child(render_agent_end(end, view.agent_draft(&end.call_id), palette))
            }
            // A linked session's message: attributed to it, never drawn as the person's (`session-message.tsx`).
            Some(Block::User {
                seq,
                text,
                from: Some(from),
                ..
            }) => column
                .pt(px(if first { 28. } else { 14. }))
                .child(self.render_session_message(*seq, text, from, palette, cx)),
            Some(Block::Thinking(text, ms)) => {
                let open = view.thoughts_open.contains(&(ix as isize - view.shifted as isize));
                column.pt(top).child(self.thought_row(ix, text, *ms, open, palette, cx))
            }
            // Sent into the turn and not read yet, the message waits on the composer, "On queue" (`code_dock.rs`).
            Some(Block::User { seq, from: None, .. }) if self.waits_on_composer(*seq) => {
                return div().into_any_element();
            }
            Some(Block::User {
                seq,
                at,
                text,
                from: None,
                rewound,
                ..
            }) => {
                let message = self
                    .render_user(
                        UserMessage {
                            seq: *seq,
                            at: *at,
                            text,
                            linked: false,
                            rewound: *rewound,
                            marks: view.find_marks(ix, text),
                        },
                        images,
                        palette,
                        cx,
                    )
                    .into_any_element();
                // The session's own first message, landing while the start still moves, takes over where the draft's
                // was, on the same clock (`launch_motion.rs`).
                let message = if ix == 0 {
                    self.launch_part(super::launch_motion::Part::Message, message)
                } else {
                    message
                };
                column.pt(top).child(message)
            }
            Some(Block::Turn {
                seq,
                at,
                status,
                duration_ms,
                cost_usd,
                tokens,
                model,
                reverted,
            }) => {
                let turn = TurnFooter {
                    seq: *seq,
                    at: *at,
                    status: *status,
                    duration_ms: *duration_ms,
                    cost_usd: *cost_usd,
                    tokens: *tokens,
                    model: model.clone(),
                    reverted: *reverted,
                    answer: turn_answer(&view.blocks, ix),
                };
                column
                    .pt(px(if first { 28. } else { 4. }))
                    .child(self.render_turn(turn, palette, cx))
            }
            Some(Block::Tool {
                call_id,
                name,
                summary,
                error: Some(error),
                ..
            }) => column.pt(top).child(self.render_failed_tool(
                ix,
                call_id,
                name,
                summary,
                error,
                view.open_errors.contains(call_id),
                palette,
                cx,
            )),
            Some(Block::Page { page_id, title, .. }) => {
                column.child(self.render_code_page(ix, page_id, title.as_deref(), palette, cx))
            }
            Some(Block::Tool {
                name, summary, call, ..
            }) => column
                .pt(top)
                .child(self.render_tool(ix, name, summary, call, palette, cx)),
            Some(Block::Error(error, detail)) if crate::machine_view::needs_nexus_sign_in(error) => column
                .pt(top)
                .child(self.render_sign_in_error(ix, error, detail.as_deref(), palette, cx)),
            // The agent is missing or signed out: setup, not a failure.
            Some(Block::Error(error, _)) if crate::agents::setup::gap(error).is_some() => {
                let gap = crate::agents::setup::gap(error).unwrap_or(crate::agents::setup::Gap::SignedOut);
                column.pt(top).child(self.render_agent_error(gap, palette, cx))
            }
            Some(block) => column.pt(top).child(render_block(
                ix,
                block,
                view.texts.get(ix).cloned().flatten(),
                matches!(block, Block::Text(_))
                    .then(|| self.run_command(&view.id, cx))
                    .flatten(),
                Some(view.list.clone()),
                palette,
                self.context_link_handler(|href, _, _, cx| crate::external_link::open(href, cx), cx),
                self.file_chips(palette, cx),
            )),
            None => match self.render_code_tail(view, palette, cx) {
                // The dock reserves its own space below the list.
                Some(tail) => column.pt(top).pb(px(36.)).child(tail),
                None => column.pb(px(36.)),
            },
        };
        // Under a message, the row of links to what its answer made (`answer_links.rs`).
        let item = item.children(self.render_answer_links(ix, palette, cx));
        let item = if rail { make_column().child(item) } else { item };
        // A block a message search hit opened at lights up for a moment (`code_jump.rs`).
        let lit = self.jump_lit(ix);
        let turn = turn_of(&view.blocks, ix);
        div()
            .id(("code-row", ix))
            .w_full()
            .when_some(turn, |row, seq| {
                row.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if let View::Code(view) = &mut this.view {
                        if *hovered {
                            view.hovered_turn = Some((ix, seq));
                        } else if view.hovered_turn.is_some_and(|(row, _)| row == ix) {
                            view.hovered_turn = None;
                        }
                        cx.notify();
                    }
                }))
            })
            .when(lit, |row| row.bg(palette.selected))
            .children(lead)
            .child(item.children(media))
            .into_any_element()
    }

    /// A message the person sent, with its time and, on hover, Rewind, Edit and Copy (`user-message.tsx`), Rewind's
    /// confirmation once asked, or the editor in its place. A message a rewind took back shows dimmed, with Copy only.
    fn render_user(
        &self,
        message: UserMessage,
        images: Option<AnyElement>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let View::Code(view) = &self.view else {
            return div().into_any_element();
        };
        let UserMessage {
            seq,
            at,
            text,
            linked,
            rewound,
            marks,
        } = message;
        let copied = view.copied == Some(seq);
        let asking = view.rewinding.as_ref().filter(|rewinding| rewinding.seq == seq);
        let editing = view.editing.as_ref().filter(|editing| editing.seq == seq);
        let actionable = can_act_on(self.runner.is_some(), linked, rewound, view.pending.is_some());
        let running = self.code_running();
        let action = |id: (&'static str, u64), icon: Lucide, tip: &'static str| {
            div()
                .id(id)
                .size(px(24.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                .child(glyph(icon, palette.faint))
        };
        let live = |button: Stateful<Div>| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .active(|style| style.opacity(0.7))
        };
        let copy = {
            let text = text.to_string();
            live(action(
                ("user-copy", seq as u64),
                if copied { Lucide::Check } else { Lucide::Copy },
                if copied { "Copied" } else { "Copy your message" },
            ))
            .on_click(cx.listener(move |this, _, _, cx| this.copy_message(seq, text.clone(), cx)))
        };
        let rewind = (actionable && editing.is_none()).then(|| {
            let button = action(
                ("user-rewind", seq as u64),
                Lucide::Undo2,
                if running {
                    "Stop the agent before rewinding"
                } else {
                    "Rewind to this message"
                },
            );
            if running {
                button.opacity(0.4)
            } else {
                live(button).on_click(cx.listener(move |this, _, _, cx| this.ask_rewind(seq, cx)))
            }
        });
        let edit = (actionable && editing.is_none()).then(|| {
            let button = action(
                ("user-edit", seq as u64),
                Lucide::Pencil,
                if running {
                    "Stop the agent before editing"
                } else {
                    "Edit this message"
                },
            );
            if running {
                button.opacity(0.4)
            } else {
                let text = text.to_string();
                live(button)
                    .on_click(cx.listener(move |this, _, window, cx| this.start_edit(seq, text.clone(), window, cx)))
            }
        });
        // The time and the actions show on hover, and stay while "Copied" or the confirmation shows. The time keeps its
        // place while hidden, so the actions do not move when it shows.
        let keep = copied || asking.is_some();
        let shown = view.hovered_message == Some(seq) || keep;
        let time = (at > 0).then(|| {
            let exact: SharedString = code::message_time_exact(at).into();
            div()
                .id(("user-time", seq as u64))
                .text_size(px(12.))
                .text_color(palette.faint)
                .whitespace_nowrap()
                .when(!shown, |time| time.invisible())
                .tooltip(move |window, cx| Tooltip::new(exact.clone()).build(window, cx))
                .child(code::message_time(at, code::now_ms()))
        });
        let agent = self
            .code_session()
            .map_or("The agent", |session| nexus_workbench::driver::label(session.agent));
        // Under the bubble, at its right: one check once sent, two once the agent read it. The hover spells it out.
        let dropped = matches!(view.queue.delivery.get(&seq), Some(Delivery::Dropped));
        let receipt = (!rewound && !linked && !dropped && editing.is_none())
            .then(|| receipt_of(seq, &view.reads, &view.blocks))
            .map(|receipt| render_receipt(seq, receipt, agent, shown, palette));
        let frame =
            view.message_actions
                .borrow_mut()
                .entry(seq)
                .or_default()
                .update(shown, Instant::now(), cx.reduce_motion());
        let count = 1 + usize::from(rewind.is_some()) + usize::from(edit.is_some());
        let actions = h_flex().gap(px(2.)).children(rewind).children(edit).child(copy);
        v_flex()
            .id(("user-message", seq as u64))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if let View::Code(view) = &mut this.view {
                    if *hovered {
                        view.hovered_message = Some(seq);
                    } else if view.hovered_message == Some(seq) {
                        view.hovered_message = None;
                    }
                    cx.notify();
                }
            }))
            .w_full()
            .items_end()
            .gap(px(4.))
            .pt(px(10.))
            .map(|column| {
                let content = match editing {
                    Some(editing) => v_flex()
                        .w_full()
                        .gap(px(8.))
                        .children(images)
                        .child(render_editor(editing, running, palette, cx)),
                    None => user_prompt(text.to_string(), marks, images, palette, Some(self.artifact_opener(cx))),
                };
                column.child(div().w_full().when(rewound, |row| row.opacity(0.5)).child(content))
            })
            .child(
                // Keep the footer's height stable while its hidden actions take no horizontal space.
                h_flex()
                    .h(px(24.))
                    .items_center()
                    .children(time)
                    .child(super::message_actions::render(seq, actions, count, at > 0, frame))
                    .children(receipt),
            )
            // Not sent: the turn stopped before the agent read it. Waiting or on its way, it is on the composer instead.
            .when(
                matches!(view.queue.delivery.get(&seq), Some(Delivery::Dropped)) && !rewound && !linked,
                |column| {
                    column.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(Delivery::Dropped.note(running, agent, |_| String::new())),
                    )
                },
            )
            .when_some(asking, |column, rewinding| {
                column.child(render_rewind_confirm(rewinding, palette, cx))
            })
            .into_any_element()
    }

    /// What Run on a shell block of session `id` does: run it in the session's Commands (`run_in_side_terminal`). A
    /// server session's Commands run on the server ([`run_offered`]); a cloud session has no Run.
    fn run_command(&self, id: &str, cx: &Context<Self>) -> Option<transcript::RunCommand> {
        let machines = crate::machines::get();
        let offered = run_offered(
            machines.machine_of(id).as_deref(),
            |machine| machines.is_cloud(machine),
            |machine| machines.has_feature(machine, nexus_workbench::terminal::FEATURE),
        );
        if !offered {
            return None;
        }
        let workspace = cx.weak_entity();
        Some(std::sync::Arc::new(move |command, window, cx| {
            let _ = workspace.update(cx, |this, cx| this.run_in_side_terminal(&command, None, window, cx));
        }))
    }

    /// Copy a message's text, and show a check on its Copy for a moment.
    pub(super) fn copy_message(&mut self, seq: i64, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let View::Code(view) = &mut self.view else {
            return;
        };
        view.copied = Some(seq);
        view.copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = this.update(cx, |this, cx| {
                if let View::Code(view) = &mut this.view
                    && view.copied == Some(seq)
                {
                    view.copied = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// Ask to rewind to before message `seq`: its confirmation shows under it.
    fn ask_rewind(&mut self, seq: i64, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else {
            return;
        };
        view.rewinding = Some(Rewinding {
            seq,
            busy: false,
            error: None,
        });
        cx.notify();
    }

    fn cancel_rewind(&mut self, cx: &mut Context<Self>) {
        if let View::Code(view) = &mut self.view
            && !view.rewinding.as_ref().is_some_and(|rewinding| rewinding.busy)
        {
            view.rewinding = None;
            cx.notify();
        }
    }

    /// Rewind the conversation to before the message being confirmed (`user-message.tsx`'s `rewind(false)`): the
    /// later messages stay, dimmed, and the message goes back to the composer. Files stay as they are.
    fn confirm_rewind(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(runner) = &self.runner else {
            return;
        };
        let View::Code(view) = &mut self.view else {
            return;
        };
        let Some(rewinding) = view.rewinding.as_mut().filter(|rewinding| !rewinding.busy) else {
            return;
        };
        let seq = rewinding.seq;
        // The message as it reads now, before the rewind dims it.
        let Some(prompt) = view.blocks.iter().find_map(|block| match block {
            Block::User {
                seq: at,
                text,
                image_data,
                ..
            } if *at == seq => Some((text.clone(), image_data.clone())),
            _ => None,
        }) else {
            return;
        };
        rewinding.busy = true;
        rewinding.error = None;
        let thread = view.id.clone();
        let rewound = runner.rewind(thread.clone(), seq);
        cx.spawn_in(window, async move |this, cx| {
            let answer = rewound
                .await
                .unwrap_or_else(|_| Err("The rewind was cancelled.".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                let View::Code(view) = &mut this.view else {
                    return;
                };
                if view.id != thread {
                    return;
                }
                match answer {
                    Ok(()) => {
                        view.rewinding = None;
                        this.put_back(&prompt.0, window, cx);
                        crate::attachments::append_restored(
                            &mut this.draft,
                            crate::attachments::restored_images(&thread, seq, &prompt.1),
                        );
                        this.refresh_code(cx);
                    }
                    Err(error) => {
                        if let Some(rewinding) = &mut view.rewinding {
                            rewinding.busy = false;
                            rewinding.error = Some(error.into());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Put a message back in the composer, under anything already typed (`edit-prompt.ts`'s `withRestored`).
    pub(super) fn put_back(&mut self, prompt: &str, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.composer.read(cx).value().to_string();
        let text = restored_draft(&draft, prompt);
        self.composer
            .update(cx, |state, cx| state.set_value(text.clone(), window, cx));
        self.composer_empty = text.trim().is_empty();
        self.focus_composer(window, cx);
    }

    /// Edit message `seq` in place: its text goes in an editor where the message was.
    fn start_edit(&mut self, seq: i64, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_running() {
            return;
        }
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 12));
        input.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        // Ctrl+Enter (Cmd+Enter on a Mac) sends, as the Nexus app's editor; Enter is a new line.
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = event {
                this.send_edit(window, cx);
            }
        });
        let View::Code(view) = &mut self.view else {
            return;
        };
        let images = view
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::User {
                    seq: at, image_data, ..
                } if *at == seq => Some(image_data.clone()),
                _ => None,
            })
            .unwrap_or_default();
        view.rewinding = None;
        view.editing = Some(Editing {
            seq,
            input,
            images,
            busy: false,
            error: None,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let View::Code(view) = &mut self.view
            && !view.editing.as_ref().is_some_and(|editing| editing.busy)
        {
            view.editing = None;
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    /// Send the edited message (`user-message.tsx`'s `rewind(true)`): rewind to before it, then send the new text.
    /// The later messages stay, dimmed; files stay as they are. A failed rewind keeps the editor open with the error.
    fn send_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_running() {
            return;
        }
        let View::Code(view) = &mut self.view else {
            return;
        };
        let Some(editing) = view.editing.as_mut().filter(|editing| !editing.busy) else {
            return;
        };
        let text = editing.input.read(cx).value().to_string();
        let images = editing.images.clone();
        let Some([EditCall::Rewind { thread, seq }, EditCall::Send { text, .. }]) =
            edit_calls(&view.id, editing.seq, &text, images.len())
        else {
            return;
        };
        editing.busy = true;
        editing.error = None;
        let Some(runs) = self.runs(&thread) else {
            return;
        };
        let rewind = runs.call("wb_rewind_message", serde_json::json!({"threadId": thread, "seq": seq}));
        cx.spawn_in(window, async move |this, cx| {
            let rewind = rewind.await.unwrap_or_else(|_| Err("The rewind was cancelled.".into()));
            let rewound = rewind.is_ok();
            let answer = match rewind {
                Ok(_) => runs
                    .call(
                        "wb_send",
                        serde_json::json!({"threadId": thread, "text": text, "images": images}),
                    )
                    .await
                    .unwrap_or_else(|_| Err("The edited message's answer was lost.".into())),
                Err(error) => Err(error),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let View::Code(view) = &mut this.view else {
                    return;
                };
                if view.id != thread {
                    return;
                }
                match answer {
                    Ok(_) => {
                        view.editing = None;
                        this.refresh_code(cx);
                        this.focus_composer(window, cx);
                    }
                    Err(error) if rewound => {
                        view.editing = None;
                        this.put_back(&text, window, cx);
                        crate::attachments::append_restored(
                            &mut this.draft,
                            crate::attachments::restored_images(&thread, seq, &images),
                        );
                        this.composer_note = Some(error.into());
                        this.refresh_code(cx);
                    }
                    Err(error) => {
                        if let Some(editing) = &mut view.editing {
                            editing.busy = false;
                            editing.error = Some(error.into());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Whether "Restore to here" shows on the open session's turns (`session.tsx`'s `revertable`): this build runs
    /// it, nothing runs now, and its turns' files are recorded (a worktree, or a project that is a repository).
    fn can_restore(&self) -> bool {
        let View::Code(view) = &self.view else {
            return false;
        };
        let Some(session) = self.code_session() else {
            return false;
        };
        let repository = session.branch.is_some()
            || self
                .projects
                .iter()
                .any(|project| project.id == session.project_id && project.branch.is_some());
        self.runner.is_some() && view.pending.is_none() && !self.code_running() && repository
    }

    /// Ask "Restore to here" on the turn ending at event `turn`: its confirmation opens and counts the files.
    fn ask_restore(&mut self, turn: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(runner) = self.runner.clone() else {
            return;
        };
        let View::Code(view) = &mut self.view else {
            return;
        };
        if view
            .restoring
            .as_ref()
            .is_some_and(|restoring| matches!(restoring.step, RestoreStep::Restoring(_)))
        {
            return;
        }
        let ask = view.restoring.as_ref().map_or(0, |restoring| restoring.ask) + 1;
        view.restoring = Some(Restoring {
            turn,
            step: RestoreStep::Counting,
            ask,
        });
        let thread = view.id.clone();
        let counted = runner.revert_preview(thread.clone(), turn);
        cx.spawn_in(window, async move |this, cx| {
            let answer = counted.await.unwrap_or_else(|_| Err("The count was cancelled.".into()));
            let _ = this.update(cx, |this, cx| {
                if let View::Code(view) = &mut this.view
                    && view.id == thread
                    && let Some(restoring) = view.restoring.as_mut().filter(|restoring| restoring.ask == ask)
                {
                    restoring.step = match answer {
                        Ok(files) => RestoreStep::Ready(files),
                        Err(error) => RestoreStep::Failed(error.into()),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn cancel_restore(&mut self, cx: &mut Context<Self>) {
        if let View::Code(view) = &mut self.view
            && !view
                .restoring
                .as_ref()
                .is_some_and(|restoring| matches!(restoring.step, RestoreStep::Restoring(_)))
        {
            view.restoring = None;
            cx.notify();
        }
    }

    /// Put the files back as they were after the turn being confirmed (`revert-turn.tsx`'s `revert`). Then the
    /// first message it undid goes back in the composer (`session.tsx`'s `onReverted`).
    fn confirm_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(runner) = self.runner.clone() else {
            return;
        };
        let running = self.code_running();
        let View::Code(view) = &mut self.view else {
            return;
        };
        let Some(restoring) = view.restoring.as_mut() else {
            return;
        };
        if running {
            // A turn started after the count: say so, rather than a button that does nothing.
            if matches!(restoring.step, RestoreStep::Ready(_)) {
                restoring.step = RestoreStep::Failed("Stop the agent before restoring".into());
                cx.notify();
            }
            return;
        }
        let RestoreStep::Ready(files) = restoring.step else {
            return;
        };
        let turn = restoring.turn;
        restoring.step = RestoreStep::Restoring(files);
        restoring.ask += 1;
        let ask = restoring.ask;
        // The message to put back, read before the revert dims it.
        let prompt = prompt_after(&view.blocks, turn);
        let thread = view.id.clone();
        let reverted = runner.revert_to_turn(thread.clone(), turn);
        cx.spawn_in(window, async move |this, cx| {
            // Gives up waiting after a minute (`revert.ts`'s `revertDeadline`), so the confirmation can close.
            let deadline = cx.background_executor().timer(REVERT_DEADLINE);
            let answer = match futures::future::select(reverted, deadline).await {
                futures::future::Either::Left((answer, _)) => {
                    answer.unwrap_or_else(|_| Err("The revert was cancelled.".into()))
                }
                futures::future::Either::Right(_) => Err("The revert did not finish. Try again.".into()),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let View::Code(view) = &mut this.view else {
                    return;
                };
                if view.id != thread {
                    return;
                }
                let Some(restoring) = view.restoring.as_mut().filter(|restoring| restoring.ask == ask) else {
                    return;
                };
                match answer {
                    Ok(_) => {
                        view.restoring = None;
                        if let Some(prompt) = &prompt {
                            this.put_back(&prompt.text, window, cx);
                            crate::attachments::append_restored(
                                &mut this.draft,
                                crate::attachments::restored_images(&thread, prompt.seq, &prompt.images),
                            );
                        }
                        this.refresh_code(cx);
                    }
                    Err(error) => restoring.step = RestoreStep::Failed(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A sub-agent's row (`tool-view.tsx`'s `AgentRow`): how it stands, what it does, its type or what it does now,
    /// and its steps. A click opens it to what it did, or folds it to this one line.
    fn render_agent(
        &self,
        ix: usize,
        row: &AgentRow,
        open: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let call = row.call_id.clone();
        let inspect = call.clone();
        let tip: SharedString = [Some(row.title.as_str()), row.role.as_deref(), row.prompt.as_deref()]
            .into_iter()
            .flatten()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .into();
        let title = if row.title.is_empty() {
            "Sub-agent".to_string()
        } else {
            row.title.clone()
        };
        h_flex()
            .id(("agent", ix))
            .w_full()
            .min_h(px(28.))
            .px(px(8.))
            .mx(px(-8.))
            .gap(px(8.))
            .rounded(px(8.))
            .text_size(px(13.))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .child(if row.policy.is_some() {
                glyph(Lucide::ShieldOff, palette.faint).into_any_element()
            } else {
                agent_mark(ix, row.status, palette)
            })
            .when(row.policy.is_some(), |line| {
                line.child(div().text_color(palette.faint).child("Blocked"))
            })
            .child(div().flex_none().text_color(palette.foreground).child(title))
            .when(row.status == AgentStatus::Needs, |line| {
                line.child(div().flex_none().text_color(palette.foreground).child("Needs you"))
            })
            .when(row.status == AgentStatus::Stopped, |line| {
                line.child(div().flex_none().text_color(palette.faint).child("Stopped"))
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(palette.faint)
                    .child(row.aside().unwrap_or_default().to_string()),
            )
            .when_some(row.meta(), |line, meta| {
                line.child(div().flex_none().text_color(palette.faint).child(meta))
            })
            .when(row.status == AgentStatus::Failed && row.policy.is_none(), |line| {
                line.child(div().flex_none().text_color(palette.destructive).child("Failed"))
            })
            .child(
                div()
                    .id(("inspect-agent", ix))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(8.))
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|s| s.bg(palette.selected))
                    .child("Open")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.inspect_subagent(inspect.clone(), window, cx);
                    })),
            )
            .child(super::disclosure("disclosure", open, palette.faint))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let View::Code(view) = &mut this.view {
                    view.toggle_agent(&call);
                    cx.notify();
                }
            }))
    }

    /// A tool call that failed: a red ✕ in place of its icon, its words as any call's, and "Failed" after them.
    /// A click opens the error under the row (`tool-row.tsx` expands a call to its output).
    #[allow(clippy::too_many_arguments)]
    fn render_failed_tool(
        &self,
        ix: usize,
        call_id: &str,
        name: &str,
        summary: &str,
        error: &str,
        open: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let row = tool_row(
            ("tool", ix),
            glyph(Lucide::CircleX, palette.destructive),
            name,
            summary,
            false,
            palette,
        )
        .child(
            div()
                .flex_none()
                .text_size(px(13.))
                .text_color(palette.destructive)
                .child("Failed"),
        );
        // With no error text there is nothing to open: no chevron, no pointer.
        let call_id = call_id.to_string();
        let row = if error.is_empty() {
            row.into_any_element()
        } else {
            row.cursor_pointer()
                .child(super::disclosure("disclosure", open, palette.faint))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let View::Code(view) = &mut this.view {
                        if !view.open_errors.remove(&call_id) {
                            view.open_errors.insert(call_id.clone());
                        }
                        cx.notify();
                    }
                }))
                .into_any_element()
        };
        v_flex()
            .w_full()
            .gap(px(4.))
            .child(row)
            .when(open && !error.is_empty(), |column| {
                column.child(with_copy(
                    output_box(palette).child(error.to_string()),
                    error.to_string().into(),
                    palette,
                ))
            })
    }

    /// A turn's end: the shared answer footer (`answer_footer.rs`, one look with chat). Only the maker's mark shows,
    /// and "Stopped" or "Failed" when the turn didn't finish: the title bar already shows the session's cost, so the
    /// model, the cost, how long it worked and its tokens wait in the mark's tooltip. On hover anywhere in
    /// the answer, "Restore to here", Fork (a new session from this turn) and Copy, with Restore's confirmation under it once asked.
    /// The answer's time shows with them, after the mark, as the person's time does under a message: when the turn ended,
    /// with the exact time in its tooltip.
    fn render_turn(&self, turn: TurnFooter, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let View::Code(view) = &self.view else {
            return div().into_any_element();
        };
        let TurnFooter {
            seq,
            at,
            status,
            duration_ms,
            cost_usd,
            tokens,
            model,
            reverted,
            answer,
        } = turn;
        let asking = view.restoring.as_ref().filter(|restoring| restoring.turn == seq);
        // Its own key in the copied slot, apart from the messages' seqs.
        let copy_key = -seq - 1;
        let copied = view.copied == Some(copy_key);
        let forking = view.forking == Some(seq);
        let keep = asking.is_some() || copied || forking;
        let hovered = view.hovered_turn.is_some_and(|(_, turn)| turn == seq);
        let restore = (!reverted && self.can_restore()).then(|| {
            super::answer_footer::hover_button(
                ("turn-restore", seq as u64),
                Lucide::RotateCcwClock,
                "Restore to here",
                palette,
            )
            .when(asking.is_some(), |button| button.bg(palette.selected))
            .on_click(cx.listener(move |this, _, window, cx| this.ask_restore(seq, window, cx)))
        });
        let fork = (!reverted && view.pending.is_none() && self.runs(&view.id).is_some()).then(|| {
            let id = view.id.clone();
            let button = super::answer_footer::hover_button(
                ("turn-fork", seq as u64),
                Lucide::GitFork,
                if forking { "Forking…" } else { "Fork from here" },
                palette,
            );
            // One fork at a time: a second click while the worktree is made would make a second session.
            if view.forking.is_some() || self.forking.is_some() {
                return button.opacity(0.4).cursor_default();
            }
            button.on_click(cx.listener(move |this, _, window, cx| {
                if let View::Code(view) = &mut this.view {
                    view.forking = Some(seq);
                }
                this.fork_session(id.clone(), Some(seq), None, window, cx);
            }))
        });
        let copy = answer.filter(|text| !text.trim().is_empty()).map(|text| {
            super::answer_footer::hover_button(
                ("turn-copy", seq as u64),
                if copied { Lucide::Check } else { Lucide::Copy },
                if copied { "Copied" } else { "Copy" },
                palette,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.copy_message(copy_key, text.clone(), cx)))
        });
        let ended = match status {
            TurnStatus::Interrupted => Some("Stopped"),
            TurnStatus::Error => Some("Failed"),
            TurnStatus::Completed => None,
        };
        let name = model.as_deref().map(code::model_label);
        let about = [
            Some(super::answer_footer::words(name.as_deref(), cost_usd, None)).filter(|words| !words.is_empty()),
            (duration_ms > 0).then(|| format!("Worked for {}", code::duration(duration_ms))),
            (tokens > 0).then(|| format!("{} tokens", code::compact(tokens))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        // Hidden, it keeps its place, as the person's time does, so nothing moves when it shows.
        let time = (at > 0).then(|| {
            let exact: SharedString = code::message_time_exact(at).into();
            div()
                .id(("turn-time", seq as u64))
                .flex_none()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(palette.faint)
                .whitespace_nowrap()
                .when(!keep && !hovered, |time| time.invisible())
                .tooltip(move |window, cx| Tooltip::new(exact.clone()).build(window, cx))
                .child(code::message_time(at, code::now_ms()))
        });
        v_flex()
            .w_full()
            .gap(px(4.))
            .child(
                h_flex()
                    .w_full()
                    .h(px(28.))
                    .gap(px(8.))
                    .when(reverted, |row| row.opacity(0.5))
                    .children(time)
                    .child(super::answer_footer::line(
                        ("turn-line", seq as u64),
                        ended.map(str::to_string).unwrap_or_default(),
                        (!about.is_empty()).then(|| about.into()),
                        status == TurnStatus::Error,
                        keep || hovered,
                        palette,
                    ))
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(2.))
                            .when(!keep && !hovered, |row| row.opacity(0.))
                            .children(restore)
                            .children(fork)
                            .children(copy),
                    ),
            )
            .when_some(asking, |column, restoring| {
                column.child(h_flex().w_full().justify_end().child(render_restore_confirm(
                    &restoring.step,
                    palette,
                    cx,
                )))
            })
            .into_any_element()
    }

    /// Implement the plan the agent waits on in a new session (`plan-card.tsx`'s `implementElsewhere`): a fresh
    /// session in the same project, with the same agent and model, in Auto-accept edits, starts on the plan. Once it
    /// is made, this session is told to stop (`pending_created`), so a failure loses nothing.
    pub(super) fn implement_in_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.runner.is_none() {
            return;
        }
        if self.code_form.starting.is_some() {
            self.composer_note = Some("The last session is still starting.".into());
            cx.notify();
            return;
        }
        let View::Code(view) = &self.view else {
            return;
        };
        let Some(Block::Approval {
            request_id,
            tool,
            input,
            ..
        }) = code::pending_approval(&view.blocks)
        else {
            return;
        };
        if tool != PLAN_TOOL || plan_of(input).trim().is_empty() {
            return;
        }
        let Some(session) = self.code_session() else {
            return;
        };
        let (new, moved) = implement_elsewhere(session, request_id, plan_of(input));
        self.start_pending(new, None, window, cx);
        self.title = moved.title.clone().into();
        if let View::Code(view) = &mut self.view
            && let Some(pending) = &mut view.pending
        {
            pending.moved_from = Some(moved);
        }
        cx.notify();
    }

    /// What follows the blocks: a note while the session opens or when it is empty, the draft's message, the working
    /// line, why the session could not start, or why it could not be read.
    fn render_code_tail(&self, view: &CodeView, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = view.pending.is_some();
        let cloud = view.pending.as_ref().is_some_and(|pending| pending.cloud.is_some());
        let mut tail = Vec::new();
        // A cloud session's machine coming up (or waking) shows its line above the composer instead (`cloud_view.rs`).
        if !pending && !view.loaded && view.error.is_none() && !self.machine_coming(view) {
            // A sleeping cloud session opens from Gateway's history (`machines/history.rs`): until it shows, its note
            // says it is on its way, that there is none, or why it failed.
            let asleep = self.open_session_asleep() && !self.cloud_delivering(&view.id);
            let note = if asleep {
                self.asleep_note(view)
            } else {
                super::cloud_view::AsleepNote::Opening
            };
            tail.push(self.render_asleep_note(note, palette, cx));
        }
        if !pending && view.loaded && view.blocks.is_empty() {
            tail.push(
                div()
                    .text_sm()
                    .text_color(palette.faint)
                    .child("Nothing in this session yet")
                    .into_any_element(),
            );
        }
        // The answer the agent is still writing, until its text event lands.
        if let Some((_, text)) = &view.streamed {
            tail.push(
                render_block(
                    usize::MAX,
                    &Block::Text(String::new()),
                    Some(text.clone()),
                    None,
                    Some(view.list.clone()),
                    palette,
                    self.context_link_handler(|href, _, _, cx| crate::external_link::open(href, cx), cx),
                    self.file_chips(palette, cx),
                )
                .into_any_element(),
            );
        }
        // The draft's message, until the session's own events replace it: at the first message's place (its 28px
        // top), so nothing moves when they do. A session started from the home rides the start's clock
        // (`launch_motion.rs`); one started from another session arrives as a chat message does.
        // A cloud session's first message waits for its machine, and says so.
        if let Some(pending) = view.pending.as_ref() {
            let waits = pending.cloud.as_ref().is_some_and(|start| start.waiting.is_open());
            let message = v_flex()
                .w_full()
                .gap(px(4.))
                .child(user_prompt(
                    pending.new.text.clone(),
                    Vec::new(),
                    (!pending.images.is_empty()).then(|| {
                        self.sent_images(
                            format!("pending-images-{}", view.id),
                            pending.images.clone(),
                            palette,
                            cx,
                        )
                    }),
                    palette,
                    Some(self.artifact_opener(cx)),
                ))
                .when(waits, |message| {
                    message.child(
                        h_flex()
                            .w_full()
                            .justify_end()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(crate::cloud::words::SENDS_WHEN_READY),
                    )
                });
            tail.push(if self.launch_owns_arrival(cx) {
                self.launch_part(super::launch_motion::Part::Message, message.into_any_element())
            } else {
                motion::arrive(message, ElementId::Name(format!("arrive-{}", view.id).into()), 12.).into_any_element()
            });
        }
        // Cloud startup has its own status line.
        if !cloud {
            // While the lead waits on its sub-agents, the crew's line takes the working line's place (`CrewWaiting`).
            let working = self
                .render_working(view, palette)
                .map(|working| self.launch_part(super::launch_motion::Part::Working, working));
            let approval = code::pending_approval(&view.blocks).is_some();
            tail.extend(
                self.render_crew_waiting(working.is_some(), approval, palette)
                    .or(working),
            );
            let failed = view.pending.as_ref().and_then(|pending| pending.failed.clone());
            // A missing Nexus sign-in is setup, not a failure: its line with Sign in (`nexus_sign_in.rs`).
            // Drawn each frame, so it leaves with its motion once the sign-in starts the session again.
            let sign_in = failed.as_deref().and_then(|error| self.start_sign_in_line(error, cx));
            let setup = sign_in.is_some();
            let fixer = self.fixer(cx);
            tail.extend(crate::setup::slot("start-sign-in", sign_in, &fixer, palette, cx));
            if let Some(pending) = view.pending.as_ref()
                && let Some(error) = failed.filter(|_| !setup)
            {
                // An agent that is missing or signed out is setup: its line installs or signs it in, then starts again.
                tail.push(match crate::agents::setup::gap(&error) {
                    Some(gap) => self.render_start_setup(&view.id, pending.new.agent, gap, palette, cx),
                    None => self.render_start_failed(error, palette, cx).into_any_element(),
                });
            }
        }
        if let Some(error) = view.error.clone().filter(|_| !view.is_draft()) {
            tail.push(
                div()
                    .text_sm()
                    .text_color(palette.destructive)
                    .child(with_copy(div().child(error.clone()), error, palette))
                    .child(
                        Button::new("code-log-retry")
                            .ghost()
                            .label("Try again")
                            .on_click(cx.listener(|this, _, _, cx| this.retry_code_log(cx))),
                    )
                    .into_any_element(),
            );
        }
        (!tail.is_empty()).then(|| v_flex().gap(px(14.)).children(tail).into_any_element())
    }

    /// The now line at the end of the transcript: the one spinner for the session's
    /// agent, what it is doing right now ("Running" + the command in mono, "Reading" + the file, "Thinking"), for how
    /// long, and "Esc to stop". While it waits on the person the card above the composer speaks: the line holds a
    /// pause mark and no spinner. The turn's cost so far shows beside the top bar's amount: cost has one home.
    fn render_working(&self, view: &CodeView, palette: Palette) -> Option<AnyElement> {
        let since = view.working_since?;
        let pending = view
            .pending
            .as_ref()
            .map(|pending| start_words(&pending.new, pending.cloud.is_some()));
        let waiting = matches!(code::pending_approval(&view.blocks), Some(_));
        if pending.is_none() && !waiting && !self.code_running() {
            return None;
        }
        let start = pending.map(|words| (words, None)).or_else(|| {
            setup_running(&view.blocks).map(|command| ("Running the setup script".to_string(), Some(command)))
        });
        let (now, verb, target) = now_line(start, waiting, now_doing(&view.blocks), thought_summary(&view.blocks));
        Some(working_line(
            now,
            verb,
            target,
            since,
            now != Now::Waiting && view.pending.is_none(),
            turn_seed(&view.id, &view.blocks),
            palette,
        ))
    }

    /// A session that failed because its server is not signed in to Nexus: why, then a button that opens Settings ›
    /// Remote servers with that server's sign-in started (`Machines::ask_sign_in`). On a machine that is not one of
    /// the servers, the error alone.
    fn render_sign_in_error(
        &self,
        ix: usize,
        error: &str,
        detail: Option<&str>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = error_row(ix, error, detail, palette);
        let machine = self.code_session().and_then(|session| session.machine.clone());
        let Some(server) = machine.and_then(|machine| {
            crate::machines::get()
                .servers()
                .into_iter()
                .find(|server| server.id == machine)
        }) else {
            return text.into_any_element();
        };
        let id = server.id.clone();
        v_flex()
            .gap(px(10.))
            .child(text)
            .child(
                h_flex().child(
                    div()
                        .id(("sign-in-server", ix))
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(palette.border)
                        .text_size(px(13.))
                        .text_color(palette.foreground)
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.hover))
                        .child(format!("Sign {} in to Nexus", server.name))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            crate::machines::get().ask_sign_in(&id);
                            this.open_settings(SettingsSection::Page(PageId::Servers), window, cx);
                        })),
                ),
            )
            .into_any_element()
    }

    /// A session that could not start: why, with Copy, then Try again and Edit.
    fn render_start_failed(&self, error: SharedString, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let action = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px(px(10.))
                .py(px(4.))
                .rounded(px(6.))
                .border_1()
                .border_color(palette.border)
                .text_size(px(13.))
                .text_color(palette.foreground)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .child(label)
        };
        v_flex()
            .gap(px(10.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(12.))
            .border_1()
            .border_color(palette.border)
            .child(
                h_flex()
                    .gap(px(8.))
                    .text_sm()
                    .text_color(palette.destructive)
                    .child(glyph(Lucide::CircleAlert, palette.destructive))
                    .child(div().flex_1().min_w(px(0.)).child(error.clone()))
                    .child(super::code_block::copy_button(error, palette)),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        action("start-retry", "Try again")
                            .on_click(cx.listener(|this, _, window, cx| this.retry_pending(window, cx))),
                    )
                    .child(
                        action("start-edit", "Edit")
                            .on_click(cx.listener(|this, _, window, cx| this.edit_pending(window, cx))),
                    ),
            )
    }

    /// Under the session: its cloud machine's line while it comes up, then the dock.
    fn render_code_dock(&self, view: &CodeView, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let notices: Vec<_> = [
            self.render_start_line(view, palette, cx),
            self.render_background_workbench(palette, cx),
            self.render_browser_permission(&view.id, palette, cx),
        ]
        .into_iter()
        .flatten()
        .collect();
        if notices.is_empty() {
            view.dock.tray_layout.clear_notices();
        }
        let notices = (!notices.is_empty()).then(|| {
            div()
                .relative()
                .w_full()
                .child(view.dock.tray_layout.measure_notices(cx))
                .child(v_flex().w_full().gap(px(8.)).children(notices))
                .into_any_element()
        });
        let priority = self.open_secret_request().is_some()
            || matches!(code::pending_approval(&view.blocks), Some(Block::Approval { tool, .. }) if tool == QUESTION_TOOL || tool == PLAN_TOOL);
        let activity = self.side.command_activity(cx);
        let permissions =
            !priority && !view.dock.permissions.reviewing && !view.dock.permissions.pending(&view.blocks).is_empty();
        let groups = usize::from(activity.is_some()) + usize::from(permissions);
        let limit = view.dock.tray_layout.budget(groups);
        if let Some(activity) = &activity {
            activity.update(cx, |activity, cx| {
                activity.set_inline_limit(if priority { crate::command_tray::HEADER } else { limit }, cx)
            });
        }
        // The scroll box clips to its bounds: `halo_room` keeps the composer's halos inside them. The limit and the
        // measure are the content's, without that room.
        let room = 2. * super::composer::HALO_WIDTH;
        let dock = super::composer::halo_room(
            div()
                .id("code-primary-dock")
                .debug_selector(|| "code-primary-dock".into())
                .min_h(px(0.))
                .max_h(px(view.dock.tray_layout.primary_limit(groups) + room))
                .overflow_y_scroll(),
        )
        .child(
            div()
                .relative()
                .child(view.dock.tray_layout.measure(true, cx))
                .child(self.render_dock(view, palette, cx)),
        );
        let dock = v_flex()
            .w_full()
            .gap(px(8.))
            .children(notices)
            .children(activity)
            .when(permissions, |tray| {
                tray.children(self.render_permission_queue(view, palette, limit, cx))
            })
            .child(dock)
            .into_any_element();
        dock
    }

    /// The composer; the question the agent waits on, in its place; or why it is read-only.
    fn render_dock(&self, view: &CodeView, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if let Some(switch) = &view.agent_switch {
            return self.render_agent_switch(switch, palette, cx);
        }
        if self.runner.is_none() {
            // The composer, dimmed under who runs the session (`read_only.rs`); the questions it may wait on are
            // answered where it runs.
            let composer = self.render_code_composer(view, palette, cx).into_any_element();
            return self.render_owned_dock(true, composer, palette, cx);
        }
        let dock = self.render_code_dock_card(view, palette, cx);
        // Taken over: the composer brightens in place while the line leaves.
        let dock = self.render_owned_dock(false, dock, palette, cx);
        // Over a question or an approval too: a sub-agent's question outlives the Interrupt.
        match self.render_still_running(view, palette, cx) {
            Some(still) => v_flex()
                .w_full()
                .gap(px(8.))
                .child(still)
                .child(dock)
                .into_any_element(),
            None => dock,
        }
    }

    /// What the dock holds: a secret request, the question or approval the agent waits on, or the composer.
    fn render_code_dock_card(&self, view: &CodeView, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if let Some(session) = self.code_session()
            && let Some(choice) = session.start_choices.first()
        {
            return self.render_session_start(view, choice, session.start_choices.len(), palette, cx);
        }
        // A request for a secret comes before the agent's questions, as in the Nexus app.
        if let Some(request) = self.open_secret_request() {
            return self.render_secret_card(view, &request, palette, cx);
        }
        match code::pending_approval(&view.blocks) {
            Some(Block::Approval { tool, input, .. }) if tool == QUESTION_TOOL => {
                self.render_question(view, input, palette, cx)
            }
            Some(Block::Approval { tool, input, .. }) if tool == PLAN_TOOL => {
                self.render_plan(view, input, palette, cx)
            }
            _ => v_flex()
                .w_full()
                .gap(px(8.))
                .child(self.render_code_composer(view, palette, cx))
                .into_any_element(),
        }
    }
}

/// The open Code session's minimap state.
fn code_minimap(this: &mut Workspace) -> Option<&mut super::minimap::MinimapState> {
    match &mut this.view {
        View::Code(view) => Some(&mut view.minimap),
        _ => None,
    }
}

/// A Code session's minimap rows: one per message the person sent, and one per answer. An answer is the agent's turn,
/// from its first row (text, thinking, a tool, an approval) through its end line; its tick jumps to that first row.
/// Tool blocks get no tick of their own, and what takes no row (plumbing tools, queued messages, the plan) none at
/// all. The tail (the streamed answer, the working line, a start that failed) is not a block, so it gets none either.
pub(super) fn minimap_rows(blocks: &[Block]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut answering = false;
    for (item, block) in blocks.iter().enumerate() {
        match block {
            Block::User { .. } => {
                rows.push(Row { item, user: true });
                answering = false;
            }
            Block::Tool { name, .. } if hidden_tool(name) => {}
            Block::Setup(_) | Block::Queued { .. } | Block::Plan(_) => {}
            _ => {
                if !answering {
                    rows.push(Row { item, user: false });
                }
                // A turn's end closes its answer: what comes after it starts another.
                answering = !matches!(block, Block::Turn { .. });
            }
        }
    }
    rows
}

/// A minimap row's label and first line: the message's, or the answer's first text, else its first tool's.
pub(super) fn minimap_preview(blocks: &[Block], rows: &[Row], row: usize) -> (&'static str, String) {
    let start = rows[row].item;
    let end = rows.get(row + 1).map_or(blocks.len(), |next| next.item);
    let preview = |text: &str| first_line(text, PREVIEW);
    if let Some(Block::User { text, images, .. }) = blocks.get(start) {
        let line = preview(text);
        let line = match line.is_empty() {
            false => line,
            true if *images > 0 => "Attachment".into(),
            true => "No text".into(),
        };
        return ("You", line);
    }
    let answer = &blocks[start..end.min(blocks.len())];
    let text = answer.iter().find_map(|block| match block {
        Block::Text(markdown) => Some(preview(markdown)).filter(|line| !line.is_empty()),
        _ => None,
    });
    let tool = || {
        answer.iter().find_map(|block| match block {
            // A path or a command, as written: Markdown's marks are its own characters here.
            Block::Tool { name, summary, .. } if !hidden_tool(name) => {
                let line = format!("{} {}", tool_verb(name), summary.lines().next().unwrap_or("").trim());
                Some(match line.chars().count() > PREVIEW {
                    true => format!("{}…", line.chars().take(PREVIEW - 1).collect::<String>().trim_end()),
                    false => line,
                })
            }
            _ => None,
        })
    };
    ("Answer", text.or_else(tool).unwrap_or_else(|| "No text".into()))
}

/// The transcript's list: it starts at the end, and follows it while the end shows.
/// What the agent is doing right now, from the last block of its work (`code::work_tail`): a call still waiting on its
/// result reads as its verb and target ("Running", `cargo bench`); a thought, or anything else, as None (the line says
/// "Thinking").
pub(super) fn now_doing(blocks: &[Block]) -> Option<(&'static str, String)> {
    let last = &blocks[code::work_tail(blocks)?];
    let Block::Tool {
        name,
        summary,
        error: None,
        call,
        ..
    } = last
    else {
        return None;
    };
    if call.output.is_some() || hidden_tool(name) {
        return None;
    }
    let verb = match name.as_str() {
        "Bash" | "BashOutput" => "Running",
        "Read" | "NotebookRead" => "Reading",
        "Edit" | "MultiEdit" | "NotebookEdit" => "Editing",
        "Write" => "Writing",
        "Grep" | "Glob" | "LS" | "WebSearch" => "Searching",
        "WebFetch" => "Fetching",
        "Task" | "Agent" => "Starting a sub-agent",
        _ => "Working",
    };
    Some((verb, summary.clone()))
}

/// What a turn's footer shows: how and when the turn ended, and its answer's text for Copy.
struct TurnFooter {
    seq: i64,
    /// When the turn ended (ms since 1970); 0 when the log has no time, and then no time shows.
    at: i64,
    status: TurnStatus,
    duration_ms: u64,
    cost_usd: Option<f64>,
    tokens: u64,
    model: Option<String>,
    reverted: bool,
    answer: Option<String>,
}

/// The answer of the turn that ends at block `end`: its top-level text since the message it answers.
/// The turn a row belongs to: the seq of the turn's end at or after it, before the next message. None for a message,
/// or for a turn still running, which has no footer yet.
fn turn_of(blocks: &[Block], ix: usize) -> Option<i64> {
    blocks.get(ix..)?.iter().find_map(|block| match block {
        Block::User { .. } => Some(None),
        Block::Turn { seq, .. } => Some(Some(*seq)),
        _ => None,
    })?
}

pub(super) fn turn_answer(blocks: &[Block], end: usize) -> Option<String> {
    let start = blocks[..end.min(blocks.len())]
        .iter()
        .rposition(|block| matches!(block, Block::User { .. } | Block::Turn { .. }))
        .map_or(0, |at| at + 1);
    let texts: Vec<&str> = blocks[start..end.min(blocks.len())]
        .iter()
        .filter_map(|block| match block {
            Block::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n\n"))
}

fn transcript_list() -> ListState {
    let list = ListState::new(1, ListAlignment::Top, OVERDRAW);
    list.set_follow_mode(FollowMode::Tail);
    list
}

/// The arrow that brings the transcript back to its end, while it is scrolled up (`transcript.rs`'s, for the list).
fn render_jump(
    list: ListState,
    phase: motion::Phase,
    palette: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let button = jump_circle("jump-to-latest", palette).on_click(cx.listener(move |_, _, _, cx| {
        list.set_follow_mode(FollowMode::Tail);
        cx.notify();
    }));
    h_flex()
        .absolute()
        .bottom(px(12.))
        .left_0()
        .right_0()
        .justify_center()
        .child(phase.menu("code-jump", 6., button))
}

/// What the agent is doing as the now line shows it, which picks the line's motion: one before the model starts,
/// another while it generates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Now {
    /// The session is not generating yet (its worktree, its checkout, its machine, its setup script): the live mark
    /// breathes, the line says the step, and a strip of dots runs under it.
    Starting,
    /// The model works: a 3D icon picked for the turn (`motion::solid`), and a live
    /// summary (the call that runs, else the latest thought).
    Generating,
    /// The agent waits on the person: a pause mark, nothing moves.
    Waiting,
}

/// The now line's state and words. `start`: a step before the model starts, in words and an optional target; `doing`:
/// the call still running (from [`now_doing`]: "Running" and `cargo test`); `thought`: the latest thought's summary
/// (from [`thought_summary`]). Starting comes first, then waiting on the person, then the call, then the thought, else
/// "Thinking…".
pub(super) fn now_line(
    start: Option<(String, Option<String>)>,
    waiting: bool,
    doing: Option<(&'static str, String)>,
    thought: Option<String>,
) -> (Now, SharedString, Option<String>) {
    if let Some((words, target)) = start {
        return (Now::Starting, words.into(), target.filter(|target| !target.is_empty()));
    }
    if waiting {
        return (Now::Waiting, "Waiting for you".into(), None);
    }
    match (doing, thought) {
        (Some((verb, target)), _) => (
            Now::Generating,
            verb.into(),
            Some(target).filter(|target| !target.is_empty()),
        ),
        (None, Some(thought)) => (Now::Generating, thought.into(), None),
        (None, None) => (Now::Generating, "Thinking…".into(), None),
    }
}

/// What a session that is being made does, as the now line says it, from what it was asked to make: a pull request's
/// checkout, a worktree and its base, a cloud machine, or the session alone.
pub(super) fn start_words(new: &NewSession, cloud: bool) -> String {
    match (new.review, new.worktree, cloud) {
        (Some(number), _, _) => format!("Checking out pull request #{number}…"),
        (None, true, _) => {
            let base = new.base.as_deref().unwrap_or("main");
            format!(
                "Creating a worktree from {}…",
                base.strip_prefix("origin/").unwrap_or(base)
            )
        }
        (None, false, true) => "Starting the cloud machine…".into(),
        (None, false, false) => "Starting the session…".into(),
    }
}

/// The command of the setup script still running at the end of the session's work, if one is.
pub(super) fn setup_running(blocks: &[Block]) -> Option<String> {
    blocks.iter().rev().find_map(|block| match block {
        Block::Setup(row) if row.status == nexus_workbench::event::SetupStatus::Running => Some(row.command.clone()),
        _ => None,
    })
}

/// How long a summary may be, in characters, before it is cut at a word.
const SUMMARY: usize = 64;

/// The latest thought of the turn under way, as one line: its first sentence, without Markdown's marks,
/// cut at a word to 64 characters, with one ellipsis. None before the turn's first thought.
pub(super) fn thought_summary(blocks: &[Block]) -> Option<String> {
    let text = blocks.iter().rev().find_map(|block| match block {
        Block::User { from: None, .. } => Some(None),
        Block::Thinking(text, _) => Some(Some(text.as_str())),
        _ => None,
    })??;
    let plain: String = text.chars().filter(|c| !matches!(c, '*' | '`' | '#' | '_')).collect();
    let first = plain
        .split(['\n', '.', '?', '!', ':'])
        .map(str::trim)
        .find(|sentence| !sentence.is_empty())?;
    let line = if first.chars().count() > SUMMARY {
        let cut: String = first.chars().take(SUMMARY).collect();
        cut.rsplit_once(' ').map_or(cut.clone(), |(head, _)| head.to_string())
    } else {
        first.to_string()
    };
    Some(format!("{}…", line.trim_end_matches([',', ';', ' '])))
}

/// A turn's identity, to pick its 3D icon: the session (`id`) and the person's latest message in it. The icon holds
/// for the turn, and the next message draws a new one.
pub(super) fn turn_seed(id: &str, blocks: &[Block]) -> u64 {
    use std::hash::{Hash, Hasher};
    let message = blocks.iter().rev().find_map(|block| match block {
        Block::User { seq, from: None, .. } => Some(*seq),
        _ => None,
    });
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (id, message).hash(&mut hasher);
    hasher.finish()
}

/// The now line's drawing (`Workspace::render_working`). Starting: the live mark breathes beside the step, and a strip
/// of dots runs under the words. Generating: the 3D icon `turn` picks beside the live summary. Waiting: a pause mark.
/// A new summary rises in. Then the target in mono, for how long, and "Esc to stop" when `stoppable`.
pub(super) fn working_line(
    now: Now,
    verb: SharedString,
    target: Option<String>,
    since: Instant,
    stoppable: bool,
    turn: u64,
    palette: Palette,
) -> AnyElement {
    let mark = match now {
        Now::Waiting => glyph(Lucide::CirclePause, palette.foreground).into_any_element(),
        Now::Starting => {
            motion::breathing_mark("now-line-breathing", motion::LIVE_MARK, palette.foreground).into_any_element()
        }
        Now::Generating => motion::solid(
            "now-line-solid",
            motion::Solid::pick(turn),
            motion::LIVE_MARK,
            palette.foreground,
        )
        .into_any_element(),
    };
    // Keyed by what it says, so each new summary rises in and the same one stays put.
    let key = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (verb.as_ref(), target.as_deref()).hash(&mut hasher);
        hasher.finish()
    };
    let words = motion::arrive(
        div().min_w(px(0.)).truncate().child(verb),
        SharedString::from(format!("now-line-words-{key:x}")),
        6.,
    );
    let line = h_flex()
        .w_full()
        .h(px(28.))
        .gap(px(8.))
        .text_size(px(14.))
        .text_color(palette.muted)
        .child(
            div()
                .size(px(motion::LIVE_MARK))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(mark),
        )
        .child(
            div()
                .min_w(px(0.))
                .flex_shrink_0()
                .max_w(px(560.))
                .overflow_hidden()
                .child(words),
        )
        .when_some(target, |line, target| {
            line.child(
                div()
                    .min_w(px(0.))
                    .truncate()
                    .font_family("Geist Mono")
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child(target),
            )
        })
        .child(
            div()
                .flex_none()
                .text_size(px(13.))
                .text_color(palette.faint)
                .child(format!("· {}", elapsed(since.elapsed()))),
        )
        .child(div().flex_1())
        .when(stoppable, |line| {
            line.child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child("Esc to stop"),
            )
        });
    if now != Now::Starting {
        return line.into_any_element();
    }
    // Under the step's words, as wide as a short line: the strip starts where the words do.
    v_flex()
        .w_full()
        .child(line)
        .child(div().pl(px(motion::LIVE_MARK + 8.)).child(motion::dot_strip(
            "now-line-strip",
            240.,
            8.,
            palette.foreground,
        )))
        .into_any_element()
}

/// The round arrow that brings a transcript back to its end.
pub(super) fn jump_circle(id: &'static str, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(32.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(palette.pop)
        .border_1()
        .border_color(palette.border)
        .shadow_md()
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .active(|style| style.opacity(0.8))
        .child(glyph(gpui_kit::component::IconName::ArrowDown, palette.foreground))
}

/// An elapsed time as the working line shows it: `8s`, `2m 05s`, `1h 03m`.
pub(super) fn elapsed(time: Duration) -> String {
    let seconds = time.as_secs();
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

/// What the agent asks to do, as the card's title says it (`askVerb`).
pub(super) fn ask_verb(tool: &str) -> String {
    match tool {
        "Bash" => "run a command".into(),
        "Edit" | "MultiEdit" | "NotebookEdit" => "edit a file".into(),
        "Write" => "write a file".into(),
        "Read" | "NotebookRead" => "read a file".into(),
        "WebFetch" => "fetch a page".into(),
        "WebSearch" => "search the web".into(),
        "ExitPlanMode" => "start on its plan".into(),
        "AskUserQuestion" => "ask you a question".into(),
        // The raw server shows unless it is named as the app: a server called `evil-slack` must not read as Slack.
        tool => match mcp_tool(tool) {
            Some(mcp) => format!("use {} · {}", mcp.server_name(), mcp.tool),
            None => format!("use {}", tool_verb(tool)),
        },
    }
}

/// Whether a session on `machine` (None: this computer) offers Run on its shell blocks: this computer's, and an SSH
/// server's whose host shares its sessions' commands (`terminal` in its hello), where Run uses the server's Commands. A
/// cloud machine's never: its host refuses the shared terminal to a client that Gateway relays.
fn run_offered(machine: Option<&str>, is_cloud: impl Fn(&str) -> bool, shares_commands: impl Fn(&str) -> bool) -> bool {
    machine.is_none_or(|machine| !is_cloud(machine) && shares_commands(machine))
}

/// `run`: what Run on a shell block does; None while the answer streams, as the Nexus app offers it on a finished one.
/// `list`: the transcript's list, which Find's step scrolls; None outside a list (a side chat).
/// `files`: the session's file chips (`file_chips.rs`); None where a file has nowhere to open.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_block(
    ix: usize,
    block: &Block,
    text: Option<Entity<TextViewState>>,
    run: Option<transcript::RunCommand>,
    list: Option<ListState>,
    palette: Palette,
    link: super::link_menu::LinkHandler,
    files: Option<super::file_chips::FileChips>,
) -> AnyElement {
    match block {
        // A person's message has its actions; `Workspace::render_user` draws it.
        Block::User { text, .. } => user_bubble(text.clone(), Vec::new(), palette).into_any_element(),
        Block::Text(_) => match text {
            Some(view) => {
                let view = crate::external_link::text_view(&view)
                    .on_link_click(move |href, event, window, cx| link(href, event, window, cx))
                    .selectable(true)
                    .selection_format(gpui_kit::component::text::SelectionFormat::Plain);
                let view = match files {
                    Some(files) => view.plugin(files),
                    None => view,
                };
                // Find's step scrolls the list to the match's line (`code_find.rs`).
                let view = match list {
                    Some(list) => view.on_reveal(move |bounds, _, _| super::find_bar::reveal_bounds(&list, bounds)),
                    None => view,
                };
                div()
                    .w_full()
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .child(transcript::prose_running(view, palette, run))
                    .into_any_element()
            }
            None => div().into_any_element(),
        },
        // A thought draws as `code_folds::thought_row`, which opens to its text.
        Block::Thinking(..) => div().into_any_element(),
        Block::Tool { name, summary, .. } => {
            tool_row(("tool", ix), tool_mark(name, palette), name, summary, false, palette).into_any_element()
        }
        Block::Approval {
            tool,
            answer: Some(Decision::Allow),
            input,
            message: Some(message),
            ..
        } if tool == QUESTION_TOOL
            && let Some(rows) = super::code_dock::receipt_rows(&super::code_dock::questions_of(input), message) =>
        {
            // An answered question reads back as "✓ 3 of 3 answered", then "1. Question → answer" (`question.tsx`).
            let numbered = rows.len() > 1;
            let head = if numbered {
                format!("{} of {} answered", rows.len(), rows.len())
            } else {
                "Answered".to_string()
            };
            v_flex()
                .id(("approval", ix))
                .w_full()
                .gap(px(2.))
                .child(
                    h_flex()
                        .min_h(px(28.))
                        .gap(px(8.))
                        .text_size(px(13.))
                        .child(glyph(Lucide::Check, palette.muted))
                        .child(div().text_color(palette.muted).child(head)),
                )
                .children(rows.into_iter().enumerate().map(|(ix, (question, answer))| {
                    super::code_dock::receipt_row(ix, &question, Some(answer), numbered, palette).py(px(2.))
                }))
                .into_any_element()
        }
        Block::Approval {
            tool, answer, moved, ..
        } => {
            let (said, color) = match answer {
                None => ("Waiting for you", palette.foreground),
                Some(Decision::Deny) if tool == PLAN_TOOL && *moved => ("Moved to a new session", palette.faint),
                Some(Decision::Allow) if tool == QUESTION_TOOL => ("Answered", palette.faint),
                Some(Decision::Deny) if tool == QUESTION_TOOL => ("Skipped", palette.faint),
                Some(Decision::Allow | Decision::AllowSession) if tool == PLAN_TOOL => ("Approved", palette.faint),
                Some(Decision::Deny) if tool == PLAN_TOOL => ("Not approved", palette.faint),
                Some(Decision::Allow) => ("Allowed", palette.faint),
                Some(Decision::AllowSession) => ("Allowed for this session", palette.faint),
                Some(Decision::Deny) => ("Denied", palette.destructive),
                Some(Decision::Expired) => ("Not answered", palette.faint),
            };
            h_flex()
                .id(("approval", ix))
                .w_full()
                .min_h(px(28.))
                .gap(px(8.))
                .text_size(px(13.))
                .child(tool_mark(tool, palette))
                .child(div().text_color(palette.muted).child(ask_line(tool)))
                .child(div().text_color(color).child(said))
                .into_any_element()
        }
        // Waiting behind the turn, a message shows in the dock.
        // A turn's end has Restore to here; `Workspace::render_turn` draws it.
        Block::Setup(_) | Block::Queued { .. } | Block::Plan(_) | Block::Turn { .. } | Block::Page { .. } => {
            div().into_any_element()
        }
        // A sub-agent's row and its end have their own drawing (`Workspace::render_agent`, `render_agent_end`).
        Block::Agent(_) | Block::AgentEnd(_) => div().into_any_element(),
        Block::Inside { block, .. } => render_block(ix, block, text, run, list, palette, link, files),
        Block::Error(error, detail) => error_row(ix, error, detail.as_deref(), palette).into_any_element(),
        Block::Omitted => div()
            .text_sm()
            .text_color(palette.faint)
            .child(crate::machines::OMITTED)
            .into_any_element(),
        // A notice reads as a divider: a hairline on each side of its text.
        Block::Notice(notice) => notice_row(ix, notice.clone(), palette),
    }
}

/// The box a tool's output shows in: monospace on a raised ground.
fn output_box(palette: Palette) -> Div {
    div()
        .w_full()
        .px(px(12.))
        .py(px(8.))
        .rounded(px(8.))
        .bg(palette.raised)
        .font_family("Geist Mono")
        .text_size(px(11.5))
        .line_height(px(18.))
        .text_color(palette.muted)
}

/// An error, and what the agent printed under it in a tool output's box (`timeline-view.tsx`'s `wb-error`). A person
/// can select both and copy them, as the side pane's `error_note` allows, or copy both at once with Copy.
fn error_row(ix: usize, error: &str, detail: Option<&str>, palette: Palette) -> Div {
    with_copy(
        error_body(ix, error, detail, palette),
        error_copy(error, detail).into(),
        palette,
    )
}

/// What an error's Copy puts on the clipboard: the error, then what the agent printed under it.
fn error_copy(error: &str, detail: Option<&str>) -> String {
    match detail {
        Some(detail) => format!("{error}\n\n{detail}"),
        None => error.to_string(),
    }
}

/// An error's content beside its Copy (`code_block::copy_button`), at the top right.
fn with_copy(content: impl IntoElement, text: SharedString, palette: Palette) -> Div {
    h_flex()
        .w_full()
        .items_start()
        .gap(px(8.))
        .child(div().flex_1().min_w(px(0.)).child(content))
        .child(super::code_block::copy_button(text, palette))
}

fn error_body(ix: usize, error: &str, detail: Option<&str>, palette: Palette) -> Div {
    v_flex()
        .w_full()
        .gap(px(6.))
        .child(
            div()
                .debug_selector(|| "error-text".into())
                .text_sm()
                .text_color(palette.destructive)
                .child(MessageText::new(("error-text", ix), error.to_string())),
        )
        .when_some(detail, |column, detail| {
            column.child(
                output_box(palette)
                    .debug_selector(|| "error-detail".into())
                    .child(MessageText::new(("error-detail", ix), detail.to_string())),
            )
        })
}

/// A notice as a divider: its text between two hairlines. The text has its own box, allowed to shrink: unboxed, it keeps
/// one line as wide as the sentence and runs past the column. A long notice wraps between the lines, its lines starting
/// at the box's left edge: GPUI hit-tests a text's lines as if they start there, so a drag over a centred line would
/// select other characters than the ones under the pointer.
fn notice_row(ix: usize, notice: String, palette: Palette) -> AnyElement {
    div()
        .id(("notice", ix))
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.))
        .py(px(6.))
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(div().flex_1().min_w(px(16.)).h(px(1.)).bg(palette.border))
        .child(
            div()
                .debug_selector(|| "notice-text".into())
                .min_w(px(0.))
                .child(MessageText::new(("notice-text", ix), notice)),
        )
        .child(div().flex_1().min_w(px(16.)).h(px(1.)).bg(palette.border))
        .into_any_element()
}

/// Shared one-ink status shapes, as the sidebar and Nexus AI's Parallel agents guidelines.
pub(super) fn agent_mark(ix: usize, status: AgentStatus, palette: Palette) -> AnyElement {
    let mark = |status| super::sidebar::status_mark(&format!("agent-{ix}"), status, palette);
    match status {
        AgentStatus::Running | AgentStatus::Waiting => mark(Status::Running),
        AgentStatus::Needs => mark(Status::NeedsYou),
        AgentStatus::Done => glyph(Lucide::Check, palette.faint).into_any_element(),
        AgentStatus::Failed => mark(Status::Failed),
        AgentStatus::Stopped => div().size(px(8.)).mx(px(4.)).bg(palette.faint).into_any_element(),
        AgentStatus::Idle => glyph(Lucide::Check, palette.faint).into_any_element(),
    }
}

/// The end of a sub-agent's open body (`AgentBody`): "Starting…" or "No steps recorded." when nothing went under it,
/// the text it is still writing, and its result once it stopped running.
fn render_agent_end(end: &AgentEnd, draft: Option<&str>, palette: Palette) -> AnyElement {
    let draft = draft.filter(|_| end.running);
    let quiet = (end.empty && draft.is_none()).then_some(if end.running {
        "Starting…"
    } else {
        "No steps recorded."
    });
    let outcome = end.outcome.as_deref().filter(|_| !end.running);
    v_flex()
        .w_full()
        .gap(px(6.))
        .text_size(px(13.))
        .line_height(px(20.))
        .when_some(quiet, |body, quiet| {
            body.child(div().text_color(palette.faint).child(quiet))
        })
        .when_some(draft, |body, draft| {
            body.child(div().text_color(palette.muted).child(draft.to_string()))
        })
        .when_some(outcome, |body, outcome| {
            body.child(
                v_flex()
                    .w_full()
                    .gap(px(4.))
                    .pt(px(4.))
                    .child(div().text_size(px(11.5)).text_color(palette.faint).child("Result"))
                    .child(div().text_color(palette.muted).child(outcome.to_string())),
            )
        })
        .into_any_element()
}

/// A tool call's step row: its icon 14, its verb in muted words ("Ran" once `done`, "Run"
/// before), and what it was called on in mono 13; facts the caller adds sit at the right edge, faint and tabular.
pub(super) fn tool_row(
    id: (&'static str, usize),
    icon: impl IntoElement,
    name: &str,
    summary: &str,
    running: bool,
    palette: Palette,
) -> Stateful<Div> {
    // A call still running says so with a band of light across its verb, as Zeron's tool titles do.
    let verb = if running {
        crate::motion::shimmer("verb", tool_verb(name), palette.muted, palette.strong).into_any_element()
    } else {
        div()
            .text_color(palette.muted)
            .child(tool_done_verb(name))
            .into_any_element()
    };
    h_flex()
        .id(id)
        .w_full()
        .min_h(px(28.))
        .px(px(8.))
        .mx(px(-8.))
        .gap(px(8.))
        .rounded(px(6.))
        .text_size(px(14.))
        .hover(|style| style.bg(palette.hover))
        .child(
            div()
                .size(px(14.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(icon),
        )
        .child(div().flex_none().child(verb))
        .when(!summary.is_empty(), |row| {
            row.child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_family("Geist Mono")
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child(summary.to_string()),
            )
        })
}

/// What a finished call did, in the past tense the fold heads use ("Ran", "Edited"); other tools keep their name.
pub(super) fn tool_done_verb(name: &str) -> SharedString {
    match name {
        "Bash" => "Ran".into(),
        "Write" => "Wrote".into(),
        "Edit" | "MultiEdit" => "Edited".into(),
        "Grep" => "Searched".into(),
        "Glob" => "Found files".into(),
        "LS" => "Listed".into(),
        "WebFetch" => "Fetched".into(),
        "WebSearch" => "Searched the web".into(),
        "TodoWrite" => "Updated the plan".into(),
        name => tool_verb(name),
    }
}

/// What an approval row says the agent asked: "Asked to run a command", but a question is "Asked you a question".
pub(super) fn ask_line(tool: &str) -> String {
    match tool {
        QUESTION_TOOL => "Asked you a question".into(),
        tool => format!("Asked to {}", ask_verb(tool)),
    }
}

/// A tool's icon by its kind (`workbench/tool-row.tsx`'s `toolKind`).
/// An MCP tool gets the MCP mark, retaining the raw server identity in the adjacent label.
pub(super) fn tool_mark(name: &str, palette: Palette) -> AnyElement {
    let app = mcp_tool(name).and_then(|tool| tool.app.map(|(name, _)| name));
    if app == Some("Nexus") {
        return super::model_menu::maker_mark("nexus", 14., palette);
    }
    match app.and_then(crate::assets::connector) {
        Some((path, color)) => svg()
            .path(path)
            .size(px(14.))
            .flex_none()
            .text_color(color.unwrap_or(palette.foreground))
            .into_any_element(),
        None => glyph(tool_icon(name), palette.faint).into_any_element(),
    }
}

pub(super) fn tool_icon(name: &str) -> Lucide {
    match name {
        "Bash" | "BashOutput" | "KillShell" => Lucide::SquareTerminal,
        "Read" | "NotebookRead" => Lucide::FileText,
        "Edit" | "MultiEdit" | "NotebookEdit" => Lucide::FilePenLine,
        "Write" => Lucide::FilePlus,
        "Grep" => Lucide::Search,
        "Glob" | "LS" => Lucide::FolderSearch,
        "WebFetch" | "WebSearch" => Lucide::Globe,
        "Task" | "Agent" => Lucide::Bot,
        "TodoWrite" => Lucide::ListTodo,
        "ExitPlanMode" => Lucide::Map,
        QUESTION_TOOL => Lucide::CircleQuestionMark,
        name if name.starts_with("mcp__") => Lucide::Plug,
        _ => Lucide::Wrench,
    }
}

/// What a tool does, as a verb (`toolVerb`); other tools keep their name.
pub(super) fn tool_verb(name: &str) -> SharedString {
    match name {
        "Bash" => "Run".into(),
        "Read" => "Read".into(),
        "Write" => "Write".into(),
        "Edit" | "MultiEdit" => "Edit".into(),
        "Grep" => "Search".into(),
        "Glob" => "Find files".into(),
        "LS" => "List".into(),
        "WebFetch" => "Fetch".into(),
        "WebSearch" => "Search the web".into(),
        "Task" | "Agent" => "Agent".into(),
        "TodoWrite" => "Update the plan".into(),
        "ExitPlanMode" => "Plan".into(),
        name => match mcp_tool(name) {
            Some(mcp) => mcp.label().into(),
            None => name.to_string().into(),
        },
    }
}

/// Tools whose calls take no row. `ToolSearch` is the agent's plumbing: it loads a deferred tool's definition (its
/// summary reads `select:mcp__nexus__createPage`), and the tool's own call follows, and shows. A question's call is
/// said by its "Asked you a question" line, as the Nexus app places no row for it (`timeline.ts`).
pub(super) fn hidden_tool(name: &str) -> bool {
    matches!(name, "ToolSearch" | QUESTION_TOOL)
}

/// An app an MCP server talks to (`mcp-apps.ts`'s `APPS`): its name, and the names its servers go by (lowercase,
/// without `_` and `-`). Nexus's own built-in server first.
const MCP_APPS: &[(&str, &[&str])] = &[
    ("Nexus", &["nexus"]),
    ("Linear", &["linear"]),
    ("Slack", &["slack"]),
    ("GitHub", &["github"]),
    ("Notion", &["notion"]),
    ("Google Drive", &["googledrive", "gdrive"]),
    ("Gmail", &["gmail"]),
    ("Google Calendar", &["googlecalendar", "gcal"]),
    ("Vercel", &["vercel"]),
    ("Figma", &["figma"]),
    ("Sentry", &["sentry"]),
    ("Stripe", &["stripe"]),
    ("Atlassian", &["atlassian"]),
    ("Jira", &["jira"]),
    ("Confluence", &["confluence"]),
    ("PostHog", &["posthog"]),
    ("Chrome DevTools", &["chromedevtools", "chrome"]),
];

/// An MCP tool's call (`mcp__<server>__<tool>`), read as `mcp-apps.ts`'s `mcpTool` does.
#[derive(Debug, PartialEq)]
pub(super) struct McpTool {
    /// The server's name as the tool's name carries it (`claude_ai_Linear`).
    pub server: String,
    /// The tool's own name, in words (`list issues`, `create page`).
    pub tool: String,
    /// The app's name and aliases, when the server's name points to one.
    app: Option<(&'static str, &'static [&'static str])>,
}

impl McpTool {
    /// How its row reads: "Linear · list issues", "Nexus · create page". The app's name only for its genuine server,
    /// so a lookalike such as `evil-slack` keeps its raw name and cannot pass for Slack.
    pub fn label(&self) -> String {
        let app = self
            .app
            .map(|(name, _)| name)
            .filter(|name| self.server_name() == *name);
        format!("{} · {}", app.unwrap_or(&self.server), self.tool)
    }

    /// The server's name where it decides what runs (`mcpServerName`): the app's name alone when the server is named
    /// as the app is, else the app's name with the server's own ("Slack (evil-slack)"); no app, the server's name.
    pub fn server_name(&self) -> String {
        let Some((name, aliases)) = self.app else {
            return self.server.clone();
        };
        let mut words = server_words(&self.server);
        if words.first().is_some_and(|word| word == "plugin") {
            words.remove(0);
        }
        // A plugin's server is `plugin_<plugin>_<server>`: Linear's own reads `plugin_linear_linear`.
        let half = words.len() / 2;
        let twice = words.len() % 2 == 0 && words[..half].concat() == words[half..].concat();
        let own = if twice { words[..half].concat() } else { words.concat() };
        if aliases.contains(&own.as_str()) {
            name.to_string()
        } else {
            format!("{name} ({})", self.server)
        }
    }
}

/// An MCP tool's server, tool and app, or None when `name` is not an MCP tool's.
pub(super) fn mcp_tool(name: &str) -> Option<McpTool> {
    let (server, tool) = name.strip_prefix("mcp__")?.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    let app = mcp_app(server);
    let tool = match app {
        Some((_, aliases)) => without_app(tool, aliases),
        None => tool.to_string(),
    };
    Some(McpTool {
        server: server.to_string(),
        tool: words_of(&tool),
        app,
    })
}

/// The app a server's name points to: the whole name, then its trailing words (a plugin's `plugin_linear_linear`),
/// then its leading ones (`github_tools`).
fn mcp_app(server: &str) -> Option<(&'static str, &'static [&'static str])> {
    let mut words = server_words(server);
    if words.first().is_some_and(|word| word == "plugin") {
        words.remove(0);
    }
    let find = |key: String| {
        MCP_APPS
            .iter()
            .copied()
            .find(|(_, aliases)| aliases.contains(&key.as_str()))
    };
    (0..words.len())
        .find_map(|start| find(words[start..].concat()))
        .or_else(|| (1..words.len()).rev().find_map(|end| find(words[..end].concat())))
}

/// A server's name as words, without the connector's prefix and the words that say nothing (`mcp`, `server`).
fn server_words(server: &str) -> Vec<String> {
    let server = match server.get(..10) {
        Some(prefix) if prefix.eq_ignore_ascii_case("claude_ai_") => &server[10..],
        _ => server,
    };
    server
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty() && *word != "mcp" && *word != "server")
        .map(str::to_string)
        .collect()
}

/// A tool's name without the app's own at its start (Slack's `slack_read_channel` reads `read channel`), if more
/// follows.
fn without_app(tool: &str, aliases: &[&str]) -> String {
    let words: Vec<&str> = tool.split(['_', '-']).filter(|word| !word.is_empty()).collect();
    for end in 1..words.len() {
        if aliases.contains(&words[..end].concat().to_lowercase().as_str()) {
            return words[end..].join(" ");
        }
    }
    tool.to_string()
}

/// A tool's name as lowercase words: `_` and camelCase split (`createPage` reads `create page`), as Nexus's own
/// tools are named after its API's operations.
fn words_of(tool: &str) -> String {
    let mut words = String::new();
    let mut previous: Option<char> = None;
    for c in tool.chars() {
        if c == '_' || c == '-' || c == ' ' {
            if !words.ends_with(' ') && !words.is_empty() {
                words.push(' ');
            }
        } else {
            if c.is_uppercase() && previous.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit()) {
                words.push(' ');
            }
            words.extend(c.to_lowercase());
        }
        previous = Some(c);
    }
    words.trim_end().to_string()
}

/// The opening prompt and the stored message share the same image and bubble layout. The artifacts it referenced
/// (`artifact_refs.rs`) show as chips over the bubble, not as the block the agent read; `on_artifact` opens one.
pub(super) fn user_prompt(
    text: String,
    marks: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    images: Option<AnyElement>,
    palette: Palette,
    on_artifact: Option<std::rc::Rc<dyn Fn(crate::artifact_refs::ArtifactRef, &mut Window, &mut App)>>,
) -> Div {
    let (body, refs) = crate::artifact_refs::split(&text);
    let chips = (!refs.is_empty()).then(|| {
        let mut draft = crate::attachments::Draft::default();
        let _ = draft.add_artifacts(
            refs.into_iter()
                .map(|reference| crate::attachments::Attachment::artifact(reference, None))
                .collect(),
        );
        h_flex().w_full().justify_end().child(super::composer::artifact_chips(
            "code-sent",
            &draft,
            palette,
            None,
            None,
            None,
            on_artifact,
        ))
    });
    // Find's marks hold only before the block: the block shows as chips, and text after it moved up.
    let end = crate::artifact_refs::block_start(&text).unwrap_or(text.len());
    let marks: Vec<_> = marks.into_iter().filter(|(range, _)| range.end <= end).collect();
    let text = body;
    // A message of images only has no bubble.
    let bubble = (!text.is_empty()).then(|| user_bubble(text, marks, palette));
    v_flex()
        .w_full()
        .items_end()
        .gap(px(8.))
        .children(images)
        .children(chips)
        .children(bubble)
}

/// A person's message as a bubble at the column's right (`.msg-user`: the bubble tone, 14/22).
/// `marks`: find's matches in the text (`code_find.rs`).
pub(super) fn user_bubble(
    text: impl Into<SharedString>,
    marks: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    palette: Palette,
) -> impl IntoElement {
    let text = text.into();
    let empty = text.is_empty();
    // A message that quotes passages (stacked replies, `stacked_replies.rs`) draws each quote muted behind a bar. A
    // search's marks index the whole text, so a marked message stays one run.
    let segments = super::stacked_replies::segments(&text);
    let quoted = marks.is_empty() && segments.iter().any(|(quote, _)| *quote);
    let body = if quoted {
        v_flex()
            .min_w(px(0.))
            .gap(px(4.))
            .children(segments.into_iter().enumerate().map(|(ix, (quote, part))| {
                let part = super::message_text::MessageText::new(("user-text", ix), part);
                if quote {
                    h_flex()
                        .items_stretch()
                        .gap(px(10.))
                        .text_color(palette.muted)
                        .child(
                            div()
                                .flex_none()
                                .w(px(2.))
                                .my(px(2.))
                                .rounded(px(1.))
                                .bg(palette.border),
                        )
                        .child(div().min_w(px(0.)).child(part))
                        .into_any_element()
                } else {
                    div().min_w(px(0.)).child(part).into_any_element()
                }
            }))
            .into_any_element()
    } else {
        div()
            .min_w(px(0.))
            .child(super::message_text::MessageText::new("user-text", text).with_highlights(marks))
            .into_any_element()
    };
    h_flex().w_full().justify_end().when(!empty, |row| {
        row.child(
            h_flex()
                .debug_selector(|| "user-bubble".into())
                .max_w(relative(0.8))
                .items_end()
                .gap(px(10.))
                .px(px(14.))
                .py(px(8.))
                .rounded(px(12.))
                .bg(palette.bubble)
                .text_size(px(14.))
                .line_height(px(22.))
                .child(body),
        )
    })
}

/// Where a message the person sent stands with the agent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Receipt {
    /// In the transcript, not read yet.
    Sent,
    /// Read, at that time when the agent said so. An agent that never says (the Nexus agent, older logs) read it
    /// once it answered after it.
    Read(Option<i64>),
}

/// Message `seq`'s receipt: read when the agent said so (`code::Transcript::reads`) or answered after it.
pub(super) fn receipt_of(seq: i64, reads: &std::collections::BTreeMap<i64, i64>, blocks: &[Block]) -> Receipt {
    if let Some(at) = reads.get(&seq) {
        return Receipt::Read(Some(*at));
    }
    let answered = blocks
        .iter()
        .skip_while(|block| !matches!(block, Block::User { seq: at, .. } if *at == seq))
        .skip(1)
        .any(|block| {
            matches!(
                block,
                Block::Text(_)
                    | Block::Thinking(..)
                    | Block::Tool { .. }
                    | Block::Turn {
                        status: TurnStatus::Completed,
                        ..
                    }
            )
        });
    if answered { Receipt::Read(None) } else { Receipt::Sent }
}

/// The receipt under a message, at its right: one check once sent, two once read, quiet. While the message is
/// `hovered` the words show beside them ("Read 14:02", "Sent"); hidden, they keep their place, so nothing moves.
fn render_receipt(seq: i64, receipt: Receipt, agent: &'static str, hovered: bool, palette: Palette) -> AnyElement {
    let clock = |at: i64| {
        chrono::DateTime::from_timestamp_millis(at)
            .map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string())
            .unwrap_or_default()
    };
    let (words, tip): (String, SharedString) = match receipt {
        Receipt::Sent => ("Sent".into(), format!("Sent, not read by {agent} yet").into()),
        Receipt::Read(Some(at)) => (
            format!("Read {}", clock(at)),
            format!("Read by {agent} at {}", clock(at)).into(),
        ),
        Receipt::Read(None) => ("Read".into(), format!("Read by {agent}").into()),
    };
    let check = || Icon::new(Lucide::Check).size(px(12.)).text_color(palette.faint);
    h_flex()
        .id(("user-read", seq as u64))
        .debug_selector(|| "user-read".into())
        .flex_none()
        .h(px(24.))
        .pl(px(4.))
        .gap(px(4.))
        .text_size(px(12.))
        .text_color(palette.faint)
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(
            div()
                .whitespace_nowrap()
                .when(!hovered, |words| words.invisible())
                .child(words),
        )
        .child(
            h_flex()
                .debug_selector(|| "user-read-checks".into())
                .child(check())
                .when(matches!(receipt, Receipt::Read(_)), |checks| {
                    checks.child(div().ml(px(-7.)).child(check()))
                }),
        )
        .into_any_element()
}

/// Rewind's confirmation under a message (`user-message.tsx`'s popover): what it does and what it leaves, then
/// Cancel and Rewind.
fn render_rewind_confirm(rewinding: &Rewinding, palette: Palette, cx: &mut Context<Workspace>) -> impl IntoElement {
    let busy = rewinding.busy;
    v_flex()
        .w(px(320.))
        .gap(px(8.))
        .p(px(12.))
        .rounded(px(12.))
        .floating_ground(12., palette.pop, false, &palette)
        .shadow_md()
        .child(
            div()
                .text_size(px(13.5))
                .font_weight(FontWeight::MEDIUM)
                .child("Rewind to before this message?"),
        )
        .child(div().text_size(px(12.5)).line_height(px(18.)).text_color(palette.muted).child(
            "The prompt returns to the composer. Later messages stay visible, dimmed. Files, past commands, connector \
             changes, and pushes stay unchanged.",
        ))
        .when_some(rewinding.error.clone(), |card, error| {
            card.child(div().text_size(px(12.5)).text_color(palette.destructive).child(error))
        })
        .child(
            h_flex()
                .justify_end()
                .gap(px(8.))
                .child(
                    super::code_dock::answer_button("rewind-cancel", "Cancel", false, !busy, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_rewind(cx))),
                )
                .child(
                    super::code_dock::answer_button(
                        "rewind-confirm",
                        if busy { "Rewinding…" } else { "Rewind" },
                        true,
                        !busy,
                        palette,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_rewind(window, cx))),
                ),
        )
}

/// A person's message, as `Workspace::render_user` draws it.
struct UserMessage<'a> {
    seq: i64,
    at: i64,
    text: &'a str,
    linked: bool,
    rewound: bool,
    /// Find's matches in the text.
    marks: Vec<(std::ops::Range<usize>, HighlightStyle)>,
}

/// Whether a message takes Rewind and Edit (`edit-prompt.ts`'s `canEditMessage`): the person's own, still in the
/// conversation, in a session this build runs and has made. While the agent runs they show, but disabled.
pub(super) fn can_act_on(runs_here: bool, linked: bool, rewound: bool, pending: bool) -> bool {
    runs_here && !linked && !rewound && !pending
}

/// What an edit asks the runner for, in order (`user-message.tsx`'s `rewind(true)`).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum EditCall {
    /// `wb_rewind_message`: the conversation goes back to before the message.
    Rewind { thread: String, seq: i64 },
    /// `wb_send`: the edited text goes as a new message.
    Send { thread: String, text: String },
}

/// The calls that send `text` in place of message `seq`; none for an empty edit.
pub(super) fn edit_calls(thread: &str, seq: i64, text: &str, images: usize) -> Option<[EditCall; 2]> {
    if text.trim().is_empty() && images == 0 {
        return None;
    }
    Some([
        EditCall::Rewind {
            thread: thread.to_string(),
            seq,
        },
        EditCall::Send {
            thread: thread.to_string(),
            text: text.to_string(),
        },
    ])
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RestoredPrompt {
    seq: i64,
    text: String,
    images: Vec<nexus_workbench::event::Image>,
}

/// The first of the person's messages after the turn ending at event `turn` that a restore undoes, to put back in the
/// composer (`edit-prompt.ts`'s `promptAfter`); none from a linked session or already undone.
pub(super) fn prompt_after(blocks: &[Block], turn: i64) -> Option<RestoredPrompt> {
    blocks.iter().find_map(|block| match block {
        Block::User {
            seq,
            text,
            from: None,
            rewound: false,
            image_data,
            ..
        } if *seq > turn && (!text.trim().is_empty() || !image_data.is_empty()) => Some(RestoredPrompt {
            seq: *seq,
            text: text.clone(),
            images: image_data.clone(),
        }),
        _ => None,
    })
}

/// How long a restore may run before its confirmation gives up waiting (`revert.ts`'s `REVERT_DEADLINE_MS`).
const REVERT_DEADLINE: Duration = Duration::from_secs(60);

/// "Restore to here"'s line: what the restore would do to the files (`revert.ts`'s `revertQuestion`).
pub(super) fn restore_question(files: u32) -> String {
    match files {
        0 => "The files are already as they were after this turn. The turns after it stay here, dimmed, and the agent \
              moves on without them."
            .into(),
        1 => "1 file will change back to its version from this turn. The turns after it stay here, dimmed, and the \
              agent moves on without them."
            .into(),
        files => format!(
            "{files} files will change back to their version from this turn. The turns after it stay here, dimmed, \
             and the agent moves on without them."
        ),
    }
}

/// The editor in a message's place (`user-message.tsx`): the text, what sending does, then Cancel and Save & send.
fn render_editor(editing: &Editing, running: bool, palette: Palette, cx: &mut Context<Workspace>) -> impl IntoElement {
    let busy = editing.busy;
    let empty = editing.input.read(cx).value().trim().is_empty() && editing.images.is_empty();
    v_flex()
        .w(relative(0.8))
        .gap(px(8.))
        .p(px(12.))
        .rounded(px(12.))
        .bg(palette.bubble)
        // Esc leaves the editor, as the Nexus app's.
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                this.cancel_edit(window, cx);
            }
        }))
        .child(
            div()
                .text_size(px(14.))
                .line_height(px(22.))
                // The field pads its own text; pulled out by that much, it lines up with the note under it.
                .mx(px(-10.))
                .child(Textarea::new(&editing.input).appearance(false)),
        )
        .child(div().text_size(px(12.5)).line_height(px(18.)).text_color(palette.muted).child(
            "Sending replaces this message and later replies. Files, past commands, connector changes, and pushes stay \
             unchanged.",
        ))
        .when_some(editing.error.clone(), |card, error| {
            card.child(div().text_size(px(12.5)).text_color(palette.destructive).child(error))
        })
        .child(
            h_flex()
                .justify_end()
                .gap(px(8.))
                .child(
                    super::code_dock::answer_button("edit-cancel", "Cancel", false, !busy, palette)
                        .on_click(cx.listener(|this, _, window, cx| this.cancel_edit(window, cx))),
                )
                .child(
                    super::code_dock::answer_button(
                        "edit-send",
                        if busy { "Sending…" } else { "Save & send" },
                        true,
                        !busy && !empty && !running,
                        palette,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.send_edit(window, cx))),
                ),
        )
}

/// "Restore to here"'s confirmation (`revert-turn.tsx`'s popover): how many files would change, then Cancel and
/// Restore. A failure says why, with Close.
fn render_restore_confirm(step: &RestoreStep, palette: Palette, cx: &mut Context<Workspace>) -> impl IntoElement {
    let (said, failed): (SharedString, bool) = match step {
        RestoreStep::Counting => ("Checking what would change…".into(), false),
        RestoreStep::Ready(files) | RestoreStep::Restoring(files) => (restore_question(*files).into(), false),
        RestoreStep::Failed(error) => (error.clone(), true),
    };
    let restoring = matches!(step, RestoreStep::Restoring(_));
    v_flex()
        .w(px(320.))
        .gap(px(8.))
        .p(px(12.))
        .rounded(px(12.))
        .floating_ground(12., palette.pop, false, &palette)
        .shadow_md()
        .child(
            div()
                .text_size(px(13.5))
                .font_weight(FontWeight::MEDIUM)
                .child("Restore to this turn?"),
        )
        .child(
            div()
                .text_size(px(12.5))
                .line_height(px(18.))
                .text_color(if failed { palette.destructive } else { palette.muted })
                .child(said),
        )
        .child(
            h_flex()
                .justify_end()
                .gap(px(8.))
                .child(
                    super::code_dock::answer_button(
                        "restore-cancel",
                        if failed { "Close" } else { "Cancel" },
                        false,
                        !restoring,
                        palette,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_restore(cx))),
                )
                .when(!failed, |row| {
                    row.child(
                        super::code_dock::answer_button(
                            "restore-confirm",
                            if restoring { "Restoring…" } else { "Restore" },
                            true,
                            matches!(step, RestoreStep::Ready(_)),
                            palette,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.confirm_restore(window, cx))),
                    )
                }),
        )
}

/// The composer's text after a rewind puts the message back (`edit-prompt.ts`'s `withRestored`): what is already
/// typed stays first.
pub(super) fn restored_draft(draft: &str, prompt: &str) -> String {
    nexus_desktop_core::chat_rewind::restored_text(draft, prompt)
}

/// The new session's first message (`implementPrompt`): the plan, said to be approved, so the agent starts on it.
pub(super) fn implement_prompt(plan: &str) -> String {
    format!(
        "Implement this plan. It was made and approved in an earlier session.\n\n{}",
        plan.trim()
    )
}

/// The new session's title (`implementTitle`).
pub(super) fn implement_title(plan: &str) -> String {
    match plan_title(plan) {
        Some(title) => format!("Implement {title}"),
        None => "Implement the plan".into(),
    }
}

/// "Implement in a new session" for `session`'s plan: the session to make (same project, agent, model, effort and
/// payer, in Auto-accept edits, in a worktree when this one has one), and what to do once it is made.
pub(super) fn implement_elsewhere(session: &CodeSession, request: &str, plan: &str) -> (NewSession, MovedFrom) {
    let new = NewSession {
        project_id: session.project_id.clone(),
        agent: session.agent,
        model: session.model.clone(),
        permission_mode: PermissionMode::AcceptEdits.as_str().to_string(),
        effort: session.effort,
        via: session.via,
        provider: None,
        worktree: session.branch.is_some(),
        review: None,
        base: None,
        text: implement_prompt(plan),
        attachments: Default::default(),
    };
    let moved = MovedFrom {
        thread: session.id.clone(),
        request: request.to_string(),
        title: implement_title(plan),
    };
    (new, moved)
}

/// What the runner is asked once the new session `id` exists: its title, then the planning session told to stop.
#[derive(Debug)]
pub(super) enum MovedOnCall {
    Title(UpdateArgs),
    Stop {
        thread: String,
        request: String,
        message: String,
    },
}

pub(super) fn moved_on(id: &str, moved: MovedFrom) -> Vec<MovedOnCall> {
    vec![
        MovedOnCall::Title(UpdateArgs {
            thread_id: id.to_string(),
            title: Some(moved.title),
            ..UpdateArgs::default()
        }),
        MovedOnCall::Stop {
            thread: moved.thread,
            request: moved.request,
            message: MOVED_ON.to_string(),
        },
    ]
}

#[cfg(test)]
mod tests;

/// What a sleeping cloud session shows when Gateway holds none of its messages (cloud history off for the
/// organization, or a session from before it was kept) and its machine has not been read.
pub(super) const ASLEEP_UNREAD: &str = "Its messages show here once you send one.";

/// A missing session is conclusive only after the overview loads and remote starts settle.
fn missing_session(loaded: bool, found: bool, draft: bool, connecting: bool) -> bool {
    loaded && !found && !draft && !connecting
}

#[cfg(test)]
mod run_tests {
    use super::run_offered;
    // `gpui_kit::*` brings a `test` attribute of its own: the standard one is named.
    use core::prelude::v1::test;

    #[test]
    fn run_shows_here_and_on_a_server_that_shares_its_commands_never_on_a_cloud_machine() {
        let is_cloud = |machine: &str| machine == "m_cloud";
        let shares = |machine: &str| machine != "m_old";
        assert!(run_offered(None, is_cloud, shares), "this computer");
        assert!(run_offered(Some("m_server"), is_cloud, shares), "a server's Commands");
        assert!(
            !run_offered(Some("m_old"), is_cloud, shares),
            "a host without the shared terminal"
        );
        assert!(!run_offered(Some("m_cloud"), is_cloud, shares), "a cloud machine");
    }
}

#[cfg(test)]
mod missing_tests {
    use super::missing_session;
    #[test]
    fn missing_state_waits_for_sources_and_preserves_drafts_and_existing_sessions() {
        assert!(missing_session(true, false, false, false));
        assert!(!missing_session(false, false, false, false));
        assert!(!missing_session(true, true, false, false));
        assert!(!missing_session(true, false, true, false));
        assert!(!missing_session(true, false, false, true));
    }
}

#[cfg(test)]
#[path = "tool_row_editor_tests.rs"]
mod tool_row_editor_tests;

#[cfg(test)]
#[path = "code_view_scroll_tests.rs"]
mod scroll_tests;

#[cfg(test)]
mod notice_tests {
    use super::{Palette, notice_row};
    // `gpui_kit::*` brings a `test` attribute of its own: the standard one is named, so `#[gpui_kit::test]` expands.
    use core::prelude::v1::test;
    use gpui_kit::prelude::*;
    use gpui_kit::*;

    struct Column {
        width: f32,
        text: String,
    }

    impl Render for Column {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(self.width)).child(notice_row(
                0,
                self.text.clone(),
                Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark),
            ))
        }
    }

    #[gpui_kit::test]
    fn long_notice_wraps_inside_the_column(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (column, cx) = cx.add_window_view(|_, _| Column {
            width: 300.,
            text: "Compacted.".into(),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let one_line = cx.debug_bounds("notice-text").expect("the notice is drawn").size.height;
        column.update(cx, |column, cx| {
            column.text =
                "The provider requested a command outside the workbench, and this notice is longer than the column."
                    .into();
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let shown = cx.debug_bounds("notice-text").expect("the notice is drawn");
        assert!(shown.right() <= px(300.), "{shown:?} runs past the column");
        assert!(shown.size.height >= one_line * 2., "{shown:?} kept one line");
        // A hairline (16) and the gap (10) stay before the text.
        assert!(shown.left() >= px(26.), "{shown:?} covers the hairline");
    }
}

/// An error, its detail and a notice can be selected and copied, as a message can.
#[cfg(test)]
mod copy_tests {
    use super::{Palette, error_row, notice_row};
    // `gpui_kit::*` brings a `test` attribute of its own: the standard one is named, so `#[gpui_kit::test]` expands.
    use core::prelude::v1::test;
    use gpui_kit::base::{TextSelection, TextSelectionLayer};
    use gpui_kit::prelude::*;
    use gpui_kit::*;

    struct Rows;

    impl Render for Rows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
            div().size_full().child(TextSelectionLayer).child(
                div()
                    .w(px(600.))
                    .child(error_row(
                        0,
                        "Antigravity could not start (exit 1).",
                        Some("bwrap failed: No such file or directory"),
                        palette,
                    ))
                    .child(notice_row(1, "Compacted the conversation.".into(), palette)),
            )
        }
    }

    /// Double-clicks the first word of the text under `selector`, `inset` from its left edge, and reads the selection.
    fn first_word(selector: &'static str, inset: f32, cx: &mut VisualTestContext) -> String {
        let bounds = cx.debug_bounds(selector).expect("the row is drawn");
        let at = point(bounds.left() + px(inset), bounds.center().y);
        cx.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::none(),
            first_mouse: false,
        });
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            TextSelection::selected_text(window, cx).to_string()
        })
    }

    #[gpui_kit::test]
    fn error_its_detail_and_notice_text_can_be_selected(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| Rows);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(first_word("error-text", 4., cx), "Antigravity");
        // The detail's box has 12 of padding before its text.
        assert_eq!(first_word("error-detail", 16., cx), "bwrap");
        assert_eq!(first_word("notice-text", 4., cx), "Compacted");
    }

    /// An error's Copy puts the error and what the agent printed under it on the clipboard, in one click.
    #[gpui_kit::test]
    fn copy_takes_the_error_and_its_detail(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| Rows);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let copy = cx.debug_bounds("code-copy").expect("the error's Copy is drawn");
        cx.simulate_click(copy.center(), Modifiers::none());
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
            Some("Antigravity could not start (exit 1).\n\nbwrap failed: No such file or directory")
        );
    }
}
