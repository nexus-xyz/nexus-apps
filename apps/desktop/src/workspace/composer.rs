//! The prompt box, as the Nexus app's (`ui/src/app/app/v5/composer.css`, `composer.tsx`): the
//! text, then a toolbar with attach and the model on the left and dictation and send (or stop) on the right. At rest
//! it is the board's plain surface, a hairline ring and a soft shadow; focused, a stronger neutral ring. A suggested next prompt shows as the
//! placeholder, and Tab takes it. Attached images and files wait over the text as chips (`attachments.rs`): attach
//! with the button, a paste, or a drop on the window.
//!
//! While its session works or waits on the person, the composer says so where the eye already is: a pill in the
//! toolbar (a breathing dot, what happens and for how long) and an arc turning on Stop's ring while there is something
//! to stop (`Activity`). The box itself stays neutral.

use crate::workspace::overlay::{Overlay, OverlaySlot};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Enter, Escape, IndentInline, MoveDown, MoveUp, Textarea};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{PLACEHOLDER, View, Workspace};
use crate::artifact_refs::ArtifactRef;
use crate::attachments::{self, Draft, Target};
use crate::code::{self, Status};
use crate::file_icons::FileIcon;
use crate::i18n::tr;
use crate::motion::{self, TweenExt as _};
use crate::palette::Palette;

/// The composer's corners (`--radius-composer`).
pub(super) const RADIUS: f32 = crate::tokens::radius::COMPOSER;

/// How far the composer's ring and near shadow reach outside its box, the room a scroll box leaves them.
pub(super) const HALO_WIDTH: f32 = 4.;

/// A scroll box that holds a composer as wide as itself: room for what lies outside the composer's box.
///
/// A scroll box clips what it holds to its own bounds. Around a composer as wide as itself, it cut what lies outside the
/// box down to the corners. The box takes that reach as padding and gives it back as a negative margin, so
/// its clip holds it and the composer keeps its place. Leave the box's width to its parent's stretch: a full width
/// would ignore the margins and narrow the composer.
pub(super) fn halo_room(scroll: Stateful<Div>) -> Stateful<Div> {
    scroll.p(px(HALO_WIDTH)).m(px(-HALO_WIDTH))
}

/// The composer's fill: the board's Composer surface, white or
/// #323236, at the window's transparency (`frost::Look::surface`): 94% on Frost, see-through on Glass, solid on Opaque.
fn composer_fill(palette: Palette) -> Hsla {
    crate::frost::Look::now(&palette).surface
}

/// The composer's soft shadow, the board's: a 1px drop and a 6px one.
fn composer_shadow(palette: Palette) -> Vec<BoxShadow> {
    let (near, far) = if palette.dark { (0.15, 0.18) } else { (0.05, 0.06) };
    let drop = |alpha: f32, y: f32, blur: f32| BoxShadow {
        color: black().opacity(alpha),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    };
    vec![drop(near, 1., 2.), drop(far, 6., 16.)]
}

/// The composer's ring: the board's hairline at rest; focused, or with files dragged over the window, a neutral ink a
/// few steps stronger. No hue and no glow: the toolbar's pill and Stop's arc say what the session does.
fn composer_ring(focused: bool, palette: Palette) -> Hsla {
    let alpha = if focused { 0.2 } else { 0.06 };
    if palette.dark {
        white().opacity(alpha)
    } else {
        black().opacity(alpha)
    }
}

/// The composer's frame: the box (`composer_shell`), with its ring.
pub(super) fn composer_frame(box_: Div, focused: bool, palette: Palette) -> Div {
    div()
        .relative()
        .w_full()
        .child(box_.border_color(composer_ring(focused, palette)))
}

/// Whether the composer's session works or waits on the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActivityKind {
    Working,
    Waiting,
}

impl ActivityKind {
    fn tone(self, palette: Palette) -> Hsla {
        match self {
            ActivityKind::Working => palette.working(),
            ActivityKind::Waiting => palette.waiting(),
        }
    }
}

/// What the composer's session is doing now, as its pill says it: the kind, the words, and since when (the clock
/// shows only where the transcript keeps one, a Code turn's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Activity {
    pub kind: ActivityKind,
    pub label: &'static str,
    pub since: Option<Instant>,
}

/// Where a new Code session's start is: none in this view, under way, or failed (the view shows why, with Try again).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Start {
    None,
    Underway,
    Failed,
}

impl Start {
    fn of(pending: Option<&super::code_view::Pending>) -> Start {
        match pending {
            None => Start::None,
            Some(pending) if pending.failed.is_some() => Start::Failed,
            Some(_) => Start::Underway,
        }
    }
}

/// The composer's activity from the view's state. A wait outranks work: the turn holds until the person answers.
/// `approval`: a card has no answer yet. It counts only while the session runs or needs the person: an approval gets
/// its answer from a later event, and a stream that ended without one leaves the card unanswered on a session at rest.
/// `session`: the open Code session's state. `start`: a new Code session's start; a failed one shows nothing, since
/// the view already says why it failed. `asking`: a chat's answer is under way.
pub(super) fn activity_of(
    approval: bool,
    session: Option<Status>,
    start: Start,
    asking: bool,
    since: Option<Instant>,
) -> Option<Activity> {
    let activity = |kind, label, since| Some(Activity { kind, label, since });
    let live = matches!(session, Some(Status::Running | Status::NeedsYou));
    if (approval && live) || session == Some(Status::NeedsYou) {
        activity(ActivityKind::Waiting, "Waiting for you", None)
    } else if start == Start::Underway {
        activity(ActivityKind::Working, "Starting", None)
    } else if session == Some(Status::Running) {
        activity(ActivityKind::Working, "Working", since)
    } else if asking {
        activity(ActivityKind::Working, "Answering", None)
    } else {
        None
    }
}

/// Shared shell geometry, with the board's plain surface: its fill and its soft shadow. The surface is anchored
/// independently of content padding. On a new session's art (`frosted`) the fill lies on the art's frost
/// (`backdrop::frost`); elsewhere on a blur of what is under it (`glass.rs`).
pub(super) fn composer_shell(frosted: bool, palette: Palette) -> Div {
    let fill = composer_fill(palette);
    let surface = composer_surface().debug_selector(|| "composer-surface".into());
    let surface = if frosted {
        surface.child(super::backdrop::frost(RADIUS, fill, None))
    } else {
        surface.child(super::glass::glass(RADIUS, fill, None))
    };
    v_flex()
        .relative()
        .min_w_0()
        .w_full()
        .rounded(px(RADIUS))
        .border_1()
        .shadow(composer_shadow(palette))
        .child(surface)
}

/// Where the composer's glass or frost lies: over the shell's whole box, border included, with the shell's corners.
/// An absolute child is laid out in its parent's padding box, 1px inside the border, so `inset_0` would leave the
/// surface's 16px corners rounder than the border's 15px inner curve: at each corner a sliver of what lies under the
/// box (the art) showed between the two. The surface draws no ring of its own: the shell's border
/// (`composer_frame`) is the one ring, and GPUI paints it after the children, over the surface's edge.
pub(super) fn composer_surface() -> Div {
    div().absolute().inset(px(-1.))
}

