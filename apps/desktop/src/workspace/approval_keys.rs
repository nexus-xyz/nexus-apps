//! The keys that answer a tool approval, as the Nexus app's `ApprovalPanel` (`composer.tsx`):
//!
//! - A new request focuses Allow, so Enter (or Space) allows it once. If you typed in the composer in the last
//!   1.5 seconds, the composer keeps focus, so its Enter does not approve a request.
//! - Esc never answers a card (the canon, `canon_transcript.py`'s approval cards). A reason with text keeps its Esc: it
//!   clears the reason; an empty one closes. A text field outside the card keeps its Esc too, and the Allow always menu
//!   closes on its own Esc. Esc in the find field closes find first (`code_find.rs`). While the card shows, Esc never
//!   stops the turn.
//! - For 600 ms after the card appears, Enter does not answer it, so a key meant for the composer misses it.
//!
//! Enter in the reason denies; that is its own field's event (`code_dock.rs`), under the same 600 ms.

use std::cell::Cell;
use std::time::{Duration, Instant};

use gpui_kit::*;
use nexus_workbench::event::Decision;

use super::code_dock::{Menu, PLAN_TOOL, QUESTION_TOOL};
use super::{View, Workspace};
use crate::code::Block;

/// How long after a request appears its keys wait (`ARMING_MS`).
const ARMING: Duration = Duration::from_millis(600);
/// How recent a key in the composer counts as typing when a request appears.
const TYPING: Duration = Duration::from_millis(1500);

thread_local! {
    /// When a key was last pressed in the composer (`lastTyped`).
    static LAST_TYPED: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// The card's keys: its focus targets, and when the request on it appeared.
pub(super) struct ApprovalKeys {
    /// Allow: focused when a request appears, so Enter answers it.
    pub(super) allow: FocusHandle,
    /// The card itself: focused instead when you were typing, so Enter answers nothing.
    pub(super) card: FocusHandle,
    shown_at: Option<Instant>,
    /// A request appeared and its focus is not set yet.
    focus_due: bool,
}

impl ApprovalKeys {
    pub(super) fn new(cx: &mut App) -> ApprovalKeys {
        ApprovalKeys {
            allow: cx.focus_handle(),
            card: cx.focus_handle(),
            shown_at: None,
            focus_due: false,
        }
    }

    /// A new tool request is on the card.
    pub(super) fn shown(&mut self) {
        self.shown_at = Some(Instant::now());
        self.focus_due = true;
    }

    /// No tool request is on the card.
    pub(super) fn gone(&mut self) {
        self.shown_at = None;
        self.focus_due = false;
    }

    fn arming(&self, now: Instant) -> bool {
        self.shown_at
            .is_some_and(|at| now.saturating_duration_since(at) < ARMING)
    }
}

/// Where the keys go when one is pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    /// Allow.
    Allow,
    /// The card, or nothing that takes keys (the window, the hidden composer).
    Card,
    /// The reason sent with Deny, and whether it is empty.
    Reason { empty: bool },
    /// A text field or a terminal outside the card.
    Field,
}

/// What a key does to the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyAnswer {
    Allow,
    ClearReason,
    /// An empty reason closes, and the keys go back to the card.
    CloseReason,
    CloseMenu,
    /// Taken, and nothing happens: Allow's Enter while the card arms, and Esc on the card.
    Nothing,
}

