//! The Assistant's composer pieces (brand v5): the Auto chip with its model picker, the `@` context
//! menu, the `/` skills menu and the chips they leave in the box, as Nexus UI's `agent-composer.tsx` draws them. The
//! reads, the menus' rules and the words are `nexus_desktop_core::agent_composer`'s.
//!
//! Menus open upward from the composer and take ↑ ↓ Enter Tab and Esc, read before the text field's own keys
//! (`AgentPage::new`'s keystroke hook). Enter still sends when no menu is open. Each read is quiet: while the agent API
//! has no route for it, or on any failure, the chip is not drawn and `@` lists nothing. The chat still sends.

use crate::workspace::FloatingGround;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::AgentPage;
use crate::agent::Call;
use crate::pages::assistant_views::call;
use crate::palette::Palette;
use nexus_desktop_core::agent_composer::{
    self as words, ContextItem, MAX_CONTEXT, ModelView, Read, Skill, Trigger, Wire,
};

/// One row of an open menu.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub hint: Option<String>,
    pub group: Option<&'static str>,
    pub pick: Pick,
}

/// What a row picks.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Skill(Skill),
    Context(ContextItem),
}

/// The open `@` or `/` menu.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub trigger: Trigger,
    pub rows: Vec<Row>,
    pub active: usize,
}

/// The rows the trigger's menu shows. `@` leaves out what is picked already and shows each kind under its heading.
pub fn rows(trigger: &Trigger, items: &[ContextItem], picks: &[ContextItem]) -> Vec<Row> {
    if trigger.skill {
        return words::filter_skills(&trigger.query)
            .into_iter()
            .map(|skill| Row {
                label: format!("/{}", skill.id),
                hint: Some(skill.hint.into()),
                group: None,
                pick: Pick::Skill(skill),
            })
            .collect();
    }
    let open: Vec<ContextItem> = items.iter().filter(|item| !picks.contains(item)).cloned().collect();
    words::filter_context(&open, &trigger.query)
        .into_iter()
        .map(|item| Row {
            label: item.label.clone(),
            hint: item.detail.clone(),
            group: Some(item.kind.group()),
            pick: Pick::Context(item),
        })
        .collect()
}

/// What the box holds beside its words, and the reads behind the menus.
#[derive(Default)]
pub struct ComposerBox {
    /// The saved model and the models to pick from; None while unread, off or failed (no chip).
    pub model: Option<ModelView>,
    pub model_open: bool,
    saving: bool,
    /// What `@` may pick, read each time `@` opens.
    pub items: Vec<ContextItem>,
    pub picks: Vec<ContextItem>,
    pub skill: Option<Skill>,
    pub menu: Option<Menu>,
    /// The trigger the person closed with Esc, by where its sign is.
    dismissed: Option<usize>,
    _model: Option<Task<()>>,
    _context: Option<Task<()>>,
    _save: Option<Task<()>>,
}

impl ComposerBox {
    /// The send's wire from the box's text, with the line the thread shows; None when there is nothing to send.
    pub fn wire(&self, text: &str) -> Option<(String, Wire)> {
        let wire = words::wire_of(text, &self.picks, self.skill)?;
        Some((words::display_of(text, &self.picks, self.skill), wire))
    }

    /// After a send: the chips go with the words.
    pub fn clear(&mut self) {
        self.picks.clear();
        self.skill = None;
        self.menu = None;
        self.dismissed = None;
    }

    fn has_chips(&self) -> bool {
        self.skill.is_some() || !self.picks.is_empty()
    }
}

