//! The Assistant thread's cards and first open (brand v5 A1, A6·e, A9, A19), drawn from what
//! `nexus_desktop_core::agent_cards` reads off the agent API's answers. `pages/agent.rs` places them in the thread.
//!
//! - The first open (A19): under the header's mark, Nexus and scope, one warm line, what the agent can read, up to
//!   four starters and two quiet links.
//! - The Connect card (A6·e): under an answer that needed an account. Connect opens the connector's card in Nexus UI.
//! - The approval card (A9): a write that waits. Approve and Decline reach the agent API with the person's own token
//!   (`decisions.rs`); a write that always asks is approved on its page in Nexus alone, so it has "Review in Nexus".

use std::collections::HashMap;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputState, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::decisions::{Decisions, Phase};
use super::{AgentPage, body, card, card_title, meta, open_link};
use crate::pages::ui;
use crate::palette::Palette;
use nexus_desktop_core::agent as rules;
use nexus_desktop_core::agent_cards::{
    self as cards, Choices, Connect, Decision, EditField, EditKind, Intro, TextParts, WriteAsk,
};

impl AgentPage {
    /// The first open (A19), under the header's mark, Nexus and scope: one warm line, what the agent can read, up to
    /// four starters that each send their line, and two quiet links. Never a wizard.
    pub(super) fn first_open_view(&self, intro: &Intro, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.pane.busy();
        let headline = cards::intro_headline(intro, self.person.first_name.as_deref(), &self.name);
        let reads = cards::intro_reads(intro, Some(&self.scope), self.person.personal, &self.name);
        let starters: Vec<_> = intro
            .starters
            .iter()
            .enumerate()
            .map(|(ix, line)| {
                let text = line.clone();
                div()
                    .id(("agent-starter", ix))
                    .px(px(14.))
                    .py(px(7.))
                    .rounded(px(18.))
                    .border_1()
                    .border_color(palette.border)
                    .bg(palette.raised)
                    .text_size(px(13.5))
                    .text_color(palette.foreground)
                    .when(!busy, |chip| {
                        chip.cursor_pointer()
                            .hover(|style| style.bg(palette.hover))
                            .on_click(cx.listener(move |this, _, window, cx| this.start_with(&text, window, cx)))
                    })
                    .child(line.clone())
            })
            .collect();
        let quiet = |id: &'static str| {
            div()
                .id(id)
                .text_size(px(12.5))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.text_color(palette.strong).underline())
        };
        let mut links = h_flex().gap(px(16.)).justify_center();
        if intro.accounts {
            links = links.child(
                quiet("agent-intro-accounts")
                    .on_click(cx.listener(|this, _, window, cx| {
                        (this.ctx.navigate_settings)("/settings/connectors".into(), window, cx)
                    }))
                    .child(INTRO_ACCOUNTS),
            );
        }
        if intro.discover {
            links = links.child(
                quiet("agent-intro-discover")
                    .on_click(cx.listener(|this, _, window, cx| this.start_with(cards::DISCOVER_LINE, window, cx)))
                    .child(INTRO_DISCOVER),
            );
        }
        v_flex()
            .w_full()
            .items_center()
            .gap(px(14.))
            .pb(px(8.))
            .child(
                div()
                    .max_w(px(460.))
                    .text_center()
                    .text_size(px(16.))
                    .line_height(relative(1.4))
                    .text_color(palette.strong)
                    .child(headline),
            )
            .children(reads.map(|reads| ui::note(reads, palette)))
            .child(
                h_flex()
                    .max_w(px(520.))
                    .flex_wrap()
                    .justify_center()
                    .gap(px(8.))
                    .children(starters),
            )
            .when(intro.accounts || intro.discover, |view| view.child(links))
    }
}

/// The first open's quiet links (A19): Accounts (A6) and Discover (A15).
pub const INTRO_ACCOUNTS: &str = "Connect your accounts";
pub const INTRO_DISCOVER: &str = "See what I can do";