pub(super) fn composer_input(textarea: Textarea) -> Div {
    div()
        .debug_selector(|| "composer-input".into())
        .flex()
        .flex_col()
        .min_h(px(62.))
        .px(px(6.))
        .pt(px(6.))
        .text_size(px(14.5))
        .line_height(relative(1.5))
        // The entire visible writing area must receive the textarea's mouse events.
        .child(textarea.flex_grow_1())
}

pub(super) fn composer_toolbar() -> Div {
    h_flex()
        .debug_selector(|| "composer-toolbar".into())
        .min_h(px(28.))
        .pt(px(14.))
        .pr(px(12.))
        .pb(px(10.))
        .pl(px(16.))
        .gap(px(8.))
}

/// Attach (`.composer-attach`): a 28px ring with a plus; filled while its menu is open.
pub(super) fn attach_ring(id: impl Into<ElementId>, open: bool, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(palette.control_ring(open))
        .text_color(if open { palette.foreground } else { palette.icon })
        .when(open, |button| button.bg(palette.control_fill(true)))
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(palette.control_fill(false))
                .border_color(palette.control_ring(true))
                .text_color(palette.foreground)
        })
        .active(|style| style.bg(palette.control_fill(true)))
        .child(Icon::new(Lucide::Plus).size(px(16.)))
}

/// Send (`.composer-send`): a 28px filled circle with an arrow, faded while there is nothing to send.
pub(super) fn send_circle(id: impl Into<ElementId>, enabled: bool, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .ml(px(4.))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(palette.primary)
        .when(!enabled, |button| button.opacity(0.4))
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.opacity(0.88))
                .active(|style| style.opacity(0.72))
        })
        .child(
            Icon::new(Lucide::ArrowUp)
                .size(px(16.))
                .text_color(palette.primary_foreground),
        )
}

/// Queue beside Send while a turn runs: a hairline 28px pill with the list glyph.
pub(super) fn queue_pill(id: impl Into<ElementId>, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(28.))
        .ml(px(4.))
        .pl(px(10.))
        .pr(px(12.))
        .gap(px(6.))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(palette.border)
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|style| style.bg(palette.raised))
        .active(|style| style.opacity(0.72))
        .child(Icon::new(Lucide::ListOrdered).size(px(14.)))
        .child(tr("Queue"))
}

/// Stop: a 28px ring with a square, and an arc in `tone` turning on it while the work goes on (still when `still`).
pub(super) fn stop_ring(id: &'static str, tone: Hsla, still: bool, palette: Palette) -> Stateful<Div> {
    // The ring's own box (the border is drawn inside it), so the arc lies on the hairline, not beside it.
    let arc = svg()
        .path(crate::assets::STOP_ARC)
        .absolute()
        .top(px(-1.))
        .left(px(-1.))
        .size(px(28.))
        .text_color(tone);
    let arc = if still {
        arc.into_any_element()
    } else {
        arc.with_animation(
            "stop-arc",
            Animation::new(Duration::from_millis(1100))
                .repeat()
                .with_max_fps(crate::motion::LOOP_FPS),
            |arc, delta| arc.with_transformation(Transformation::rotate(percentage(delta))),
        )
        .into_any_element()
    };
    div()
        .id(id)
        .group(id)
        .relative()
        .size(px(28.))
        .ml(px(4.))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(palette.control_ring(false))
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(palette.control_fill(false))
                .border_color(palette.control_ring(true))
        })
        .active(|style| style.bg(palette.control_fill(true)))
        .child(arc)
        .child(
            div()
                .size(px(9.))
                .rounded(px(2.))
                .bg(palette.foreground)
                .opacity(0.85)
                .group_hover(id, |style| style.opacity(1.)),
        )
}

/// Container breakpoints, based on this composer's measured width.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ComposerLayout {
    pub control_icons: bool,
    pub chat_icons: bool,
    pub wrap: bool,
}

impl ComposerLayout {
    fn at(width: Pixels) -> Self {
        Self {
            control_icons: width <= px(520.),
            chat_icons: width <= px(600.),
            wrap: width <= px(320.),
        }
    }
}

/// The note under the composer's field (a failed send, a budget, a bad attachment), with the link that may end it
/// (dictation's). A person can select and copy the note, to paste into a report or a search.
pub(super) fn composer_note_line(note: SharedString, link: Option<impl IntoElement>, palette: Palette) -> Div {
    h_flex()
        .flex_wrap()
        .gap(px(6.))
        .pt(px(10.))
        .px(px(16.))
        .text_size(px(13.5))
        .text_color(palette.muted)
        .child(super::message_text::MessageText::new("composer-note", note))
        .children(link)
}