/// The card's answer to a plain key, or `None` when the key is not the card's (`ApprovalPanel`'s listeners).
pub(super) fn answer_key(key: &str, focus: Focus, arming: bool, menu_open: bool) -> Option<KeyAnswer> {
    match key {
        // The Always allow menu closes on its own Esc; that Esc denies nothing.
        "escape" if menu_open => Some(KeyAnswer::CloseMenu),
        // A reason with text in it clears, at any time; an empty one closes.
        "escape" if focus == (Focus::Reason { empty: false }) => Some(KeyAnswer::ClearReason),
        "escape" if focus == (Focus::Reason { empty: true }) => Some(KeyAnswer::CloseReason),
        "escape" if focus == Focus::Field => None,
        // Esc never answers a card, and while it shows it stops nothing either.
        "escape" => Some(KeyAnswer::Nothing),
        // A focused button's Enter or Space; right as the card appears, it was meant for the composer.
        "enter" | "space" if focus == Focus::Allow && !menu_open => {
            Some(if arming { KeyAnswer::Nothing } else { KeyAnswer::Allow })
        }
        _ => None,
    }
}

/// Whether a key in the composer came less than 1.5 seconds before the request appeared.
fn typing_when_shown(shown_at: Instant, last_typed: Option<Instant>) -> bool {
    last_typed.is_some_and(|typed| shown_at.saturating_duration_since(typed) < TYPING)
}

impl Workspace {
    /// Whether the open Code session shows a tool approval card (not a question, a plan or a secret).
    fn tool_approval_shown(&self) -> bool {
        let View::Code(view) = &self.view else {
            return false;
        };
        self.runner.is_some()
            && self.open_secret_request().is_none()
            && matches!(view.dock.permissions.current(&view.blocks),
                Some(Block::Approval { tool, .. }) if tool != QUESTION_TOOL && tool != PLAN_TOOL)
    }

    /// Whether the card appeared too recently for its keys.
    pub(super) fn approval_arming(&self) -> bool {
        match &self.view {
            View::Code(view) => view.dock.keys.arming(Instant::now()),
            _ => false,
        }
    }

    /// Note a key in the composer: a request that appears right after it does not take the next Enter.
    pub(super) fn note_composer_key(&self, window: &Window, cx: &App) {
        if self.composer.focus_handle(cx).is_focused(window) {
            LAST_TYPED.with(|last| last.set(Some(Instant::now())));
        }
    }

    /// Focus a new request's Allow, unless the person was typing in the composer.
    pub(super) fn focus_approval(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.tool_approval_shown();
        let View::Code(view) = &mut self.view else {
            return;
        };
        let keys = &mut view.dock.keys;
        if view.dock.permissions.keep_focus {
            keys.focus_due = false;
            return;
        }
        if !shown || !keys.focus_due {
            return;
        }
        keys.focus_due = false;
        view.dock.reason.update(cx, |state, cx| state.set_value("", window, cx));
        let typing = keys
            .shown_at
            .is_some_and(|at| typing_when_shown(at, LAST_TYPED.with(Cell::get)));
        if typing {
            return;
        }
        let target = keys.allow.clone();
        window.defer(cx, move |window, cx| window.focus(&target, cx));
    }

    /// Where a key goes, for the card.
    fn approval_focus(&self, window: &Window, cx: &App) -> Option<Focus> {
        let View::Code(view) = &self.view else {
            return None;
        };
        let dock = &view.dock;
        let reason = dock.reason.read(cx);
        Some(if dock.keys.allow.is_focused(window) {
            Focus::Allow
        } else if reason.focus_handle(cx).is_focused(window) {
            Focus::Reason {
                empty: reason.value().is_empty(),
            }
        } else if dock.keys.card.is_focused(window) || self.focus.is_focused(window) || window.focused(cx).is_none() {
            Focus::Card
        } else {
            Focus::Field
        })
    }

