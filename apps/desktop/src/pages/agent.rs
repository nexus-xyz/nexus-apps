//! The Assistant (the Nexus app's Assistant page, `/app/<org>/agent`): the person's own agent, the one
//! `@nexus` is in Slack, with the same notes, saved model and opt-ins. It shows only with an agent API
//! (`nexus_desktop_core::agent::configured`); the workspace's sidebar and `open_agent` hold that gate.
//!
//! The pane's state is `crate::agent::Pane`; this file draws it and runs its calls through the backend, which holds
//! the token. The answer streams as Markdown, with its sources and the line under it. The pane never writes: a write
//! the agent asks for is the approval card in the thread. When a long answer or an approval arrives while no app window
//! has the focus, a system notification says so, without the reply's words (`notify::agent`).
//!
//! It reads as a text thread with Nexus (brand v5 A1, A13): a header with the Nexus mark at 60 px, "Nexus" and the
//! scope (Personal or the organization), condensed into one line at the top once the thread scrolls under it; Nexus's
//! replies in bubbles beside its 28 px mark, which breathes while it thinks; the person's own bubbles with Delivered,
//! Read or Not sent under the last one; and a line when the app is offline. The workspace keeps the page while the
//! person is elsewhere (`workspace/assistant.rs`), so an answer goes on streaming and the sidebar row can say so.
//!
//! The first time the person opens it, the empty chat is the first open (A19): under the header, one warm line, what
//! the agent can read, up to four starters and two quiet links. Under an answer that needed an account, a Connect card
//! (A6·e) opens the connector's card in Nexus UI in the browser. A write that waits draws the approval card (A9): its
//! tier, what, To, the amount, Why, the exact text and when it expires, then Decline and Approve, which reach the agent
//! API with the person's own token (`decisions.rs`). A write that always asks has "Review in Nexus", in the browser.
//!
//! It feels like texting a friend, drawn as brand v5 A1, A22 and A24 draw it. The header is Nexus's
//! contact: a click or ⌘I opens its card (`contact_card.rs`). The person's bubbles are raised with a hairline on the
//! right, Nexus's in the select tone on the left with its avatar, and each reply has its time under it in mono. A
//! centered time line comes before the first message and after 15 quiet minutes or on a new day (`thread_layout.rs`).
//! Before its first word Nexus types: three pulsing dots in its bubble. A sent bubble rises from the composer, a reply
//! pops in, and a receipt cross-fades into the next one; under less motion they show in place. A soft sound plays on
//! send and on a reply (`sounds.rs`). Hovering a bubble offers the six tapbacks; the one picked sits in a 22 px pill on
//! its corner (`reactions.rs`).
//!
//! Voice mode (A18, A25) is `crate::voice`: a voice button before the send circle opens its pane beside the thread.
//! When the call ends, its card lands here, then the writes it asked for as approval cards. macOS only for now.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Ctx, palette, ui};
use crate::agent::{Call, Chunk, Effect, Entry, Message, Pane, Sent, placeholder};
use crate::palette::Palette;
use nexus_desktop_core::agent as rules;
use nexus_desktop_core::agent_cards as cards;

/// Whether the app draws this page (true) or opens it in the browser.
pub const NATIVE: bool = true;

/// Nexus's avatar beside its bubbles, and in the expanded header (brand v5 A13·A).
pub const AVATAR: f32 = 28.;
pub const HEADER_AVATAR: f32 = 60.;
/// The condensed header's mark: under 24 px, the one-ink small cut (A13).
const BAR_AVATAR: f32 = 18.;
/// The thread condenses its header once it has scrolled this far: about where the name leaves the view.
const CONDENSE_AT: f32 = 96.;
/// While the app is offline, a message not sent is tried again this often, as "Messages send when you're back" says.
const RESEND_EVERY: Duration = Duration::from_secs(15);
/// What the thread says while the agent service does not answer (A1·C·i).
pub const OFFLINE: &str = "You’re offline. Messages send when you’re back.";

/// The page, kept by the workspace while the person is elsewhere (`workspace/assistant.rs`). `scope` is "Personal" or
/// the organization's name, under Nexus in the header.
pub fn build(
    ctx: Ctx,
    conversation: Option<String>,
    scope: SharedString,
    person: Person,
    reactions: Reactions,
    window: &mut Window,
    cx: &mut App,
) -> Entity<AgentPage> {
    cx.new(|cx| {
        let mut page = AgentPage::new(ctx, conversation, scope, person, window, cx);
        page.reactions = reactions;
        page
    })
}

mod brief;
mod composer;
mod contact_card;
mod decisions;
mod reactions;
mod sign_in;
mod thread_cards;
mod thread_layout;
mod thread_links;
mod watch_card;
#[cfg(test)]
pub use contact_card::{ACCOUNTS_PATH as CARD_ACCOUNTS, SETTINGS_WEB as CARD_SETTINGS};
pub use decisions::Decisions;
#[cfg(test)]
pub use decisions::Phase;
use nexus_desktop_core::agent_cards::Decision;
pub use reactions::{Reactions, Tapback};
use thread_cards::connect_card;
pub use thread_cards::{Approval, Decides, EditBoxes, approval_card};
pub use thread_layout::{Item, label as time_label};

/// Who the first open greets: the first word of their name, and whether this is their Personal account. `assistant`:
/// the name they gave their Assistant, Nexus when None.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Person {
    pub first_name: Option<String>,
    pub personal: bool,
    pub assistant: Option<SharedString>,
}

/// Which call a stream answers.
#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Ask,
    Read,
    Intro,
    /// `POST /agent/intro/seen`: its answer changes nothing.
    Seen,
    /// A decision on this card (A9).
    Decide(String),
    /// A Save on this card's Edit (A24·b).
    Edit(String),
}

/// What the sidebar's Assistant row shows (brand v5 A21·c): Nexus's replies the person has not seen, an answer
/// streaming, and a write that waits for the person's approval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowState {
    pub unread: u32,
    pub thinking: bool,
    pub needs_you: bool,
}

pub struct AgentPage {
    ctx: Ctx,
    pane: Pane,
    composer: Entity<TextareaState>,
    composer_empty: bool,
    /// The Auto chip, the `@` and `/` menus and the chips they leave in the box (A24·c, `composer.rs`).
    boxed: composer::ComposerBox,
    /// The Markdown of each answer, by entry and message, and of the answer being written.
    answers: HashMap<(usize, usize), Entity<TextViewState>>,
    draft: Option<Entity<TextViewState>>,
    /// The ask and the read-back in flight: dropping one stops its call.
    asking: Option<Task<()>>,
    _reading: Option<Task<()>>,
    /// The first open's read, then the call that keeps it seen.
    _intro: Option<Task<()>>,
    _seen: Option<Task<()>>,
    person: Person,
    /// The person's decisions on waiting writes, and their calls in flight by card.
    decisions: Decisions,
    /// The inputs of the cards whose Edit is open (A24·b).
    edit_boxes: EditBoxes,
    _deciding: HashMap<String, Task<()>>,
    /// The next try of a message not sent while the app is offline.
    _resend: Option<Task<()>>,
    scroll: ScrollHandle,
    /// "Personal" or the organization's name.
    scope: SharedString,
    /// The Assistant's name: the person's choice, Nexus by default. The header, the thread's own lines and
    /// the notification use it; "Nexus" stays where it names the product.
    name: SharedString,
    /// The person is looking at this page (`Workspace::sync_assistant`): a reply counts as seen.
    viewing: bool,
    /// Nexus's replies since the person last looked.
    unread: u32,
    /// The thread's groups and time lines, made when the thread changes (`thread_layout.rs`).
    items: Vec<Item>,
    /// What arrived just now, so it moves once: a sent bubble, a reply, the typing bubble.
    arrivals: Arrivals,
    /// The receipt under the person's last message before it changed, and when: the two cross-fade.
    receipt_was: Option<(Sent, Instant)>,
    /// The typing bubble's dots, a view of their own so their frames redraw only them.
    typing: Entity<crate::motion::TypingDots>,
    /// Nexus's contact card is open (the header, or ⌘I).
    card_open: bool,
    /// The bubble whose tapback picker is open, by its key (`reactions::key`).
    picking: Option<String>,
    /// The tapbacks, per conversation, kept on this computer with the Assistant's marks.
    reactions: Reactions,
    /// The window the page draws in: ⌘I and Esc act only there.
    window_id: WindowId,
    /// The errands that need the person, for the overview card at the top (A12·c), and how a row opens one.
    errands: Vec<nexus_desktop_core::agent_errands::Errand>,
    open_errand: Option<OpenErrand>,
    /// Open one of the Assistant's views from the contact card (A24·d): the workspace gives it.
    open_view: Option<crate::pages::assistant_views::Open>,
    /// Every errand as the latest list has it, by id: an errand card shows its state now (A1·5).
    errand_now: HashMap<String, nexus_desktop_core::agent_errands::Errand>,
    /// How a desktop session card's `nexus://new?` link opens: a New session draft, never sent (A1·5).
    open_draft: Option<OpenErrand>,
    /// Where each watch proposal in the thread stands (A3·b, `watch_card.rs`), and the Keep calls in flight.
    watch_stands: HashMap<(usize, usize), watch_card::Stand>,
    _watch_keeps: Vec<Task<()>>,
    /// The thread's quick replies and link cards, read when the thread changes (`thread_links.rs`).
    links: thread_links::Links,
    /// The arc of each card that runs now, by card key: a view each, so only the arc redraws.
    arcs: HashMap<String, Entity<thread_links::TurningArc>>,
    /// The live step line while Nexus thinks (A1·C b), a view of its own.
    step: Entity<thread_links::StepLine>,
    /// Voice mode's pane while a call is open, and until an answer still coming has landed.
    voice: Option<(Entity<crate::voice::VoicePane>, Subscription)>,
    /// Today's morning brief (A1), at the top of the thread (`brief.rs`).
    brief: brief::BriefState,
    /// A login Nexus asks to use waits over the chat for the person's answer (A8·b), as on Passwords and payments.
    vault: Entity<crate::pages::assistant_views::VaultPage>,
    _subscriptions: Vec<Subscription>,
}