impl Workspace {
    pub(super) fn render_composer(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.composer_focused || self.file_drag;
        // Focused, or with files dragged over the window, the ring turns from the hairline to a neutral ink.
        let note = self
            .mic_note
            .clone()
            .or_else(|| self.composer_note.clone())
            .or_else(|| self.draft_problem(cx));
        // A Code session's agent and model are the session's own: no attach or chat model chip, but the session's own
        // model and effort chips (`code_dock.rs`), and Stop while it runs. Enter still queues a message then.
        let code = matches!(self.view, View::Code(_) | View::NewCode);
        let session = matches!(self.view, View::Code(_)) && self.code_composer_available();
        let workspace = cx.weak_entity();
        let paste = {
            let workspace = workspace.clone();
            move |item: &ClipboardItem, _: &mut Window, cx: &mut App| {
                let pasted = attachments::pasted(item);
                if !pasted.files.is_empty() {
                    let _ = workspace.update(cx, |this, cx| this.attach(pasted.files, cx));
                }
                pasted.take
                    || workspace
                        .update(cx, |this, cx| this.paste_large_text(item, cx))
                        .unwrap_or(false)
            }
        };
        let open: Rc<dyn Fn(usize, &mut Window, &mut App)> = {
            let workspace = workspace.clone();
            Rc::new(move |index, _, cx| {
                let _ = workspace.update(cx, |this, cx| {
                    let images = this.draft.images.clone();
                    this.open_image(&images, index, cx);
                });
            })
        };
        let open_artifact = self.artifact_opener(cx);
        let remove: Rc<dyn Fn(u64, &mut Window, &mut App)> = Rc::new(move |id, _, cx| {
            let _ = workspace.update(cx, |this, cx| {
                this.draft.remove(id);
                this.composer_note = None;
                cx.notify();
            });
        });
        let busy = self.asking.is_some() || self.code_running();
        let activity = self.composer_activity();
        let home = matches!(self.view, View::NewCode);
        // A session just started trades the home's toolbar chips for its own on the start's clock (`launch_motion.rs`):
        // the home's fade out over the first quarter, then the session's fade in, so the box keeps its size.
        let launch = self.launch_frame();
        let trading = launch.is_some_and(|frame| frame.trading);
        let fade = |opacity: f32, element: AnyElement| -> AnyElement {
            h_flex().min_w_0().opacity(opacity).child(element).into_any_element()
        };
        let leaving = |element: AnyElement| match launch {
            Some(frame) if !home => fade(frame.home, element),
            _ => element,
        };
        let arriving = |element: AnyElement| match launch {
            Some(frame) => fade(frame.arriving, element),
            None => element,
        };
        // On a new session's background the box is frosted glass over the art (`backdrop.rs`).
        let frosted = self.frosted();
        let layout = self.composer_layout;
        let measured_workspace = cx.weak_entity();
        let box_ = composer_shell(frosted, palette)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let next = ComposerLayout::at(bounds.size.width);
                        if next != layout {
                            let workspace = measured_workspace.clone();
                            cx.defer(move |cx| {
                                let _ = workspace.update(cx, |this, cx| {
                                    if this.composer_layout != next {
                                        this.composer_layout = next;
                                        cx.notify();
                                    }
                                });
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            // Esc closes the "+" or microphone menu before anything else hears it.
            .when(self.overlay.attach_menu() || self.overlay.mic_menu(), |shell| {
                shell.on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.overlay.close_attach_menu();
                        this.close_mic_menu();
                        cx.notify();
                    }
                }))
            })
            // Each row over the text opens and closes in height (`motion::reveal`), so the box grows and shrinks
            // instead of jumping.
            .child(section("composer-plans-off", self.render_plans_off(palette, cx)))
            .child(section(
                "composer-blockers",
                self.render_start_blockers(palette, cx)
                    .or_else(|| self.render_limit_line(palette, cx)),
            ))
            // Passages of answers with their replies, sent ahead of the text (`stacked_replies.rs`).
            .child(section("composer-replies", self.render_reply_chips(palette, cx)))
            .child({
                // The note's words stay while it closes. Dictation's note may end with a link (`mic.rs`).
                let shown = note.is_some();
                let link = self
                    .render_mic_note_link(palette, cx)
                    .or_else(|| self.render_pick_project_link(note.as_ref(), palette, cx));
                let note = motion::presence_of("composer-note", note, cx)
                    .map(|(_, note)| composer_note_line(note, link, palette).into_any_element());
                motion::reveal("composer-note", shown, note)
            })
            // A chat's follow-ups queued behind its answer (`chat_queue.rs`), at the box's top as the Nexus app's.
            .child(section("composer-chat-queue", self.render_chat_queue(palette, cx)))
            // A Code session's messages the agent has not read yet, "On queue" (`code_dock.rs`), in the same place.
            .child(section("composer-on-queue", self.render_code_on_queue(palette, cx)))
            // A cloud session's machine holds commits on no remote: "Archive anyway" or "Delete anyway".
            .child(section(
                "composer-cloud-question",
                self.render_cloud_question(palette, cx),
            ))
            // A sent message's files while they go up in pieces, then what waits for the next message.
            .child(section("composer-uploading", self.render_uploading(palette, cx)))
            .child(section(
                "composer-diff-comments",
                self.render_diff_comments(palette, cx),
            ))
            .child(section(
                "composer-attachments",
                (!self.draft.is_empty()).then(|| {
                    div()
                        .pt(px(14.))
                        .pr(px(12.))
                        .pl(px(16.))
                        .child(artifact_chips(
                            "composer",
                            &self.draft,
                            palette,
                            Some(remove),
                            None,
                            Some(open),
                            Some(open_artifact),
                        ))
                        .into_any_element()
                }),
            ))
            .child(motion::reveal(
                "composer-attaching",
                self.attaching > 0,
                Some(
                    div()
                        .pt(px(10.))
                        .px(px(16.))
                        .text_size(px(13.))
                        .text_color(palette.muted)
                        .child(tr("Preparing attachments…"))
                        .into_any_element(),
                ),
            ))
            // The text sits 16px in and 14px down, as the Nexus app's: the text area keeps 10px and 8px of its own, so
            // 6px here. 62px tall at least puts the toolbar's middle 90px down, where the Nexus app's is.
            .child(
                composer_input(Textarea::new(&self.composer).appearance(false).on_paste(paste))
                    .id("composer-text")
                    .when_some(
                        match &self.view {
                            View::Code(view) => Some((view.dock.tray_layout.pane_height() * 0.25).clamp(62., 240.)),
                            _ => None,
                        },
                        |area, height| area.max_h(px(height)).overflow_y_scroll(),
                    ),
            )
            .child(
                composer_toolbar()
                    .gap(px(if code { 3. } else { 8. }))
                    .when(layout.wrap, |bar| bar.flex_wrap())
                    .when(!code || session || home, |bar| {
                        bar.child(self.render_attach(palette, cx))
                    })
                    .when(!code, |bar| bar.child(self.render_model_chip(palette, cx)))
                    .when(!code, |bar| bar.children(self.render_chat_team_chip(palette, cx)))
                    .when(session && !trading, |bar| {
                        bar.child(arriving(self.render_code_chips(palette, cx).into_any_element()))
                    })
                    // The plan's 5-hour limit, after the effort (`new_code.rs`).
                    .when(code, |bar| bar.children(self.render_usage_donut(palette, cx)))
                    // Once something is stashed (`stash.rs`), after the chips, as the Nexus app's.
                    .children(self.render_stash(palette, cx))
                    .child(div().flex_1())
                    // Home: who runs it (agent and model) sits right, before the mic.
                    .when(home || code && trading, |bar| {
                        bar.child(leaving(self.render_launch_right(palette, cx)))
                    })
                    // The Code now line already says what the agent is doing and for how long (`render_working`), so the
                    // pill stays for a wait and for chat: one timer on screen, not two. It and the buttons after the
                    // mic open and close in width (`motion::reveal_width`): Stop slides in as Send slides out.
                    .child({
                        let pill = activity.filter(|activity| {
                            !trading && (!matches!(self.view, View::Code(_)) || activity.kind == ActivityKind::Waiting)
                        });
                        let shown = pill.is_some();
                        let pill = motion::presence_of("composer-activity", pill, cx)
                            .map(|(_, pill)| arriving(self.render_activity(pill, palette, cx).into_any_element()));
                        motion::reveal_width("composer-activity-slot", shown, pill)
                    })
                    .child(self.render_mic(palette, cx))
                    .child(toolbar_slot(
                        "stop-slot",
                        busy && !trading,
                        arriving(self.render_stop(activity, palette, cx).into_any_element()),
                    ))
                    // While a chat answers, the primary becomes Queue, which Enter presses.
                    .when(!code, |bar| {
                        bar.child(toolbar_slot(
                            "queue-slot",
                            busy && !trading,
                            arriving(self.render_queue(palette, cx).into_any_element()),
                        ))
                    })
                    // A Code turn takes a message while it runs: once there is one, Queue (after the turn) and Send
                    // (into it now) stand beside Stop.
                    .when(session, |bar| {
                        bar.child(toolbar_slot(
                            "code-queue-slot",
                            busy && self.composer_send_enabled() && !trading,
                            arriving(self.render_code_queue(palette, cx).into_any_element()),
                        ))
                    })
                    .map(|bar| {
                        let send = !busy || (code && self.composer_send_enabled());
                        // A session just started keeps the home's Send while the home's chips leave.
                        let button = if send {
                            self.render_send(palette, cx).into_any_element()
                        } else {
                            leaving(self.render_send(palette, cx).into_any_element())
                        };
                        bar.child(toolbar_slot("send-slot", send || trading, button))
                    }),
            );
        // The slash menu over the composer, or below a new session's (as the Nexus app's `.wb-new .wb-slash`).
        let below = matches!(self.view, View::NewCode);
        let slash = self.render_slash_menu(below, palette, cx);
        // The "@" and "#" menu (`mention.rs`), in the same place; it stays closed while the slash menu is open.
        let mention = self.render_mention_menu(below, palette, cx);
        let composer = composer_frame(box_, focused, palette)
            // Up, Down, Enter, Tab and Esc go to the slash or mention menu while one is open, before the text takes them.
            // With no menu open, they walk through the session's sent messages (`prompt_history.rs`).
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if this.chat_skill_move(false, cx)
                    || this.slash_move(false, cx)
                    || this.mention_move(false, cx)
                    || this.recall_sent(false, window, cx)
                {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if this.chat_skill_move(true, cx)
                    || this.slash_move(true, cx)
                    || this.mention_move(true, cx)
                    || this.recall_sent(true, window, cx)
                {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, action: &Enter, window, cx| {
                if action.shift && !action.secondary && matches!(this.view, View::Code(_) | View::NewCode) {
                    let composer = this.composer.read(cx);
                    let range = composer.selected_range();
                    let at = if range.is_empty() {
                        composer.cursor()..composer.cursor()
                    } else {
                        range
                    };
                    if let Some(edit) =
                        nexus_workbench::markdown_newline::markdown_newline(&composer.value(), at.start, at.end)
                    {
                        this.composer.update(cx, |state, cx| {
                            state.set_selected_range(edit.range, cx);
                            state.replace(edit.insert, window, cx);
                            state.set_selected_range(edit.caret..edit.caret, cx);
                        });
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                }
                if !action.shift
                    && !action.secondary
                    && (this.chat_skill_complete(window, cx)
                        || this.slash_complete_active(true, window, cx)
                        || this.mention_pick_active(window, cx))
                {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.chat_skill_complete(window, cx)
                    || this.slash_complete_active(false, window, cx)
                    || this.mention_pick_active(window, cx)
                {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.chat_skill_dismiss(cx) || this.slash_dismiss(cx) || this.mention_dismiss(cx) {
                    cx.stop_propagation();
                }
            }))
            .children(self.render_chat_skills(palette, cx))
            .children(slash)
            .children(mention)
            .when(self.file_drag, |frame| {
                // `.composer-drop`: what a drop does, over the box while files are dragged over the window.
                frame.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(RADIUS))
                        .border_1()
                        .border_color(palette.foreground)
                        .bg(palette.raised.opacity(0.92))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.5))
                        .text_color(palette.foreground)
                        .child(if matches!(self.view, View::Chat) {
                            tr("Drop images or files to attach them")
                        } else {
                            tr("Drop files to attach them")
                        }),
                )
            });
        // Chat only, above the box: whether Nexus can pay the next call. A Code session runs on its agent's
        // own plan.
        let pay = if code {
            None
        } else {
            self.render_pay_notice(palette, cx)
        };
        // The 8px under the notice is its own, so it opens and closes with it.
        let pay = pay.map(|pay| div().pb(px(8.)).child(pay).into_any_element());
        self.move_launch_composer(
            v_flex()
                .w_full()
                .child(motion::reveal("composer-pay", pay.is_some(), pay))
                .child(composer),
            cx,
        )
    }

    /// What the composer's session is doing now (`activity_of`): the open Code session's state and its pending
    /// approval, or the open chat's answer.
    pub(super) fn composer_activity(&self) -> Option<Activity> {
        match &self.view {
            View::Code(view) => activity_of(
                code::pending_approval(&view.blocks).is_some(),
                self.code_session().map(|session| session.status),
                Start::of(view.pending.as_ref()),
                false,
                view.working_since,
            ),
            View::Chat => activity_of(false, None, Start::None, self.asking.is_some(), None),
            _ => None,
        }
    }

    /// Whether the composer's activity marks (the pill's dot, Stop's arc) hold still: while the person asked for less
    /// motion, and on a wait, where nothing works.
    fn activity_still(&self, activity: Activity, cx: &App) -> bool {
        cx.reduce_motion() || activity.kind == ActivityKind::Waiting
    }

    /// The activity status (`Activity`), before dictation and Stop: quiet text as the toolbar's chips, no fill. The
    /// dot in the state's color, the words, and the turn's clock in tabular figures, so it does not jitter as it
    /// counts. The dot breathes as the brand's calm working mark (`motion::BREATHE`), not a blink. It holds still while
    /// the person asked for less motion, or on a wait.
    /// A narrow composer keeps the dot and the clock; the words move to the tooltip. Its slot opens and closes with
    /// the turn (`render_composer`).
    fn render_activity(&self, activity: Activity, palette: Palette, cx: &Context<Self>) -> AnyElement {
        let tone = activity.kind.tone(palette);
        let still = self.activity_still(activity, cx);
        let dot = div().size(px(6.)).flex_none().rounded_full().bg(tone);
        let dot = if still {
            dot.into_any_element()
        } else {
            dot.with_animation(
                "composer-activity-dot",
                Animation::new(motion::BREATHE)
                    .repeat()
                    .with_max_fps(crate::motion::LOOP_FPS)
                    .with_easing(pulsating_between(0.62, 1.0)),
                |dot, delta| dot.opacity(delta),
            )
            .into_any_element()
        };
        let narrow = self.composer_layout.control_icons;
        // The label stays English in `Activity` (it is `Copy`, and tests read it); it is translated here, where shown.
        let label = tr(activity.label);
        let tip = label.clone();
        let status = h_flex()
            .id("composer-activity")
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .h(px(28.))
            .px(px(6.))
            .gap(px(7.))
            .flex_none()
            .text_size(px(13.))
            .text_color(palette.muted)
            .child(dot)
            .when(!narrow, |pill| pill.child(label))
            .when_some(activity.since, |pill, since| {
                pill.child(
                    div()
                        .font_features(FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])))
                        .text_color(palette.faint)
                        .child(super::code_view::elapsed(since.elapsed())),
                )
            });
        status.into_any_element()
    }

    /// Whether a composer is on screen to take files: a chat or a Code session, the Workbench not over it.
    pub(super) fn composer_shown(&self) -> bool {
        self.workspace_open()
            && matches!(self.view, View::Chat | View::Code(_) | View::NewCode)
            && !(self.side.open && self.side.maximized)
    }

    /// Ask for files to attach (`.composer-attach`, the file picker).
    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = this.update(cx, |this, cx| this.attach_paths(paths, cx));
            }
        })
        .detach();
    }

    /// Files dropped on the window or picked: they attach to the composer on screen.
    pub(super) fn attach_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let files = paths.iter().map(|path| attachments::from_path(path)).collect();
        self.attach(files, cx);
    }

    /// Read and prepare `files` off the UI thread, then add what fits to the draft and say what did not, as the
    /// Nexus app does (`attachments::take`, `prepare`, `merge`).
    pub(super) fn attach(&mut self, files: Vec<attachments::Incoming>, cx: &mut Context<Self>) {
        if !self.composer_shown() || files.is_empty() {
            return;
        }
        let target = if matches!(self.view, View::Chat) {
            Target::Chat
        } else {
            Target::Code
        };
        let (taken, after, folders) = attachments::take(target, files);
        // The draft they were dropped on, even if another session is open when they are read.
        let owner = self.draft_owner(cx);
        self.attaching += 1;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut prepared = cx
                .background_executor()
                .spawn(async move { attachments::prepare(taken) })
                .await;
            prepared.errors.extend(folders);
            let _ = this.update(cx, |this, cx| {
                this.attaching -= 1;
                let problem = this.merge_attachments(owner, prepared, cx).or(after);
                this.composer_note = problem.map(Into::into);
                cx.notify();
            });
        })
        .detach();
    }

    /// Show the suggestion as the composer's placeholder, or the usual one without it.
    pub(super) fn sync_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder: SharedString = self.suggestion.clone().unwrap_or_else(|| {
            if matches!(self.view, View::NewCode) {
                PLACEHOLDER.into()
            } else if matches!(self.view, View::Code(_)) {
                if self.code_running() {
                    tr("Enter sends into this turn · {keys} queues for after it")
                        .replace("{keys}", queue_keys())
                        .into()
                } else {
                    tr("Reply to the agent")
                }
            } else if self.asking.is_some() {
                tr("Queue a follow-up while this answers")
            } else if self.current.is_some() {
                tr("Reply")
            } else if self.panel {
                tr("Ask Nexus")
            } else {
                PLACEHOLDER.into()
            }
        });
        if self.placeholder != placeholder {
            self.placeholder = placeholder.clone();
            self.composer
                .update(cx, |state, cx| state.set_placeholder(placeholder, window, cx));
        }
    }

    /// Attach (`.composer-attach`): a 28px ring with a plus, opening a small menu of the ways in this app to add to
    /// the prompt. Nexus UI's "+" opens the file picker straight away; the menu keeps that one click away and names
    /// the clipboard too, which otherwise only Ctrl+V knew about.
    fn render_attach(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.overlay.attach_menu();
        let button = attach_ring("attach", open, palette)
            .debug_selector(|| "composer-attach".into())
            .when(!open, |button| {
                button.tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tr("Add files and more")).build(window, cx)
                })
            })
            // This press only opens the menu. While it is open, the window's menu backdrop covers this button, so a
            // second press closes the menu there, and `motion::presence` plays its exit.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_overlay(Overlay::AttachMenu);
                    this.overlay.close_model_menu();
                    cx.notify();
                }),
            );
        div()
            .relative()
            .child(button)
            .when_some(motion::presence("attach-menu", open, cx), |frame, phase| {
                let menu = phase.menu("attach-menu", 4., self.render_attach_menu(palette, cx));
                frame.child(super::code_dock::menu_layer(menu, Anchor::BottomLeft))
            })
    }

    /// The "+" menu: the app's menu box (`.wb-pop`) with the chat menu's icon rows.
    fn render_attach_menu(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |id: &'static str, icon: Lucide, label: SharedString, keys: Option<&'static str>| {
            h_flex()
                .id(id)
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| style.bg(palette.menu_hover))
                .child(super::glyph(icon, palette.icon).size(px(15.)))
                .child(div().flex_1().child(label))
                .when_some(keys, |row, keys| {
                    row.child(div().text_size(px(12.)).text_color(palette.faint).child(keys))
                })
        };
        let paste_keys = if cfg!(target_os = "macos") { "⌘V" } else { "Ctrl+V" };
        super::code_dock::menu_box("attach-menu", 232., palette)
            .text_size(px(13.))
            .child(
                item("attach-files", Lucide::Paperclip, tr("Attach images or files"), None).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.overlay.close_attach_menu();
                        this.pick_files(cx);
                    },
                )),
            )
            .child(
                item("attach-artifacts", Lucide::Layers, tr("Add artifacts…"), None).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.overlay.close_attach_menu();
                        this.open_artifact_picker(window, cx);
                    },
                )),
            )
            .child(
                item(
                    "attach-paste",
                    Lucide::Clipboard,
                    tr("Paste from clipboard"),
                    Some(paste_keys),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.overlay.close_attach_menu();
                    this.attach_clipboard(cx);
                })),
            )
    }

    /// The clipboard's images and files, as a paste in the prompt would take them.
    fn attach_clipboard(&mut self, cx: &mut Context<Self>) {
        let files = cx
            .read_from_clipboard()
            .map(|item| attachments::pasted(&item).files)
            .unwrap_or_default();
        if files.is_empty() {
            self.composer_note = Some(tr("The clipboard has no images or files to attach"));
            cx.notify();
        } else {
            self.attach(files, cx);
        }
    }

    /// The model, as plain text (`.wb-model-chip`): a sparkle for Auto, else the maker's mark, the name, and a chevron
    /// that turns over while the menu is open.
    fn render_model_chip(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.overlay.model_menu();
        let (label, maker): (SharedString, Option<String>) = match &self.model {
            Some(model) => (model.name.clone().into(), Some(model.maker.clone())),
            None => (tr("Auto"), None),
        };
        let mark = match maker.as_deref() {
            Some(maker) => super::model_menu::maker_mark(maker, 14., palette),
            None => Icon::new(Lucide::Sparkles)
                .size(px(14.))
                .text_color(palette.icon)
                .into_any_element(),
        };
        let chip = h_flex()
            .id("model-chip")
            .role(Role::Button)
            .aria_label(label.clone())
            .tooltip({
                let label = label.clone();
                move |window, cx| Tooltip::new(label.clone()).build(window, cx)
            })
            .h(px(26.))
            .pl(px(4.))
            .pr(px(2.))
            .gap(px(5.))
            .rounded(px(6.))
            .text_size(px(13.))
            .text_color(if open { palette.foreground } else { palette.muted })
            .cursor_pointer()
            .hover(|style| style.text_color(palette.foreground))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_model_menu(window, cx)))
            .child(mark)
            .min_w_0()
            .flex_shrink(1.)
            .child(
                div()
                    .min_w_0()
                    .max_w(px(if self.composer_layout.control_icons { 96. } else { 160. }))
                    .truncate()
                    .child(label),
            )
            .when(!self.composer_layout.chat_icons, |chip| {
                chip.child(
                    Icon::new(Lucide::ChevronDown)
                        .size(px(13.))
                        .text_color(palette.faint)
                        .with_tween(
                            "model-chip-chevron",
                            if open { 1. } else { 0. },
                            motion::Tween::new(motion::COLLAPSE, motion::ease_out),
                            |icon, turn| icon.rotate(radians(std::f32::consts::PI * turn)),
                        ),
                )
            });
        // The menu opens from the chip, its bottom-left 8px over the chip's top: over the prompt box, as the Nexus
        // app's, and above everything else.
        div().min_w_0().flex_shrink(1.).relative().child(chip).when_some(
            motion::presence("model-menu", open, cx),
            |frame, phase| {
                let menu = phase.menu("model-menu", 4., self.render_model_menu(palette, cx));
                frame.child(
                    div().absolute().top_0().left_0().child(
                        deferred(
                            anchored()
                                .anchor(Anchor::BottomLeft)
                                .offset(point(px(0.), px(-8.)))
                                .snap_to_window_with_margin(px(12.))
                                .child(menu),
                        )
                        .with_priority(1),
                    ),
                )
            },
        )
    }

    pub(super) fn composer_send_enabled(&self) -> bool {
        (!self.composer_empty
            || !self.draft.is_empty()
            || !self.pending_diff_comments().is_empty()
            || !self.pending_replies().is_empty())
            && self.attaching == 0
            && !self.start_blocked()
            && self.budget_line().is_none()
    }

    /// Send (`.composer-send`): a 28px filled circle with an arrow, faded while there is nothing to send.
    fn render_send(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.composer_send_enabled();
        let title: SharedString = self
            .budget_line()
            .unwrap_or_else(|| {
                if self.code_running() {
                    tr("Send now: the agent reads it at its next step (Enter)").to_string()
                } else {
                    tr("Send").to_string()
                }
            })
            .into();
        send_circle("send", enabled, palette)
            .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
    }

    /// A running Code session's Queue: a hairline 28px pill with the list glyph, beside Send. It holds the message
    /// until the turn ends, where it waits as a chip that can be rewritten, sent now or taken back.
    fn render_code_queue(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = tr("Queue: sent when this turn ends ({keys})")
            .replace("{keys}", queue_keys())
            .into();
        queue_pill("code-queue", palette)
            .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.submit_now(true, window, cx)))
    }

    /// Queue (`.wb-composer-queue`): a filled 28px pill with the list glyph, faded while there is nothing to queue.
    fn render_queue(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled =
            (!self.composer_empty || !self.draft.is_empty()) && self.attaching == 0 && self.budget_line().is_none();
        let now = if cfg!(target_os = "macos") {
            "⌘↩"
        } else {
            "Ctrl+Enter"
        };
        let title: SharedString = self
            .budget_line()
            .unwrap_or_else(|| tr("Queue: sent when this answer ends (Enter). Send now: {keys}").replace("{keys}", now))
            .into();
        h_flex()
            .id("queue")
            .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
            .h(px(28.))
            .ml(px(4.))
            .pl(px(10.))
            .pr(px(12.))
            .gap(px(6.))
            .flex_none()
            .rounded_full()
            .bg(palette.primary)
            .text_color(palette.primary_foreground)
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .when(!enabled, |button| button.opacity(0.4))
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.88))
                    .active(|style| style.opacity(0.72))
            })
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
            .child(
                Icon::new(Lucide::ListOrdered)
                    .size(px(14.))
                    .text_color(palette.primary_foreground),
            )
            .child(tr("Queue"))
    }

    /// Stop (`.wb-composer-stop`): a 28px hairline ring around a filled 9px square (`.wb-stop-square`). In a Code
    /// session it interrupts the turn, and says what that does to the agent's sub-agents (`interrupt.ts`). While the
    /// turn runs, a 1.5px arc in the activity's color turns on the ring itself (`assets::STOP_ARC`), so the one button
    /// that ends the work also shows it is under way. It holds still where the pill's dot does: less motion, a wait.
    fn render_stop(&self, activity: Option<Activity>, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let title: SharedString = self
            .code_session()
            .map(|session| super::code_dock::interrupt_title(session.agent).into())
            .unwrap_or_else(|| tr("Stop (Esc)"));
        // The arc wears the activity's color and holds still when the pill's dot does (`activity_still`).
        let (tone, still) = match activity {
            Some(activity) => (activity.kind.tone(palette), self.activity_still(activity, cx)),
            None => (palette.working(), cx.reduce_motion()),
        };
        stop_ring("stop", tone, still, palette)
            .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.stop(window, cx)))
            .into_any_element()
    }
}

