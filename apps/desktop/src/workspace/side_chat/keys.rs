//! The side composer's keys, as the session composer's (`composer.rs`): Up and Down in an empty composer walk back
//! through what was sent in the side chat (`prompt_history.rs`), and Shift+Enter continues a Markdown list or quote.
use super::*;
use crate::workspace::prompt_history::{self, Caret};
use gpui_kit::component::input::{Enter, MoveDown, MoveUp};

impl SideChat {
    /// Up (`down` false) or Down in the composer: the next sent message, when the walk applies.
    pub(super) fn recall_sent(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.can_compose() {
            return false;
        }
        let input = self.input.read(cx);
        let (text, selected) = (input.value().to_string(), input.selected_range());
        let entries = prompt_history::sent_prompts(&self.blocks, nexus_workbench::CONTINUE);
        let Some(step) = prompt_history::recall(down, &text, selected, &entries, self.recall.as_ref()) else {
            return false;
        };
        self.recall = step.walk;
        let at = match step.caret {
            Caret::Start => 0,
            Caret::End => step.text.len(),
        };
        self.input.update(cx, |input, cx| {
            if input.value().as_ref() != step.text {
                input.set_value(step.text, window, cx);
            }
            input.set_selected_range(at..at, cx);
        });
        cx.notify();
        true
    }

    /// Shift+Enter: the next line of a Markdown list or quote, as the session's. False when a plain newline goes.
    fn markdown_newline(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let input = self.input.read(cx);
        let range = input.selected_range();
        let at = if range.is_empty() {
            input.cursor()..input.cursor()
        } else {
            range
        };
        let Some(edit) = nexus_workbench::markdown_newline::markdown_newline(&input.value(), at.start, at.end) else {
            return false;
        };
        self.input.update(cx, |input, cx| {
            input.set_selected_range(edit.range, cx);
            input.replace(edit.insert, window, cx);
            input.set_selected_range(edit.caret..edit.caret, cx);
        });
        cx.notify();
        true
    }

    /// Files dragged over the window will attach here, or not: the composer shows the drop outline.
    pub(in crate::workspace) fn show_file_drag(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.file_drag != on {
            self.file_drag = on;
            cx.notify();
        }
    }

    /// The handlers on the composer's box: they take a key before the text field does.
    pub(super) fn composer_keys(&self, shell: Div, cx: &mut Context<Self>) -> Div {
        shell
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if this.recall_sent(false, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if this.recall_sent(true, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, action: &Enter, window, cx| {
                if action.shift && !action.secondary && this.markdown_newline(window, cx) {
                    cx.stop_propagation();
                }
            }))
    }
}
