//! The session composer, scoped to the side thread: Attach and its menu, the model and effort chips with their
//! menus, then Stop, Queue and Send (`composer.rs`, `code_dock.rs`).
use super::composer::Section;
use super::*;
use crate::motion;
use crate::palette::Palette;
use crate::workspace::MenuTrigger;
use crate::workspace::code_dock::{self, EFFORTS, menu_box, menu_layer};
use crate::workspace::composer::{
    attach_ring, composer_frame, composer_input, composer_shell, composer_toolbar, queue_pill, send_circle, stop_ring,
    toolbar_slot,
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use std::rc::Rc;

impl SideChat {
    /// Match the session composer: large text becomes a file, and mixed file/text pastes stay inline.
    pub(super) fn paste(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        let pasted = crate::attachments::pasted(item);
        if !pasted.files.is_empty() {
            self.attach_files(pasted.files, cx);
            return pasted.take;
        }
        if !self.can_compose() || self.request.is_some() || self.secret_id.is_some() {
            return false;
        }
        let Some(file) =
            crate::workspace::text_paste::pasted_text_file(item, self.draft.all().map(|file| file.name.as_str()))
        else {
            return false;
        };
        // Already in memory: prepare synchronously so another paste sees the name we just reserved.
        let prepared = crate::attachments::prepare(vec![file]);
        self.problem = crate::attachments::merge(&mut self.draft, prepared);
        self.draft_revision = self.draft_revision.wrapping_add(1);
        cx.notify();
        true
    }

    /// Claim literal paste before the textarea's platform paste action can turn it into a file.
    pub(super) fn paste_as_text(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !crate::workspace::text_paste::paste_as_text_key(key)
            || !self.can_compose()
            || self.request.is_some()
            || self.secret_id.is_some()
            || !self.input.focus_handle(cx).is_focused(window)
        {
            return false;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.input.update(cx, |input, cx| input.replace(text, window, cx));
            cx.notify();
        }
        true
    }

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

    fn attach_clipboard(&mut self, cx: &mut Context<Self>) {
        let files = cx
            .read_from_clipboard()
            .map(|item| crate::attachments::pasted(&item).files)
            .unwrap_or_default();
        if files.is_empty() {
            self.note = Some("The clipboard has no images or files to attach".into());
            cx.notify();
        } else {
            self.attach_files(files, cx);
        }
    }

    /// Attach and its menu, as the session composer's.
    fn render_attach(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.attach_open;
        let button = attach_ring("attach-side-file", open, p)
            .when(!open, |button| {
                button.tooltip(|window, cx| Tooltip::new("Add files and more").build(window, cx))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.attach_open = !this.attach_open;
                    this.settings = None;
                    cx.notify();
                }),
            );
        let item = |id: &'static str, icon: Lucide, label: &'static str, keys: Option<&'static str>| {
            h_flex()
                .id(id)
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| style.bg(p.menu_hover))
                .child(crate::workspace::glyph(icon, p.icon).size(px(15.)))
                .child(div().flex_1().child(label))
                .when_some(keys, |row, keys| {
                    row.child(div().text_size(px(12.)).text_color(p.faint).child(keys))
                })
        };
        let paste_keys = if cfg!(target_os = "macos") { "⌘V" } else { "Ctrl+V" };
        div()
            .relative()
            .child(button)
            .when_some(motion::presence("side-attach-menu", open, cx), |frame, phase| {
                let menu = menu_box("side-attach-menu", 232., p)
                    .text_size(px(13.))
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.attach_open = false;
                        cx.notify();
                    }))
                    .child(
                        item("side-attach-files", Lucide::Paperclip, "Attach images or files", None).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.attach_open = false;
                                this.pick_files(cx);
                            }),
                        ),
                    )
                    .child(
                        item(
                            "side-attach-paste",
                            Lucide::Clipboard,
                            "Paste from clipboard",
                            Some(paste_keys),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.attach_open = false;
                            this.attach_clipboard(cx);
                        })),
                    );
                frame.child(menu_layer(phase.menu("side-attach-menu", 4., menu), Anchor::BottomLeft))
            })
    }

    /// The model and effort chips, each opening its menu above the composer.
    fn render_chips(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let model: SharedString = self
            .models
            .iter()
            .find(|m| Some(m.id.as_str()) == self.summary.model.as_deref())
            .map(|m| m.name.clone())
            .or_else(|| self.summary.model.clone())
            .unwrap_or_else(|| nexus_workbench::driver::label(self.summary.agent).to_string())
            .into();
        let maker = self
            .summary
            .model
            .as_deref()
            .and_then(crate::workspace::answer_footer::maker_of)
            .unwrap_or_else(|| crate::workspace::new_code::agent_maker(self.summary.agent));
        let efforts = self.efforts();
        let (mode, _) = code_dock::permission_copy(self.summary.agent, self.summary.permission_mode);
        let effort: SharedString = if efforts.is_empty() {
            mode.into()
        } else {
            EFFORTS
                .iter()
                .find(|(effort, _)| Some(*effort) == self.summary.effort)
                .map(|(_, label)| *label)
                .unwrap_or("Default")
                .into()
        };
        let open = self.settings;
        let tip = format!("Effort: {effort}; permissions: {mode}");
        let model_tip = model.clone();
        h_flex()
            .min_w_0()
            .flex_shrink(1.)
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .relative()
                    .child(
                        code_dock::chip("side-model", maker, model, open == Some(Section::Model), p)
                            .debug_selector(|| "side-model".into())
                            .min_w_0()
                            .flex_shrink(1.)
                            .tooltip(move |window, cx| Tooltip::new(model_tip.clone()).build(window, cx))
                            .when(self.sending, |chip| chip.opacity(0.4))
                            .menu_trigger(
                                open == Some(Section::Model),
                                cx,
                                |this| this.settings == Some(Section::Model),
                                |this, window, cx| this.settings(Section::Model, window, cx),
                            ),
                    )
                    .when_some(
                        motion::presence("side-model-menu", open == Some(Section::Model), cx),
                        |frame, phase| {
                            let menu = phase.menu("side-model-menu", 4., self.render_model_menu(p, cx));
                            frame.child(menu_layer(menu, Anchor::BottomLeft))
                        },
                    ),
            )
            .child(
                div()
                    .relative()
                    .child(
                        code_dock::effort_chip(
                            "side-effort",
                            effort,
                            open == Some(Section::Effort),
                            false,
                            if efforts.is_empty() {
                                crate::workspace::new_code::mode_icon(self.summary.permission_mode)
                            } else {
                                Lucide::Brain
                            },
                            p,
                        )
                        .debug_selector(|| "side-effort".into())
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .when(self.sending, |chip| chip.opacity(0.4))
                        .menu_trigger(
                            open == Some(Section::Effort),
                            cx,
                            |this| this.settings == Some(Section::Effort),
                            |this, window, cx| this.settings(Section::Effort, window, cx),
                        ),
                    )
                    .when_some(
                        motion::presence("side-effort-menu", open == Some(Section::Effort), cx),
                        |frame, phase| {
                            let menu = phase.menu("side-effort-menu", 4., self.render_effort_menu(p, cx));
                            frame.child(menu_layer(menu, Anchor::BottomLeft))
                        },
                    ),
            )
    }

    pub(super) fn render_message_composer(&self, window: &Window, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let remove = Rc::new(move |id, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| {
                this.draft.remove(id);
                cx.notify();
            });
        });
        let weak = cx.weak_entity();
        let cancel: Rc<dyn Fn(u64, &mut Window, &mut App)> = Rc::new(move |id, _, cx| {
            let _ = weak.update(cx, |this, cx| this.cancel_upload(id, cx));
        });
        let weak = cx.weak_entity();
        let paste = move |item: &ClipboardItem, _: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| this.paste(item, cx)).unwrap_or(false)
        };
        let busy = self.summary.status.busy();
        let has_text = !(self.input.read(cx).value().trim().is_empty() && self.draft.is_empty());
        // A send in flight does not hold the next message: it waits in line (`commands.rs`).
        let enabled = self.can_compose() && self.request.is_none() && self.secret_id.is_none() && has_text;
        let send_tip = if busy {
            "Send now: the agent reads it at its next step (Enter)"
        } else {
            "Send"
        };
        let send = send_circle("send-side-chat", enabled, p)
            .debug_selector(|| "side-send".into())
            .tooltip(move |window, cx| Tooltip::new(send_tip).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.send(false, window, cx)));
        let queue_tip: SharedString = format!(
            "Queue: sent when this turn ends ({})",
            crate::workspace::composer::queue_keys()
        )
        .into();
        let queue = queue_pill("queue-side-chat", p)
            .debug_selector(|| "side-queue".into())
            .tooltip(move |window, cx| Tooltip::new(queue_tip.clone()).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.send(true, window, cx)));
        let stop = stop_ring("stop-side-chat", p.working(), cx.reduce_motion(), p)
            .debug_selector(|| "side-stop".into())
            .tooltip(|window, cx| Tooltip::new("Stop (Esc)").build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.command(
                    "wb_interrupt",
                    json!({"threadId":this.summary.id}),
                    false,
                    None,
                    window,
                    cx,
                )
            }));
        let focused = self.input.focus_handle(cx).contains_focused(window, cx);
        // The sent message's files stay over the composer while they go up, as the session's (`render_uploading`).
        let uploading = self.uploading.as_ref().filter(|draft| !draft.is_empty());
        let shown = uploading.is_some() || !self.draft.is_empty();
        let shell = composer_shell(false, p)
            .debug_selector(|| "side-chat-composer".into())
            // The attachments open and close in height, as the session composer's.
            .child(motion::reveal(
                "side-draft-row",
                shown,
                shown.then(|| {
                    div()
                        .pt(px(14.))
                        .pr(px(12.))
                        .pl(px(16.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .when_some(uploading, |row, up| {
                            row.child(crate::workspace::composer::attachment_chips(
                                "side-uploading",
                                up,
                                p,
                                None,
                                Some(cancel),
                                None,
                            ))
                        })
                        .when(!self.draft.is_empty(), |row| {
                            row.child(crate::workspace::composer::attachment_chips(
                                "side-draft",
                                &self.draft,
                                p,
                                Some(remove),
                                None,
                                None,
                            ))
                        })
                        .into_any_element()
                }),
            ))
            .child(composer_input(
                Textarea::new(&self.input)
                    .appearance(false)
                    .bordered(false)
                    .on_paste(paste),
            ))
            .child(
                composer_toolbar()
                    .w_full()
                    .gap(px(3.))
                    .child(self.render_attach(p, cx))
                    .child(self.render_chips(p, cx))
                    .child(div().flex_1())
                    // As the session's: Stop slides in as Send slides out (`toolbar_slot`), and once there is a
                    // message, Queue (after the turn) and Send (into it now) stand beside Stop.
                    .child(toolbar_slot("side-stop-slot", busy, stop))
                    .child(toolbar_slot("side-queue-slot", busy && enabled, queue))
                    .child(toolbar_slot("side-send-slot", !busy || enabled, send)),
            );
        composer_frame(shell, focused, p).into_any_element()
    }
}
