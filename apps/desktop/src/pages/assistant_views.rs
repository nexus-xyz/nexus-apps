//! The Assistant's views beside its chat, as Nexus UI's `/app/<org>/agent?view=…`: Errands, Discover,
//! What Nexus knows, Approvals, Passwords and payments and Privacy. Each is its own GPUI view in
//! `assistant_views/`, drawn from the design boards. Each reads the agent API as the chat does, through the backend that holds the person's token
//! (`Backend::agent`), with only the calls `crate::agent::Call` builds from the core's rules. The strict readers and the
//! words are `nexus_desktop_core`'s (`agent_approvals`, `agent_discover`, `agent_errands`, `agent_memory`,
//! `agent_privacy`, `agent_vault`).
//!
//! The workspace draws the strip over the chat and these views ([`strip`]), and opens them with its own [`Moves`]:
//! Discover's "Try it" hands a line to the chat, and What Nexus knows opens an errand. A part that cannot read says so
//! in one line, and the rest of the view still works.

use crate::workspace::glass::FloatingGround;
use std::rc::Rc;

use futures::StreamExt as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

use super::{Ctx, PageId, ui};
use crate::agent::{Call, Chunk};
use crate::palette::Palette;
use nexus_desktop_core::agent::Problem;

mod approvals;
mod discover;
mod errands;
mod memory;
mod privacy;
mod proactivity;
mod vault;
mod watches;

#[cfg(test)]
pub use approvals::ApprovalsPage;
#[cfg(test)]
pub use discover::{DiscoverPage, Section};
pub use errands::{ErrandsPage, mark as errand_mark};
#[cfg(test)]
pub use memory::{LineMode, MemoryPage};
#[cfg(test)]
pub use privacy::{Asking, PrivacyPage};
#[cfg(test)]
pub use proactivity::{ProactivityPanel, lede as proactivity_lede, local_zone};
pub use proactivity::{WhySheet, why_sheet};
#[cfg(test)]
pub use vault::VaultPart;
pub use vault::{VaultPage, asks as vault_asks};

/// What a view gets from the window: whether the account is Personal, the Assistant's name and the person's, how to
/// hand a line to the chat and open another view (an errand, for Errands), what Errands read or changed, and how
/// Privacy tells the sidebar's row that the person paused or resumed.
#[derive(Clone, Default)]
pub struct Moves {
    pub personal: bool,
    /// The Assistant's name, as the person named it; Nexus when None. Read it with [`Moves::name`].
    pub name: Option<SharedString>,
    /// The person's display name, for Approvals' two voices.
    pub person: Option<String>,
    pub hand_off: Option<HandOff>,
    pub open: Option<Open>,
    /// Tell the window what Errands read or changed: the sidebar's rows and the chat's card follow it.
    pub errands: Option<ErrandsNote>,
    /// Open Errands with the New errand box holding this text (⇧⌘J, or ⌘K's New errand).
    pub new_errand: Option<String>,
    pub paused: Option<Paused>,
    /// Open Errands on this project's page: its errands, and Ask Nexus about it.
    pub project: Option<String>,
    /// The organization's name, for "Acme projects only" and "Acme · project"; "Personal" for the person's own.
    pub organization: Option<String>,
    /// Open a `nexus://new?…` link as a New session draft, never sent: an errand's Hand off to a session. The
    /// same path as the chat's session card (`Workspace::open_prompt_link`). None: the menu has no Hand off.
    pub open_draft: Option<HandOff>,
}

/// The pause changed: the sidebar's Assistant row says Paused, or no longer.
pub type Paused = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// What Errands tells the window.
#[derive(Clone, Debug, PartialEq)]
pub enum Noted {
    /// The list as it read it.
    Listed(Vec<nexus_desktop_core::agent_errands::Errand>),
    /// An errand started or changed: the window reads the list again.
    Changed,
    /// The errand open now, or none.
    Opened(Option<String>),
    /// Open this project's page: a filed errand's "in Fundraise" link.
    Project(String),
}

/// Errands' word to the window.
pub type ErrandsNote = Rc<dyn Fn(Noted, &mut Window, &mut App)>;

