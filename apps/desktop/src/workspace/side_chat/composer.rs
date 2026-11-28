//! Side-chat settings and queue rewrites stay scoped to the side thread. Their menus and queued chips are the session
//! composer's (`code_dock.rs`).
use super::*;
use crate::palette::Palette;
use crate::workspace::code_dock::{self, MODES, menu_box, menu_label, menu_row};
use crate::workspace::queued_row;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use nexus_workbench::event::{Effort, SessionAgent};

/// The composer menu that is open: the model's, or reasoning and permissions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Section {
    Model,
    Effort,
}

pub(super) struct Choice {
    pub(super) id: String,
    pub(super) name: String,
    efforts: Vec<Effort>,
}
pub(super) struct Rewrite {
    pub seq: i64,
    pub from: String,
    pub input: Entity<TextareaState>,
    /// The side thread's queue holds while the rewrite is open (`queue_hold.rs`). Dropped only once the save
    /// answered, so letting go never races the new text.
    hold: crate::workspace::queue_hold::QueueHold,
}

impl Rewrite {
    pub(super) fn new(chat: &SideChat, seq: i64, from: String, input: Entity<TextareaState>) -> Rewrite {
        let hold = crate::workspace::queue_hold::QueueHold::new(chat.runs.clone(), chat.summary.id.clone(), seq);
        Rewrite { seq, from, input, hold }
    }

    /// The save answered: `wb_queue_edit` let the queue go, so closing the rewrite says nothing more.
    pub(super) fn saved(self) {
        self.hold.edited();
    }
}

