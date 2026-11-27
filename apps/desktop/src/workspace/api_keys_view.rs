//! Settings' API keys section. One read, `GET /v1/auth/tokens`, splits on each token's `kind`: the keys a person made
//! (`api`), and the sign-ins that apps made (`cli`, `ui`), one row per app that opens to show each sign-in. A key just
//! made shows its secret once, in place of the field that made it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDate};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::settings_view::{SettingsSection, error_line, note};
use super::{Workspace, disclosure, glyph};
use crate::motion;
use crate::pages::{PageId, ui};
use crate::palette::Palette;
use crate::settings::{ApiToken, NewToken};

/// How long Copy shows its check after a press.
const COPIED_FOR: Duration = Duration::from_millis(1500);
/// The column an app's icon sits in.
const APP_ICON: f32 = 22.;
/// The card row's inset: where a key's text, and a rule between two keys, start.
const ROW_INSET: f32 = 16.;
/// Where a sign-in's text starts inside its app's card: after the row's inset, the app's icon and the gap.
const SIGN_IN_INSET: f32 = ROW_INSET + APP_ICON + 16.;
/// A sign-in's row, shorter than its app's: it has one line.
const SIGN_IN_HEIGHT: f32 = 44.;

/// One app's sign-ins: the tokens of one kind that carry one name.
pub(super) struct App<'a> {
    /// What `SettingsState::open_apps` keeps for an opened app.
    pub key: String,
    pub title: String,
    pub kind: &'a str,
    /// This app's own sign-in first, then the newest.
    pub sign_ins: Vec<&'a ApiToken>,
}

impl App<'_> {
    /// Whether this app is the one signed in here.
    fn current(&self) -> bool {
        self.sign_ins.iter().any(|token| token.current)
    }

    /// What "Sign out" revokes: every sign-in but this app's own.
    fn others(&self) -> Vec<String> {
        self.sign_ins
            .iter()
            .filter(|token| !token.current)
            .map(|token| token.id.clone())
            .collect()
    }

    fn latest(&self) -> &str {
        self.sign_ins
            .iter()
            .map(|token| token.created_at.as_str())
            .max()
            .unwrap_or_default()
    }
}

/// The keys a person made, in Gateway's order.
pub(super) fn keys(tokens: &[ApiToken]) -> Vec<&ApiToken> {
    tokens.iter().filter(|token| token.kind == "api").collect()
}

/// The sign-ins, one entry per app: the app signed in here first, then by each app's newest sign-in.
pub(super) fn apps(tokens: &[ApiToken]) -> Vec<App<'_>> {
    let mut by_app: BTreeMap<(&str, String), Vec<&ApiToken>> = BTreeMap::new();
    for token in tokens.iter().filter(|token| token.kind != "api") {
        by_app
            .entry((token.kind.as_str(), token.title()))
            .or_default()
            .push(token);
    }
    let mut apps: Vec<App> = by_app
        .into_iter()
        .map(|((kind, title), mut sign_ins)| {
            sign_ins.sort_by(|a, b| b.current.cmp(&a.current).then_with(|| b.created_at.cmp(&a.created_at)));
            App {
                key: format!("{kind}\u{1f}{title}"),
                title,
                kind,
                sign_ins,
            }
        })
        .collect();
    apps.sort_by(|a, b| b.current().cmp(&a.current()).then_with(|| b.latest().cmp(a.latest())));
    apps
}

/// A day as a row says it: `Nov 12`, with the year only when it is not this year. A time that does not read as one
/// stays as Gateway sent it.
pub(super) fn short_day(at: &str, this_year: i32) -> String {
    let Some(date) = at
        .get(..10)
        .and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
    else {
        return at.to_string();
    };
    if date.year() == this_year {
        date.format("%b %-d").to_string()
    } else {
        date.format("%b %-d, %Y").to_string()
    }
}