/// Send a line to the chat.
pub type HandOff = Rc<dyn Fn(String, &mut Window, &mut App)>;
/// Open a view of the Assistant, on one errand for Errands.
pub type Open = Rc<dyn Fn(PageId, Option<String>, &mut Window, &mut App)>;
/// The strip's pick.
pub type Pick = Rc<dyn Fn(PageId, &mut Window, &mut App)>;

impl Moves {
    /// The Assistant's name: the person's choice, else Nexus.
    pub fn name(&self) -> SharedString {
        self.name
            .clone()
            .unwrap_or_else(|| nexus_desktop_core::agent_cards::DEFAULT_ASSISTANT_NAME.into())
    }

    /// Send `line` to Nexus as the person's message; without a window to hand it to, open the chat.
    pub fn hand_off(&self, ctx: &Ctx, line: &str, window: &mut Window, cx: &mut App) {
        match &self.hand_off {
            Some(hand_off) => hand_off(line.to_string(), window, cx),
            None => (ctx.navigate)(PageId::Agent, window, cx),
        }
    }

    /// Open a view of the Assistant, on one errand for Errands.
    pub fn open(&self, ctx: &Ctx, id: PageId, errand: Option<String>, window: &mut Window, cx: &mut App) {
        match &self.open {
            Some(open) => open(id, errand, window, cx),
            None => (ctx.navigate)(id, window, cx),
        }
    }
}

/// The view for `id`, one of `PageId::is_assistant_view`.
pub fn build(id: PageId, ctx: Ctx, moves: Moves, window: &mut Window, cx: &mut App) -> AnyView {
    build_on(id, ctx, moves, None, window, cx)
}

/// The view for `id`; Errands opens on `errand` once its list is read.
pub fn build_on(
    id: PageId,
    ctx: Ctx,
    moves: Moves,
    errand: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    match id {
        PageId::AgentErrands => errands::build(ctx, moves, errand, window, cx).into(),
        PageId::AgentDiscover => discover::build(ctx, moves, window, cx).into(),
        PageId::AgentMemory => memory::build(ctx, moves, window, cx).into(),
        PageId::AgentVault => vault::build(ctx, window, cx).into(),
        PageId::AgentPrivacy => privacy::build(ctx, moves, window, cx).into(),
        PageId::AgentWatches => watches::build(ctx, moves, window, cx).into(),
        _ => approvals::build(ctx, moves, window, cx).into(),
    }
}

/// The views, in the strip's order: the chat first.
pub const STRIP: [PageId; 8] = [
    PageId::Agent,
    PageId::AgentErrands,
    PageId::AgentDiscover,
    PageId::AgentMemory,
    PageId::AgentApprovals,
    PageId::AgentVault,
    PageId::AgentWatches,
    PageId::AgentPrivacy,
];

/// The strip's word for a view: "Chat" for the Assistant itself.
pub fn strip_label(id: PageId) -> &'static str {
    if id == PageId::Agent { "Chat" } else { id.label() }
}

/// The strip over the Assistant (the header's details): one tab per view, `open` the picked one.
pub fn strip(open: PageId, pick: Pick, palette: Palette) -> Div {
    h_flex()
        .w_full()
        .flex_none()
        .justify_center()
        .py(px(8.))
        .border_b_1()
        .border_color(palette.border)
        .bg(crate::frost::cover(&palette))
        .child(ui::tabs(palette).children(STRIP.map(|id| {
            let pick = pick.clone();
            ui::tab(("assistant-view", id as usize), strip_label(id), id == open, palette)
                .on_click(move |_, window, cx| pick(id, window, cx))
        })))
}

/// The answer of one call: its status and its JSON body, when it had one.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Option<Value>,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The chunks of one answer, gathered into a [`Reply`]. A body that broke off reads as a 502.
#[derive(Debug, Default)]
pub struct Gather {
    status: Option<u16>,
    bytes: Vec<u8>,
}

impl Gather {
    /// The next chunk; the reply once the answer ended.
    pub fn push(&mut self, chunk: Chunk) -> Option<Reply> {
        match chunk {
            Chunk::Head { status, .. } => self.status = Some(status),
            Chunk::Data(bytes) => self.bytes.extend_from_slice(&bytes),
            Chunk::Broken => {
                return Some(Reply {
                    status: 502,
                    body: None,
                });
            }
            Chunk::End => {
                return Some(Reply {
                    status: self.status.unwrap_or(502),
                    body: serde_json::from_slice(&self.bytes).ok(),
                });
            }
        }
        None
    }
}