/// The Connect card (A6·e): the maker's mark, the service's name, one line of what Nexus would do with it, and
/// Connect, which opens the connector's card in Nexus UI in the browser. A card, since it carries an action (A1·D).
/// `assistant`: the Assistant's name, for the line when the answer gives none.
pub(super) fn connect_card(id: usize, connect: Connect, assistant: &str, palette: Palette) -> impl IntoElement {
    let mark = match crate::assets::connector_named(&connect.connector)
        .or_else(|| crate::assets::connector_named(&connect.label))
    {
        Some((path, color)) => ui::mark_tile(path, color, palette),
        None => ui::letter_tile(&connect.label, palette),
    };
    let line = cards::connect_line(&connect, assistant);
    let url = connect.url.clone();
    let label: SharedString = format!("Connect your {} account", connect.label).into();
    h_flex()
        .w_full()
        .max_w(px(440.))
        .gap(px(12.))
        .px(px(14.))
        .py(px(10.))
        .rounded(px(12.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
        .child(mark)
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.foreground)
                        .child(connect.label.clone()),
                )
                .child(div().text_size(px(12.5)).text_color(palette.muted).child(line)),
        )
        .child(
            ui::primary_button(("agent-connect", id), "Connect", palette)
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx))
                .on_click(move |_, _, cx| open_link(&url, cx)),
        )
}

/// What the approval card (A9) shows for a write that waits.
#[derive(Clone, Debug, PartialEq)]
pub struct Approval {
    /// The card's id, which a decision names.
    pub card: String,
    /// What the person may decide here.
    pub choices: Choices,
    /// What, a verb first: the write's summary.
    pub summary: String,
    pub irreversible: bool,
    /// The A9 card's To, amount, Why and exact text; None for a write from before it.
    pub ask: Option<WriteAsk>,
    /// The exact text in its parts (A7·a): an email's subject, the body, and the signature or "Sent using Nexus".
    pub text: Option<TextParts>,
    pub minutes_left: Option<i64>,
    /// The approval page in Nexus, http(s) only.
    pub review: Option<String>,
    /// The API's words when there is no page to review it on.
    pub detail: String,
    /// The fields Edit opens (A24·b); none when the card offers no Edit.
    pub edit: Vec<EditField>,
    /// The text those fields start with.
    pub start: Vec<(String, String)>,
    /// The person edited the write it replaces: the card says "Edited by you".
    pub edited: bool,
}

impl Approval {
    /// The card's lines under the title, as label and value: To, Amount and Why. While Edit is open, To is a field.
    #[cfg(test)]
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        self.rows_for(false)
    }

    pub fn rows_for(&self, editing: bool) -> Vec<(&'static str, String)> {
        let Some(ask) = &self.ask else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        if !ask.to.is_empty() && !editing {
            rows.push(("To", cards::write_to_text(ask)));
        }
        if let Some(amount) = ask.amount_usd.as_deref().and_then(cards::write_amount_text) {
            rows.push(("Amount", amount));
        }
        if let Some(said) = &ask.why_said {
            let at = ask.why_at.as_deref().and_then(clock);
            rows.push(("Why", cards::write_why_text(said, at.as_deref())));
        }
        rows
    }

    /// The line at the card's foot: whether it can be undone, and that nothing changes before the person approves.
    pub fn undo_line(&self) -> &'static str {
        if self.irreversible {
            "It cannot be undone. Nothing changes until you approve it."
        } else {
            "It can be undone. Nothing changes until you approve it."
        }
    }
}

/// The minutes until an ISO 8601 time, rounded up as the kit's `writeExpiryText`; None when it is not a time.
pub fn minutes_left(at: &str, now: chrono::DateTime<chrono::Utc>) -> Option<i64> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    let seconds = (at.with_timezone(&chrono::Utc) - now).num_seconds();
    Some(seconds.div_euclid(60) + i64::from(seconds.rem_euclid(60) > 0))
}

/// An ISO 8601 time on this computer's clock: `08:24`.
fn clock(at: &str) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    Some(at.with_timezone(&chrono::Local).format("%H:%M").to_string())
}