/// When a token was made and when it ends: `Made Nov 12 · Expires Feb 10, 2027`.
pub(super) fn dates(token: &ApiToken, just_made: bool, this_year: i32) -> String {
    let made = if just_made {
        "Made just now".to_string()
    } else {
        format!("Made {}", short_day(&token.created_at, this_year))
    };
    let ends = match &token.expires_at {
        Some(at) => format!("Expires {}", short_day(at, this_year)),
        None => "Never expires".to_string(),
    };
    format!("{made} · {ends}")
}

/// A token's second line: its prefix in monospace, whole, then `text`, which may be cut.
fn detail_line(token: &ApiToken, text: String, palette: Palette) -> Div {
    h_flex()
        .min_w(px(0.))
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(palette.faint)
        .when(!token.prefix.is_empty(), |line| {
            line.child(
                div()
                    .flex_none()
                    .font_family(ui::MONO)
                    .text_color(palette.muted)
                    .child(format!("{}…", token.prefix)),
            )
            .child("·")
        })
        .child(div().min_w(px(0.)).truncate().child(text))
}

/// A section head with how many rows the section holds after it.
fn head(title: &'static str, count: Option<usize>, palette: Palette) -> Div {
    ui::section_head(title, palette)
        .flex()
        .gap(px(8.))
        .when_some(count, |head, count| {
            head.child(
                div()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(palette.faint)
                    .child(count.to_string()),
            )
        })
}

/// A card of rows, each with the inset its rule starts at, so a sign-in's rule lines up with its text.
fn card(rows: Vec<(f32, AnyElement)>, palette: Palette) -> Div {
    let mut card = ui::card_frame(palette);
    for (ix, (inset, row)) in rows.into_iter().enumerate() {
        if ix > 0 {
            card = card.child(div().h(px(1.)).ml(px(inset)).bg(palette.border));
        }
        card = card.child(row);
    }
    card
}

/// A card that holds one message: loading, nothing yet.
fn message_card(title: Option<&'static str>, detail: &'static str, palette: Palette) -> Div {
    ui::card_frame(palette).child(
        v_flex()
            .gap(px(4.))
            .px(px(ROW_INSET))
            .py(px(20.))
            .when_some(title, |column, title| {
                column.child(div().text_size(px(13.)).text_color(palette.strong).child(title))
            })
            .child(note(detail, palette)),
    )
}

/// What one Revoke does, for `Workspace::revoke`.
struct Revoke {
    /// The element ids' stem: one per row.
    id: String,
    /// What `confirm_revoke` holds while this one is asked.
    ask: String,
    /// The tokens it revokes.
    ids: Vec<String>,
    /// The quiet button's word, and the filled one's after the first press.
    label: SharedString,
    confirm: &'static str,
    /// The row's hover group: the quiet button shows only on the row's hover or its own focus. `None`: always shown.
    group: Option<SharedString>,
}