/// A row over the composer's text (a note, the queue, the attachments): it opens and closes in height
/// (`motion::reveal`). Its content is gone as it closes, so it closes as space.
fn section(id: &'static str, content: Option<AnyElement>) -> impl IntoElement {
    motion::reveal(id, content.is_some(), content)
}

/// A button that comes and goes in a composer's toolbar (Stop, Queue, Send): it opens and closes in width
/// (`motion::reveal_width`), so Stop slides in where Send slides out. While it leaves it takes no clicks.
pub(super) fn toolbar_slot(id: &'static str, shown: bool, button: impl IntoElement) -> impl IntoElement {
    let button = if shown {
        button.into_any_element()
    } else {
        div()
            .relative()
            .child(button)
            .child(div().id("leaving").absolute().inset_0().occlude())
            .into_any_element()
    };
    motion::reveal_width(id, shown, Some(button))
}

/// Attachment chips (`.wb-attachment-tray`, brand v5): 26px, radius 6, the strong fill; an image's thumbnail or a file
/// glyph, the name, the size, and a remove button when `on_remove` is given (the composer's, not a sent question's).
/// While the draft's files go up in pieces (`Draft::upload`), such a file's chip shows how much has gone, as a percent
/// and a 2px bar along its foot (`.wb-attachment-percent`, `.wb-attachment-progress`), and its ✕ calls `on_cancel`.
/// A click on an image's chip calls `on_open` with its place among the draft's images, to show it full size
/// (`image_viewer.rs`).
pub(super) fn attachment_chips(
    id: impl Into<SharedString>,
    draft: &Draft,
    palette: Palette,
    on_remove: Option<Rc<dyn Fn(u64, &mut Window, &mut App)>>,
    on_cancel: Option<Rc<dyn Fn(u64, &mut Window, &mut App)>>,
    on_open: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
) -> Div {
    artifact_chips(id, draft, palette, on_remove, on_cancel, on_open, None)
}

