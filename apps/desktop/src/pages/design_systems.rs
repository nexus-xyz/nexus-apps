//! Organization design systems. Anyone with chat.use reads;
//! org.settings permits choosing the default. Repository sync stays in the CLI.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde::Deserialize;
use serde_json::Value;

use super::{Ctx, Load, Verb, fetch, palette, read, ui};
use crate::palette::Palette;

pub const NATIVE: bool = true;
const SYNC: &str = "nexus design-system sync . --default";

pub fn build(ctx: Ctx, _window: &mut Window, cx: &mut App) -> Option<AnyView> {
    Some(cx.new(|cx| DesignSystemsPage::new(ctx, cx)).into())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct System {
    id: String,
    name: String,
    built_in: bool,
    is_default: bool,
    source: String,
    variable_count: u64,
    component_count: u64,
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Systems {
    design_systems: Vec<System>,
    default_id: String,
}

impl Systems {
    fn selected(&self, chosen: Option<&str>) -> Option<&System> {
        chosen
            .and_then(|id| self.design_systems.iter().find(|system| system.id == id))
            .or_else(|| self.design_systems.iter().find(|system| system.id == self.default_id))
            .or_else(|| self.design_systems.first())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Variable {
    name: String,
    value: String,
    selector: String,
    kind: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Detail {
    id: String,
    name: String,
    variables: Vec<Variable>,
    notes: String,
    #[serde(default)]
    components: Vec<String>,
}

impl Detail {
    fn selectors(&self) -> Vec<String> {
        let mut selectors = Vec::new();
        for variable in &self.variables {
            if !selectors.contains(&variable.selector) {
                selectors.push(variable.selector.clone());
            }
        }
        if let Some(at) = selectors.iter().position(|selector| selector == ":root") {
            let root = selectors.remove(at);
            selectors.insert(0, root);
        }
        selectors
    }

    fn selector<'a>(&'a self, chosen: &'a str) -> &'a str {
        if self.variables.iter().any(|variable| variable.selector == chosen) {
            chosen
        } else {
            self.variables
                .iter()
                .find(|variable| variable.selector == ":root")
                .or_else(|| self.variables.first())
                .map_or(":root", |variable| variable.selector.as_str())
        }
    }
}

fn selector_label(selector: &str) -> String {
    if selector == ":root" {
        return "Default".into();
    }
    let name = selector
        .split("data-theme=\"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .unwrap_or_else(|| selector.trim_start_matches('.'));
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn can_manage(permissions: &Load<Vec<String>>) -> bool {
    matches!(permissions, Load::Ready(permissions) if permissions.iter().any(|permission| permission == "org.settings"))
}

struct DesignSystemsPage {
    ctx: Ctx,
    permissions: Load<Vec<String>>,
    systems: Load<Systems>,
    chosen: Option<String>,
    detail: Load<Detail>,
    detail_id: Option<String>,
    selector: String,
    busy: Option<String>,
    outcome: Option<Result<String, String>>,
    copied: bool,
    show_other: bool,
    show_notes: bool,
    list_revision: u64,
    detail_revision: u64,
    #[cfg(test)]
    detail_responses: Vec<(u64, String, bool)>,
}

impl DesignSystemsPage {
    fn new(ctx: Ctx, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            ctx,
            permissions: Load::Loading,
            systems: Load::Loading,
            chosen: None,
            detail: Load::Loading,
            detail_id: None,
            selector: ":root".into(),
            busy: None,
            outcome: None,
            copied: false,
            show_other: false,
            show_notes: false,
            list_revision: 0,
            detail_revision: 0,
            #[cfg(test)]
            detail_responses: Vec::new(),
        };
        page.load(cx);
        page
    }

    fn can_manage(&self) -> bool {
        can_manage(&self.permissions)
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.permissions = Load::Loading;
        read(
            cx,
            self.ctx.gateway.query("/v1/tenants/me".into()),
            |this, answer, _| {
                this.permissions = match answer {
                    Ok(answer) => Load::Ready(
                        answer
                            .get("permissions")
                            .and_then(Value::as_array)
                            .map(|permissions| permissions.iter().filter_map(Value::as_str).map(String::from).collect())
                            .unwrap_or_default(),
                    ),
                    Err(error) => Load::Failed(error.into()),
                };
            },
        );
        self.read_systems(cx);
    }

    fn read_systems(&mut self, cx: &mut Context<Self>) {
        self.list_revision += 1;
        let revision = self.list_revision;
        read(
            cx,
            self.ctx.gateway.query("/v1/design-systems".into()),
            move |this, answer, cx| {
                if this.list_revision != revision {
                    return;
                }
                this.systems = match answer {
                    Ok(answer) => serde_json::from_value::<Systems>(answer)
                        .map(Load::Ready)
                        .unwrap_or_else(|_| Load::Failed("Gateway sent design systems this app cannot read.".into())),
                    Err(error) => Load::Failed(error.into()),
                };
                if let Load::Ready(systems) = &this.systems {
                    let selected = systems.selected(this.chosen.as_deref()).map(|system| system.id.clone());
                    if selected != this.detail_id {
                        this.detail_id = selected;
                        this.read_detail(cx);
                    }
                }
            },
        );
    }

    fn pick(&mut self, id: String, cx: &mut Context<Self>) {
        if self.detail_id.as_ref() == Some(&id) {
            return;
        }
        self.chosen = Some(id.clone());
        self.detail_id = Some(id);
        self.read_detail(cx);
        cx.notify();
    }

    fn read_detail(&mut self, cx: &mut Context<Self>) {
        self.detail_revision += 1;
        let revision = self.detail_revision;
        self.detail = Load::Loading;
        self.selector = ":root".into();
        let Some(id) = self.detail_id.clone() else {
            return;
        };
        let path = format!("/v1/design-systems/{}", super::usage::encode(&id));
        read(cx, self.ctx.gateway.query(path), move |this, answer, _| {
            // Tests observe the real GPUI response callback, including replies rejected below.
            #[cfg(test)]
            this.detail_responses.push((revision, id.clone(), answer.is_ok()));
            // Selecting another system while this read is in flight must not replace its tokens.
            if this.detail_revision != revision || this.detail_id.as_ref() != Some(&id) {
                return;
            }
            this.detail = match answer {
                Ok(answer) => match serde_json::from_value::<Detail>(answer) {
                    Ok(detail) if detail.id == id => Load::Ready(detail),
                    _ => Load::Failed("Gateway sent tokens this app cannot read.".into()),
                },
                Err(error) => Load::Failed(error.into()),
            };
        });
    }

    fn make_default(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.can_manage() || self.busy.is_some() {
            return;
        }
        let Load::Ready(systems) = &self.systems else {
            return;
        };
        if !systems
            .design_systems
            .iter()
            .any(|system| system.id == id && !system.is_default)
        {
            return;
        }
        self.busy = Some(id.clone());
        self.outcome = None;
        let path = format!("/v1/design-systems/{}/default", super::usage::encode(&id));
        fetch(
            cx,
            self.ctx.gateway.request(Verb::Post, path, None),
            move |this, answer, cx| {
                this.busy = None;
                this.outcome = Some(match answer {
                    Ok(answer) => match serde_json::from_value::<System>(answer) {
                        Ok(system) if system.id == id && system.is_default => {
                            if let Load::Ready(systems) = &mut this.systems {
                                systems.default_id = id.clone();
                                for system in &mut systems.design_systems {
                                    system.is_default = system.id == id;
                                }
                            }
                            this.read_systems(cx);
                            Ok(format!("New designs use {}.", system.name))
                        }
                        _ => Err("Gateway did not confirm the new default. Try refreshing.".into()),
                    },
                    Err(error) => Err(error),
                });
            },
        );
        cx.notify();
    }

    fn render_tokens(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let detail = match &self.detail {
            Load::Loading => return ui::skeleton(6, palette).into_any_element(),
            Load::Failed(error) => {
                return v_flex()
                    .child(ui::failed(error.clone(), palette))
                    .child(
                        ui::button("design-tokens-retry", "Retry tokens", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.read_detail(cx))),
                    )
                    .into_any_element();
            }
            Load::Ready(detail) => detail,
        };
        let selected = detail.selector(&self.selector);
        let mut body = v_flex()
            .gap(px(8.))
            .child(ui::section_head(format!("Tokens of {}", detail.name), palette));
        let selectors = detail.selectors();
        if selectors.len() > 1 {
            body = body.child(
                h_flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(selectors.iter().enumerate().map(|(at, selector)| {
                        let selector = selector.clone();
                        ui::chip(
                            ("design-theme", at),
                            selector_label(&selector),
                            selector == selected,
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selector = selector.clone();
                            cx.notify();
                        }))
                    })),
            );
        }
        let shown: Vec<_> = detail
            .variables
            .iter()
            .filter(|variable| variable.selector == selected)
            .collect();
        if shown.is_empty() {
            body = body.child(ui::note("No tokens in this theme.", palette));
        }
        for (heading, kinds) in [
            ("Colours", &["color"][..]),
            ("Sizes and spacing", &["length"][..]),
            ("Type and the rest", &["font", "other"][..]),
        ] {
            let variables: Vec<_> = shown
                .iter()
                .filter(|variable| kinds.contains(&variable.kind.as_str()))
                .collect();
            if variables.is_empty() {
                continue;
            }
            if heading == "Type and the rest" {
                body = body.child(
                    ui::button(
                        "design-toggle-other",
                        format!("{} · {}", heading, variables.len()),
                        palette,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_other = !this.show_other;
                        cx.notify();
                    })),
                );
                if !self.show_other {
                    continue;
                }
            } else {
                body = body.child(ui::section_head(heading, palette));
            }
            for (at, variable) in variables.into_iter().enumerate() {
                let mut row = h_flex().gap(px(10.)).items_center().py(px(6.));
                if variable.kind == "color" {
                    if let Ok(color) = Rgba::try_from(variable.value.as_str()) {
                        row = row.child(div().w(px(28.)).h(px(28.)).flex_none().rounded(px(6.)).bg(color));
                    }
                }
                body = body.child(
                    row.child(
                        ui::selectable_note(
                            SharedString::from(format!("design-token-{}-{at}", variable.kind)),
                            format!("{}   {}", variable.name, variable.value),
                            palette,
                        )
                        .font_family(ui::MONO),
                    ),
                );
            }
        }
        if !detail.notes.trim().is_empty() {
            body = body.child(notes_button(palette, true).on_click(cx.listener(|this, _, _, cx| {
                this.show_notes = !this.show_notes;
                cx.notify();
            })));
            if self.show_notes {
                body = body
                    .child(ui::selectable_note("design-notes", detail.notes.clone(), palette).font_family(ui::MONO));
            }
        }
        if !detail.components.is_empty() {
            body = body
                .child(ui::section_head("Components", palette))
                .child(ui::selectable_note(
                    "design-components",
                    detail.components.join(" · "),
                    palette,
                ));
        }
        body.into_any_element()
    }
}

fn notes_button(palette: Palette, enabled: bool) -> Stateful<Div> {
    if enabled {
        ui::button("design-toggle-notes", "Notes for the model", palette)
    } else {
        ui::disabled_button("design-toggle-notes", "Notes for the model", palette)
    }
}

pub(super) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        v_flex().child(notes_button(palette, false)).into_any_element()
    })
}

