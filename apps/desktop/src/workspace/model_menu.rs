//! The model menu, as the Nexus app's (`ui/src/components/nexus-ui/model-menu.tsx`, `model-menu.css`, v5's
//! `find-artifacts.css`): a rail of makers on the left; on the right a search, Auto, the maker's current models, and
//! its older ones folded under "Older models"; a line on what the policy turns off at the foot. Typing searches every
//! maker. Up and Down move, Enter picks, Esc closes.

use crate::workspace::menu_item_hover;
use crate::workspace::overlay::OverlaySlot;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use crate::model::maker_name;
use crate::palette::Palette;

pub(super) const WIDTH: f32 = 320.;
/// The menu's body (rail and list), between these: the Nexus app's popover is at most 520px with its foot.
const MIN_HEIGHT: f32 = 300.;
const MAX_HEIGHT: f32 = 460.;

/// One row of the menu's list, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum MenuRow {
    Auto,
    /// A model, by its index in `models`.
    Model(usize),
    /// The fold over the maker's older models, with how many.
    Older(usize),
}

impl Workspace {
    /// Leave global search when a rail section is selected, and show its list from the top.
    pub(super) fn reset_model_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        super::picker_navigation::clear_search(&mut self.model_query, &self.model_search, window, cx);
        self.model_cursor = 0;
        self.model_cursor_on_open = false;
        self.model_visible_rows.clear();
        self.model_scroll.set_offset(point(px(0.), px(0.)));
        self.reveal_model_cursor();
    }

    pub(super) fn reveal_model_cursor(&self) {
        self.model_scroll.scroll_to_item(self.model_cursor);
    }

    /// Keep the active identity through asynchronous catalog changes and Favorites edits.
    pub(super) fn sync_model_rows(&mut self) {
        let keys = if self.overlay.model_menu() {
            self.menu_rows()
                .iter()
                .map(|row| match row {
                    MenuRow::Auto => "chat:auto".into(),
                    MenuRow::Model(ix) => format!("chat:{}", self.models[*ix].id),
                    MenuRow::Older(_) => format!("chat:older:{}", self.menu_maker().unwrap_or_default()),
                })
                .collect::<Vec<_>>()
        } else if self.overlay.launch_menu() == Some(super::new_code::LaunchMenu::Model)
            || self.dock_menu_is(super::code_dock::Menu::Model)
        {
            self.launch_rows()
                .iter()
                .map(|row| {
                    format!(
                        "code:{}:{}",
                        row.agent.as_str(),
                        row.model.as_deref().unwrap_or("<default>")
                    )
                })
                .chain(self.missing_panel_rows().into_iter().map(|row| row.key()))
                .collect()
        } else {
            self.model_visible_rows.clear();
            return;
        };
        if keys != self.model_visible_rows {
            self.model_cursor = super::picker_navigation::reconcile(self.model_cursor, &self.model_visible_rows, &keys);
            if self.model_cursor_on_open && !keys.is_empty() {
                let picked = if self.overlay.model_menu() {
                    self.model
                        .as_ref()
                        .map_or_else(|| "chat:auto".into(), |model| format!("chat:{}", model.id))
                } else {
                    let (agent, model) = self
                        .session_pick()
                        .unwrap_or_else(|| (self.code_form.agent, self.code_form.model.clone()));
                    let model = self.listed_pick(agent, model.as_deref());
                    format!("code:{}:{}", agent.as_str(), model.as_deref().unwrap_or("<default>"))
                };
                self.model_cursor = keys.iter().position(|key| key == &picked).unwrap_or(0);
                self.model_cursor_on_open = false;
            }
            self.model_visible_rows = keys;
            self.reveal_model_cursor();
        }
    }

    /// The makers on the rail, in the catalog's order.
    pub(super) fn menu_makers(&self) -> Vec<String> {
        super::chat_picker::makers(&self.models)
    }

    pub(super) fn menu_maker(&self) -> Option<String> {
        super::chat_picker::maker(
            &self.models,
            self.model_maker.as_deref(),
            self.model.as_ref().map(|model| model.id.as_str()),
        )
    }

    pub(super) fn menu_rows(&self) -> Vec<MenuRow> {
        super::chat_picker::rows(
            &self.models,
            &self.model_query,
            self.menu_maker().as_deref(),
            self.model.as_ref().map(|model| model.id.as_str()),
            self.model_older,
        )
    }

    /// A click keeps its model identity even if the catalog reordered since this row was drawn.
    fn pick_menu_model(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.overlay.model_menu()
            || !self
                .menu_rows()
                .iter()
                .any(|row| matches!(row, MenuRow::Model(ix) if self.models[*ix].id == id))
        {
            return;
        }
        if let Some(model) = super::chat_picker::pickable(&self.models, id).cloned() {
            self.pick_model(Some(model), window, cx);
        }
    }

    /// Pick the row under the cursor (Enter): a model, Auto, or the older fold.
    pub(super) fn pick_menu_row(&mut self, row: MenuRow, window: &mut Window, cx: &mut Context<Self>) {
        if !self.overlay.model_menu() || !self.menu_rows().contains(&row) {
            return;
        }
        match row {
            MenuRow::Auto => self.pick_model(None, window, cx),
            MenuRow::Model(ix) => {
                if let Some(id) = self.models.get(ix).map(|model| model.id.clone()) {
                    self.pick_menu_model(&id, window, cx);
                }
            }
            MenuRow::Older(_) => {
                self.model_older = !self.model_older;
                self.sync_model_rows();
                self.reveal_model_cursor();
                cx.notify();
            }
        }
    }

    pub(super) fn render_model_menu(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.menu_rows();
        let searching = !self.model_query.trim().is_empty();
        let maker = self.menu_maker();
        let picked = self.model.as_ref().map(|model| model.id.clone());
        // While the free start pays: which models it pays for, as Nexus UI's menu and the phone's sheet say.
        let free_start = self.free_start_cap();
        let free_hint = |model: &crate::model::ModelChoice| -> Option<SharedString> {
            let free = free_start.is_some_and(|cap| model.usable && model.free_start_pays(cap));
            match (free, model.hint.clone()) {
                (true, Some(hint)) => Some(format!("Free start · {hint}").into()),
                (true, None) => Some("Free start".into()),
                (false, hint) => hint.map(Into::into),
            }
        };
        let rail = self.menu_makers().into_iter().map(|id| {
            let selected = !searching && Some(&id) == maker.as_ref();
            let clicked = id.clone();
            rail_button(
                SharedString::from(format!("maker-{id}")),
                maker_name(&id).into(),
                selected,
                maker_mark(&id, 15., palette),
                palette,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if !this.menu_makers().contains(&clicked) {
                    return;
                }
                this.model_maker = Some(clicked.clone());
                this.model_older = false;
                this.reset_model_search(window, cx);
                cx.notify();
            }))
        });
        let mut list = v_flex().p(px(4.)).gap(px(1.));
        if rows.is_empty() {
            list = list.child(
                div()
                    .py(px(28.))
                    .text_center()
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .child("No model matches."),
            );
        }
        for (ix, row) in rows.iter().copied().enumerate() {
            let cursor = ix == self.model_cursor;
            let element = match row {
                MenuRow::Auto => model_row(
                    "Auto".into(),
                    Some(
                        if free_start.is_some() {
                            "Free start · Nexus picks the model"
                        } else {
                            "Nexus picks the model"
                        }
                        .into(),
                    ),
                    picked.is_none(),
                    true,
                    cursor,
                    palette,
                ),
                MenuRow::Model(model_ix) => {
                    let model = &self.models[model_ix];
                    model_row(
                        model.name.clone().into(),
                        free_hint(model),
                        picked.as_deref() == Some(model.id.as_str()),
                        model.usable,
                        cursor,
                        palette,
                    )
                }
                MenuRow::Older(count) => h_flex()
                    .min_h(px(32.))
                    .px(px(8.))
                    .py(px(6.))
                    .gap(px(8.))
                    .rounded(px(8.))
                    .text_size(px(14.))
                    .text_color(palette.foreground)
                    .when(cursor, |row| row.bg(palette.menu_hover))
                    .child(div().child("Older models"))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(count.to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        Icon::new(Lucide::ChevronRight)
                            .size(px(15.))
                            .text_color(palette.faint)
                            .when(self.model_older, |icon| {
                                icon.rotate(radians(std::f32::consts::FRAC_PI_2))
                            }),
                    ),
            };
            let model_id = match row {
                MenuRow::Model(model_ix) => Some(self.models[model_ix].id.clone()),
                _ => None,
            };
            list = list.child(
                div()
                    .id(("menu-row", ix))
                    .flex_none()
                    .when(matches!(row, MenuRow::Older(_)), |row| row.mt(px(12.)))
                    .cursor_pointer()
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.model_cursor != ix {
                            this.model_cursor = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(id) = &model_id {
                            this.pick_menu_model(id, window, cx);
                        } else {
                            this.pick_menu_row(row, window, cx);
                        }
                    }))
                    .child(element),
            );
        }
        let off = self.models.iter().filter(|model| model.off).count();
        // One height whatever the maker or the fold: the rail's, kept between a floor and a ceiling; the list scrolls
        // inside it. Clicking through makers never resizes the menu.
        let makers = self.menu_makers().len() as f32;
        let body = (12. + makers * 32. + (makers - 1.).max(0.) * 2.).clamp(MIN_HEIGHT, MAX_HEIGHT);
        let org = self
            .account
            .as_ref()
            .map(|account| account.org_name.clone())
            .unwrap_or_else(|| "your organization".into());
        let menu = v_flex()
            .id("model-menu")
            .occlude()
            .w(px(WIDTH))
            .rounded(px(12.))
            .bg(palette.menu)
            .shadow(palette.pop_shadow(palette.pop_ring()))
            .overflow_hidden()
            .text_color(palette.foreground)
            .child(
                h_flex()
                    .h(px(body))
                    .items_start()
                    .child(
                        v_flex()
                            .id("model-rail")
                            .overflow_y_scroll()
                            .w(px(46.))
                            .flex_none()
                            .self_stretch()
                            .items_center()
                            .py(px(6.))
                            .gap(px(2.))
                            .children(rail),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .child(
                                h_flex()
                                    .flex_none()
                                    .h(px(38.))
                                    .px(px(12.))
                                    .gap(px(8.))
                                    .child(Icon::new(Lucide::Search).size(px(15.)).text_color(palette.faint))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(14.))
                                            .child(Input::new(&self.model_search).appearance(false)),
                                    ),
                            )
                            .when(free_start.is_some(), |column| {
                                column.child(
                                    div()
                                        .flex_none()
                                        .px(px(12.))
                                        .pb(px(8.))
                                        .text_size(px(12.))
                                        .text_color(palette.faint)
                                        .child(crate::pay_notice::FREE_START_NOTE),
                                )
                            })
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h(px(0.))
                                    .child(
                                        list.id("model-list")
                                            .size_full()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.model_scroll),
                                    )
                                    .child(bottom_fade(palette.menu)),
                            ),
                    ),
            )
            .when(off > 0, |menu| {
                let models = if off == 1 { "model is" } else { "models are" };
                menu.child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .px(px(14.))
                        .py(px(10.))
                        .bg(palette.hover.opacity(0.5))
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(format!("{off} {models} off for {org}."))
                        .child(
                            div()
                                .id("model-policy")
                                .text_color(palette.muted)
                                .cursor_pointer()
                                .hover(|style| style.text_color(palette.foreground))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.overlay.close_model_menu();
                                    this.open_models(window, cx);
                                }))
                                .child("Model policy"),
                        ),
                )
            });
        // Opens rising 4px into place, and leaves sinking back (`crate::motion::presence`, where the chip draws it).
        menu
    }
}