impl AgentPage {
    /// Reads the saved model once the page opens: its chip shows Auto or the saved model.
    pub(super) fn load_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.boxed._model = Some(call(&self.ctx, Call::Model, window, cx, |page, reply, _, _| {
            if let Read::Ok(view) = words::model_read(reply.status, reply.body.as_ref()) {
                page.boxed.model = Some(view);
            }
        }));
    }

    /// Saves a model from the picker; the chip shows it once saved.
    fn choose_model(&mut self, model: &str, window: &mut Window, cx: &mut Context<Self>) {
        let same = self.boxed.model.as_ref().is_some_and(|view| view.model == model);
        if same || self.boxed.saving {
            self.boxed.model_open = false;
            return cx.notify();
        }
        let Some(save) = Call::save_model(model) else {
            return;
        };
        self.boxed.saving = true;
        self.boxed._save = Some(call(&self.ctx, save, window, cx, |page, reply, _, _| {
            page.boxed.saving = false;
            if let Read::Ok(view) = words::model_read(reply.status, reply.body.as_ref()) {
                page.boxed.model = Some(view);
                page.boxed.model_open = false;
            }
        }));
        cx.notify();
    }

    /// The text changed: open, filter or close the menu the caret is in. `@` reads what it may pick each time it opens.
    pub(super) fn composer_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (text, caret) = {
            let state = self.composer.read(cx);
            (state.value().to_string(), state.cursor())
        };
        let found = words::trigger(&text, caret).filter(|found| {
            if found.skill {
                self.boxed.skill.is_none()
            } else {
                self.boxed.picks.len() < MAX_CONTEXT
            }
        });
        // Closed with Esc, and the sign gone: the next one opens its menu again.
        if found.is_none() {
            self.boxed.dismissed = None;
        }
        let found = found.filter(|found| Some(found.start) != self.boxed.dismissed);
        let opened = found
            .as_ref()
            .is_some_and(|found| !found.skill && self.boxed.menu.as_ref().is_none_or(|menu| menu.trigger.skill));
        self.boxed.menu = found.map(|trigger| {
            let rows = rows(&trigger, &self.boxed.items, &self.boxed.picks);
            let active = self
                .boxed
                .menu
                .as_ref()
                .filter(|menu| menu.trigger.query == trigger.query && menu.trigger.skill == trigger.skill)
                .map_or(0, |menu| menu.active.min(rows.len().saturating_sub(1)));
            Menu { trigger, rows, active }
        });
        if opened {
            self.boxed._context = Some(call(&self.ctx, Call::Context, window, cx, |page, reply, window, cx| {
                page.boxed.items = match words::context_read(reply.status, reply.body.as_ref()) {
                    Read::Ok(view) => view.items,
                    _ => Vec::new(),
                };
                page.composer_changed(window, cx);
            }));
        }
        cx.notify();
    }

    /// A menu is open and has rows.
    pub(super) fn menu_open(&self) -> bool {
        self.boxed.menu.as_ref().is_some_and(|menu| !menu.rows.is_empty())
    }

    /// Whether a key belongs to the composer's menus or chips now: ↑ ↓ Enter Tab and Esc in an open menu, Esc in the
    /// model picker, and Backspace in an empty box with a chip.
    pub(super) fn composer_wants(&self, key: &str, empty: bool) -> bool {
        match key {
            "up" | "down" | "enter" | "tab" => self.menu_open(),
            "escape" => self.boxed.menu.is_some() || self.boxed.model_open,
            "backspace" => empty && self.boxed.has_chips() && self.boxed.menu.is_none(),
            _ => false,
        }
    }

    /// Acts on a key `composer_wants` accepted.
    pub(super) fn composer_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        match key {
            "up" | "down" => {
                if let Some(menu) = &mut self.boxed.menu {
                    menu.active = words::move_active(menu.active, menu.rows.len(), key == "down");
                }
            }
            "enter" | "tab" => {
                let active = self.boxed.menu.as_ref().map_or(0, |menu| menu.active);
                self.pick_row(active, window, cx);
            }
            "escape" => {
                if self.boxed.model_open {
                    self.boxed.model_open = false;
                } else if let Some(menu) = self.boxed.menu.take() {
                    self.boxed.dismissed = Some(menu.trigger.start);
                }
            }
            "backspace" => {
                if self.boxed.picks.pop().is_none() {
                    self.boxed.skill = None;
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// Picks a row: its sign and typed word leave the text, and a chip takes their place.
    fn pick_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.boxed.menu.take() else {
            return;
        };
        let Some(row) = menu.rows.get(index) else {
            return;
        };
        let (text, caret) = {
            let state = self.composer.read(cx);
            (state.value().to_string(), state.cursor())
        };
        let (next, at) = words::without_trigger(&text, &menu.trigger, caret);
        match &row.pick {
            Pick::Skill(skill) => self.boxed.skill = Some(*skill),
            Pick::Context(item) if self.boxed.picks.len() < MAX_CONTEXT => self.boxed.picks.push(item.clone()),
            Pick::Context(_) => {}
        }
        self.boxed.dismissed = None;
        self.composer_empty = next.trim().is_empty();
        self.put_text(&next, at, window, cx);
        cx.notify();
    }

    /// Sets the field's text with the caret at byte `at`: `set_value` alone leaves a multi-line field's caret at 0.
    fn put_text(&mut self, text: &str, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let prefix = &text[..at.min(text.len())];
        let position = gpui_kit::component::input::Position {
            line: prefix.bytes().filter(|byte| *byte == b'\n').count() as u32,
            character: prefix.rsplit('\n').next().unwrap_or("").encode_utf16().count() as u32,
        };
        self.composer.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx);
            state.set_cursor_position(position, window, cx);
            state.focus(window, cx);
        });
    }

    /// The open menu, above the composer: a listbox, the active row marked.
    pub(super) fn composer_menu(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.boxed.menu.as_ref().filter(|menu| !menu.rows.is_empty())?;
        let mut list = v_flex()
            .id("agent-composer-menu")
            .gap(px(1.))
            .max_h(px(280.))
            .overflow_y_scroll();
        let mut group = None;
        for (index, row) in menu.rows.iter().enumerate() {
            if row.group.is_some() && row.group != group {
                list = list.child(
                    div()
                        .px(px(8.))
                        .pt(px(6.))
                        .pb(px(2.))
                        .text_size(px(11.))
                        .text_color(super::quiet_ink(palette))
                        .child(row.group.unwrap_or_default()),
                );
            }
            group = row.group;
            list = list.child(menu_row(index, row, index == menu.active, palette, cx));
        }
        Some(
            popover("agent-menu-pop", list, palette)
                .left_0()
                .right_0()
                .into_any_element(),
        )
    }

    /// The skill and picks as chips above the field, each with a remove button.
    pub(super) fn composer_chips(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        if !self.boxed.has_chips() {
            return None;
        }
        let mut chips = h_flex().flex_wrap().gap(px(6.));
        if let Some(skill) = self.boxed.skill {
            chips = chips.child(chip(
                "agent-chip-skill".into(),
                format!("/{}", skill.id),
                palette,
                cx.listener(|this, _, _, cx| {
                    this.boxed.skill = None;
                    cx.notify();
                }),
            ));
        }
        for (index, pick) in self.boxed.picks.iter().enumerate() {
            chips = chips.child(chip(
                ElementId::NamedInteger("agent-chip".into(), index as u64),
                format!("@{}", pick.label),
                palette,
                cx.listener(move |this, _, _, cx| {
                    if index < this.boxed.picks.len() {
                        this.boxed.picks.remove(index);
                    }
                    cx.notify();
                }),
            ));
        }
        Some(chips)
    }

    /// The Auto chip and its picker; None while the model is unread, off or failed.
    pub(super) fn model_chip(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.boxed.model.as_ref()?;
        let open = self.boxed.model_open;
        let chip = h_flex()
            .id("agent-model")
            .h(px(26.))
            .pl(px(10.))
            .pr(px(6.))
            .gap(px(4.))
            .rounded_full()
            .border_1()
            .border_color(palette.border)
            .text_size(px(12.5))
            .text_color(if open { palette.strong } else { palette.muted })
            .cursor_pointer()
            .when(open, |chip| chip.bg(palette.hover))
            .hover(|style| style.bg(palette.hover))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Model").build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| {
                this.boxed.model_open = !this.boxed.model_open;
                cx.notify();
            }))
            .child(words::chip_label(&view.model, &view.models))
            .child(
                gpui_kit::component::Icon::new(gpui_kit::assets::IconName::ChevronDown)
                    .size(px(12.))
                    .text_color(palette.muted),
            );
        let mut picker = v_flex()
            .id("agent-model-menu")
            .w(px(260.))
            .max_h(px(320.))
            .overflow_y_scroll();
        for (index, (id, label, hint)) in words::MODEL_PICKS.iter().enumerate() {
            picker = picker.child(model_row(index, id, label, Some(hint), &view.model, palette, cx));
        }
        let mut makers: Vec<&str> = Vec::new();
        for option in &view.models {
            if !makers.contains(&option.maker.as_str()) {
                makers.push(&option.maker);
            }
        }
        let mut slot = words::MODEL_PICKS.len();
        for maker in makers {
            picker = picker.child(
                div()
                    .px(px(8.))
                    .pt(px(6.))
                    .pb(px(2.))
                    .text_size(px(11.))
                    .text_color(super::quiet_ink(palette))
                    .child(maker.to_string()),
            );
            for option in view.models.iter().filter(|option| option.maker == maker) {
                slot += 1;
                picker = picker.child(model_row(
                    slot,
                    &option.id,
                    &option.name,
                    None,
                    &view.model,
                    palette,
                    cx,
                ));
            }
        }
        if let Some(error) = &view.models_error {
            picker = picker.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(11.5))
                    .text_color(super::quiet_ink(palette))
                    .child(error.clone()),
            );
        }
        Some(
            div()
                .relative()
                .child(chip)
                .when(open, |host| {
                    host.child(popover("agent-model-pop", picker, palette).left_0())
                })
                .into_any_element(),
        )
    }
}