/// Open one errand's thread, from the chat's overview card.
pub type OpenErrand = std::rc::Rc<dyn Fn(String, &mut Window, &mut App)>;

/// How long after it arrives a message may still move: a frame drawn late still plays the whole motion, and a thread
/// shown again later does not move.
const FRESH: Duration = Duration::from_millis(600);

/// What arrived just now, by entry.
#[derive(Default)]
struct Arrivals {
    sent: Option<(usize, Instant)>,
    received: Option<(usize, Instant)>,
    typing: Option<Instant>,
}

impl Arrivals {
    fn fresh(mark: Option<(usize, Instant)>, ix: usize) -> bool {
        mark.is_some_and(|(at, since)| at == ix && since.elapsed() < FRESH)
    }
}

impl AgentPage {
    fn new(
        ctx: Ctx,
        conversation: Option<String>,
        scope: SharedString,
        person: Person,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AgentPage {
        let (mut pane, read) = Pane::new(conversation.as_deref());
        let intro = pane.intro_call();
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(placeholder(&pane))
                .submit_on_enter(true)
                .auto_grow(1, 8)
        });
        composer.update(cx, |state, cx| state.focus(window, cx));
        let this = cx.entity().downgrade();
        let subscriptions = vec![
            // Enter sends; Shift+Enter is a new line.
            cx.subscribe_in(
                &composer,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                    InputEvent::Change => {
                        let empty = state.read(cx).value().trim().is_empty();
                        if empty != this.composer_empty {
                            this.composer_empty = empty;
                            cx.notify();
                        }
                        this.composer_changed(window, cx);
                    }
                    _ => {}
                },
            ),
            // On this page: ⌘I (Ctrl+I) opens or closes Nexus's contact card, and Esc closes the card or the tapback
            // picker first. Esc in the composer then stops the answer, as the Nexus app's textarea. Read before the text
            // field's own keys.
            cx.intercept_keystrokes({
                let this = this.clone();
                move |event, window, cx| {
                    let keystroke = &event.keystroke;
                    let modifiers = keystroke.modifiers;
                    let card_key = keystroke.key == "i"
                        && modifiers.secondary(cx)
                        && !modifiers.shift
                        && !modifiers.alt
                        && modifiers.number_of_modifiers() == 1;
                    let escape = keystroke.key == "escape" && !modifiers.modified();
                    // ↑ ↓ Enter Tab and Backspace belong to the composer's menus and chips while they are open (A24·c).
                    let menu_key = !modifiers.modified()
                        && matches!(keystroke.key.as_str(), "up" | "down" | "enter" | "tab" | "backspace");
                    if !card_key && !escape && !menu_key {
                        return;
                    }
                    let handled = this.update(cx, |this, cx| {
                        let here = this.viewing && window.window_handle().window_id() == this.window_id;
                        if !here {
                            return false;
                        }
                        let in_composer = this.composer.read(cx).focus_handle(cx).is_focused(window);
                        let key = keystroke.key.as_str();
                        if (menu_key || escape)
                            && (in_composer || escape)
                            && this.composer_wants(key, this.composer_empty)
                        {
                            // The picker's Esc works from anywhere on the page; the rest only from the field.
                            if in_composer || key == "escape" {
                                this.composer_key(key, window, cx);
                                return true;
                            }
                        }
                        if menu_key {
                            return false;
                        }
                        if card_key {
                            this.picking = None;
                            this.toggle_card(cx);
                            return true;
                        }
                        if this.card_open || this.picking.is_some() {
                            this.card_open = false;
                            this.picking = None;
                            cx.notify();
                            return true;
                        }
                        let focused = this.composer.read(cx).focus_handle(cx).is_focused(window);
                        if focused && this.pane.busy() {
                            this.stop(window, cx);
                            return true;
                        }
                        false
                    });
                    if handled.unwrap_or(false) {
                        cx.stop_propagation();
                    }
                }
            }),
        ];
        let name = person
            .assistant
            .clone()
            .unwrap_or_else(|| cards::DEFAULT_ASSISTANT_NAME.into());
        let vault = crate::pages::assistant_views::vault_asks(ctx.clone(), window, cx);
        let mut page = AgentPage {
            vault,
            ctx,
            pane,
            composer,
            composer_empty: true,
            boxed: composer::ComposerBox::default(),
            answers: HashMap::new(),
            draft: None,
            asking: None,
            _reading: None,
            _intro: None,
            _seen: None,
            person,
            decisions: Decisions::default(),
            edit_boxes: EditBoxes::default(),
            _deciding: HashMap::new(),
            _resend: None,
            scroll: ScrollHandle::new(),
            scope,
            name,
            // Built to be shown: the workspace says otherwise when the person leaves.
            viewing: true,
            unread: 0,
            items: Vec::new(),
            arrivals: Arrivals::default(),
            receipt_was: None,
            typing: cx.new(|_| crate::motion::TypingDots),
            card_open: false,
            picking: None,
            reactions: Reactions::default(),
            window_id: window.window_handle().window_id(),
            errands: Vec::new(),
            open_errand: None,
            open_view: None,
            errand_now: HashMap::new(),
            open_draft: None,
            watch_stands: HashMap::new(),
            _watch_keeps: Vec::new(),
            links: thread_links::Links::default(),
            arcs: HashMap::new(),
            step: cx.new(|_| thread_links::StepLine::new()),
            voice: None,
            brief: brief::BriefState::default(),
            _subscriptions: subscriptions,
        };
        page.read_brief(window, cx);
        page.load_model(window, cx);
        if let Some(call) = read {
            page._reading = Some(page.run(call, Kind::Read, window, cx));
        }
        if let Some(call) = intro {
            page._intro = Some(page.run(call, Kind::Intro, window, cx));
        }
        page
    }

    /// What the sidebar's row shows now.
    pub fn row(&self) -> RowState {
        RowState {
            unread: self.unread,
            thinking: self.pane.busy(),
            needs_you: self.pane.needs_you() && !self.decided_last(),
        }
    }

    /// Where the person's last message stands, and whether the thread says the app is offline.
    #[cfg(test)]
    pub fn sent(&self) -> (Option<Sent>, bool) {
        (self.pane.sent, self.pane.offline)
    }

    /// The Assistant's name, as the page draws it.
    #[cfg(test)]
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// The person's profile was read (`workspace/assistant.rs`): the page names the Assistant as they named it.
    pub fn set_name(&mut self, name: SharedString, cx: &mut Context<Self>) {
        if name != self.name {
            self.name = name;
            cx.notify();
        }
    }

    /// How the contact card reaches the Assistant's views (`workspace/assistant_errands.rs`).
    pub fn set_open_view(&mut self, open: crate::pages::assistant_views::Open) {
        self.open_view = Some(open);
    }

    /// The errands that need the person (`workspace/assistant_errands.rs`): the overview card names them, and a row
    /// opens its thread with `open`.
    pub fn set_errands(
        &mut self,
        errands: Vec<nexus_desktop_core::agent_errands::Errand>,
        open: OpenErrand,
        cx: &mut Context<Self>,
    ) {
        self.open_errand = Some(open);
        if errands != self.errands {
            self.errands = errands;
            cx.notify();
        }
    }

    /// Every errand as the agent API listed it last, open or closed: an errand card in the thread shows its state now,
    /// and an errand the list no longer has keeps the state its notice carried.
    pub fn set_errand_states(&mut self, rows: Vec<nexus_desktop_core::agent_errands::Errand>, cx: &mut Context<Self>) {
        let mut changed = false;
        for row in rows {
            if self.errand_now.get(&row.id) != Some(&row) {
                self.errand_now.insert(row.id.clone(), row);
                changed = true;
            }
        }
        if changed {
            self.keep_arcs(cx);
            cx.notify();
        }
    }

    /// How a desktop session card opens its draft (`Workspace::open_prompt_link`).
    pub fn set_open_draft(&mut self, open: OpenErrand) {
        self.open_draft = Some(open);
    }

    /// Open a desktop session card's `nexus://new?` link as a New session draft. It never sends.
    pub(crate) fn open_draft(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.open_draft.clone() {
            open(url.to_string(), window, cx);
        }
    }

    /// A turning arc for each card that runs now, and none for the rest.
    fn keep_arcs(&mut self, cx: &mut Context<Self>) {
        let turning = self.links.turning(&self.errand_now);
        self.arcs.retain(|key, _| turning.contains(key));
        for key in turning {
            self.arcs
                .entry(key)
                .or_insert_with(|| cx.new(|_| thread_links::TurningArc));
        }
    }

    /// The quick replies under the thread's last reply, for the view tests.
    #[cfg(test)]
    pub fn quick_replies(&self) -> Vec<String> {
        self.shown_replies().unwrap_or_default().to_vec()
    }

    /// The step line drawn now, while Nexus thinks, for the view tests.
    #[cfg(test)]
    pub fn step_line(&self, cx: &App) -> Option<String> {
        self.pane
            .draft
            .as_ref()
            .and_then(crate::agent::Draft::thinking_line)
            .map(|_| self.step.read(cx).line().to_string())
    }

    /// The link cards drawn now, for the view tests: each errand card's name, state and line, and each session card's
    /// title and line.
    #[cfg(test)]
    pub fn link_cards(&self) -> Vec<String> {
        use nexus_desktop_core::agent_thread as words;
        let mut keys: Vec<_> = self.links.messages.keys().copied().collect();
        keys.sort();
        keys.iter()
            .flat_map(|key| {
                let links = &self.links.messages[key];
                let errand = links.errand.as_ref().map(|errand| {
                    let now = self.errand_now.get(&errand.id).unwrap_or(errand);
                    format!(
                        "errand {} · {} · {}",
                        nexus_desktop_core::agent_errands::errand_name(now),
                        words::errand_state_word(now.status),
                        words::errand_card_line(now)
                    )
                });
                let sessions = links.sessions.iter().map(|session| {
                    format!(
                        "session {} · {}",
                        session.title,
                        words::session_card_line(session).lead()
                    )
                });
                errand.into_iter().chain(sessions).collect::<Vec<_>>()
            })
            .collect()
    }

    /// The cards that turn now, for the view tests.
    #[cfg(test)]
    pub fn turning(&self) -> usize {
        self.arcs.len()
    }

    /// The quick replies to draw: the last reply's, while nothing is sent after it and no answer is under way.
    fn shown_replies(&self) -> Option<&[String]> {
        if self.pane.busy() {
            return None;
        }
        let last = self.pane.entries.len().checked_sub(1)?;
        self.links.replies.get(&last).map(Vec::as_slice)
    }

    /// The grant sheets over the chat, for the view tests.
    #[cfg(test)]
    pub fn vault(&self) -> Entity<crate::pages::assistant_views::VaultPage> {
        self.vault.clone()
    }

    /// The errands the overview card names, for the view tests.
    #[cfg(test)]
    pub fn errands_card(&self) -> Option<String> {
        nexus_desktop_core::agent_errands::overview_title(self.errands.len())
    }

    /// The overview card (A12·c): "2 errands need you", then one row per errand, at most three: the still mark, its
    /// name and what it needs. A row opens that errand's thread. Nothing when none needs the person.
    fn errands_overview(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        use nexus_desktop_core::agent_errands as words;
        let title = words::overview_title(self.errands.len())?;
        let rows = self
            .errands
            .iter()
            .take(words::OVERVIEW_ERRANDS)
            .enumerate()
            .map(|(at, errand)| {
                let id = errand.id.clone();
                h_flex()
                    .id(("agent-errand-row", at))
                    .w_full()
                    .gap(px(10.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .child(crate::pages::assistant_views::errand_mark(
                        errand.status,
                        false,
                        palette,
                    ))
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(220.))
                            .truncate()
                            .text_size(px(14.))
                            .text_color(palette.foreground)
                            .child(words::errand_name(errand).to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .child(words::errand_line(errand)),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(open) = this.open_errand.clone() {
                            open(id.clone(), window, cx);
                        }
                    }))
            });
        let card = card(palette)
            .gap(px(2.))
            .px(px(8.))
            .py(px(8.))
            .child(
                div()
                    .px(px(8.))
                    .pb(px(4.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.foreground)
                    .child(title),
            )
            .children(rows);
        Some(from_nexus(
            usize::MAX,
            false,
            v_flex().flex_1().min_w(px(0.)).child(card),
            palette,
        ))
    }

    /// The conversation the thread is in, kept for the next start (`workspace/assistant.rs`).
    pub fn conversation(&self) -> Option<&str> {
        self.pane.conversation.as_deref()
    }

    /// The person looks at the page, or leaves it. Looking reads every reply.
    pub fn set_viewing(&mut self, viewing: bool, cx: &mut Context<Self>) {
        if self.viewing == viewing {
            return;
        }
        self.viewing = viewing;
        if viewing {
            self.unread = 0;
        } else if let Some((voice, _)) = &self.voice {
            // Leaving the page ends the call: the microphone never stays open behind it.
            voice.update(cx, |voice, cx| voice.end(cx));
        }
        cx.notify();
    }

    /// The thread reads `conversation` again, for turns asked elsewhere (the Mac chat bar): not while an answer is
    /// written or a read is under way, and only for the conversation the page holds. The read ends scrolled to the last
    /// answer ([`AgentPage::read_chunk`]).
    pub fn read_again(&mut self, conversation: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane.busy() || self.pane.loading || self.pane.conversation.as_deref() != Some(conversation) {
            self.scroll.scroll_to_bottom();
            return;
        }
        let (pane, read) = Pane::new(Some(conversation));
        let Some(call) = read else { return };
        self.pane = pane;
        self.answers.clear();
        self.arrivals = Arrivals::default();
        self.draft = None;
        self.asking = None;
        self.relayout(cx);
        self._reading = Some(self.run(call, Kind::Read, window, cx));
        cx.notify();
    }

    /// Hand `text` to Nexus from elsewhere (the palette's Hand off, brand v5 A21·e): it is sent as if typed here. While
    /// an answer is written it waits in the composer, so nothing is lost.
    pub fn hand_off(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.composer.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        self.composer_empty = false;
        if !self.pane.busy() {
            self.send(window, cx);
        }
        cx.notify();
    }

    /// Run a call through the backend; its chunks change the pane as they arrive.
    fn run(&self, call: Call, kind: Kind, window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        // Read the setting at each call.
        let base = rules::base_url(crate::desktop_settings::load().agents_api_url.as_deref());
        let crate::agent::Stream { mut chunks, _stop } = self.ctx.gateway.agent(base, call);
        cx.spawn_in(window, async move |this, cx| {
            // Held until the page drops this task: then the call stops.
            let _stop = _stop;
            while let Some(chunk) = chunks.next().await {
                let applied = this.update_in(cx, |page, window, cx| match &kind {
                    Kind::Ask => page.ask_chunk(chunk, window, cx),
                    Kind::Read => page.read_chunk(chunk, window, cx),
                    Kind::Intro => page.intro_chunk(chunk, window, cx),
                    Kind::Seen => {}
                    Kind::Decide(card) => {
                        page.decisions.chunk(card, chunk);
                        cx.notify();
                    }
                    Kind::Edit(card) => {
                        page.decisions.edit_chunk(card, chunk);
                        // The edited card replaced it: its inputs are closed.
                        if page.decisions.editing(card).is_none() {
                            page.edit_boxes.close(card);
                        }
                        cx.notify();
                    }
                });
                if applied.is_err() {
                    break;
                }
            }
        })
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // In a call the composer waits: the call asks in this conversation.
        if self.in_call(cx) {
            return;
        }
        let text = self.composer.read(cx).value().to_string();
        let Some((shown, wire)) = self.boxed.wire(&text) else {
            return;
        };
        let Some(call) = self.pane.ask_with(&shown, wire, Instant::now()) else {
            return;
        };
        self.boxed.clear();
        self.composer.update(cx, |state, cx| state.set_value("", window, cx));
        self.start(call, window, cx);
        crate::sounds::play(crate::sounds::Cue::Send, window, cx);
    }

    /// The message that was not sent, asked again (Retry, or the app back online).
    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(call) = self.pane.retry(Instant::now()) {
            self.start(call, window, cx);
        }
    }

    fn start(&mut self, call: Call, window: &mut Window, cx: &mut Context<Self>) {
        self._resend = None;
        self.draft = Some(cx.new(|cx| TextViewState::markdown("", cx)));
        self.step.update(cx, |step, cx| step.set("Thinking", cx));
        self.asking = Some(self.run(call, Kind::Ask, window, cx));
        // The question rises from the composer; Nexus's typing bubble pops in under it.
        let now = Instant::now();
        self.arrivals.sent = Some((self.pane.entries.len().saturating_sub(1), now));
        self.arrivals.typing = Some(now);
        self.receipt_was = None;
        self.picking = None;
        self.relayout(cx);
        self.placeholder(window, cx);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// A voice call is open (its pane shows).
    fn in_call(&self, cx: &App) -> bool {
        self.voice.as_ref().is_some_and(|(voice, _)| !voice.read(cx).ended())
    }

    /// The voice button: open voice mode in this conversation. The microphone is asked for only now.
    pub fn start_voice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let senses: std::rc::Rc<dyn crate::voice::pane::Senses> =
            std::rc::Rc::new(crate::voice::BackendSenses(self.ctx.gateway.clone()));
        self.start_voice_with(senses, crate::voice::speaker::system(), window, cx);
    }

    /// [`AgentPage::start_voice`] with its senses and voice given, for the tests.
    pub fn start_voice_with(
        &mut self,
        senses: std::rc::Rc<dyn crate::voice::pane::Senses>,
        speaker: Box<dyn crate::voice::speaker::Speaker>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.in_call(cx) || self.pane.busy() {
            return;
        }
        let conversation = self.pane.conversation.clone();
        let voice = cx.new(|cx| crate::voice::VoicePane::new(senses, speaker, conversation, window, cx));
        let subscription = cx.subscribe_in(&voice, window, |this, _, event, window, cx| {
            this.voice_event(event, window, cx)
        });
        self.voice = Some((voice, subscription));
        cx.notify();
    }

    fn voice_event(&mut self, event: &crate::voice::VoiceEvent, window: &mut Window, cx: &mut Context<Self>) {
        use crate::voice::VoiceEvent;
        match event {
            VoiceEvent::Kept(conversation) => self.pane.conversation = Some(conversation.clone()),
            VoiceEvent::Ended { summary, writes } => {
                self.voice = None;
                if self.pane.land_voice(summary.clone(), writes.clone()) {
                    self.arrivals.received = Some((self.pane.entries.len() - 1, Instant::now()));
                    self.relayout(cx);
                    self.scroll.scroll_to_bottom();
                }
                if self.viewing {
                    self.composer.update(cx, |state, cx| state.focus(window, cx));
                }
            }
            VoiceEvent::SwitchToText => self.composer.update(cx, |state, cx| state.focus(window, cx)),
        }
        cx.notify();
    }

    /// The voice pane, while a call is open, for the view tests.
    #[cfg(test)]
    pub fn voice(&self) -> Option<Entity<crate::voice::VoicePane>> {
        self.voice.as_ref().map(|(voice, _)| voice.clone())
    }

    /// The thread changed: its time lines again (`thread_layout.rs`), and its quick replies and link cards
    /// (`thread_links.rs`).
    fn relayout(&mut self, cx: &mut Context<Self>) {
        // One slot per entry, so an entry without a time still draws.
        let times: Vec<_> = (0..self.pane.entries.len())
            .map(|ix| self.pane.times.get(ix).copied().flatten())
            .collect();
        self.items = thread_layout::layout(&times, chrono::Local::now());
        self.links = thread_links::Links::of(&self.pane.entries);
        self.keep_arcs(cx);
    }

    /// The tapback on a bubble of this conversation.
    fn tapback(&self, key: &str) -> Option<Tapback> {
        self.reactions.get(self.pane.conversation.as_deref()?, key)
    }

    /// Pick `tapback` on the bubble `key`: the same again takes it off. The workspace keeps them (`assistant.rs`).
    pub fn react(&mut self, key: &str, tapback: Tapback, cx: &mut Context<Self>) {
        self.picking = None;
        if let Some(conversation) = self.pane.conversation.clone() {
            self.reactions.toggle(&conversation, key, tapback);
        }
        cx.notify();
    }

    /// The tapbacks kept for every conversation, for the workspace's marks.
    pub fn reactions(&self) -> &Reactions {
        &self.reactions
    }

    /// The thread's lines now, for the view tests.
    #[cfg(test)]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane.stop() {
            self.asking = None;
            self.draft = None;
            self.placeholder(window, cx);
            cx.notify();
        }
    }

    /// Whether New conversation has a conversation to leave: it has messages, and no answer streams.
    pub fn can_start_over(&self) -> bool {
        !self.pane.entries.is_empty() && !self.pane.busy()
    }

    /// New conversation, from the title bar's ⋯ (A24·c draws no link in the composer).
    pub fn start_over(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane.start_over() {
            self.answers.clear();
            self.decisions.clear();
            self.edit_boxes = EditBoxes::default();
            self._deciding.clear();
            self._resend = None;
            self.arrivals = Arrivals::default();
            self.receipt_was = None;
            self.picking = None;
            self.relayout(cx);
            self.placeholder(window, cx);
            self.composer.update(cx, |state, cx| state.focus(window, cx));
            cx.notify();
        }
    }

    /// Put the keys in the message box: the page shows again, ready for typing.
    pub fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |state, cx| state.focus(window, cx));
    }

    fn placeholder(&self, window: &mut Window, cx: &mut Context<Self>) {
        let text = placeholder(&self.pane);
        self.composer
            .update(cx, |state, cx| state.set_placeholder(text, window, cx));
    }

    /// A chunk of the ask's answer: the draft follows it, and a finished answer may notify or confirm.
    fn ask_chunk(&mut self, chunk: Chunk, window: &mut Window, cx: &mut Context<Self>) {
        let replies = self.replies();
        let entries = self.pane.entries.len();
        let sent = self.pane.sent;
        // A reply streamed into the typing bubble is already on screen; one that came whole pops in.
        let streamed = self.pane.draft.as_ref().is_some_and(|draft| !draft.text.is_empty());
        let effects = self.pane.ask_chunk(chunk, Instant::now());
        if let Some(was) = sent
            && self.pane.sent != sent
        {
            self.receipt_was = Some((was, Instant::now()));
        }
        if self.pane.entries.len() != entries {
            if self.replies() > replies && !streamed {
                self.arrivals.received = Some((self.pane.entries.len() - 1, Instant::now()));
            }
            self.relayout(cx);
        }
        if self.replies() > replies && self.viewing {
            crate::sounds::play(crate::sounds::Cue::Receive, window, cx);
        }
        // The step line follows the stream; only its own view redraws when it changes.
        if let Some(line) = self.pane.draft.as_ref().and_then(crate::agent::Draft::thinking_line) {
            let line = line.to_string();
            self.step.update(cx, |step, cx| step.set(&line, cx));
        }
        match (&self.pane.draft, &self.draft) {
            (Some(draft), Some(view)) => {
                let text = draft.text.clone();
                view.update(cx, |state, cx| state.set_text(&text, cx));
            }
            (None, _) => {
                self.draft = None;
                let ended = self.asking.take().is_some();
                self.placeholder(window, cx);
                // The answer may have asked for a login: its sheet shows over the chat. Read once per answer.
                if ended {
                    self.vault.update(cx, |vault, cx| vault.reread(window, cx));
                }
            }
            _ => {}
        }
        // A reply while the person is elsewhere is one more for the sidebar's count.
        if self.replies() > replies && !self.viewing {
            self.unread += 1;
        }
        if self.pane.offline && self.pane.sent == Some(Sent::NotSent) && self._resend.is_none() {
            self._resend = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(RESEND_EVERY).await;
                let _ = this.update_in(cx, |page, window, cx| {
                    page._resend = None;
                    if page.pane.offline {
                        page.retry(window, cx);
                    }
                });
            }));
        }
        for effect in effects {
            let Effect::Notify { kind, conversation } = effect;
            crate::notify::agent(kind, conversation, &self.name);
        }
        self.wait_for_sign_in(cx);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Nexus's replies in the thread.
    fn replies(&self) -> usize {
        self.pane
            .entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Agent(_)))
            .count()
    }

    /// A chunk of the first open's answer: once it shows, the agent API keeps it seen, for every app. A failure only
    /// means it may show once more.
    fn intro_chunk(&mut self, chunk: Chunk, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(call) = self.pane.intro_chunk(chunk) {
            self._seen = Some(self.run(call, Kind::Seen, window, cx));
            cx.notify();
        }
    }

    /// Approve or Decline `card` in the thread (A9): the agent API writes it once Policy has the person's approval.
    #[cfg(test)]
    pub fn decide(&mut self, card: &str, decision: Decision, window: &mut Window, cx: &mut Context<Self>) {
        Decides::decide(self, card, decision, window, cx);
    }

    /// Where `card`'s decision stands, for the view tests.
    #[cfg(test)]
    pub fn decision(&self, card: &str) -> Option<&Phase> {
        self.decisions.phase(card)
    }

    /// The latest reply's waiting writes have all been decided: nothing needs the person.
    fn decided_last(&self) -> bool {
        let Some(Entry::Agent(messages)) = self.pane.entries.last() else {
            return false;
        };
        messages.iter().all(|message| match self.decisions.resolve(message) {
            Message::PendingAction { card, .. } => self.decisions.ended(card),
            _ => true,
        })
    }

    /// A starter, or "See what I can do": the line goes as the person's first question (A19).
    pub fn start_with(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane.busy() {
            return;
        }
        self.composer.update(cx, |state, cx| state.set_value(line, window, cx));
        self.send(window, cx);
    }

    /// The first open drawn now, for the view tests.
    #[cfg(test)]
    pub fn first_open(&self) -> Option<&cards::Intro> {
        self.pane.first_open()
    }

    fn read_chunk(&mut self, chunk: Chunk, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.pane.entries.len();
        self.pane.read_chunk(chunk);
        if self.pane.entries.len() != before {
            // The turns read back go first: the answers' places moved, and nothing read back moves in.
            self.answers.clear();
            self.arrivals = Arrivals::default();
            self.relayout(cx);
            self.placeholder(window, cx);
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// An answer's Markdown view, made once.
    fn answer_view(&mut self, key: (usize, usize), text: &str, cx: &mut Context<Self>) -> Entity<TextViewState> {
        self.answers
            .entry(key)
            .or_insert_with(|| cx.new(|cx| TextViewState::markdown(text, cx)))
            .clone()
    }
}

/// Open a link of a reply in the browser: http and https only.
pub(crate) fn open_link(href: &str, cx: &mut App) {
    if let Some(url) = rules::safe_url(href) {
        crate::external_link::open(&url, cx);
    }
}

/// Nexus's avatar: the F5 mark in one ink, still at rest and breathing while it thinks (A13·A). `id` keeps its turn,
/// so a reply's mark eases to rest where the thinking one was.
pub fn avatar(id: impl Into<ElementId>, size: f32, thinking: bool, palette: Palette) -> impl IntoElement {
    crate::motion::breathing_mark(id, size, palette.foreground).breathing(thinking)
}

/// The board's quiet ink for the thread's small lines (A1·C, A22): receipts, times, time lines, the typing dots and
/// the scope. #8A8A8E in dark and #6E6E73 in light, between the palette's muted and faint.
pub fn quiet_ink(palette: Palette) -> Hsla {
    if palette.dark {
        rgb(0x8A8A8E).into()
    } else {
        rgb(0x6E6E73).into()
    }
}

/// Which side of the thread a bubble is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Person,
    Nexus,
}