/// Make `call` to the agent API and give its reply to `apply`, then redraw. Keep the task: dropping it stops the call.
pub fn call<V: 'static>(
    ctx: &Ctx,
    call: Call,
    window: &mut Window,
    cx: &mut Context<V>,
    apply: impl FnOnce(&mut V, Reply, &mut Window, &mut Context<V>) + 'static,
) -> Task<()> {
    // Read at each call, as the chat reads the setting.
    let base = nexus_desktop_core::agent::base_url(crate::desktop_settings::load().agents_api_url.as_deref());
    let crate::agent::Stream { mut chunks, _stop } = ctx.gateway.agent(base, call);
    cx.spawn_in(window, async move |this, cx| {
        let _stop = _stop;
        let mut gather = Gather::default();
        let mut reply = Reply {
            status: 502,
            body: None,
        };
        while let Some(chunk) = chunks.next().await {
            if let Some(done) = gather.push(chunk) {
                reply = done;
                break;
            }
        }
        // A view that closed first has no one to show it to.
        this.update_in(cx, |view, window, cx| {
            apply(view, reply, window, cx);
            cx.notify();
        })
        .ok();
    })
}

/// A view's frame (the boards' main area): it scrolls, with the title and the lede over a column of `width`.
pub fn frame(
    id: &'static str,
    title: &'static str,
    lede: &'static str,
    width: f32,
    palette: Palette,
) -> (Stateful<Div>, Div) {
    let outer = div().id(id).size_full().overflow_y_scroll();
    let column = v_flex()
        .w_full()
        .max_w(px(width))
        .mx_auto()
        .px(px(28.))
        .pt(px(20.))
        .pb(px(40.))
        .gap(px(16.))
        .child(
            v_flex()
                .gap(px(2.))
                .child(div().text_size(px(20.)).text_color(palette.foreground).child(title))
                .child(div().text_size(px(13.)).text_color(palette.muted).child(lede)),
        );
    (outer, column)
}

/// The boards' pill button: 28px tall, round, with a hairline ring and no fill.
pub fn pill(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    pill_base(id, text, palette).hover(|style| style.bg(palette.hover))
}

/// A pill's shape and words, without its hover.
fn pill_base(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    ui::focusable(h_flex().id(id), palette)
        .flex_none()
        .h(px(28.))
        .px(px(12.))
        .justify_center()
        .rounded(px(14.))
        .border_1()
        .border_color(palette.border)
        .text_size(px(12.5))
        .text_color(palette.foreground)
        .whitespace_nowrap()
        .cursor_pointer()
        .child(text.into())
}

/// The boards' filled pill: the one primary action.
pub fn primary_pill(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    pill_base(id, text, palette)
        .bg(palette.primary)
        .border_color(palette.primary)
        .text_color(palette.primary_foreground)
        .hover(|style| style.opacity(0.9))
}

/// A pill that does nothing now: faint, while a change is sent.
pub fn quiet_pill(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    pill_base(id, text, palette).opacity(0.5).cursor_default()
}

/// The boards' red pill: an action that ends something, such as Revoke all.
pub fn danger_pill(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    pill_base(id, text, palette)
        .bg(palette.destructive)
        .border_color(palette.destructive)
        .text_color(gpui_kit::white())
        .hover(|style| style.opacity(0.9))
}

/// A modal over a view (the web's dialogs): the scrim over the whole view, and `sheet` in its middle. The caller keeps
/// Esc on the sheet (`on_key_down`) and the focus in it.
pub fn modal(id: &'static str, sheet: impl IntoElement, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(palette.scrim)
        .occlude()
        .child(sheet)
}

/// A modal's card, `width` wide: raised, round, with the pop shadow.
pub fn sheet(id: &'static str, focus: &FocusHandle, width: f32, palette: Palette) -> Stateful<Div> {
    v_flex()
        .id(id)
        .track_focus(focus)
        .occlude()
        .w(px(width))
        .max_w(relative(0.94))
        .p(px(20.))
        .gap(px(12.))
        .rounded(px(14.))
        .floating_ground(14., palette.pop, true, &palette)
        .border_1()
        .border_color(palette.border)
        .shadow(palette.pop_shadow(palette.border))
        .text_color(palette.foreground)
}