impl Render for DesignSystemsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let mut body = v_flex().w_full().gap(px(8.)).child(ui::lead(
            "The tokens and notes Nexus Design makes designs with, serves them with and checks them against.",
            palette,
        ));
        let systems = match &self.systems {
            Load::Loading => return body.child(ui::skeleton(3, palette)).into_any_element(),
            Load::Failed(error) => {
                return body
                    .child(ui::failed(error.clone(), palette))
                    .child(
                        ui::button("design-systems-retry", "Retry", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                    )
                    .into_any_element();
            }
            Load::Ready(systems) => systems,
        };
        if systems.design_systems.is_empty() {
            body = body.child(ui::empty(
                "No design systems",
                "Add one from a repository with the command below.",
                palette,
            ));
        }
        for (at, system) in systems.design_systems.iter().enumerate() {
            let id = system.id.clone();
            let mut label = system.name.clone();
            if system.is_default {
                label.push_str(" · Default");
            }
            let source = if system.built_in {
                "Built in"
            } else if system.source.is_empty() {
                "Synced"
            } else {
                &system.source
            };
            let mut description = format!(
                "{} tokens · {} components · {source}",
                system.variable_count, system.component_count
            );
            if let Some(at) = &system.updated_at {
                description.push_str(&format!(" · {}", ui::day(at)));
            }
            let mut actions = h_flex().flex_wrap().gap(px(8.)).child(
                ui::chip(
                    ("design-system", at),
                    label,
                    self.detail_id.as_ref() == Some(&id),
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick(id.clone(), cx))),
            );
            if self.can_manage() && !system.is_default {
                let id = system.id.clone();
                actions = if self.busy.is_some() {
                    actions.child(ui::disabled_button(
                        ("design-default", at),
                        if self.busy.as_ref() == Some(&id) {
                            "Saving…"
                        } else {
                            "Make default"
                        },
                        palette,
                    ))
                } else {
                    actions.child(
                        ui::button(("design-default", at), "Make default", palette)
                            .on_click(cx.listener(move |this, _, _, cx| this.make_default(id.clone(), cx))),
                    )
                };
            }
            body = body.child(
                v_flex()
                    .py(px(8.))
                    .gap(px(4.))
                    .child(actions)
                    .child(ui::note(description, palette)),
            );
        }
        match &self.permissions {
            Load::Ready(_) if !self.can_manage() => {
                body = body.child(ui::note("Only owners and admins can change the default.", palette))
            }
            Load::Failed(error) => {
                body = body.child(ui::error_line(error.clone(), palette)).child(
                    ui::button("design-access-retry", "Retry permissions", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                )
            }
            _ => {}
        }
        if let Some(outcome) = &self.outcome {
            body = body.child(match outcome {
                Ok(message) => ui::note(message.clone(), palette),
                Err(error) => ui::error_line(error.clone(), palette),
            });
        }
        body = body.child(ui::section_head("Add one from a repository", palette))
            .child(ui::note("Run it where the tokens live. It reads CSS variables, a Tailwind config, tokens.json, DESIGN.md and the components folder.", palette))
            .child(ui::selectable_note("design-sync-command", SYNC, palette).font_family(ui::MONO))
            .child(ui::button("design-copy-command", if self.copied { "Copied" } else { "Copy command" }, palette)
                .on_click(cx.listener(|this, _, _, cx| { cx.write_to_clipboard(ClipboardItem::new_string(SYNC.into())); this.copied = true; cx.notify(); })));
        if self.detail_id.is_some() {
            body = body.child(self.render_tokens(palette, cx));
        }
        body.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Detail, Load, Systems, can_manage, selector_label};

    #[test]
    fn design_system_default_changes_require_confirmed_org_settings_permission() {
        assert!(!can_manage(&Load::Loading));
        assert!(!can_manage(&Load::Failed("offline".into())));
        assert!(!can_manage(&Load::Ready(vec!["chat.use".into(), "org.billing".into()])));
        assert!(can_manage(&Load::Ready(vec!["chat.use".into(), "org.settings".into()])));
    }

    #[test]
    fn design_system_selection_survives_refresh_and_falls_back_after_removal() {
        let systems: Systems = serde_json::from_value(serde_json::json!({
            "defaultId": "built-in", "designSystems": [
                {"id":"built-in","name":"Nexus","builtIn":true,"isDefault":true,"source":"","variableCount":4,"componentCount":0},
                {"id":"custom","name":"Acme","builtIn":false,"isDefault":false,"source":"acme/ui","variableCount":3,"componentCount":2}
            ]
        })).unwrap();
        assert_eq!(systems.selected(None).unwrap().id, "built-in");
        assert_eq!(systems.selected(Some("custom")).unwrap().id, "custom");
        assert_eq!(systems.selected(Some("removed")).unwrap().id, "built-in");
        assert!(serde_json::from_value::<Systems>(serde_json::json!({"defaultId":"x"})).is_err());
    }

    #[test]
    fn design_system_themes_preserve_selector_order_and_do_not_mix_tokens() {
        let detail: Detail = serde_json::from_value(serde_json::json!({
            "id":"custom","name":"Acme","notes":"Keep contrast", "variables":[
                {"name":"--ink","value":"#fff","kind":"color","selector":"[data-theme=\"dark\"]"},
                {"name":"--gap","value":"8px","kind":"length","selector":":root"},
                {"name":"--font","value":"Inter","kind":"font","selector":":root"}
            ]
        }))
        .unwrap();
        assert_eq!(detail.selectors(), [":root", "[data-theme=\"dark\"]"]);
        assert_eq!(detail.selector("removed"), ":root");
        assert_eq!(selector_label("[data-theme=\"dark\"]"), "Dark");
        assert_eq!(
            detail
                .variables
                .iter()
                .filter(|variable| variable.selector == detail.selector(":root"))
                .count(),
            2
        );
    }
}

#[cfg(test)]
#[path = "design_systems_tests.rs"]
mod interaction_tests;