/// A raised menu box above its parent, 8 px clear of it.
fn popover(id: &'static str, content: impl IntoElement, palette: Palette) -> Stateful<Div> {
    div().id(id).absolute().bottom(relative(1.)).pb(px(8.)).occlude().child(
        div()
            .p(px(6.))
            .rounded(px(12.))
            .floating_ground(12.0, palette.menu, true, &palette)
            .border_1()
            .border_color(palette.border)
            .child(content),
    )
}

fn menu_row(index: usize, row: &Row, active: bool, palette: Palette, cx: &mut Context<AgentPage>) -> Stateful<Div> {
    h_flex()
        .id(ElementId::NamedInteger("agent-menu-row".into(), index as u64))
        .px(px(8.))
        .py(px(7.))
        .gap(px(12.))
        .justify_between()
        .rounded(px(8.))
        .text_size(px(13.))
        .text_color(palette.foreground)
        .cursor_pointer()
        .when(active, |row| row.bg(palette.menu_hover))
        .hover(|style| style.bg(palette.menu_hover))
        .on_click(cx.listener(move |this, _, window, cx| this.pick_row(index, window, cx)))
        .child(
            div()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(row.label.clone()),
        )
        .when_some(row.hint.clone(), |line, hint| {
            line.child(
                div()
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(super::quiet_ink(palette))
                    .child(hint),
            )
        })
}