/// Whether a message of Nexus's reply is a bubble: a card only when it carries an action (A1·D).
fn is_bubble(message: &Message) -> bool {
    match message {
        Message::Answer { .. } | Message::Notice { .. } | Message::Memories { .. } | Message::Forgotten { .. } => true,
        Message::Insight { url, .. } => rules::safe_url(url).is_none(),
        _ => false,
    }
}

/// The contact card's shortcut, as this platform writes it.
pub const CARD_KEY: &str = if cfg!(target_os = "macos") { "⌘I" } else { "Ctrl+I" };

/// About where the expanded header ends, at the top of the thread: the contact card opens under it.
const HEADER_BOTTOM: f32 = 150.;

/// The space between the thread's rows (A22's desktop thread).
const ROW_GAP: f32 = 14.;

impl Decides for AgentPage {
    fn decisions(&self) -> &Decisions {
        &self.decisions
    }

    fn decisions_mut(&mut self) -> &mut Decisions {
        &mut self.decisions
    }

    fn decide(&mut self, card: &str, decision: Decision, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(call) = self.decisions.start(card, decision) {
            let task = self.run(call, Kind::Decide(card.to_string()), window, cx);
            self._deciding.insert(card.to_string(), task);
            cx.notify();
        }
    }

    fn edit_boxes(&self) -> &EditBoxes {
        &self.edit_boxes
    }