/// The models a picker lists beside its "Default" row: the agent's own, without its default entry and without the
/// model that entry runs, which the "Default" row already names ("Opus 5.5 · Default"). Claude Code reports Opus both
/// as its default and as itself; listing both shows "Opus 5.5" twice.
pub(super) fn listed_models(
    models: &[nexus_workbench::agent_models::Model],
) -> Vec<nexus_workbench::agent_models::Model> {
    // "anthropic/claude-opus-5-5" and "claude-opus-5-5-20261101" are the same model; "claude-opus-5-5[1m]" isn't.
    let raw = |id: &str| crate::code::model_label(id.rsplit('/').next().unwrap_or(id));
    let runs = models
        .iter()
        .find(|model| crate::code::is_default_entry(model))
        .and_then(|model| model.resolved_id.as_deref())
        .map(raw);
    models
        .iter()
        .filter(|model| !crate::code::is_default_entry(model))
        .filter(|model| {
            runs.as_ref().is_none_or(|runs| {
                ![Some(model.id.as_str()), model.resolved_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|id| raw(id) == *runs)
            })
        })
        .cloned()
        .collect()
}

/// A 16px fade into the menu's fill at the foot of a scrolling list, so a row the edge cuts reads as "more below"
/// rather than as clipped.
pub(super) fn bottom_fade(color: Hsla) -> impl IntoElement {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h(px(16.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(color.opacity(0.), 0.),
            linear_color_stop(color, 1.),
        ))
}