/// [`attachment_chips`], and a referenced artifact's chip (`artifact_refs.rs`): its kind's icon, its title and the
/// version it was added at, with a hairline ring that sets it apart from a file. A click calls `on_artifact`, to open
/// it beside the chat. In the composer (`on_remove` given) a new chip slides 8 px in from the right as it fades in
/// (brand v5's attaching motion, 200 ms); a sent message's chips load still.
pub(super) fn artifact_chips(
    id: impl Into<SharedString>,
    draft: &Draft,
    palette: Palette,
    on_remove: Option<Rc<dyn Fn(u64, &mut Window, &mut App)>>,
    on_cancel: Option<Rc<dyn Fn(u64, &mut Window, &mut App)>>,
    on_open: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
    on_artifact: Option<Rc<dyn Fn(ArtifactRef, &mut Window, &mut App)>>,
) -> Div {
    let id: SharedString = id.into();
    let images = draft.images.len();
    let arriving = on_remove.is_some();
    h_flex()
        .flex_wrap()
        .gap(px(6.))
        .children(draft.all().enumerate().map(|(index, attachment)| {
            if let Some(reference) = attachment.artifact.clone() {
                let chip = artifact_chip(
                    &id,
                    attachment.id,
                    &reference,
                    palette,
                    on_remove.clone(),
                    on_artifact.clone(),
                );
                return if arriving {
                    chip.with_animation(
                        ElementId::NamedInteger("artifact-arrive".into(), attachment.id),
                        Animation::new(motion::UI_LONG).with_easing(motion::ease_out),
                        |chip, t| chip.opacity(t).left(px(8. * (1. - t))),
                    )
                    .into_any_element()
                } else {
                    chip.into_any_element()
                };
            }
            let glyph = match &attachment.thumbnail {
                Some(thumbnail) => img(thumbnail.clone())
                    .size(px(16.))
                    .rounded(px(3.))
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                None => img(FileIcon::file(&attachment.name).asset(palette.dark))
                    .flex_none()
                    .size(px(14.))
                    .into_any_element(),
            };
            let attachment_id = attachment.id;
            let open = on_open.clone().filter(|_| index < images);
            let share = draft.upload.as_ref().and_then(|run| run.share(attachment_id));
            // A file going up in pieces cancels its send; any other chip removes itself, or has no button while a send runs.
            let action = match share {
                Some(_) => on_cancel
                    .clone()
                    .map(|on_cancel| ("cancel-attachment", tr("Cancel"), on_cancel)),
                None => on_remove
                    .clone()
                    .map(|on_remove| ("remove-attachment", tr("Remove"), on_remove)),
            };
            h_flex()
                .id(ElementId::NamedInteger(id.clone(), attachment_id))
                .relative()
                .h(px(26.))
                .max_w(px(260.))
                .pl(px(8.))
                .pr(px(if action.is_some() { 4. } else { 8. }))
                .gap(px(6.))
                .rounded(px(6.))
                .bg(palette.selected)
                .text_size(px(12.))
                .child(
                    h_flex()
                        .id(ElementId::NamedInteger("open-attachment".into(), attachment_id))
                        .h_full()
                        .min_w(px(0.))
                        .gap(px(6.))
                        .child(glyph)
                        .child(div().min_w(px(0.)).truncate().child(attachment.name.clone()))
                        .child(
                            div()
                                .flex_none()
                                .text_color(palette.muted)
                                .child(attachments::size_label(attachment.size())),
                        )
                        .when_some(open, |name, open| {
                            name.cursor_pointer()
                                .tooltip(|window, cx| Tooltip::new(tr("Open full size")).build(window, cx))
                                .on_click(move |_, window, cx| open(index, window, cx))
                        }),
                )
                .when_some(share, |chip, share| {
                    chip.child(
                        div()
                            .flex_none()
                            .text_color(palette.muted)
                            .child(format!("{}%", attachments::percent(share))),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(6.))
                            .right(px(6.))
                            .bottom_0()
                            .h(px(2.))
                            .rounded(px(1.))
                            .bg(palette.hover)
                            .overflow_hidden()
                            .child(div().h_full().w(relative(share)).bg(palette.muted)),
                    )
                })
                .when_some(action, |chip, (button, label, action)| {
                    chip.child(
                        div()
                            .id(ElementId::NamedInteger(button.into(), attachment_id))
                            .size(px(18.))
                            .flex_none()
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(palette.muted)
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
                            .on_click(move |_, window, cx| action(attachment_id, window, cx))
                            .child(Icon::new(Lucide::X).size(px(12.))),
                    )
                })
                .into_any_element()
        }))
}