impl Approval {
    /// The card for a `pending_action` message; None for any other message.
    pub fn of(message: &rules::Message) -> Option<Approval> {
        let rules::Message::PendingAction {
            write,
            detail,
            expires_at,
            approve_url,
            card,
            ..
        } = message
        else {
            return None;
        };
        let choices = cards::write_choices(message);
        let edit = if choices.approve {
            cards::write_edit_fields(message)
        } else {
            Vec::new()
        };
        Some(Approval {
            card: card.clone(),
            choices,
            start: if edit.is_empty() {
                Vec::new()
            } else {
                cards::write_edit_values(message)
            },
            edit,
            edited: cards::write_edited(message),
            summary: write.summary.clone(),
            irreversible: write.effect == "irreversible",
            ask: cards::write_ask(message),
            text: cards::write_ask(message)
                .and_then(|ask| ask.text)
                .map(|text| cards::write_text_parts(&text, &write.arguments)),
            minutes_left: expires_at
                .as_deref()
                .and_then(|at| minutes_left(at, chrono::Utc::now())),
            review: rules::safe_url(approve_url),
            detail: detail.clone(),
        })
    }
}

/// A view whose thread draws approval cards and decides them: the chat, and an errand's own thread.
pub trait Decides: Sized + 'static {
    fn decisions(&self) -> &Decisions;
    fn decisions_mut(&mut self) -> &mut Decisions;
    /// Send the person's decision on `card`.
    fn decide(&mut self, card: &str, decision: Decision, window: &mut Window, cx: &mut Context<Self>);
    /// The inputs of the cards whose Edit is open (A24·b).
    fn edit_boxes(&self) -> &EditBoxes;
    fn edit_boxes_mut(&mut self) -> &mut EditBoxes;
    /// Send `card`'s Save: the changed fields, as the inputs hold them.
    fn save_edit(&mut self, card: &str, window: &mut Window, cx: &mut Context<Self>);

    /// Edit on `card`: its fields open, each with the text it started with.
    fn start_edit(&mut self, approval: &Approval, window: &mut Window, cx: &mut Context<Self>) {
        let opened = self
            .decisions_mut()
            .begin_edit(&approval.card, approval.edit.clone(), approval.start.clone());
        if opened {
            self.edit_boxes_mut()
                .open(&approval.card, &approval.edit, &approval.start, window, cx);
            cx.notify();
        }
    }

    /// Cancel on `card`: its fields close, and nothing is sent.
    fn cancel_edit(&mut self, card: &str, cx: &mut Context<Self>) {
        self.decisions_mut().cancel_edit(card);
        if self.decisions().editing(card).is_none() {
            self.edit_boxes_mut().close(card);
        }
        cx.notify();
    }
}

/// One input of an open Edit: a line, or several lines.
enum EditBox {
    Line(Entity<InputState>),
    Lines(Entity<TextareaState>),
}

/// The inputs of every card whose Edit is open, by card, in its fields' order (A24·b).
#[derive(Default)]
pub struct EditBoxes(HashMap<String, Vec<(String, EditBox)>>);