/// A button on a menu's rail: a 32px square holding a mark, filled when it is the one shown, named in its tooltip.
pub(super) fn rail_button(
    id: SharedString,
    name: SharedString,
    selected: bool,
    mark: AnyElement,
    palette: Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .cursor_pointer()
        .when(selected, |button| button.bg(palette.selected))
        .when(!selected, |button| {
            button.hover(|style| menu_item_hover(style, palette))
        })
        .tooltip(move |window, cx| Tooltip::new(name.clone()).build(window, cx))
        .child(mark)
}

/// A maker's mark (`ModelLogo`): its icon in its brand colour, or in the strongest ink for the black-and-white marks;
/// a neutral initial chip where the app carries no mark (xAI).
pub(super) fn maker_mark(maker: &str, size: f32, palette: Palette) -> AnyElement {
    // Nexus's mark is the app icon's spiral without its tile, in the theme's ink.
    if maker == "nexus" {
        return svg()
            .path(crate::assets::APP_MARK)
            .size(px(size))
            .flex_none()
            .text_color(palette.foreground)
            .into_any_element();
    }
    match crate::assets::mark(maker) {
        Some(path) => svg()
            .path(path)
            .size(px(size))
            .flex_none()
            .text_color(crate::assets::mark_color(maker).unwrap_or(palette.strong))
            .into_any_element(),
        None => div()
            .size(px(size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.))
            .bg(palette.selected)
            .text_size(px((size * 0.64).round()))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(palette.muted)
            .child(
                maker_name(maker)
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string(),
            )
            .into_any_element(),
    }
}