fn model_row(
    slot: usize,
    id: &str,
    label: &str,
    hint: Option<&&str>,
    saved: &str,
    palette: Palette,
    cx: &mut Context<AgentPage>,
) -> Stateful<Div> {
    let picked = id == saved;
    let id = id.to_string();
    h_flex()
        .id(ElementId::NamedInteger("agent-model-row".into(), slot as u64))
        .px(px(8.))
        .py(px(7.))
        .gap(px(12.))
        .justify_between()
        .rounded(px(8.))
        .text_size(px(13.))
        .text_color(palette.foreground)
        .cursor_pointer()
        .hover(|style| style.bg(palette.menu_hover))
        .on_click(cx.listener(move |this, _, window, cx| this.choose_model(&id, window, cx)))
        .child(div().child(if picked {
            format!("{label} ·")
        } else {
            label.to_string()
        }))
        .when_some(hint.map(|hint| hint.to_string()), |line, hint| {
            line.child(
                div()
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(super::quiet_ink(palette))
                    .child(hint),
            )
        })
}

/// A chip: the word in a pill with a remove mark.
fn chip(
    id: ElementId,
    label: String,
    palette: Palette,
    remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(24.))
        .max_w_full()
        .pl(px(9.))
        .pr(px(4.))
        .gap(px(4.))
        .rounded_full()
        .bg(palette.selected)
        .text_size(px(12.5))
        .text_color(palette.foreground)
        .child(div().min_w(px(0.)).overflow_hidden().whitespace_nowrap().child(label))
        .child(
            div()
                .id("agent-chip-remove")
                .size(px(18.))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .on_click(remove)
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::X)
                        .size(px(10.))
                        .text_color(palette.icon),
                ),
        )
}