impl Workspace {
    pub(super) fn render_api_keys(
        &self,
        account: Option<&crate::pages::Ctx>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = &self.settings;
        let this_year = chrono::Local::now().year();
        let mut column = v_flex()
            .child(
                ui::lead(
                    "Keys let scripts and tools call Nexus Gateway as you. Each key reaches what you can reach.",
                    palette,
                )
                .mb(px(0.)),
            )
            .child(head("New key", None, palette))
            .child(match state.new_token.as_ref().filter(|_| account.is_some()) {
                Some(made) => self.render_new_key(made, palette, cx),
                None => self.render_create(account.is_none(), palette, cx),
            })
            .when_some(
                state.token_note.clone().filter(|_| account.is_some()),
                |column, error| column.child(error_line(error, palette).mt(px(8.))),
            );
        let Some(account) = account else {
            return column
                .child(head("Your keys", None, palette))
                .child(message_card(None, "Sign in to view and revoke your keys.", palette))
                .child(self.render_explorer_link(palette, cx));
        };
        let identity = (account.gateway.marker(), account.org.clone(), account.email.clone());
        match &state.tokens {
            None => {
                column = column.child(head("Your keys", None, palette)).child(message_card(
                    None,
                    "Reading your keys…",
                    palette,
                ));
            }
            Some(Err(error)) => {
                column = column.child(head("Your keys", None, palette)).child(
                    ui::card_frame(palette).child(
                        ui::card_row()
                            .child(
                                error_line(format!("Couldn't read your keys: {error}").into(), palette)
                                    .flex_1()
                                    .min_w(px(0.)),
                            )
                            .child(
                                ui::button("settings-keys-retry", "Try again", palette).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if let Some(account) = this.page_ctx(cx)
                                            && (account.gateway.marker(), account.org, account.email) == identity
                                        {
                                            this.settings.tokens = None;
                                            account.gateway.list_tokens();
                                            cx.notify();
                                        }
                                    },
                                )),
                            ),
                    ),
                );
            }
            Some(Ok(tokens)) => {
                let keys = keys(tokens);
                column = column.child(head("Your keys", Some(keys.len()), palette));
                column = if keys.is_empty() {
                    column.child(message_card(
                        Some("No keys yet"),
                        "Name one above to call Gateway from a script, a CI job or another tool.",
                        palette,
                    ))
                } else {
                    let rows = keys
                        .iter()
                        .enumerate()
                        .map(|(ix, token)| (ROW_INSET, self.key_row(ix, token, this_year, palette, cx)))
                        .collect();
                    column.child(card(rows, palette))
                };
                let apps = apps(tokens);
                if !apps.is_empty() {
                    let count = apps.iter().map(|app| app.sign_ins.len()).sum();
                    let mut rows = Vec::new();
                    for (ix, app) in apps.iter().enumerate() {
                        self.app_rows(ix, app, this_year, palette, cx, &mut rows);
                    }
                    column = column
                        .child(head("Signed-in apps", Some(count), palette))
                        .child(card(rows, palette))
                        .child(
                            div()
                                .mt(px(8.))
                                .text_size(px(12.))
                                .text_color(palette.faint)
                                .child("Apps make these when you sign in. Revoke one to sign that app out."),
                        );
                }
            }
        }
        column.child(self.render_explorer_link(palette, cx))
    }

    /// API Explorer is this build's own page, kept under Keys.
    fn render_explorer_link(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        v_flex().child(head("Try the API", None, palette)).child(
            ui::card_frame(palette).child(
                ui::card_row()
                    .child(
                        ui::setting_lines(
                            "API Explorer",
                            "Call Gateway's endpoints with your access, and see each request and answer.",
                            palette,
                        )
                        .flex_1(),
                    )
                    .child(
                        ui::button("settings-api-explorer", "Open", palette).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.open_settings(SettingsSection::Page(PageId::ApiExplorer), window, cx)
                            },
                        )),
                    ),
            ),
        )
    }

    /// The field that names a new key, and the button that makes it.
    fn render_create(&self, locked: bool, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.settings;
        ui::card_frame(palette)
            .child(
                ui::card_row()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(ui::CONTROL_HEIGHT))
                            .px(px(10.))
                            .rounded(px(crate::tokens::radius::CONTROL))
                            .bg(ui::inset(palette))
                            .text_size(px(13.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .child(Input::new(&state.token_label).appearance(false).disabled(locked)),
                            ),
                    )
                    .child(if locked {
                        ui::disabled_button("settings-create-key", "Create key", palette)
                            .debug_selector(|| "settings-create-key-locked".into())
                    } else if state.creating {
                        ui::disabled_button("settings-create-key", "Creating…", palette)
                    } else {
                        ui::primary_button("settings-create-key", "Create key", palette)
                            .on_click(cx.listener(|this, _, window, cx| this.create_token(window, cx)))
                    }),
            )
            .into_any_element()
    }

    /// The key just made: its secret, shown this once, Copy, and Done to go back to the field.
    fn render_new_key(&self, made: &NewToken, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let copied = self.settings.copied.is_some_and(|at| at.elapsed() < COPIED_FOR);
        let secret = made.secret.clone();
        let copy = ui::button("settings-copy-key", if copied { "Copied" } else { "Copy" }, palette)
            .gap(px(6.))
            .when(copied, |button| {
                button
                    .text_color(Palette::success())
                    .child(glyph(Lucide::Check, Palette::success()))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(secret.clone()));
                this.settings.copied = Some(Instant::now());
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(COPIED_FOR).await;
                    let _ = this.update(cx, |this, cx| {
                        // A later press keeps its own check for its full time.
                        if this.settings.copied.is_some_and(|at| at.elapsed() >= COPIED_FOR) {
                            this.settings.copied = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
                cx.notify();
            }));
        let done =
            ui::ghost_button("settings-key-done", "Done", palette).on_click(cx.listener(|this, _, window, cx| {
                this.settings.new_token = None;
                this.settings.copied = None;
                this.settings
                    .token_label
                    .update(cx, |state, cx| state.focus(window, cx));
                cx.notify();
            }));
        let body = v_flex()
            .gap(px(10.))
            .px(px(ROW_INSET))
            .py(px(14.))
            .child(
                h_flex()
                    .items_start()
                    .gap(px(10.))
                    .child(div().pt(px(2.)).child(glyph(Lucide::CircleCheck, Palette::success())))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(palette.strong)
                                    .child(format!("{} is ready", made.token.title())),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(palette.faint)
                                    .child("Copy it now. Nexus does not show it again."),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .px(px(10.))
                            .rounded(px(crate::tokens::radius::CONTROL))
                            .bg(ui::inset(palette))
                            .font_family(ui::MONO)
                            .text_size(px(12.))
                            .text_color(palette.foreground)
                            .child(div().min_w(px(0.)).truncate().child(made.secret.clone())),
                    )
                    .child(copy),
            )
            .child(h_flex().justify_end().child(done));
        // The card arrives once per key: its id carries the key's.
        motion::fade_in(
            ui::card_frame(palette).child(body),
            SharedString::from(format!("settings-new-key-{}", made.token.id)),
        )
        .into_any_element()
    }

    /// A key the person made: its name, its prefix and dates, and Revoke.
    fn key_row(
        &self,
        ix: usize,
        token: &ApiToken,
        this_year: i32,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.settings;
        let asked = state.confirm_revoke.as_deref() == Some(token.id.as_str());
        let fresh = state.new_token.as_ref().is_some_and(|made| made.token.id == token.id);
        let group = SharedString::from(format!("settings-key-{ix}"));
        let detail = if asked {
            "Scripts using it stop working.".to_string()
        } else {
            dates(token, fresh, this_year)
        };
        ui::card_row()
            .group(group.clone())
            .when(fresh, |row| row.bg(Palette::success().opacity(0.06)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.strong)
                            .truncate()
                            .child(token.title()),
                    )
                    .child(detail_line(token, detail, palette)),
            )
            .when(token.current, |row| row.child(ui::badge("This app", palette)))
            .when(!token.current, |row| {
                row.child(self.revoke(
                    Revoke {
                        id: format!("settings-revoke-{ix}"),
                        ask: token.id.clone(),
                        ids: vec![token.id.clone()],
                        label: "Revoke".into(),
                        confirm: "Revoke key",
                        group: Some(group),
                    },
                    palette,
                    cx,
                ))
            })
            .into_any_element()
    }

    /// An app's row, and when it is open, each of its sign-ins and a row that signs the others out. An app with one
    /// sign-in has nothing to open: its row is the sign-in.
    fn app_rows(
        &self,
        ix: usize,
        app: &App,
        this_year: i32,
        palette: Palette,
        cx: &mut Context<Self>,
        rows: &mut Vec<(f32, AnyElement)>,
    ) {
        let state = &self.settings;
        let icon = glyph(
            if app.kind == "ui" {
                Lucide::Globe
            } else {
                Lucide::Laptop
            },
            palette.icon,
        );
        let title = v_flex().flex_1().min_w(px(0.)).gap(px(2.)).child(
            div()
                .text_size(px(13.))
                .text_color(palette.strong)
                .truncate()
                .child(app.title.clone()),
        );
        let lead = |row: Div| row.child(div().flex_none().w(px(APP_ICON)).flex().justify_center().child(icon));
        if let [only] = app.sign_ins.as_slice() {
            let asked = state.confirm_revoke.as_deref() == Some(only.id.as_str());
            let detail = if asked {
                "Signs that app out.".to_string()
            } else {
                dates(only, false, this_year)
            };
            let group = SharedString::from(format!("settings-app-{ix}"));
            let row = lead(ui::card_row().group(group.clone()))
                .child(title.child(detail_line(only, detail, palette)))
                .when(only.current, |row| row.child(ui::badge("This app", palette)))
                .when(!only.current, |row| {
                    row.child(self.revoke(
                        Revoke {
                            id: format!("settings-app-revoke-{ix}"),
                            ask: only.id.clone(),
                            ids: vec![only.id.clone()],
                            label: "Revoke".into(),
                            confirm: "Revoke",
                            group: Some(group),
                        },
                        palette,
                        cx,
                    ))
                });
            rows.push((ROW_INSET, row.into_any_element()));
            return;
        }

        let open = state.open_apps.contains(&app.key);
        let summary = format!(
            "{} sign-ins · Latest {}",
            app.sign_ins.len(),
            short_day(app.latest(), this_year)
        );
        let key = app.key.clone();
        let header = ui::focusable(
            lead(ui::card_row())
                .id(SharedString::from(format!("settings-app-{ix}")))
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .child(
                    title.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .truncate()
                            .child(summary),
                    ),
                )
                .when(app.current(), |row| row.child(ui::badge("This app", palette)))
                .child(disclosure(
                    SharedString::from(format!("settings-app-chevron-{ix}")),
                    open,
                    palette.icon,
                )),
            palette,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if !this.settings.open_apps.remove(&key) {
                this.settings.open_apps.insert(key.clone());
            }
            this.settings.confirm_revoke = None;
            cx.notify();
        }));
        rows.push((ROW_INSET, header.into_any_element()));
        if !open {
            return;
        }

        for (sub, token) in app.sign_ins.iter().enumerate() {
            let asked = state.confirm_revoke.as_deref() == Some(token.id.as_str());
            let detail = if asked {
                "Signs that app out.".to_string()
            } else {
                dates(token, false, this_year)
            };
            let group = SharedString::from(format!("settings-sign-in-{ix}-{sub}"));
            let row = ui::card_row()
                .min_h(px(SIGN_IN_HEIGHT))
                .pl(px(SIGN_IN_INSET))
                .group(group.clone())
                .child(detail_line(token, detail, palette).flex_1())
                .when(token.current, |row| row.child(ui::badge("This app", palette)))
                .when(!token.current, |row| {
                    row.child(self.revoke(
                        Revoke {
                            id: format!("settings-sign-in-revoke-{ix}-{sub}"),
                            ask: token.id.clone(),
                            ids: vec![token.id.clone()],
                            label: "Revoke".into(),
                            confirm: "Revoke",
                            group: Some(group),
                        },
                        palette,
                        cx,
                    ))
                });
            rows.push((SIGN_IN_INSET, row.into_any_element()));
        }

        let others = app.others();
        if others.is_empty() {
            return;
        }
        let ask = format!("app:{}", app.key);
        let asked = state.confirm_revoke.as_deref() == Some(ask.as_str());
        let label = if app.current() {
            format!("Sign out the other {}", others.len())
        } else {
            format!("Sign out all {}", others.len())
        };
        // The ghost button's own inset lines its word up with the sign-ins' text.
        let footer = ui::card_row()
            .min_h(px(SIGN_IN_HEIGHT))
            .pl(px(SIGN_IN_INSET - 12.))
            .when(asked, |row| {
                row.child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .pl(px(12.))
                        .text_size(px(12.))
                        .text_color(palette.muted)
                        .child(format!("Signs out {} sign-ins.", others.len())),
                )
            })
            .child(self.revoke(
                Revoke {
                    id: format!("settings-app-sign-out-{ix}"),
                    ask,
                    ids: others,
                    label: label.into(),
                    confirm: "Sign out",
                    group: None,
                },
                palette,
                cx,
            ));
        rows.push((SIGN_IN_INSET, footer.into_any_element()));
    }

    /// A quiet red word; its first press asks, in place, with Cancel and a filled red button that revokes.
    fn revoke(&self, revoke: Revoke, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Revoke {
            id,
            ask,
            ids,
            label,
            confirm,
            group,
        } = revoke;
        if self.settings.confirm_revoke.as_deref() == Some(ask.as_str()) {
            let tint = palette.destructive;
            return h_flex()
                .flex_none()
                .gap(px(8.))
                .child(
                    ui::ghost_button(SharedString::from(format!("{id}-cancel")), "Cancel", palette).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.settings.confirm_revoke = None;
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    ui::button(SharedString::from(id), confirm, palette)
                        .bg(tint.opacity(0.16))
                        .text_color(tint)
                        .hover(move |style| style.bg(tint.opacity(0.24)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings.confirm_revoke = None;
                            if let Some(account) = this.account_gateway() {
                                account.revoke_tokens(ids.clone());
                            }
                            cx.notify();
                        })),
                )
                .into_any_element();
        }
        ui::ghost_button(SharedString::from(id), label, palette)
            .text_color(palette.destructive)
            .when_some(group, |button, group| {
                button
                    .opacity(0.)
                    .group_hover(group, |style| style.opacity(1.))
                    .in_focus(|style| style.opacity(1.))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.settings.confirm_revoke = Some(ask.clone());
                cx.notify();
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it brings gpui's own `test` attribute in with `gpui_kit::*`.
    use super::{ApiToken, apps, dates, keys, short_day};

    fn token(id: &str, kind: &str, label: &str, created_at: &str, current: bool) -> ApiToken {
        ApiToken {
            id: id.into(),
            kind: kind.into(),
            prefix: String::new(),
            label: Some(label.into()),
            created_at: created_at.into(),
            expires_at: None,
            current,
        }
    }

    #[test]
    fn keys_and_sign_ins_part_by_kind() {
        let tokens = [
            token("k1", "api", "CI deploy bot", "2026-10-30T00:00:00Z", false),
            token("d1", "cli", "Desktop", "2026-11-12T01:00:00Z", false),
            token("w1", "ui", "Nexus UI", "2026-11-12T03:00:00Z", false),
            token("d2", "cli", "Desktop", "2026-11-12T02:00:00Z", true),
            token("w2", "ui", "Nexus UI", "2026-11-11T00:00:00Z", false),
        ];
        assert_eq!(keys(&tokens).iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["k1"]);
        let apps = apps(&tokens);
        // The app signed in here comes first, though Nexus UI signed in later; its own sign-in leads its list.
        assert_eq!(
            apps.iter().map(|app| app.title.as_str()).collect::<Vec<_>>(),
            ["Desktop", "Nexus UI"]
        );
        assert_eq!(
            apps[0].sign_ins.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["d2", "d1"]
        );
        assert_eq!(apps[0].others(), ["d1"]);
        assert_eq!(apps[1].others(), ["w1", "w2"]);
    }

    #[test]
    fn a_day_shows_its_year_only_when_it_is_not_this_one() {
        assert_eq!(short_day("2026-11-12T22:18:36Z", 2026), "Nov 12");
        assert_eq!(short_day("2027-02-10T00:00:00Z", 2026), "Feb 10, 2027");
        assert_eq!(short_day("soon", 2026), "soon");
    }

    #[test]
    fn dates_say_never_when_a_token_does_not_expire() {
        let key = token("k1", "api", "Local scripts", "2026-11-03T00:00:00Z", false);
        assert_eq!(dates(&key, false, 2026), "Made Nov 3 · Never expires");
        assert_eq!(dates(&key, true, 2026), "Made just now · Never expires");
    }
}