/// A model row: the name, a quieter hint on the same line, and the check column.
fn model_row(
    name: SharedString,
    hint: Option<SharedString>,
    chosen: bool,
    usable: bool,
    cursor: bool,
    palette: Palette,
) -> Div {
    h_flex()
        .min_h(px(32.))
        .px(px(8.))
        .py(px(6.))
        .gap(px(8.))
        .rounded(px(8.))
        .text_size(px(14.))
        .when(cursor, |row| row.bg(palette.menu_hover))
        .child(
            h_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(8.))
                .items_baseline()
                .child(
                    div()
                        .flex_none()
                        .text_color(if usable { palette.foreground } else { palette.faint })
                        .child(name),
                )
                .when_some(hint, |row, hint| {
                    row.child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(hint),
                    )
                }),
        )
        .child(div().w(px(15.)).flex_none().when(chosen, |cell| {
            cell.child(Icon::new(Lucide::Check).size(px(15.)).text_color(palette.foreground))
        }))
}

#[cfg(test)]
mod tests {
    use super::listed_models;

    #[test]
    fn a_picker_lists_the_default_model_once() {
        let model = |id: &str, name: &str, resolved: Option<&str>| nexus_workbench::agent_models::Model {
            id: id.into(),
            name: name.into(),
            efforts: Vec::new(),
            resolved_id: resolved.map(Into::into),
            fast: false,
        };
        let models = vec![
            model("default", "Default (recommended)", Some("claude-opus-5-5")),
            model("opus", "Opus 5.5", Some("claude-opus-5-5")),
            model("claude-opus-5-5[1m]", "Opus 5.5 (1M)", None),
            model("sonnet", "Sonnet 5.5", Some("claude-sonnet-5-5")),
        ];
        let ids: Vec<String> = listed_models(&models).into_iter().map(|model| model.id).collect();
        assert_eq!(ids, ["claude-opus-5-5[1m]", "sonnet"]);
        // Without a resolved default, nothing but the default entry is dropped.
        let ids: Vec<String> = listed_models(&models[1..]).into_iter().map(|model| model.id).collect();
        assert_eq!(ids.len(), 3);
    }
}