impl EditBoxes {
    /// Make the inputs of `card`'s fields, each with its starting text.
    pub fn open<V: 'static>(
        &mut self,
        card: &str,
        fields: &[EditField],
        start: &[(String, String)],
        window: &mut Window,
        cx: &mut Context<V>,
    ) {
        let boxes = fields
            .iter()
            .map(|field| {
                let text = start
                    .iter()
                    .find(|(name, _)| *name == field.name)
                    .map(|(_, text)| text.clone())
                    .unwrap_or_default();
                let input = if field.kind == EditKind::Lines {
                    let state = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 10).submit_on_enter(false));
                    state.update(cx, |state, cx| state.set_value(text, window, cx));
                    EditBox::Lines(state)
                } else {
                    let state = cx.new(|cx| InputState::new(window, cx));
                    state.update(cx, |state, cx| state.set_value(text, window, cx));
                    EditBox::Line(state)
                };
                (field.name.clone(), input)
            })
            .collect();
        self.0.insert(card.to_string(), boxes);
    }

    pub fn close(&mut self, card: &str) {
        self.0.remove(card);
    }

    /// What the person typed, by field name, in the fields' order.
    pub fn values(&self, card: &str, cx: &App) -> Vec<(String, String)> {
        self.0
            .get(card)
            .map(|boxes| {
                boxes
                    .iter()
                    .map(|(name, input)| {
                        let text = match input {
                            EditBox::Line(state) => state.read(cx).value().to_string(),
                            EditBox::Lines(state) => state.read(cx).value().to_string(),
                        };
                        (name.clone(), text)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Edit's open fields (A24·b): a label over each input, as the web's card draws them.
fn edit_form(card: &str, fields: &[EditField], boxes: &EditBoxes, saving: bool, palette: Palette) -> Div {
    let Some(inputs) = boxes.0.get(card) else {
        return div();
    };
    v_flex()
        .w_full()
        .gap(px(10.))
        .children(fields.iter().filter_map(|field| {
            let (_, input) = inputs.iter().find(|(name, _)| *name == field.name)?;
            let control = match input {
                EditBox::Line(state) => ui::input_box(state, palette).into_any_element(),
                EditBox::Lines(state) => div()
                    .w_full()
                    .px(px(2.))
                    .py(px(4.))
                    .rounded(px(8.))
                    .bg(palette.hover)
                    .text_size(px(13.))
                    .child(Textarea::new(state).appearance(false))
                    .into_any_element(),
            };
            Some(ui::field(
                field.label.clone(),
                div().w_full().when(saving, |c| c.opacity(0.6)).child(control),
                palette,
            ))
        }))
}

/// The approval card (A9·b): the tier by shape (a hand for Asks first, a lock and a firmer edge for Always asks;
/// never a colour fill), what, To, the amount, Why, the exact text, then Decline and Approve, "Always allow this
/// kind" where the card offers it, and when it expires. Once decided, how it ended.
pub fn approval_card<V: Decides>(view: &V, id: usize, approval: Approval, cx: &mut Context<V>) -> impl IntoElement {
    {
        let palette = crate::pages::palette(cx);
        let always_asks = approval.ask.as_ref().is_some_and(|ask| ask.always_asks);
        let tier = cards::write_tier_label(approval.ask.as_ref());
        let editing = view.decisions().editing(&approval.card).cloned();
        let rows = approval.rows_for(editing.is_some());
        let text = approval.text.clone().filter(|_| editing.is_none());
        let expired = approval.minutes_left.is_some_and(|minutes| minutes <= 0);
        let expiry = approval.minutes_left.map(cards::write_expiry_text);
        let phase = view.decisions().phase(&approval.card).cloned();
        let card_body = card(palette)
            .when(always_asks, |card| card.border_color(palette.strong.opacity(0.5)))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(ui::icon(if always_asks { Lucide::Lock } else { Lucide::Hand }, palette.muted).size(px(13.)))
                    .child(card_title(tier, palette))
                    .children(
                        approval
                            .edited
                            .then(|| meta(format!("· {}", cards::WRITE_EDITED_LABEL), palette)),
                    )
                    .child(div().flex_1())
                    .when(!matches!(phase, Some(Phase::Ended(_))), |head| {
                        head.children(expiry.map(|expiry| meta(expiry, palette)))
                    }),
            )
            .child(body(approval.summary.clone(), palette).font_weight(FontWeight::MEDIUM))
            .children(rows.into_iter().map(|(label, value)| {
                h_flex()
                    .items_start()
                    .gap(px(10.))
                    .text_size(px(13.))
                    .child(div().w(px(52.)).flex_none().text_color(palette.muted).child(label))
                    .child(div().flex_1().min_w(px(0.)).text_color(palette.foreground).child(value))
            }))
            .children(text.map(|text| exact_text(text, palette)));
        // Edit is open (A24·b): the fields in place of the exact text, then Cancel and Save.
        if let Some(open) = editing {
            let saving = open.saving;
            let cancel_card = approval.card.clone();
            let save_card = approval.card.clone();
            let cancel = if saving {
                ui::disabled_button(("agent-edit-cancel", id), "Cancel", palette)
            } else {
                ui::button(("agent-edit-cancel", id), "Cancel", palette)
                    .on_click(cx.listener(move |this: &mut V, _, _, cx| this.cancel_edit(&cancel_card, cx)))
            };
            let save = if saving || expired {
                ui::disabled_button(
                    ("agent-edit-save", id),
                    if saving { "Saving…" } else { "Save" },
                    palette,
                )
            } else {
                ui::primary_button(("agent-edit-save", id), "Save", palette)
                    .on_click(cx.listener(move |this: &mut V, _, window, cx| this.save_edit(&save_card, window, cx)))
            };
            return card_body
                .child(edit_form(
                    &approval.card,
                    &open.fields,
                    view.edit_boxes(),
                    saving,
                    palette,
                ))
                .children(open.problem.map(|text| ui::error_line(text, palette)))
                .child(
                    h_flex()
                        .w_full()
                        .gap(px(8.))
                        .child(cancel)
                        .child(div().flex_1())
                        .child(save),
                );
        }
        if let Some(Phase::Ended(decided)) = phase {
            let url = decided.url.clone();
            let card_body = card_body.child(
                div()
                    .text_size(px(13.5))
                    .text_color(palette.foreground)
                    .child(decided.text.clone()),
            );
            // A written result with a title is its own row with Open (A7·f), as the event it made.
            let card_body = match (decided.title.clone(), url) {
                (Some(title), url) => card_body.child(result_row(id, title, url, palette)),
                (None, Some(url)) => card_body.child(super::link(("agent-written", id), "Open it", url, palette)),
                (None, None) => card_body,
            };
            return card_body.when(decided.always, |card| {
                card.child(ui::note(cards::WRITE_ALWAYS_NOTE, palette))
            });
        }
        let working = match phase {
            Some(Phase::Working(decision)) => Some(decision),
            _ => None,
        };
        let refused = match &phase {
            Some(Phase::Refused(text)) => Some(text.clone()),
            _ => None,
        };
        let choices = approval.choices;
        let off = working.is_some() || expired;
        let decide_button = |decision: Decision, cx: &mut Context<V>| {
            let (label, busy, button_id) = match decision {
                Decision::Approve => ("Approve", "Approving…", ("agent-approve", id)),
                Decision::Decline => ("Decline", "Declining…", ("agent-decline", id)),
            };
            let words = if working == Some(decision) { busy } else { label };
            if off {
                return ui::disabled_button(button_id, words, palette);
            }
            let card = approval.card.clone();
            let button = match decision {
                Decision::Approve => ui::primary_button(button_id, words, palette),
                Decision::Decline => ui::button(button_id, words, palette),
            };
            button.on_click(cx.listener(move |this: &mut V, _, window, cx| this.decide(&card, decision, window, cx)))
        };
        let review = approval.review.clone().map(|url| {
            let button = if choices.decline {
                ui::button(("agent-review", id), rules::CONFIRM_REVIEW, palette)
            } else {
                ui::primary_button(("agent-review", id), rules::CONFIRM_REVIEW, palette)
            };
            button.on_click(move |_, _, cx| open_link(&url, cx))
        });
        let mut actions = h_flex().w_full().gap(px(8.));
        if choices.approve && !approval.edit.is_empty() {
            let words = "Edit";
            let button_id = ("agent-edit", id);
            actions = actions.child(if off {
                ui::disabled_button(button_id, words, palette)
            } else {
                let opened = approval.clone();
                ui::button(button_id, words, palette)
                    .on_click(cx.listener(move |this: &mut V, _, window, cx| this.start_edit(&opened, window, cx)))
            });
        }
        if choices.decline {
            actions = actions.child(decide_button(Decision::Decline, cx));
        }
        actions = actions.child(div().flex_1());
        if choices.approve {
            actions = actions.child(decide_button(Decision::Approve, cx));
        } else if let Some(review) = review {
            actions = actions.child(review);
        }
        let always_on = view.decisions().always(&approval.card);
        let always = choices.always.then(|| {
            let card = approval.card.clone();
            h_flex()
                .gap(px(8.))
                .child(
                    ui::switch(("agent-always", id), always_on, off, palette).when(!off, |switch| {
                        switch.on_click(cx.listener(move |this, _, _, cx| {
                            this.decisions_mut().toggle_always(&card);
                            cx.notify();
                        }))
                    }),
                )
                .child(div().text_size(px(12.5)).text_color(palette.muted).child(ALWAYS_ALLOW))
        });
        card_body
            .when(
                approval.ask.as_ref().and_then(|ask| ask.text.as_ref()).is_none(),
                |card| card.child(ui::note(approval.undo_line(), palette)),
            )
            .children(refused.map(|text| ui::error_line(text, palette)))
            .when(
                choices.decline || choices.approve || approval.review.is_some(),
                |card| card.child(actions),
            )
            .children(always)
            .when(
                !choices.decline && !choices.approve && approval.review.is_none(),
                |card| card.child(ui::note(approval.detail.clone(), palette)),
            )
    }
}

/// The card's offer, beside its switch, where the card offers it (A9·b: Asks first only).
pub const ALWAYS_ALLOW: &str = "Always allow this kind";

/// The exact text as it goes (A7·a): in a 10 px inset box at 13 px, an email's subject first, the body, then the
/// signature or "Sent using Nexus" quieter, each a blank line apart. Together they are the whole text the card approves.
pub(super) fn exact_text(parts: TextParts, palette: Palette) -> Div {
    let gap = || div().h(px(19.5));
    v_flex()
        .w_full()
        .px(px(12.))
        .py(px(10.))
        .rounded(px(10.))
        .bg(ui::inset(palette))
        .border_1()
        .border_color(palette.border)
        .text_size(px(13.))
        .line_height(relative(1.5))
        .text_color(palette.foreground)
        .children(parts.subject.map(|subject| {
            v_flex()
                .child(div().id("agent-approval-subject").child(format!("Subject: {subject}")))
                .child(gap())
        }))
        .child(div().child(parts.body))
        .children(parts.end.map(|end| {
            v_flex()
                .child(gap())
                .child(div().id("agent-approval-sign").text_color(palette.muted).child(end))
        }))
}

/// What a written result made, as its own row (A7·f): its title at 14 px, and Open under it when it has a link.
fn result_row(id: usize, title: String, url: Option<String>, palette: Palette) -> Div {
    v_flex()
        .w_full()
        .gap(px(8.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(16.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
        .child(
            div()
                .id(("agent-written-title", id))
                .text_size(px(14.))
                .text_color(palette.foreground)
                .child(title),
        )
        .children(url.map(|url| {
            h_flex()
                .pt(px(4.))
                .child(ui::button(("agent-written", id), "Open", palette).on_click(move |_, _, cx| open_link(&url, cx)))
        }))
}

#[cfg(test)]
mod tests {
    use super::{Approval, WriteAsk, cards, clock, minutes_left};

    fn approval(ask: Option<WriteAsk>) -> Approval {
        Approval {
            card: "card_1".into(),
            choices: Default::default(),
            summary: "Text Ada R.".into(),
            irreversible: true,
            ask,
            text: None,
            minutes_left: Some(30),
            review: Some("https://nexus.test/app/approvals/a1".into()),
            detail: "Approve it in Nexus.".into(),
            edit: Vec::new(),
            start: Vec::new(),
            edited: false,
        }
    }

    /// A9·b: To, the amount and Why, in that order, each only when the card has it.
    #[test]
    fn the_approval_card_shows_to_the_amount_and_why() {
        let ask = WriteAsk {
            always_asks: false,
            to: vec!["Ada R.".into(), "Dev L.".into()],
            text: Some("Dinner Friday 7:30.".into()),
            amount_usd: Some("31.4".into()),
            why_said: Some("tell them".into()),
            why_at: None,
            new_to: vec!["Dev L.".into()],
        };
        let rows = approval(Some(ask)).rows();
        assert_eq!(
            rows,
            vec![
                ("To", "Ada R. · Dev L. (new to this errand)".to_string()),
                ("Amount", "$31.40".to_string()),
                ("Why", "You said “tell them”.".to_string()),
            ]
        );
        // A write from before the A9 card shows its summary alone.
        let old = approval(None);
        assert!(old.rows().is_empty());
        assert_eq!(
            old.undo_line(),
            "It cannot be undone. Nothing changes until you approve it."
        );
        assert_eq!(cards::write_tier_label(old.ask.as_ref()), "Asks first");
    }

    #[test]
    fn the_expiry_counts_whole_minutes_up() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-09T12:00:00Z")
            .expect("a time")
            .with_timezone(&chrono::Utc);
        assert_eq!(minutes_left("2026-11-09T12:30:00.000Z", now), Some(30));
        assert_eq!(minutes_left("2026-11-09T12:00:01Z", now), Some(1));
        assert_eq!(minutes_left("2026-11-09T11:59:00Z", now), Some(-1));
        assert_eq!(minutes_left("soon", now), None);
        assert_eq!(cards::write_expiry_text(-1), "Expired");
        assert!(clock("2026-11-09T08:24:00Z").is_some_and(|at| at.len() == 5));
    }
}