    fn edit_boxes_mut(&mut self) -> &mut EditBoxes {
        &mut self.edit_boxes
    }

    /// Save on a card's Edit: the changed fields go to the agent API, and its answer is the edited card.
    fn save_edit(&mut self, card: &str, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.edit_boxes.values(card, cx);
        if let Some(call) = self.decisions.save_edit(card, &values) {
            let task = self.run(call, Kind::Edit(card.to_string()), window, cx);
            self._deciding.insert(card.to_string(), task);
        }
        cx.notify();
    }
}

impl Render for AgentPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let busy = self.pane.busy();
        // The conversation's width, as the chat column's text in this window (`chat_width.rs`).
        let widths = crate::chat_width::of(window.window_handle().window_id());
        let scrolled = f32::from(self.scroll.offset().y);
        let condensed = scrolled < -CONDENSE_AT;
        let mut thread = v_flex()
            .w_full()
            .max_w(px(widths.column))
            .gap(px(ROW_GAP))
            .pb(px(24.))
            .child(header(self.name.clone(), self.scope.clone(), busy, palette, cx));
        if self.pane.loading {
            thread = thread.child(ui::note("Loading the conversation…", palette));
        }
        // Nexus writes first (A1·A): today's brief opens the thread.
        if let Some(shown) = &self.brief.shown {
            thread = thread.child(brief::row(shown, palette));
        }
        // The main chat stays the overview (A12·c): what needs the person, one click to each errand's thread.
        if !self.pane.loading && self.pane.first_open().is_none() {
            thread = thread.children(self.errands_overview(palette, cx));
        }
        if let Some(intro) = self.pane.first_open().cloned() {
            thread = thread.child(self.first_open_view(&intro, palette, cx));
        } else if self.pane.empty() && self.brief.shown.is_none() && self.errands.is_empty() {
            thread = thread.child(div().w_full().text_center().child(ui::note(
                format!(
                    "Ask {} anything. It knows the notes it keeps about you and your saved model.",
                    self.name
                ),
                palette,
            )));
        }
        let entries = self.pane.entries.clone();
        // Delivered, Read or Not sent goes under the person's last message only.
        let last_user = entries.iter().rposition(|entry| matches!(entry, Entry::User(_)));
        let items = self.items.clone();
        for item in &items {
            let ix = match item {
                Item::Separator(label) => {
                    thread = thread.child(time_line(label.clone(), palette));
                    continue;
                }
                Item::Entry(ix) => *ix,
            };
            let Some(entry) = entries.get(ix) else { continue };
            let row = match entry {
                Entry::User(text) => {
                    let sent = (Some(ix) == last_user).then_some(self.pane.sent).flatten();
                    let bubble = self.user_message(ix, text.clone(), sent, palette, cx);
                    // Just sent: it rises from the composer (`chat-land`).
                    if Arrivals::fresh(self.arrivals.sent, ix) {
                        crate::motion::arrive(bubble, ("agent-sent", ix), 16.).into_any_element()
                    } else {
                        bubble.into_any_element()
                    }
                }
                Entry::Voice(summary) => {
                    let column = v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .items_start()
                        .child(crate::voice::summary_card(ix, summary, palette, window, cx));
                    let row = from_nexus(ix, false, column, palette);
                    if Arrivals::fresh(self.arrivals.received, ix) {
                        crate::motion::pop(row, ("agent-received", ix)).into_any_element()
                    } else {
                        row.into_any_element()
                    }
                }
                Entry::Agent(list) => {
                    // The tail goes on the reply's bubble only when nothing follows it (A1·A): a card after it keeps
                    // it round.
                    let last = list.len().saturating_sub(1);
                    let carded = self.links.messages.get(&(ix, last)).is_some_and(|links| links.carded);
                    let tail = list.last().is_some_and(is_bubble) && !carded;
                    // Quick replies under the thread's last answer, while nothing is sent after it (A1·D).
                    let replies = if ix + 1 == entries.len() {
                        self.shown_replies().map(|replies| replies.to_vec())
                    } else {
                        None
                    };
                    let at = self.pane.times.get(ix).copied().flatten();
                    let footer: Vec<String> = list.iter().flat_map(rules::answer_footer).collect();
                    let column = v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .items_start()
                        .gap(px(4.))
                        .children(
                            list.iter()
                                .enumerate()
                                .map(|(n, message)| {
                                    self.render_message((ix, n), message, tail && n == last, palette, cx)
                                })
                                .collect::<Vec<_>>(),
                        )
                        .children(replies.map(|replies| thread_links::quick_replies(ix, &replies, palette, cx)))
                        .children(reply_time(ix, at, footer.join(" · "), palette));
                    let row = from_nexus(ix, false, column, palette);
                    // A reply that came whole pops in; one streamed into the typing bubble is already there.
                    if Arrivals::fresh(self.arrivals.received, ix) {
                        crate::motion::pop(row, ("agent-received", ix)).into_any_element()
                    } else {
                        row.into_any_element()
                    }
                }
            };
            thread = thread.child(row);
        }
        if let Some(draft) = &self.pane.draft {
            let thinking = draft.text.is_empty();
            let bubble = match (&self.draft, thinking) {
                // Thinking (A1·C b): the breathing mark and the live step line, no bubble.
                _ if draft.thinking_line().is_some() => div()
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .child(self.step.clone())
                    .into_any_element(),
                (Some(view), false) => bubble(palette, true)
                    .child(crate::workspace::prose(
                        crate::external_link::text_view(view).selectable(true).stream_fade(true),
                        palette,
                    ))
                    .into_any_element(),
                // Typing (A1·C·c), once the answer has a model and no tool runs: three dots in a 34 px bubble.
                _ => h_flex()
                    .h(px(34.))
                    .px(px(14.))
                    .rounded_tl(px(18.))
                    .rounded_tr(px(18.))
                    .rounded_br(px(18.))
                    .rounded_bl(px(6.))
                    .bg(palette.selected)
                    .child(self.typing.clone())
                    .into_any_element(),
            };
            let column = v_flex().flex_1().min_w(px(0.)).items_start().child(bubble);
            // The answer under way is the thread's next entry: its avatar keeps its turn as it lands.
            let row = from_nexus(entries.len(), thinking, column, palette);
            let row = if self.arrivals.typing.is_some_and(|since| since.elapsed() < FRESH) {
                crate::motion::pop(row, ("agent-typing", entries.len())).into_any_element()
            } else {
                row.into_any_element()
            };
            thread = thread.child(row);
        }
        // A sign-in problem is setup, not a failure: its line, drawn each frame so it leaves with its motion.
        let setup = (!self.pane.offline)
            .then_some(self.pane.problem.as_ref())
            .flatten()
            .and_then(|problem| sign_in::line(problem, self.unsent()));
        let is_setup = setup.is_some();
        thread = thread.children(crate::setup::slot(
            "assistant-sign-in",
            setup,
            &self.ctx.fixer,
            palette,
            cx,
        ));
        if self.pane.offline {
            thread = thread.child(offline_line(palette));
        } else if let Some(problem) = self.pane.problem.as_ref().filter(|_| !is_setup) {
            thread = thread.child(notice_error(format!("{}. {}", problem.title, problem.detail), palette));
        }

        // The composer (A22): the field over a bar, and the 28 px round button at its end in the ink, full once there
        // is text, Stop while Nexus answers.
        let in_call = self.in_call(cx);
        let send_enabled = !busy && !in_call && self.boxed.wire(self.composer.read(cx).value().as_ref()).is_some();
        let composer = v_flex()
            .relative()
            .w_full()
            .max_w(px(widths.composer))
            .gap(px(10.))
            .pt(px(12.))
            .pr(px(12.))
            .pb(px(10.))
            .pl(px(16.))
            .rounded(px(16.))
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border)
            .children(self.composer_menu(palette, cx))
            .children(self.composer_chips(palette, cx))
            .child(
                div()
                    .min_h(px(20.))
                    .text_size(px(14.5))
                    .line_height(relative(1.45))
                    .child(Textarea::new(&self.composer).appearance(false)),
            )
            .child(
                h_flex()
                    .h(px(28.))
                    .gap(px(12.))
                    // New conversation is in the title bar's ⋯ (A24·c; `workspace/assistant_title.rs`).
                    .children(self.model_chip(palette, cx))
                    .child(div().flex_1())
                    // Voice mode (A18): before the send circle, quiet as the attach button. macOS only for now.
                    .when(crate::voice::speaker::BUILT && !busy, |bar| {
                        bar.child(voice_button(in_call, palette, cx))
                    })
                    .child(if busy {
                        round_button("agent-stop", palette)
                            .border_1()
                            .border_color(palette.border)
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.hover))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Stop (Esc)").build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.stop(window, cx)))
                            .child(
                                div()
                                    .size(px(8.))
                                    .rounded(px(2.))
                                    .border(px(1.5))
                                    .border_color(palette.muted),
                            )
                    } else {
                        round_button("agent-send", palette)
                            .bg(palette.primary)
                            .when(!send_enabled, |button| button.opacity(0.4))
                            .when(send_enabled, |button| {
                                button.cursor_pointer().hover(|style| style.opacity(0.88))
                            })
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Send (Enter)").build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                            .child(
                                Icon::new(Lucide::ArrowUp)
                                    .size(px(16.))
                                    .text_color(palette.primary_foreground),
                            )
                    }),
            );
        let card = self
            .card_open
            .then(|| self.contact_card_view((HEADER_BOTTOM + scrolled).max(48.), palette, cx));

        // In a call its pane sits beside the thread; in a narrow window, over it.
        let voice = self.voice.as_ref().filter(|_| in_call).map(|(voice, _)| voice.clone());
        let beside = f32::from(window.viewport_size().width) >= 900.;
        let chat = v_flex()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            // A8·b: a login Nexus asks to use waits here, above the thread, for the person's answer.
            .child(div().w_full().flex_none().px(px(32.)).child(self.vault.clone()))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .child(
                        div()
                            .id("agent-thread")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .px(px(32.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(thread),
                    )
                    // Scrolled under the header, it condenses into one line at the top (A13·e).
                    .when(condensed, |area| {
                        area.child(crate::motion::fade_in(
                            condensed_header(self.name.clone(), self.scope.clone(), busy, palette, cx),
                            "agent-condensed-header",
                        ))
                    })
                    .children(card),
            )
            .child(
                v_flex()
                    .w_full()
                    .pt(px(12.))
                    .px(px(32.))
                    .pb(px(20.))
                    .items_center()
                    .child(composer),
            );
        h_flex().relative().size_full().child(chat).children(voice.map(|voice| {
            div()
                .h_full()
                .flex_none()
                .map(|pane| {
                    if beside {
                        pane.w(px(crate::voice::pane::WIDTH))
                            .border_l_1()
                            .border_color(palette.border)
                    } else {
                        pane.absolute().top_0().left_0().size_full()
                    }
                })
                .child(voice)
        }))
    }
}