/// One referenced artifact's chip ([`artifact_chips`]).
fn artifact_chip(
    id: &SharedString,
    attachment_id: u64,
    reference: &Arc<ArtifactRef>,
    palette: Palette,
    on_remove: Option<Rc<dyn Fn(u64, &mut Window, &mut App)>>,
    on_artifact: Option<Rc<dyn Fn(ArtifactRef, &mut Window, &mut App)>>,
) -> Stateful<Div> {
    let tip: SharedString = format!("{} · {}", reference.kind, reference.version_label()).into();
    h_flex()
        .id(ElementId::NamedInteger(format!("{id}-artifact").into(), attachment_id))
        .debug_selector(|| "artifact-chip".into())
        .relative()
        .h(px(26.))
        .max_w(px(280.))
        .pl(px(8.))
        .pr(px(if on_remove.is_some() { 4. } else { 8. }))
        .gap(px(6.))
        .rounded(px(6.))
        .bg(palette.selected)
        .border_1()
        .border_color(palette.border)
        .text_size(px(12.))
        .child(
            h_flex()
                .id(ElementId::NamedInteger("open-artifact".into(), attachment_id))
                .h_full()
                .min_w(px(0.))
                .gap(px(6.))
                .child(
                    Icon::new(super::artifact_picker::kind_icon(reference.kind()))
                        .size(px(14.))
                        .text_color(palette.foreground),
                )
                .child(div().min_w(px(0.)).truncate().child(reference.title.clone()))
                .child(
                    div()
                        .flex_none()
                        .text_color(palette.muted)
                        .child(reference.version_label()),
                )
                .when_some(on_artifact, |name, open| {
                    let reference = reference.clone();
                    name.cursor_pointer()
                        .tooltip(move |window, cx| {
                            Tooltip::new(tr("Open · {name}").replace("{name}", &tip)).build(window, cx)
                        })
                        .on_click(move |_, window, cx| open((*reference).clone(), window, cx))
                }),
        )
        .when_some(on_remove, |chip, remove| {
            chip.child(
                div()
                    .id(ElementId::NamedInteger("remove-artifact".into(), attachment_id))
                    .size(px(18.))
                    .flex_none()
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                    .tooltip(|window, cx| Tooltip::new(tr("Remove")).build(window, cx))
                    .on_click(move |_, window, cx| remove(attachment_id, window, cx))
                    .child(Icon::new(Lucide::X).size(px(12.))),
            )
        })
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude has a `test` attribute of its own.
    use gpui_kit::{
        Context, FocusHandle, InteractiveElement, IntoElement, Modifiers, MouseButton, ParentElement, Render,
        StatefulInteractiveElement, Styled, TestAppContext, Window, div, px,
    };

    use super::composer_frame;
    use crate::palette::{Palette, ThemeChoice};

    /// A composer in small: a focusable text area and a button in the frame, and the workspace's focus counter.
    struct Probe {
        text: FocusHandle,
        focused: bool,
        focus_changes: usize,
        clicks: usize,
    }

    impl Render for Probe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // As `Workspace::render` counts the composer's focus changes.
            let focused = self.text.is_focused(window);
            if focused != self.focused {
                self.focused = focused;
                self.focus_changes += 1;
            }
            let palette = Palette::for_choice(ThemeChoice::Dark, gpui_kit::WindowAppearance::Dark);
            let box_ = div()
                .flex()
                .child(div().id("text").track_focus(&self.text).size(px(100.)))
                .child(
                    div()
                        .id("send")
                        .debug_selector(|| "send".into())
                        .size(px(30.))
                        // The text area loses the focus on the mouse down, as on a click on the send button.
                        .on_mouse_down(MouseButton::Left, |_, window, cx| window.blur(cx))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.clicks += 1;
                            cx.notify();
                        })),
                );
            composer_frame(box_, self.focused, palette)
        }
    }

    /// One click runs a composer button while the text area has the focus.
    #[gpui_kit::test]
    fn one_click_runs_a_button_while_the_text_has_the_focus(cx: &mut TestAppContext) {
        let (probe, cx) = cx.add_window_view(|_, cx| Probe {
            text: cx.focus_handle(),
            focused: false,
            focus_changes: 0,
            clicks: 0,
        });
        cx.update(|window, cx| {
            let text = probe.read(cx).text.clone();
            window.focus(&text, cx);
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert_eq!(probe.read_with(cx, |probe, _| probe.focus_changes), 1);
        let send = cx.debug_bounds("send").expect("the send button is drawn").center();
        cx.simulate_mouse_down(send, MouseButton::Left, Modifiers::none());
        // A frame between mouse down and mouse up, as on screen: it sees the focus change.
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert_eq!(probe.read_with(cx, |probe, _| probe.focus_changes), 2);
        cx.simulate_mouse_up(send, MouseButton::Left, Modifiers::none());
        assert_eq!(probe.read_with(cx, |probe, _| probe.clicks), 1);
    }
}

#[cfg(test)]
mod activity_tests {
    use std::time::Instant;

    use super::{ActivityKind, Start, activity_of};
    use crate::code::Status;

    #[test]
    fn a_wait_on_the_person_outranks_work() {
        let since = Some(Instant::now());
        let approval = activity_of(true, Some(Status::Running), Start::None, false, since).unwrap();
        assert_eq!(approval.kind, ActivityKind::Waiting);
        assert_eq!(approval.since, None);
        let needs = activity_of(false, Some(Status::NeedsYou), Start::Underway, false, since).unwrap();
        assert_eq!(needs.kind, ActivityKind::Waiting);
    }

    #[test]
    fn work_shows_its_clock_only_for_a_code_turn() {
        let since = Some(Instant::now());
        let code = activity_of(false, Some(Status::Running), Start::None, false, since).unwrap();
        assert_eq!(
            (code.kind, code.label, code.since),
            (ActivityKind::Working, "Working", since)
        );
        let chat = activity_of(false, None, Start::None, true, None).unwrap();
        assert_eq!(
            (chat.kind, chat.label, chat.since),
            (ActivityKind::Working, "Answering", None)
        );
        let starting = activity_of(false, Some(Status::Idle), Start::Underway, false, since).unwrap();
        assert_eq!((starting.label, starting.since), ("Starting", None));
    }

    #[test]
    fn a_session_at_rest_or_failed_shows_nothing() {
        assert_eq!(activity_of(false, Some(Status::Idle), Start::None, false, None), None);
        assert_eq!(activity_of(false, Some(Status::Failed), Start::None, false, None), None);
        assert_eq!(activity_of(false, None, Start::None, false, None), None);
    }

    #[test]
    fn a_failed_start_shows_nothing() {
        // The view says why the start failed, with Try again: the composer does not go on saying "Starting".
        assert_eq!(activity_of(false, None, Start::Failed, false, None), None);
        assert_eq!(
            activity_of(false, Some(Status::Failed), Start::Failed, false, None),
            None
        );
    }

    #[test]
    fn an_unanswered_card_on_a_session_at_rest_is_not_a_wait() {
        // A stream that ended without the approval's answer leaves the card open: the session no longer waits.
        assert_eq!(activity_of(true, Some(Status::Idle), Start::None, false, None), None);
        assert_eq!(activity_of(true, Some(Status::Failed), Start::None, false, None), None);
        assert_eq!(activity_of(true, None, Start::None, false, None), None);
        let live = activity_of(true, Some(Status::NeedsYou), Start::None, false, None).unwrap();
        assert_eq!(live.kind, ActivityKind::Waiting);
    }

    /// The pill shows `tr(activity.label)`, which the scan for literals passed to `tr` cannot see: every label is checked here.
    #[test]
    fn every_activity_label_has_both_translations() {
        let since = Some(Instant::now());
        let activities = [
            activity_of(true, Some(Status::Running), Start::None, false, since),
            activity_of(false, None, Start::Underway, false, None),
            activity_of(false, Some(Status::Running), Start::None, false, since),
            activity_of(false, None, Start::None, true, None),
        ];
        let labels: Vec<&str> = activities
            .iter()
            .map(|activity| activity.expect("an activity").label)
            .collect();
        assert_eq!(labels, ["Waiting for you", "Starting", "Working", "Answering"]);
        for label in labels {
            for code in ["es", "pt-BR"] {
                assert!(
                    crate::i18n::_rust_i18n_try_translate(code, label).is_some(),
                    "{code}: {label}"
                );
            }
        }
    }
}

/// The keys that queue a Code session's message behind its running turn (and send a chat's next message now).
fn queue_keys() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘↩"
    } else {
        "Ctrl+Enter"
    }
}