    /// A plain key while a tool approval shows: Enter allows once; Esc answers nothing. True when the card took the key.
    pub(super) fn approval_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.tool_approval_shown() {
            return false;
        }
        let Some(focus) = self.approval_focus(window, cx) else {
            return false;
        };
        let View::Code(view) = &self.view else {
            return false;
        };
        let menu_open = view.dock.menu == Some(Menu::Always);
        let arming = view.dock.keys.arming(Instant::now());
        let Some(answer) = answer_key(key, focus, arming, menu_open) else {
            return false;
        };
        match answer {
            KeyAnswer::Allow => self.answer(Decision::Allow, None, None, None, cx),
            KeyAnswer::CloseReason => self.close_deny_reason(window, cx),
            KeyAnswer::ClearReason => {
                let reason = view.dock.reason.clone();
                reason.update(cx, |state, cx| state.set_value("", window, cx));
            }
            KeyAnswer::CloseMenu => self.close_dock_menu(cx),
            KeyAnswer::Nothing => {}
        }
        true
    }

    /// Whether Esc stops the turn: never while a tool approval shows, which has Esc of its own.
    pub(super) fn esc_stops_turn(&self) -> bool {
        (self.asking.is_some() || self.code_running()) && !self.tool_approval_shown()
    }
}

#[cfg(test)]
mod tests {
    use super::{Focus, KeyAnswer, answer_key, typing_when_shown};
    use std::time::{Duration, Instant};

    #[test]
    fn enter_on_allow_allows_once_after_the_card_arms() {
        assert_eq!(answer_key("enter", Focus::Allow, false, false), Some(KeyAnswer::Allow));
        assert_eq!(answer_key("space", Focus::Allow, false, false), Some(KeyAnswer::Allow));
        // Right as the card appears, the key was meant for the composer: taken, and nothing happens.
        assert_eq!(answer_key("enter", Focus::Allow, true, false), Some(KeyAnswer::Nothing));
    }

    #[test]
    fn enter_anywhere_but_allow_is_not_the_cards() {
        assert_eq!(answer_key("enter", Focus::Card, false, false), None);
        assert_eq!(answer_key("enter", Focus::Field, false, false), None);
        // The reason's Enter is its field's own event.
        assert_eq!(answer_key("enter", Focus::Reason { empty: false }, false, false), None);
    }

    #[test]
    fn esc_never_answers_the_card() {
        for arming in [false, true] {
            for focus in [Focus::Card, Focus::Allow] {
                assert_eq!(
                    answer_key("escape", focus, arming, false),
                    Some(KeyAnswer::Nothing),
                    "{focus:?}"
                );
            }
        }
        // An empty reason closes; nothing is denied.
        assert_eq!(
            answer_key("escape", Focus::Reason { empty: true }, false, false),
            Some(KeyAnswer::CloseReason)
        );
    }

    #[test]
    fn esc_in_a_reason_with_text_clears_it_even_while_arming() {
        let reason = Focus::Reason { empty: false };
        assert_eq!(answer_key("escape", reason, false, false), Some(KeyAnswer::ClearReason));
        assert_eq!(answer_key("escape", reason, true, false), Some(KeyAnswer::ClearReason));
    }

    #[test]
    fn esc_in_a_field_outside_the_card_is_the_fields() {
        assert_eq!(answer_key("escape", Focus::Field, false, false), None);
        assert_eq!(answer_key("escape", Focus::Field, true, false), None);
    }

    #[test]
    fn the_always_allow_menu_takes_esc_and_enter_from_the_card() {
        assert_eq!(
            answer_key("escape", Focus::Card, false, true),
            Some(KeyAnswer::CloseMenu)
        );
        assert_eq!(answer_key("enter", Focus::Allow, false, true), None);
    }

    #[test]
    fn other_keys_are_not_the_cards() {
        assert_eq!(answer_key("a", Focus::Allow, false, false), None);
        assert_eq!(answer_key("tab", Focus::Card, false, false), None);
    }

    #[test]
    fn a_key_in_the_composer_within_1_5_seconds_counts_as_typing() {
        let shown = Instant::now() + Duration::from_secs(10);
        assert!(!typing_when_shown(shown, None));
        assert!(typing_when_shown(shown, Some(shown - Duration::from_millis(1499))));
        assert!(!typing_when_shown(shown, Some(shown - Duration::from_millis(1500))));
    }
}

#[cfg(test)]
#[path = "escape_tests.rs"]
mod escape_tests;