impl AgentPage {
    /// The person's message (A1·D): raised with a hairline on the right, radius 18 with its tail at the bottom right,
    /// and under the last one where it stands. "Retry" in Not sent asks it again.
    fn user_message(
        &self,
        ix: usize,
        text: String,
        sent: Option<Sent>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let bubble = person_bubble(text, palette);
        v_flex()
            .w_full()
            .items_end()
            .gap(px(4.))
            .child(self.reactable(reactions::key(ix, None), ix * 100, Side::Person, bubble, palette, cx))
            .children(sent.map(|sent| self.receipt(sent, palette, cx)))
    }

    /// The receipt under the person's last message: Delivered, Read and the time, or Not sent with Retry. When it
    /// changes, the old one fades out as the new one fades in.
    fn receipt(&self, sent: Sent, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        // A1·C: 11 px in the quiet ink, 4 px in from the right; Retry reads as the rest of the line.
        let quiet = |line: Div| line.pr(px(4.)).text_size(px(11.)).text_color(quiet_ink(palette));
        let line = match sent {
            Sent::NotSent => quiet(h_flex())
                .child("Not sent · ")
                .child(
                    div()
                        .id("agent-retry")
                        .cursor_pointer()
                        .hover(|style| style.underline())
                        .on_click(cx.listener(|this, _, window, cx| this.retry(window, cx)))
                        .child("Retry"),
                )
                .into_any_element(),
            sent => quiet(div()).child(sent.line()).into_any_element(),
        };
        let Some((was, _)) = self.receipt_was.filter(|(_, since)| since.elapsed() < FRESH) else {
            return line;
        };
        let id = {
            use std::hash::{Hash as _, Hasher as _};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            sent.line().hash(&mut hasher);
            hasher.finish()
        };
        div()
            .relative()
            .child(crate::motion::fade_in(
                div().child(line),
                ElementId::NamedInteger("agent-receipt".into(), id),
            ))
            .child(crate::motion::fade_out(
                quiet(div())
                    .absolute()
                    .top_0()
                    .right_0()
                    .whitespace_nowrap()
                    .child(was.line()),
                ElementId::NamedInteger("agent-receipt-was".into(), id),
            ))
            .into_any_element()
    }