/// What the workspace's tests read and press (`workspace/assistant_thread_tests.rs`).
#[cfg(test)]
impl AgentPage {
    /// The Auto chip's word, or None while there is no chip.
    pub fn chip_label(&self) -> Option<String> {
        let view = self.boxed.model.as_ref()?;
        Some(words::chip_label(&view.model, &view.models))
    }

    /// The open menu's rows, by label.
    pub fn menu_labels(&self) -> Vec<String> {
        self.boxed
            .menu
            .as_ref()
            .map(|menu| menu.rows.iter().map(|row| row.label.clone()).collect())
            .unwrap_or_default()
    }

    /// The chips in the box: the skill, then the picks.
    pub fn chips(&self) -> Vec<String> {
        self.boxed
            .skill
            .iter()
            .map(|skill| format!("/{}", skill.id))
            .chain(self.boxed.picks.iter().map(|pick| format!("@{}", pick.label)))
            .collect()
    }

    /// Types `text` into the box, as the person does: the field's Change follows, which `set_value` does not send.
    pub fn type_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.composer_empty = text.trim().is_empty();
        self.put_text(text, text.len(), window, cx);
        self.composer_changed(window, cx);
    }

    /// The box's words.
    pub fn typed(&self, cx: &App) -> String {
        self.composer.read(cx).value().to_string()
    }

    /// Presses a key the menus or chips take; false when they do not take it (the text field would).
    pub fn press(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let wanted = self.composer_wants(key, self.composer_empty);
        if wanted {
            self.composer_key(key, window, cx);
        }
        wanted
    }

    /// Enter with no menu: send.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.send(window, cx);
    }

    /// Picks a model from the picker.
    pub fn pick_model(&mut self, model: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_model(model, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{ComposerBox, rows};
    use nexus_desktop_core::agent_composer::{self as words, ContextItem, ContextKind, trigger};

    fn item(kind: ContextKind, id: &str, label: &str) -> ContextItem {
        ContextItem {
            kind,
            id: id.into(),
            label: label.into(),
            detail: None,
        }
    }

    #[test]
    fn lists_skills_for_a_slash_and_groups_context_for_an_at() {
        let skills = rows(&trigger("/re", 3).unwrap(), &[], &[]);
        assert_eq!(
            skills.iter().map(|row| row.label.as_str()).collect::<Vec<_>>(),
            vec!["/remember"]
        );
        let items = vec![
            item(ContextKind::Connector, "gmail", "Gmail"),
            item(ContextKind::Connector, "linear", "Linear"),
            item(ContextKind::Errand, "i6", "Vendor contracts"),
        ];
        let picked = vec![items[1].clone()];
        let at = rows(&trigger("@", 1).unwrap(), &items, &picked);
        assert_eq!(
            at.iter().map(|row| row.label.as_str()).collect::<Vec<_>>(),
            vec!["Gmail", "Vendor contracts"]
        );
        assert_eq!(
            at.iter().map(|row| row.group).collect::<Vec<_>>(),
            vec![Some("Accounts"), Some("Errands")]
        );
        assert_eq!(rows(&trigger("@ve", 3).unwrap(), &items, &[]).len(), 1);
    }

    #[test]
    fn the_box_sends_its_chips_with_the_words_and_forgets_them_after() {
        let mut boxed = ComposerBox::default();
        boxed.picks.push(item(ContextKind::Connector, "gmail", "Gmail"));
        let (shown, wire) = boxed.wire(" plan it ").unwrap();
        assert_eq!(shown, "@Gmail plan it");
        assert_eq!(wire.message, "plan it");
        assert_eq!(wire.context, vec![(ContextKind::Connector, "gmail".to_string())]);
        boxed.skill = Some(words::SKILLS[5]);
        assert!(boxed.wire("").is_none(), "web needs words");
        boxed.clear();
        assert!(boxed.wire("").is_none() && !boxed.has_chips());
    }
}