fn catalog(value: &Value, agent: SessionAgent) -> Vec<Choice> {
    let agent = serde_json::to_value(agent).unwrap_or(Value::Null);
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["agent"] == agent)
        .flat_map(|row| row["models"].as_array().into_iter().flatten())
        .filter_map(|row| {
            Some(Choice {
                id: row["id"].as_str()?.into(),
                name: row["name"].as_str()?.into(),
                efforts: serde_json::from_value(row["efforts"].clone()).unwrap_or_default(),
            })
        })
        .collect()
}
fn kept_rewrite(current: &str, original: &str, edited: &str) -> Option<String> {
    if edited.trim().is_empty() || edited == original {
        return None;
    }
    Some(if current.trim().is_empty() {
        edited.to_owned()
    } else {
        format!("{current}\n{edited}")
    })
}
impl SideChat {
    /// Open the menu of `section`, or close it when it is open.
    pub(super) fn settings(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        if self.retired || !self.pending.valid() {
            return;
        }
        self.attach_open = false;
        if self.settings == Some(section) {
            self.settings = None;
            cx.notify();
            return;
        }
        self.settings = Some(section);
        self.model_input.update(cx, |input, cx| {
            input.set_value(self.summary.model.clone().unwrap_or_default(), window, cx)
        });
        // Focus stays in the side chat, so Esc closes the menu.
        match section {
            Section::Model => self.model_input.focus_handle(cx).focus(window, cx),
            Section::Effort => self.input.focus_handle(cx).focus(window, cx),
        }
        // A coding agent reads its own catalog; Nexus's own agent takes Gateway's models (`gateway_models`).
        if self.summary.agent.coding().is_some() {
            self.load_models(window, cx);
        }
        cx.notify();
    }
    fn load_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.models_loading {
            return;
        }
        self.models_loading = true;
        self.catalog_problem = None;
        let read = self.runs.call("wb_agent_models", json!({}));
        cx.spawn_in(window, async move |this, cx| {
            let result = read
                .await
                .unwrap_or_else(|_| Err("The model catalog answer was lost.".into()));
            let _ = this.update(cx, |this, cx| {
                if this.retired || !this.pending.valid() {
                    return;
                }
                this.models_loading = false;
                match result {
                    Ok(value) => this.models = catalog(&value, this.summary.agent),
                    Err(error) => this.catalog_problem = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn gateway_models(&mut self, models: &[crate::model::ModelChoice]) {
        if self.summary.agent.coding().is_none() {
            self.models = models
                .iter()
                .filter(|m| m.usable)
                .map(|m| Choice {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    efforts: vec![],
                })
                .collect();
        }
    }
    pub(super) fn edit_queued(&mut self, seq: i64, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.retired || !self.pending.valid() {
            return;
        }
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6).submit_on_enter(false));
        input.update(cx, |input, cx| input.set_value(text.clone(), window, cx));
        input.focus_handle(cx).focus(window, cx);
        self.rewrite = Some(Rewrite::new(self, seq, text, input));
        cx.notify();
    }
    pub(super) fn preserve_gone_rewrite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        if !self.rewrite.as_ref().is_some_and(|edit| {
            !self
                .blocks
                .iter()
                .any(|b| matches!(b,Block::Queued {seq,..} if *seq==edit.seq))
        }) {
            return;
        }
        let edit = self.rewrite.take().unwrap();
        let text = edit.input.read(cx).value().to_string();
        let current = self.input.read(cx).value().to_string();
        if let Some(merged) = kept_rewrite(&current, &edit.from, &text) {
            self.input.update(cx, |input, cx| input.set_value(merged, window, cx));
            self.note = Some("The message left the queue. Your rewrite is in the composer.".into());
        }
    }
    pub(super) fn save_rewrite(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.can_command() {
            return;
        }
        let Some(edit) = &self.rewrite else { return };
        let (seq, text) = (edit.seq, edit.input.read(cx).value().to_string());
        let effect = super::operations::Effect::Rewrite {
            seq,
            text: text.clone(),
        };
        let (runs, thread, permit) = (self.runs.clone(), self.summary.id.clone(), self.pending.permit());
        self.launch(
            effect,
            async move {
                permit.check()?;
                runs.call("wb_queue_edit", json!({"threadId":thread,"seq":seq,"text":text}))
                    .await
                    .unwrap_or_else(|_| Err("The queue edit answer was lost. Read the queue before retrying.".into()))
            },
            cx,
        );
    }

    /// The reasoning levels the side thread's model takes: its own when the agent reported them, else the agent's usual
    /// ones (`Harness::usual_efforts`).
    pub(super) fn efforts(&self) -> Vec<(Effort, &'static str)> {
        let reported = self
            .models
            .iter()
            .find(|m| Some(m.id.as_str()) == self.summary.model.as_deref())
            .filter(|m| !m.efforts.is_empty());
        side_efforts(self.summary.agent, reported.map(|model| model.efforts.as_slice()))
    }

    fn update_thread(&mut self, args: Value, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = None;
        let mut call = json!({"threadId":self.summary.id});
        if let (Some(call), Some(args)) = (call.as_object_mut(), args.as_object()) {
            call.extend(args.clone());
        }
        self.command("wb_thread_update", call, false, None, window, cx);
    }

    /// A menu's box for the side composer: a click outside closes it.
    fn side_menu(&self, id: &'static str, width: f32, p: Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        menu_box(id, width, p)
            .text_size(px(13.))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.settings = None;
                cx.notify();
            }))
    }

    /// The model menu: Default, the agent's models, and a model by its identifier.
    pub(super) fn render_model_menu(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.summary.model.as_deref();
        self.side_menu("side-model-menu", 280., p, cx)
            .child(menu_label("Model", p))
            .child(
                menu_row("side-model-default", "Default".into(), None, selected.is_none(), p)
                    .on_click(cx.listener(|this, _, window, cx| this.update_thread(json!({"model":""}), window, cx))),
            )
            .children(self.models.iter().map(|model| {
                let id = model.id.clone();
                menu_row(
                    SharedString::from(format!("side-model-{id}")),
                    model.name.clone().into(),
                    None,
                    selected == Some(id.as_str()),
                    p,
                )
                .on_click(cx.listener(move |this, _, window, cx| this.update_thread(json!({"model":id}), window, cx)))
            }))
            .when(self.models_loading, |menu| {
                menu.child(
                    div()
                        .px(px(10.))
                        .py(px(6.))
                        .text_color(p.muted)
                        .child("Loading models…"),
                )
            })
            .when_some(self.catalog_problem.clone(), |menu, error| {
                menu.child(div().px(px(10.)).py(px(6.)).text_color(p.destructive).child(error))
            })
            .when(self.summary.agent.coding().is_some(), |menu| {
                menu.child(
                    menu_row("retry-side-models", "Refresh models".into(), None, false, p)
                        .on_click(cx.listener(|this, _, window, cx| this.load_models(window, cx))),
                )
            })
            .child(div().h(px(8.)))
            .child(menu_label("Model identifier", p))
            .child(
                h_flex()
                    .px(px(6.))
                    .py(px(4.))
                    .gap(px(6.))
                    .child(code_dock::reason_field(&self.model_input, p))
                    .child(
                        code_dock::answer_button("side-custom-model", "Use", false, !self.sending, p).on_click(
                            cx.listener(|this, _, window, cx| {
                                let model = this.model_input.read(cx).value().trim().to_owned();
                                this.update_thread(json!({"model":model}), window, cx);
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }

    /// Reasoning and permissions, as the session's effort menu.
    pub(super) fn render_effort_menu(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let efforts = self.efforts();
        let mut menu = self.side_menu("side-effort-menu", 300., p, cx);
        if !efforts.is_empty() {
            menu = menu
                .child(menu_label("Reasoning", p))
                .child(
                    menu_row(
                        "side-effort-default",
                        "Default".into(),
                        None,
                        self.summary.effort.is_none(),
                        p,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.update_thread(json!({"effort":""}), window, cx))),
                )
                .children(efforts.into_iter().map(|(effort, label)| {
                    menu_row(
                        SharedString::from(format!("side-effort-{}", effort.as_str())),
                        label.into(),
                        None,
                        self.summary.effort == Some(effort),
                        p,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.update_thread(json!({"effort":effort.as_str()}), window, cx)
                    }))
                }))
                .child(div().h(px(8.)));
        }
        let agent = self.summary.agent;
        let configured = agent.permission_settings().is_some();
        let offered = nexus_workbench::harness::harness(agent).offered_modes();
        menu.child(menu_label("Permissions", p))
            .children(
                MODES
                    .iter()
                    .filter(|(mode, ..)| !configured || offered.contains(mode))
                    .map(|&(mode, ..)| {
                        let (label, hint) = code_dock::permission_copy(agent, mode);
                        menu_row(
                            SharedString::from(format!("side-mode-{}", mode.as_str())),
                            label.into(),
                            Some(hint.into()),
                            self.summary.permission_mode == mode,
                            p,
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.update_thread(json!({"permissionMode":mode.as_str()}), window, cx)
                        }))
                    }),
            )
            .into_any_element()
    }

    /// The queued messages over the composer, as the session's Queued chips: Send now, rewrite, or take back.
    pub(super) fn render_queued(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let queued: Vec<(i64, &str)> = self
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Queued { seq, text } => Some((*seq, text.as_str())),
                _ => None,
            })
            .collect();
        if queued.is_empty() {
            return None;
        }
        let rows = queued.iter().enumerate().map(|(ix, &(seq, text))| {
            if let Some(edit) = self.rewrite.as_ref().filter(|edit| edit.seq == seq) {
                return h_flex()
                    .id(("side-queued", ix))
                    .w_full()
                    .min_h(px(34.))
                    .px(px(10.))
                    .gap(px(8.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(p.faint)
                    .bg(p.raised)
                    .text_size(px(13.))
                    // Nothing in the queue goes while the rewrite is open (`queue_hold.rs`).
                    .child(
                        div()
                            .id(("side-queued-paused", ix))
                            .text_color(p.faint)
                            .child("Paused")
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "The queue waits while you edit. Save or Cancel lets it go on.",
                                )
                                .build(window, cx)
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(Textarea::new(&edit.input).appearance(false).bordered(false)),
                    )
                    .child(
                        code_dock::text_button("cancel-side-rewrite", "Cancel", false, p).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.rewrite = None;
                                this.input.focus_handle(cx).focus(window, cx);
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        code_dock::text_button("save-side-rewrite", "Save", true, p)
                            .on_click(cx.listener(|this, _, window, cx| this.save_rewrite(window, cx))),
                    )
                    .into_any_element();
            }
            let edit = text.to_string();
            let indent = queued_row::chip_indent(false);
            let open = self.history.queued.is("side", seq as u64);
            let head = queued_row::in_frame(code_dock::queued_chip(("side-queued", ix), ix, text, p))
                .when(self.sending, |chip| chip.opacity(0.5))
                .child(
                    code_dock::text_button(SharedString::from(format!("side-steer-{seq}")), "Send now", false, p)
                        .debug_selector(move || format!("side-steer-{seq}"))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Send now: the agent reads it at its next step")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.command(
                                "wb_queue_steer",
                                json!({"threadId":this.summary.id,"seq":seq}),
                                false,
                                None,
                                window,
                                cx,
                            )
                        })),
                )
                .child(
                    code_dock::icon_button(SharedString::from(format!("side-edit-{seq}")), Lucide::Pencil, p).on_click(
                        cx.listener(move |this, _, window, cx| this.edit_queued(seq, edit.clone(), window, cx)),
                    ),
                )
                .child(
                    code_dock::icon_button(SharedString::from(format!("side-cancel-{seq}")), Lucide::X, p).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.command(
                                "wb_queue_cancel",
                                json!({"threadId":this.summary.id,"seq":seq}),
                                false,
                                None,
                                window,
                                cx,
                            );
                            this.input.focus_handle(cx).focus(window, cx);
                        }),
                    ),
                );
            // Under the pointer the chip opens to its whole message, as a stacked reply does (`queued_row.rs`).
            queued_row::frame(("side-queued-frame", seq as u64), p.raised, p)
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if this.history.queued.set("side", seq as u64, *hovered) {
                        cx.notify();
                    }
                }))
                .child(head)
                .child(queued_row::body(
                    ("side-queued-open", seq as u64),
                    open,
                    text,
                    indent,
                    p,
                ))
                .into_any_element()
        });
        Some(
            v_flex()
                .w_full()
                .gap(px(4.))
                .child(code_dock::queued_header(
                    queued.len(),
                    "sent in order when this turn ends",
                    p,
                ))
                .children(rows)
                .into_any_element(),
        )
    }
}