/// A section's head inside a view: 17px, as the boards title a part of a page.
pub fn head(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .text_size(px(17.))
        .text_color(palette.foreground)
        .child(text.into())
}

/// The boards' card: raised, with a hairline ring.
pub fn card(palette: Palette) -> Div {
    v_flex()
        .min_w(px(0.))
        .p(px(14.))
        .gap(px(8.))
        .rounded(px(12.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
}

/// A line of quiet text.
pub fn quiet(text: impl Into<SharedString>, palette: Palette) -> Div {
    div().text_size(px(13.)).text_color(palette.muted).child(text.into())
}

/// A failed read or change, in its own words.
pub fn problem(problem: &Problem, palette: Palette) -> Div {
    ui::settings_notice(problem.title.clone(), problem.detail.clone(), palette)
}

/// An ISO time as the boards date it: "Nov 3".
pub fn day(iso: &str) -> String {
    crate::library::chrono_lite::month_day(iso)
}

/// A change that asks first (the web's `ConfirmButton`): its question and hint, with Yes and Cancel.
pub fn confirm(
    id: &'static str,
    question: &str,
    hint: &str,
    yes: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    no: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    palette: Palette,
) -> Div {
    h_flex()
        .gap(px(8.))
        .flex_wrap()
        .child(
            v_flex()
                .min_w(px(0.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(palette.foreground)
                        .child(question.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(hint.to_string()),
                ),
        )
        .child(primary_pill((id, 0usize), "Yes", palette).on_click(yes))
        .child(pill((id, 1usize), "Cancel", palette).on_click(no))
}

#[cfg(test)]
mod tests {
    use super::{Chunk, Gather, PageId, Reply, STRIP, strip_label};

    #[test]
    fn gathers_an_answer_and_reads_a_broken_one_as_a_502() {
        let mut gather = Gather::default();
        assert_eq!(
            gather.push(Chunk::Head {
                status: 200,
                content_type: "application/json".into()
            }),
            None
        );
        assert_eq!(gather.push(Chunk::Data(br#"{"ok":"#.to_vec())), None);
        assert_eq!(gather.push(Chunk::Data(b"true}".to_vec())), None);
        assert_eq!(
            gather.push(Chunk::End),
            Some(Reply {
                status: 200,
                body: Some(serde_json::json!({ "ok": true }))
            })
        );
        assert_eq!(
            Gather::default().push(Chunk::Broken),
            Some(Reply {
                status: 502,
                body: None
            })
        );
        assert_eq!(Gather::default().push(Chunk::End).map(|reply| reply.status), Some(502));
    }

    /// A view names the Assistant as the person named it, and Nexus without a name.
    #[test]
    fn a_view_names_the_assistant_and_nexus_by_default() {
        use super::{Moves, Section};
        assert_eq!(Moves::default().name(), "Nexus");
        let named = Moves {
            name: Some("Ada".into()),
            ..Moves::default()
        };
        assert_eq!(named.name(), "Ada");
        assert_eq!(
            Section::Try.lede(&named.name()),
            "One tap sends the words to Ada. Change them in the chat if you like."
        );
        assert_eq!(
            Section::Teach.lede("Ada"),
            "It learns as you go. Correct anything in Memory; it forgets at once."
        );
    }

    #[test]
    fn the_strip_puts_the_chat_first_then_each_view() {
        assert_eq!(
            STRIP.map(strip_label),
            [
                "Chat",
                "Errands",
                "Discover",
                "Memory",
                "Approvals",
                "Passwords and payments",
                "Watches",
                "Privacy"
            ]
        );
        assert!(
            STRIP[1..]
                .iter()
                .all(|id| id.is_assistant_view() && id.is_place() && id.is_native())
        );
        assert_eq!(
            PageId::from_web_path("agent?view=discover"),
            Some(PageId::AgentDiscover)
        );
        assert_eq!(PageId::from_web_path("agent?view=memory"), Some(PageId::AgentMemory));
        assert_eq!(PageId::from_web_path("agent?view=vault"), Some(PageId::AgentVault));
        assert_eq!(PageId::from_web_path("agent?view=privacy"), Some(PageId::AgentPrivacy));
        assert_eq!(PageId::from_web_path("agent?view=watches"), Some(PageId::AgentWatches));
    }
}