    /// A bubble the person can react to (A1·D): hovering it shows a small heart beside it, which opens the six tapbacks
    /// above it; the one picked sits on the bubble's top corner. `id` names the bubble's elements. Only once the thread
    /// has a conversation to keep them under.
    fn reactable(
        &self,
        key: String,
        id: usize,
        side: Side,
        bubble: Div,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.tapback(&key);
        let bubble = bubble.relative().when_some(current, |bubble, tapback| {
            bubble.child(self.tapback_pill(&key, id, tapback, side, palette, cx))
        });
        let row = h_flex().w_full().items_center().gap(px(6.));
        let row = if side == Side::Person { row.justify_end() } else { row };
        if self.pane.conversation.is_none() {
            return row.child(bubble).into_any_element();
        }
        let group: SharedString = format!("agent-bubble-{id}").into();
        let picking = self.picking.as_deref() == Some(key.as_str());
        let trigger = div()
            .id(("agent-react", id))
            .flex_none()
            .size(px(24.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .when(!picking, |trigger| {
                trigger.invisible().group_hover(group.clone(), |style| style.visible())
            })
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("React").build(window, cx))
            .on_mouse_down(MouseButton::Left, {
                let key = key.clone();
                cx.listener(move |this, _, _, cx| {
                    this.card_open = false;
                    this.picking = Some(key.clone());
                    cx.notify();
                })
            })
            .child(Icon::new(Lucide::Heart).size(px(13.)).text_color(palette.muted));
        let row = row.group(group).relative();
        let row = match side {
            Side::Person => row.child(trigger).child(bubble),
            Side::Nexus => row.child(bubble).child(trigger),
        };
        row.when(picking, |row| {
            row.child(self.tapback_picker(&key, id, side, palette, cx))
        })
        .into_any_element()
    }

    /// The six tapbacks in a pill above the bubble (Messages' order). A click outside closes it, as Esc does.
    fn tapback_picker(
        &self,
        key: &str,
        id: usize,
        side: Side,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let current = self.tapback(key);
        let picker = h_flex()
            .id(("agent-tapbacks", id))
            .occlude()
            .absolute()
            .top(px(-40.))
            .h(px(36.))
            .px(px(4.))
            .gap(px(2.))
            .rounded_full()
            .bg(palette.menu)
            .border_1()
            .border_color(palette.border)
            .shadow_md()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.picking = None;
                cx.notify();
            }))
            .children(Tapback::ALL.into_iter().enumerate().map(|(at, tapback)| {
                let key = key.to_string();
                div()
                    .id(("agent-tapback", id * 10 + at))
                    .size(px(28.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(16.))
                    .when(current == Some(tapback), |choice| choice.bg(palette.selected))
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.menu_hover))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tapback.name()).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.react(&key, tapback, cx)))
                    .child(tapback.glyph())
            }));
        match side {
            Side::Person => picker.right(px(30.)),
            Side::Nexus => picker.left_0(),
        }
    }

    /// The tapback on a bubble's top corner, in a 22 px pill (A1·D). A click takes it off.
    fn tapback_pill(
        &self,
        key: &str,
        id: usize,
        tapback: Tapback,
        side: Side,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let key = key.to_string();
        let pill = div()
            .id(("agent-tapback-on", id))
            .absolute()
            .top(px(-11.))
            .h(px(22.))
            .min_w(px(22.))
            .px(px(4.))
            .rounded_full()
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.))
            .cursor_pointer()
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(format!("{} · click to remove", tapback.name()))
                    .build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| this.react(&key, tapback, cx)))
            .child(tapback.glyph());
        match side {
            Side::Person => pill.left(px(-10.)),
            Side::Nexus => pill.right(px(-10.)),
        }
    }

    /// One of Nexus's bubbles, with its tapbacks.
    fn nexus_bubble(&self, key: (usize, usize), bubble: Div, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = key.0 * 100 + key.1 + 1;
        self.reactable(reactions::key(key.0, Some(key.1)), id, Side::Nexus, bubble, palette, cx)
    }

    /// An answer's session cards (A1·5), as the thread read them.
    fn session_cards(&self, key: (usize, usize), palette: Palette, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let Some(links) = self.links.messages.get(&key) else {
            return Vec::new();
        };
        links
            .sessions
            .iter()
            .enumerate()
            .map(|(ix, session)| {
                let arc = self.arcs.get(&thread_links::session_key(&session.url));
                thread_links::session_card((key.0 * 100 + key.1) * 10 + ix, session, arc, palette, cx)
            })
            .collect()
    }

    /// One message of the agent's reply, as Nexus UI's `AgentReplyView`, with the desktop's own write card. An answer
    /// or a note is a bubble; a card only when it carries an action (A1·A rules).
    fn render_message(
        &mut self,
        key: (usize, usize),
        message: &Message,
        tail: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match message {
            Message::Answer {
                text,
                sources,
                ai_notice,
                ..
            } => {
                let view = self.answer_view(key, text, cx);
                let sources: Vec<(String, String)> = sources
                    .iter()
                    .filter_map(|source| {
                        let url = rules::safe_url(&source.url)?;
                        let title = if source.title.is_empty() {
                            url.clone()
                        } else {
                            source.title.clone()
                        };
                        Some((title, url))
                    })
                    .collect();
                let answer = bubble(palette, tail).child(crate::workspace::prose(
                    crate::external_link::text_view(&view)
                        .selectable(true)
                        .on_link_click(|href, _, _, cx| open_link(href, cx)),
                    palette,
                ));
                v_flex()
                    .w_full()
                    .items_start()
                    .gap(px(4.))
                    .child(self.nexus_bubble(key, answer, palette, cx))
                    .when(!sources.is_empty(), |column| {
                        column.child(
                            v_flex()
                                .pl(px(4.))
                                .pt(px(2.))
                                .gap(px(2.))
                                .child(ui::label("Sources", palette))
                                .children(sources.into_iter().enumerate().map(|(ix, (title, url))| {
                                    link(
                                        ("agent-source", key.0 * 1000 + key.1 * 100 + ix),
                                        format!("{}. {title}", ix + 1),
                                        url,
                                        palette,
                                    )
                                })),
                        )
                    })
                    .when(*ai_notice, |column| {
                        column.child(ui::note("AI can make mistakes. Check what matters.", palette).pl(px(4.)))
                    })
                    .children(
                        cards::answer_connects(message)
                            .into_iter()
                            .enumerate()
                            .map(|(ix, connect)| {
                                connect_card((key.0 * 100 + key.1) * 10 + ix, connect, &self.name, palette)
                            }),
                    )
                    .children(self.session_cards(key, palette, cx))
                    .into_any_element()
            }
            Message::Refused { detail, billing_url } => card(palette)
                .child(card_title("Refused", palette))
                .child(body(detail.clone(), palette))
                .when_some(billing_url.as_deref().and_then(rules::safe_url), |card, url| {
                    card.child(link(
                        ("agent-billing", key.0 * 100 + key.1),
                        "Open billing",
                        url,
                        palette,
                    ))
                })
                .into_any_element(),
            Message::PendingAction { .. } => match Approval::of(self.decisions.resolve(message)) {
                Some(approval) => approval_card(self, key.0 * 100 + key.1, approval, cx).into_any_element(),
                None => div().into_any_element(),
            },
            Message::Notice { text, .. } if nexus_desktop_core::agent_watches::notice_proposal(message).is_some() => {
                // A watch Nexus proposes (A3·b): its line, then the card. Nothing is kept until Keep.
                let Some(proposal) = nexus_desktop_core::agent_watches::notice_proposal(message) else {
                    return div().into_any_element();
                };
                let line = bubble(palette, false).child(text.clone());
                v_flex()
                    .w_full()
                    .items_start()
                    .gap(px(4.))
                    .child(self.nexus_bubble(key, line, palette, cx))
                    .child(self.watch_card(key, &proposal, palette, cx))
                    .into_any_element()
            }
            Message::Notice { text, .. } => {
                // The agent opened an errand for a bigger job (A1·5): the board's line, then the errand's card, as
                // the errands list has it now.
                let opened = self.links.messages.get(&key).and_then(|links| links.errand.clone());
                let Some(opened) = opened else {
                    return self.nexus_bubble(key, bubble(palette, tail).child(text.clone()), palette, cx);
                };
                let errand = self.errand_now.get(&opened.id).cloned().unwrap_or(opened);
                let arc = self.arcs.get(&thread_links::errand_key(&errand.id)).cloned();
                let line = bubble(palette, false).child(nexus_desktop_core::agent_thread::OPENED_ERRAND);
                v_flex()
                    .w_full()
                    .items_start()
                    .gap(px(4.))
                    .child(self.nexus_bubble(key, line, palette, cx))
                    .child(thread_links::errand_card(
                        key.0 * 100 + key.1,
                        &errand,
                        arc.as_ref(),
                        palette,
                        cx,
                    ))
                    .into_any_element()
            }
            Message::Insight { text, url } => match rules::safe_url(url) {
                Some(url) => card(palette)
                    .child(body(text.clone(), palette))
                    .child(link(("agent-insight", key.0 * 100 + key.1), "Open it", url, palette))
                    .into_any_element(),
                None => self.nexus_bubble(key, bubble(palette, tail).child(text.clone()), palette, cx),
            },
            Message::Memories { enabled, notes } => {
                let bubble = bubble(palette, tail);
                let name = &self.name;
                let bubble = if !enabled {
                    bubble.child(format!("Notes are off: {name} keeps nothing about you."))
                } else if notes.is_empty() {
                    bubble.child(format!("{name} keeps no notes about you yet."))
                } else {
                    bubble.child(
                        v_flex()
                            .child(format!("What {name} keeps about you:"))
                            .children(notes.iter().map(|note| format!("• {}", note.text))),
                    )
                };
                self.nexus_bubble(key, bubble, palette, cx)
            }
            Message::Forgotten { count } => {
                let text = format!(
                    "{} forgot {count} {}.",
                    self.name,
                    if *count == 1 { "note" } else { "notes" }
                );
                self.nexus_bubble(key, bubble(palette, tail).child(text), palette, cx)
            }
            Message::Error { request_id, cut } => {
                let what = if *cut {
                    "The answer was cut off before it finished.".to_string()
                } else {
                    format!("{} could not finish this answer.", self.name)
                };
                let request = request_id
                    .as_deref()
                    .map(|id| format!(" Request {id}."))
                    .unwrap_or_default();
                notice_error(format!("{what}{request}"), palette).into_any_element()
            }
            Message::Unknown => div().into_any_element(),
        }
    }
}