/// The levels the side composer offers `agent`: the model's `reported` ones, else the agent's usual ones.
fn side_efforts(agent: SessionAgent, reported: Option<&[Effort]>) -> Vec<(Effort, &'static str)> {
    let usual = nexus_workbench::harness::harness(agent).usual_efforts();
    code_dock::EFFORTS
        .iter()
        .copied()
        .filter(|(effort, _)| reported.unwrap_or(usual).contains(effort))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{catalog, kept_rewrite, side_efforts};
    use nexus_workbench::event::{Effort, SessionAgent};
    use serde_json::json;

    /// With the model's list unknown, each agent gets its usual levels; a reported list wins.
    #[test]
    fn side_efforts_follow_the_agents_usual_levels() {
        let levels = |agent| -> Vec<Effort> {
            side_efforts(agent, None)
                .into_iter()
                .map(|(effort, _)| effort)
                .collect()
        };
        use Effort::{High, Low, Max, Medium, Ultra, Xhigh};
        assert_eq!(levels(SessionAgent::Claude), [Low, Medium, High, Xhigh, Max]);
        assert_eq!(levels(SessionAgent::Codex), [Low, Medium, High, Xhigh, Max, Ultra]);
        assert_eq!(levels(SessionAgent::Antigravity), [Low, Medium, High]);
        assert!(levels(SessionAgent::Nexus).is_empty());
        assert!(levels(SessionAgent::Cursor).is_empty());
        assert_eq!(side_efforts(SessionAgent::Nexus, Some(&[High])), [(High, "High")]);
    }
    #[test]
    fn a_consumed_queue_keeps_new_edits_without_replacing_the_composer() {
        assert_eq!(kept_rewrite("next", "old", "rewritten"), Some("next\nrewritten".into()));
        assert_eq!(kept_rewrite("", "old", "rewritten"), Some("rewritten".into()));
        assert_eq!(kept_rewrite("next", "old", "old"), None);
        assert_eq!(kept_rewrite("next", "old", "  "), None);
    }
    #[test]
    fn catalog_stays_with_its_agent_and_keeps_reported_efforts() {
        let rows = catalog(
            &json!([{"agent":"claude","models":[{"id":"c","name":"Claude","efforts":["high"]}]},{"agent":"codex","models":[{"id":"g","name":"Codex","efforts":["ultra"]}]}]),
            SessionAgent::Codex,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "g");
        assert_eq!(rows[0].efforts, vec![Effort::Ultra]);
        assert!(catalog(&json!({}), SessionAgent::Claude).is_empty());
    }
}