#[cfg(test)]
mod note_tests {
    use gpui_kit::prelude::*;
    use gpui_kit::{
        Context, Div, IntoElement, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext, Window,
        WindowAppearance, div, point, px,
    };

    use super::composer_note_line;
    use crate::palette::{Palette, ThemeChoice};

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(ThemeChoice::Light, WindowAppearance::Light);
            div().w(px(400.)).child(composer_note_line(
                "Budget used up. Ask an owner to raise it.".into(),
                None::<Div>,
                palette,
            ))
        }
    }

    /// A person copies the note's text into a report: a drag over it selects it.
    #[gpui_kit::test]
    fn the_composer_note_can_be_selected(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.add_window(|window, cx| {
            let probe = cx.new(|_| Probe);
            gpui_kit::base::Root::new(probe, window, cx)
        });
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_down(point(px(17.), px(18.)), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(point(px(380.), px(18.)), Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(point(px(380.), px(18.)), MouseButton::Left, Modifiers::none());
        let selected = cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            gpui_kit::base::TextSelection::selected_text(window, cx)
        });
        assert!(selected.starts_with("Budget"), "{selected:?}");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod context_paste_tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::component::input::TextareaState;

    struct Probe(Entity<TextareaState>);

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            composer_input(Textarea::new(&self.0).appearance(false)).w(px(400.))
        }
    }

    #[gpui_kit::test]
    fn right_click_paste_works_below_the_first_line_of_an_empty_composer(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.add_window(|window, cx| {
            let probe = cx.new(|cx| Probe(cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6))));
            gpui_kit::base::Root::new(probe, window, cx)
        });
        let input = window
            .update(cx, |root, _, cx| {
                root.view().clone().downcast::<Probe>().unwrap().read(cx).0.clone()
            })
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear(cx);
        });
        cx.write_to_clipboard(ClipboardItem::new_string("paste fixture".into()));
        let writing_area = cx.debug_bounds("composer-input").expect("the writing area is drawn");
        let press = point(writing_area.left() + px(35.), writing_area.bottom() - px(12.));
        cx.simulate_mouse_down(press, MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_up(press, MouseButton::Right, Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // Paste is the third row in the Linux context menu, after Cut and Copy.
        let paste = press + point(px(32.), px(73.));
        cx.simulate_click(paste, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |input, _| input.value()), "paste fixture");
    }
}
