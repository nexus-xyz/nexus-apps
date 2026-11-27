//! Gateway Chat's slash list. Code's reported agent skills stay in slash.rs.
mod model;
use super::{View, Workspace};
use crate::palette::Palette;
use gpui_kit::component::input::Position;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use model::{Skill, Token};

#[derive(Default)]
pub(super) struct SkillsState {
    scope: String,
    rows: Option<Vec<Skill>>,
    loading: bool,
    error: Option<String>,
    active: usize,
    key: String,
    dismissed: Option<String>,
    scroll: ScrollHandle,
}

impl Workspace {
    fn chat_skill_token(&self, cx: &App) -> Option<Token> {
        if !matches!(self.view, View::Chat) || self.asking.is_some() {
            return None;
        }
        let input = self.composer.read(cx);
        if !input.selected_range().is_empty() {
            return None;
        }
        model::token(&input.value(), input.cursor())
    }

    pub(super) fn chat_skills_typed(&mut self, cx: &mut Context<Self>) {
        self.chat_skills.dismissed = None;
        self.chat_skills.active = 0;
        self.chat_skills.key.clear();
        cx.notify();
    }

    pub(super) fn sync_chat_skills(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(account) = &self.account else {
            return;
        };
        let scope = format!(
            "{}|{}|{}",
            self.gateway.url(),
            account.org,
            self.gateway.marker().unwrap_or_default()
        );
        if self.chat_skills.scope != scope {
            self.chat_skills = SkillsState {
                scope: scope.clone(),
                ..Default::default()
            };
        }
        if self.chat_skill_token(cx).is_none()
            || self.chat_skills.loading
            || self.chat_skills.rows.is_some()
            || self.chat_skills.error.is_some()
        {
            return;
        }
        self.chat_skills.loading = true;
        let mut read = account_gateway.skill_query();
        if read.current()
            && let Some(value) = read.take_cached()
            && let Ok(rows) = model::catalog(&value)
        {
            self.chat_skills.rows = Some(rows);
            self.chat_skills.loading = false;
        }
        let generation = read.generation.clone();
        cx.spawn(async move |this, cx| {
            let result = read.await.and_then(|value| model::catalog(&value));
            if generation
                .as_ref()
                .is_some_and(|(epoch, expected)| epoch.load(std::sync::atomic::Ordering::SeqCst) != *expected)
            {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if this.chat_skills.scope != scope {
                    return;
                }
                this.chat_skills.loading = false;
                match result {
                    Ok(rows) => this.chat_skills.rows = Some(rows),
                    Err(error) => {
                        // A library failure must never offer ambiguous saved skill names.
                        this.chat_skills.rows = None;
                        this.chat_skills.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn chat_skill_rows(&self, cx: &App) -> Option<(Token, Vec<Skill>)> {
        if !self.composer_focused {
            return None;
        }
        let token = self.chat_skill_token(cx)?;
        let key = format!("{}:{}", token.start, token.query);
        if self.chat_skills.dismissed.as_deref() == Some(key.as_str()) {
            return None;
        }
        let rows = model::matches(self.chat_skills.rows.as_deref().unwrap_or_default(), &token.query)
            .into_iter()
            .filter(|s| !s.duplicate_name)
            .cloned()
            .collect();
        Some((token, rows))
    }

    pub(super) fn chat_skill_move(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some((token, rows)) = self.chat_skill_rows(cx) else {
            return false;
        };
        if rows.is_empty() {
            return false;
        }
        let key = format!("{}:{}", token.start, token.query);
        let active = if self.chat_skills.key == key {
            self.chat_skills.active.min(rows.len() - 1)
        } else {
            0
        };
        self.chat_skills.active = if down {
            (active + 1) % rows.len()
        } else {
            (active + rows.len() - 1) % rows.len()
        };
        self.chat_skills.key = key;
        self.chat_skills.scroll.scroll_to_item(self.chat_skills.active);
        cx.notify();
        true
    }

    pub(super) fn chat_skill_dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((token, _)) = self.chat_skill_rows(cx) else {
            return false;
        };
        self.chat_skills.dismissed = Some(format!("{}:{}", token.start, token.query));
        cx.notify();
        true
    }

    pub(super) fn chat_skill_complete(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((token, rows)) = self.chat_skill_rows(cx) else {
            return false;
        };
        if rows.is_empty() {
            return self.chat_skills.loading || self.chat_skills.error.is_some();
        }
        let key = format!("{}:{}", token.start, token.query);
        let active = if self.chat_skills.key == key {
            self.chat_skills.active.min(rows.len() - 1)
        } else {
            0
        };
        self.pick_chat_skill(&token, &rows[active].name, window, cx);
        true
    }

    fn pick_chat_skill(&mut self, token: &Token, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().to_string();
        let (text, caret) = model::choose(&text, token, name);
        let prefix = &text[..caret];
        let position = Position {
            line: prefix.bytes().filter(|b| *b == b'\n').count() as u32,
            character: prefix.rsplit('\n').next().unwrap_or("").encode_utf16().count() as u32,
        };
        self.composer.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.set_cursor_position(position, window, cx);
            input.focus(window, cx);
        });
        self.composer_empty = false;
        cx.notify();
    }

    /// What the skills menu shows now, or `None` while it is closed.
    fn chat_skills_menu(&self, cx: &App) -> Option<SkillMenu> {
        let (token, rows) = self.chat_skill_rows(cx)?;
        let all = self.chat_skills.rows.as_deref().unwrap_or_default();
        let blocked = model::matches(all, &token.query)
            .into_iter()
            .filter(|s| s.duplicate_name)
            .map(|s| s.name.clone())
            .collect::<std::collections::BTreeSet<_>>();
        if rows.is_empty()
            && blocked.is_empty()
            && !self.chat_skills.loading
            && self.chat_skills.error.is_none()
            && !all.is_empty()
        {
            return None;
        }
        let key = format!("{}:{}", token.start, token.query);
        let active = if self.chat_skills.key == key {
            self.chat_skills.active
        } else {
            0
        };
        let rows = rows
            .into_iter()
            .map(|skill| {
                let scope = match skill.scope.as_deref() {
                    Some("org") => "organization".to_string(),
                    Some("team") => self
                        .account
                        .iter()
                        .flat_map(|account| account.teams.iter())
                        .find(|t| Some(t.id.as_str()) == skill.team_id.as_deref())
                        .map(|t| t.name.clone())
                        .unwrap_or_else(|| "team".into()),
                    _ => "your skills".into(),
                };
                (skill.name, scope, skill.description)
            })
            .collect();
        Some(SkillMenu {
            token,
            loading: self.chat_skills.loading,
            error: self.chat_skills.error.clone(),
            none: all.is_empty(),
            rows,
            blocked: blocked.into_iter().collect(),
            active,
        })
    }

    /// The skills menu over the composer. It rises from the composer and leaves the same way, with the rows it showed
    /// last (`motion::presence_of`).
    pub(super) fn render_chat_skills(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let shown = self.chat_skills_menu(cx);
        let (
            phase,
            SkillMenu {
                token,
                loading,
                error,
                none,
                rows,
                blocked,
                active,
            },
        ) = crate::motion::presence_of("chat-skills-menu", shown, cx)?;
        let mut menu = v_flex()
            .id("chat-skills-menu")
            .w(px(380.))
            .max_h(px(320.))
            .overflow_y_scroll()
            .track_scroll(&self.chat_skills.scroll)
            .p_2()
            .gap_1()
            .rounded_md()
            .bg(p.menu)
            .shadow_sm()
            .text_size(px(13.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if loading {
            menu = menu.child("Reading skills…");
        }
        if let Some(error) = &error {
            menu = menu
                .child(
                    div()
                        .text_color(p.destructive)
                        .child(format!("Skills did not load: {error}")),
                )
                .child(
                    div()
                        .id("retry-chat-skills")
                        .cursor_pointer()
                        .child("Try again")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.chat_skills.error = None;
                            this.sync_chat_skills(cx);
                            cx.notify();
                        })),
                );
        } else if none && !loading {
            menu = menu.child("No skills yet. Add skills in Nexus UI.");
        }
        for (ix, (name, scope, description)) in rows.into_iter().enumerate() {
            let at = token.clone();
            menu = menu.child(
                v_flex()
                    .id(SharedString::from(format!("chat-skill-{name}")))
                    .p_2()
                    .gap_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(ix == active, |s| s.bg(p.menu_hover))
                    .hover(|s| s.bg(p.menu_hover))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(format!("/{name}"))
                            .child(div().text_color(p.faint).child(scope)),
                    )
                    .child(div().text_color(p.muted).child(description))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_chat_skill(&at, &name, window, cx))),
            );
        }
        for name in blocked {
            menu=menu.child(div().text_color(p.muted).child(format!("/{name} is unavailable: more than one skill has this name. Rename yours or ask an approver to withdraw a duplicate.")));
        }
        Some(
            div()
                .absolute()
                .bottom_full()
                .left_0()
                .mb(px(8.))
                .child(phase.menu("chat-skills-menu", 4., menu))
                .into_any_element(),
        )
    }
}

/// What the skills menu shows: kept while it leaves.
#[derive(Clone)]
struct SkillMenu {
    token: Token,
    loading: bool,
    error: Option<String>,
    /// No skills at all.
    none: bool,
    /// Each match's name, scope and description.
    rows: Vec<(String, String, String)>,
    /// Names more than one skill has.
    blocked: Vec<String>,
    active: usize,
}