/// The header at the top of the thread (A13·e, A22): the mark at 60 px, the Assistant's name at 17 px with a 12 px
/// chevron, and the scope quietly under it. A click opens its contact card, as ⌘I does.
fn header(
    name: SharedString,
    scope: SharedString,
    thinking: bool,
    palette: Palette,
    cx: &mut Context<AgentPage>,
) -> impl IntoElement {
    let tip: SharedString = format!("{name} ({CARD_KEY})").into();
    v_flex().w_full().items_center().child(
        v_flex()
            .id("agent-header")
            .items_center()
            .gap(px(10.))
            .pt(px(18.))
            .pb(px(8.))
            .cursor_pointer()
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_card(cx)))
            .child(avatar("agent-header-mark", HEADER_AVATAR, thinking, palette))
            .child(
                v_flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .gap(px(4.))
                            .child(div().text_size(px(17.)).text_color(palette.foreground).child(name))
                            .child(
                                Icon::new(Lucide::ChevronRight)
                                    .size(px(12.))
                                    .text_color(quiet_ink(palette)),
                            ),
                    )
                    .child(div().text_size(px(12.5)).text_color(quiet_ink(palette)).child(scope)),
            ),
    )
}

/// The header condensed into one 44 px line at the top, while the thread is scrolled (A13·e). It opens the card too.
fn condensed_header(
    name: SharedString,
    scope: SharedString,
    thinking: bool,
    palette: Palette,
    cx: &mut Context<AgentPage>,
) -> Stateful<Div> {
    h_flex()
        .id("agent-condensed-header-bar")
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(44.))
        .px(px(20.))
        .gap(px(8.))
        .bg(crate::frost::cover(&palette))
        .border_b_1()
        .border_color(palette.border)
        .text_size(px(13.))
        .cursor_pointer()
        .on_click(cx.listener(|this, _, _, cx| this.toggle_card(cx)))
        .child(avatar("agent-bar-mark", BAR_AVATAR, thinking, palette))
        .child(div().text_color(palette.foreground).child(name))
        .child(div().text_color(palette.muted).child(format!("· {scope}")))
}

/// What Nexus says (A1·A): its 28 px avatar at the column's foot, 10 px from the column. `ix` is the reply's place in
/// the thread, which keeps its avatar's turn.
pub(crate) fn from_nexus(ix: usize, thinking: bool, column: impl IntoElement, palette: Palette) -> Div {
    h_flex()
        .w_full()
        .items_end()
        .gap(px(10.))
        .child(
            div()
                .flex_none()
                .w(px(AVATAR))
                .child(avatar(("agent-avatar", ix), AVATAR, thinking, palette)),
        )
        .child(column)
}

/// The person's bubble (A1·D): raised with a hairline, radius 18 with its tail at the bottom right, at most 420 px.
pub(crate) fn person_bubble(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .max_w(px(420.))
        .px(px(14.))
        .py(px(8.))
        .rounded_tl(px(18.))
        .rounded_tr(px(18.))
        .rounded_bl(px(18.))
        .rounded_br(px(6.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
        .text_size(px(14.5))
        .line_height(relative(1.45))
        .text_color(palette.foreground)
        .child(text.into())
}

/// Nexus's bubble (A1·D): the select tone, radius 18, with its 6 px tail at the bottom left when nothing follows it in
/// the reply, 14.5/1.45 text, at most 440 px.
pub(crate) fn bubble(palette: Palette, tail: bool) -> Div {
    div()
        .max_w(px(440.))
        .px(px(14.))
        .py(px(8.))
        .rounded_tl(px(18.))
        .rounded_tr(px(18.))
        .rounded_br(px(18.))
        .rounded_bl(px(if tail { 6. } else { 18. }))
        .bg(palette.selected)
        .text_size(px(14.5))
        .line_height(relative(1.45))
        .text_color(palette.foreground)
}

/// The time under a reply (A1·A): mono 11 px, quiet, 4 px in. Its model and cost, when the answer has them, show on
/// hover; a reply read back without a time shows them in its place.
pub(crate) fn reply_time(
    ix: usize,
    at: Option<chrono::DateTime<chrono::Local>>,
    footer: String,
    palette: Palette,
) -> Option<AnyElement> {
    let line = |text: String| {
        div()
            .id(("agent-reply-time", ix))
            .pl(px(4.))
            .font_family(ui::MONO)
            .text_size(px(11.))
            .text_color(quiet_ink(palette))
            .child(text)
    };
    match at {
        Some(at) if footer.is_empty() => Some(line(at.format("%H:%M").to_string()).into_any_element()),
        Some(at) => Some(
            line(at.format("%H:%M").to_string())
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(footer.clone()).build(window, cx))
                .into_any_element(),
        ),
        None if !footer.is_empty() => Some(line(footer).into_any_element()),
        None => None,
    }
}

/// A centered time line (A1·C·j): "Today 07:58", 11.5 px, quiet.
pub(crate) fn time_line(label: String, palette: Palette) -> impl IntoElement {
    div()
        .w_full()
        .py(px(2.))
        .text_center()
        .text_size(px(11.5))
        .text_color(quiet_ink(palette))
        .child(label)
}

/// The line while the agent service does not answer (A1·C·i): the still mark and the words, centred and quiet.
fn offline_line(palette: Palette) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_center()
        .gap(px(8.))
        .text_size(px(12.))
        .text_color(quiet_ink(palette))
        .child(avatar("agent-offline-mark", 18., false, palette))
        .child(OFFLINE)
}

/// A note of the agent's that carries an action: raised, a hairline, never coloured (A1·D).
pub(crate) fn card(palette: Palette) -> Div {
    v_flex()
        .w_full()
        .max_w(px(440.))
        .gap(px(6.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(12.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
}

pub(crate) fn card_title(text: &'static str, palette: Palette) -> Div {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.muted)
        .child(text)
}

pub(crate) fn body(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .text_size(px(14.))
        .line_height(px(22.))
        .text_color(palette.foreground)
        .child(text.into())
}

/// The line under an answer.
fn meta(text: String, palette: Palette) -> Div {
    div().text_size(px(12.)).text_color(palette.faint).child(text)
}

/// A failure, as `.notice.error`.
pub(crate) fn notice_error(text: String, palette: Palette) -> Div {
    div()
        .w_full()
        .px(px(14.))
        .py(px(10.))
        .rounded(px(10.))
        .border_1()
        .border_color(palette.destructive.opacity(0.4))
        .text_size(px(13.))
        .text_color(palette.destructive)
        .child(text)
}

/// A link of a reply, opened in the browser.
pub(crate) fn link(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    url: String,
    palette: Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .text_size(px(13.))
        .text_color(palette.muted)
        .underline()
        .cursor_pointer()
        .hover(|style| style.text_color(palette.strong))
        .on_click(move |_, _, cx| open_link(&url, cx))
        .child(text.into())
}

/// The composer's voice button (A18): quiet until hovered, as Nexus UI's `composer-voice`; dimmed while a call is open.
fn voice_button(in_call: bool, palette: Palette, cx: &mut Context<AgentPage>) -> Stateful<Div> {
    div()
        .id("agent-voice")
        .size(px(30.))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .when(in_call, |button| button.opacity(0.4))
        .when(!in_call, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .on_click(cx.listener(|this, _, window, cx| this.start_voice(window, cx)))
        })
        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Talk to Nexus").build(window, cx))
        .child(Icon::new(Lucide::AudioLines).size(px(15.)).text_color(palette.muted))
}

/// The composer's 28px round button.
pub(crate) fn round_button(id: &'static str, _palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
}
