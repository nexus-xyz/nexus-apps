//! Native Share sheet rendering: the audience page, and its History page with the versions, the
//! requests waiting for you and the readers of an outside share.

use super::*;
use crate::pages::ui;
use crate::palette::Palette;
use crate::workspace::glass::FloatingGround;
use crate::workspace::glyph;
use crate::workspace::share_ui::{blocked_row_end, card_shadow, checkbox, icon_button, ring, secondary};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};

const WIDTH: f32 = 480.;

impl Render for ChatShare {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::pages::palette(cx);
        let body = if self.history && self.ready {
            self.render_history(palette, cx).into_any_element()
        } else {
            self.render_audience(palette, cx).into_any_element()
        };
        deferred(
            div()
                .id("chat-share-overlay")
                .absolute()
                .inset_0()
                .size_full()
                .occlude()
                .bg(palette.scrim)
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                )
                .child(
                    div()
                        .id("chat-share-dialog")
                        .track_focus(&self.focus)
                        .key_context("ChatShare")
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if event.keystroke.key == "tab" {
                                let fields = [&this.outside, &this.expiry];
                                let visible = match this.kind {
                                    Audience::External => Some(fields[0]),
                                    Audience::Public => Some(fields[1]),
                                    _ => None,
                                };
                                if let Some(field) = visible {
                                    if field.read(cx).focus_handle(cx).is_focused(window) {
                                        window.focus(&this.focus, cx);
                                    } else {
                                        field.update(cx, |state, cx| state.focus(window, cx));
                                    }
                                } else {
                                    window.focus(&this.focus, cx);
                                }
                            }
                            if event.keystroke.key == "enter"
                                && (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
                                && this.ready
                                && !this.busy
                                && !this.history
                            {
                                this.share(false, cx);
                            }
                            if event.keystroke.key == "escape" {
                                // Esc backs out of a revoke confirm, then the History page, then the sheet.
                                if this.confirm_revoke.take().is_none() {
                                    if this.history {
                                        this.history = false;
                                    } else {
                                        this.close(window, cx);
                                    }
                                }
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(px(WIDTH))
                        .max_w(relative(0.94))
                        .max_h(relative(0.90))
                        .flex()
                        .flex_col()
                        .rounded(px(12.))
                        .floating_ground(12., palette.menu, true, &palette)
                        .border_1()
                        .border_color(ring(palette))
                        .shadow(palette.pop_shadow(palette.border))
                        .text_size(px(13.))
                        .text_color(palette.foreground)
                        // The body scrolls inside the card, so the card's glass stays under it.
                        .child(
                            div()
                                .id("chat-share-scroll")
                                .min_h(px(0.))
                                .overflow_y_scroll()
                                .child(body),
                        ),
                ),
        )
        .with_priority(3)
    }
}

impl ChatShare {
    fn render_audience(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let mut sheet = v_flex().child(
            h_flex()
                .h(px(44.))
                .pt(px(16.))
                .pr(px(12.))
                .pl(px(20.))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(16.))
                        .line_height(px(24.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Share chat"),
                )
                .child(
                    icon_button("share-close", Lucide::X, palette)
                        .aria_label("Close")
                        .tooltip(|window, cx| Tooltip::new("Close  Esc").build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                ),
        );
        sheet = sheet.child(
            v_flex()
                .px(px(20.))
                .pt(px(4.))
                .pb(px(12.))
                .gap(px(2.))
                .child(
                    div()
                        .line_height(px(20.))
                        .text_color(palette.muted)
                        .child("Readers get a snapshot. New messages stay private until you share again."),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(palette.faint)
                        .child("They see the answers as written, including data read from connectors."),
                ),
        );
        if !self.ready {
            if self.busy {
                // Six skeleton rows while the sheet loads.
                let mut rows = v_flex().px(px(10.)).pb(px(20.)).gap(px(2.));
                for ix in 0..6 {
                    rows = rows.child(
                        h_flex()
                            .h(px(36.))
                            .px(px(10.))
                            .gap(px(12.))
                            .child(div().size(px(16.)).rounded(px(4.)).bg(palette.menu_hover))
                            .child(
                                div()
                                    .h(px(12.))
                                    .w(px([140., 120., 90., 130., 200., 150.][ix]))
                                    .rounded(px(6.))
                                    .bg(palette.menu_hover),
                            ),
                    );
                }
                return sheet.child(rows);
            }
            return sheet.child(
                v_flex()
                    .px(px(20.))
                    .pb(px(20.))
                    .gap(px(12.))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .items_start()
                            .text_color(palette.destructive)
                            .line_height(px(20.))
                            .child(glyph(Lucide::CircleX, palette.destructive).size(px(14.)).mt(px(3.)))
                            .child(
                                self.problem.clone().unwrap_or_else(|| {
                                    "Couldn't load sharing for this chat. Nothing was changed.".into()
                                }),
                            ),
                    )
                    .child(
                        secondary("share-retry", Some(Lucide::RotateCcw), "Retry", palette)
                            .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                    ),
            );
        }

        // The audiences, in the spec's order; the chosen one carries a check, never a fill.
        let mut list = v_flex().px(px(10.)).gap(px(2.));
        for kind in [
            Audience::Private,
            Audience::Org,
            Audience::Teams,
            Audience::People,
            Audience::External,
            Audience::Public,
        ] {
            if self.personal && kind == Audience::Org {
                continue;
            }
            let (icon, label) = match kind {
                Audience::Private => (Lucide::Lock, "Only you".to_string()),
                Audience::Org => (Lucide::Building2, format!("Anyone at {}", self.org)),
                Audience::Teams => (Lucide::Users, "Teams".to_string()),
                Audience::People => (Lucide::User, format!("People at {}", self.org)),
                Audience::External => (Lucide::Mail, format!("People outside {} by email", self.org)),
                Audience::Public => (Lucide::Link, "Anyone with the link".to_string()),
            };
            let blocked = kind.blocked(&self.policy, &self.role);
            let chosen = kind == self.kind;
            let ink = if blocked.is_some() {
                palette.faint
            } else {
                palette.foreground
            };
            let mut row = h_flex()
                .id(SharedString::from(format!("share-{}", kind.key())))
                .h(px(36.))
                .px(px(10.))
                .gap(px(12.))
                .rounded(px(6.))
                .text_color(ink)
                .child(glyph(icon, if blocked.is_some() { palette.faint } else { palette.icon }).size(px(16.)))
                .child(div().flex_1().min_w(px(0.)).truncate().child(label))
                .when(chosen, |row| {
                    row.child(glyph(Lucide::Check, palette.foreground).size(px(16.)))
                });
            if let Some(reason) = blocked {
                let sheet = cx.entity().downgrade();
                let close = move |window: &mut Window, cx: &mut App| {
                    if let Err(error) = sheet.update(cx, |sheet, cx| sheet.close(window, cx)) {
                        eprintln!("nexus-desktop-gpui: share: close: {error}");
                    }
                };
                row = row.child(blocked_row_end(
                    kind.key(),
                    reason,
                    &self.org,
                    close,
                    &self.fixer,
                    palette,
                    cx,
                ));
            } else if !self.busy {
                row = row
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.menu_hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick(kind, window, cx)));
            }
            list = list.child(row);
            if chosen {
                list = list.children(self.render_choices(palette, cx));
            }
        }
        sheet = sheet.child(list);
        // Teams or People with something to pick and nothing picked: say so, and keep the primary disabled.
        let options = match self.kind {
            Audience::Teams => self.teams.as_ref().map_or(0, Vec::len),
            Audience::People => self.people.as_ref().map_or(0, Vec::len),
            _ => 0,
        };
        let unpicked = options > 0 && self.picks.is_empty();
        let no_email = self.kind == Audience::External && self.outside.read(cx).value().trim().is_empty();
        if unpicked && self.problem.is_none() {
            sheet = sheet.child(
                div()
                    .px(px(20.))
                    .py(px(4.))
                    .line_height(px(20.))
                    .text_color(palette.destructive)
                    .child("Choose at least one team or person."),
            );
        }
        if let Some(problem) = &self.problem {
            sheet = sheet.child(
                div()
                    .px(px(20.))
                    .py(px(4.))
                    .line_height(px(20.))
                    .text_color(palette.destructive)
                    .child(problem.clone()),
            );
        }

        let continue_locked = self.busy || (self.kind.outside() && self.policy["artifacts.external.continue"] == false);
        sheet = sheet.child(
            switch_row("Readers can continue in their own copy", palette)
                .mt(px(8.))
                .child(
                    ui::switch(
                        "share-continue",
                        self.allow_continue && !continue_locked,
                        continue_locked,
                        palette,
                    )
                    .when(!continue_locked, |button| {
                        button.on_click(cx.listener(|this, _, _, cx| {
                            this.allow_continue = !this.allow_continue;
                            cx.notify();
                        }))
                    }),
                ),
        );
        if self.kind == Audience::Public {
            sheet = sheet.child(
                switch_row("Link expires", palette).child(
                    div()
                        .w(px(120.))
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .bg(palette.menu_hover)
                        .child(Input::new(&self.expiry).appearance(false).disabled(self.busy)),
                ),
            );
            if let Some(days) = self.policy["artifacts.external.expiry"]["maxDays"].as_u64() {
                sheet = sheet.child(
                    div()
                        .px(px(20.))
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(format!("{} allows at most {days} days.", self.org)),
                );
            }
        }

        // The link card and the status line, once the chat is shared.
        let current = &self.detail["sharing"];
        if !current.is_null() {
            let url = current["publicUrl"]
                .as_str()
                .or_else(|| current["url"].as_str())
                .unwrap_or_default();
            let url = if url.starts_with('/') {
                format!("{}{url}", self.gateway.site_url())
            } else {
                url.to_string()
            };
            sheet = sheet.child(
                h_flex()
                    .mx(px(20.))
                    .mt(px(12.))
                    .py(px(8.))
                    .pl(px(14.))
                    .pr(px(8.))
                    .gap(px(10.))
                    .rounded(px(10.))
                    .bg(palette.raised)
                    .border_1()
                    .border_color(palette.border.opacity(0.8))
                    .shadow(card_shadow())
                    .child(glyph(Lucide::Link, palette.icon).size(px(16.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .font_family(crate::pages::ui::MONO)
                            .text_size(px(12.))
                            .child(url.clone()),
                    )
                    .child(
                        secondary(
                            "share-copy",
                            Some(if self.copied { Lucide::Check } else { Lucide::Copy }),
                            if self.copied { "Copied" } else { "Copy link" },
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                            this.copied = true;
                            cx.notify();
                        })),
                    ),
            );
            let mut status = format!(
                "Version {} · {}",
                current["version"],
                audience_words(&word(&current["audience"], "kind"), &self.org)
            );
            if let Some(Ok(access)) = &self.access
                && let Some(opens) = access["opens"].as_u64()
            {
                status.push_str(&format!(
                    " · opened {opens} {}",
                    if opens == 1 { "time" } else { "times" }
                ));
            }
            let waiting = self
                .access
                .as_ref()
                .and_then(|access| access.as_ref().ok())
                .and_then(|access| access["people"].as_array())
                .map_or(0, |people| {
                    people.iter().filter(|p| word(p, "status") == "requested").count()
                });
            if waiting > 0 {
                status.push_str(&format!(" · {waiting} waiting"));
            }
            sheet = sheet.child(
                h_flex()
                    .h(px(28.))
                    .mt(px(8.))
                    .pl(px(20.))
                    .pr(px(14.))
                    .gap(px(8.))
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_color(palette.faint)
                            .child(status),
                    )
                    .child(
                        h_flex()
                            .id("share-history")
                            .h(px(24.))
                            .px(px(6.))
                            .gap(px(2.))
                            .rounded(px(6.))
                            .text_color(palette.muted)
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.menu_hover))
                            .child("History")
                            .child(glyph(Lucide::ChevronRight, palette.muted).size(px(14.)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.history = true;
                                cx.notify();
                            })),
                    ),
            );
        }

        // Make private (when shared) left; the one primary right: Share, Share latest, Save, Saving….
        let shared = !current.is_null();
        let changed = self.audience_changed(cx);
        let primary = if self.busy {
            ui::disabled_button("share-save", "Saving…", palette).into_any_element()
        } else if (!shared && self.kind == Audience::Private) || unpicked || no_email {
            ui::disabled_button("share-save", if shared { "Save" } else { "Share" }, palette).into_any_element()
        } else if shared && !changed {
            ui::primary_button("share-save", "Share latest", palette)
                .on_click(cx.listener(|this, _, _, cx| this.share(true, cx)))
                .into_any_element()
        } else {
            ui::primary_button("share-save", if shared { "Save" } else { "Share" }, palette)
                .on_click(cx.listener(|this, _, _, cx| this.share(false, cx)))
                .into_any_element()
        };
        sheet.child(
            h_flex()
                .pt(px(16.))
                .px(px(20.))
                .pb(px(20.))
                .gap(px(8.))
                .when(shared, |footer| {
                    footer.child(
                        secondary("share-private", None, "Make private", palette)
                            .h(px(32.))
                            .when(!self.busy, |button| {
                                button.on_click(cx.listener(|this, _, _, cx| {
                                    this.change(Verb::Delete, format!("{}/share", path(&this.session)), None, cx);
                                }))
                            }),
                    )
                })
                .child(div().flex_1())
                .child(primary),
        )
    }

    /// The checklist under Teams or People, or the email field under People outside.
    fn render_choices(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.kind == Audience::External {
            return Some(
                v_flex()
                    .child(
                        div()
                            .ml(px(38.))
                            .mr(px(10.))
                            .min_h(px(32.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .bg(palette.menu_hover)
                            .child(Input::new(&self.outside).appearance(false).disabled(self.busy)),
                    )
                    .child(
                        div()
                            .pt(px(6.))
                            .pr(px(10.))
                            .pb(px(2.))
                            .pl(px(38.))
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child("Readers sign in with these addresses. Your organization's domain rules apply."),
                    )
                    .into_any_element(),
            );
        }
        if !matches!(self.kind, Audience::Teams | Audience::People) {
            return None;
        }
        let picks = if self.kind == Audience::Teams {
            &self.teams
        } else {
            &self.people
        };
        let mut rows = v_flex();
        match picks {
            Err(error) => {
                rows = rows.child(
                    div()
                        .pl(px(38.))
                        .py(px(6.))
                        .text_color(palette.destructive)
                        .child(error.clone()),
                )
            }
            Ok(picks) if picks.is_empty() => {
                rows = rows.child(h_flex().h(px(32.)).pl(px(38.)).text_color(palette.faint).child(format!(
                    "No {} at {} yet",
                    if self.kind == Audience::Teams {
                        "teams"
                    } else {
                        "people"
                    },
                    self.org
                )))
            }
            Ok(picks) => {
                for (id, label) in picks {
                    let checked = self.picks.contains(id);
                    let id = id.clone();
                    rows = rows.child(
                        h_flex()
                            .id(SharedString::from(format!("share-pick-{id}")))
                            .h(px(32.))
                            .pl(px(38.))
                            .pr(px(10.))
                            .gap(px(10.))
                            .rounded(px(6.))
                            .child(checkbox(checked, palette))
                            .child(div().flex_1().min_w(px(0.)).truncate().child(label.clone()))
                            .when(!self.busy, |row| {
                                row.cursor_pointer()
                                    .hover(|style| style.bg(palette.menu_hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.picks.contains(&id) {
                                            this.picks.retain(|picked| picked != &id);
                                        } else {
                                            this.picks.push(id.clone());
                                        }
                                        this.problem = None;
                                        cx.notify();
                                    }))
                            }),
                    );
                }
            }
        }
        Some(rows.into_any_element())
    }

    /// History: requests waiting for you, the versions (open, revoke), and the readers of an outside share.
    fn render_history(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let mut page = v_flex().pb(px(16.)).child(
            h_flex()
                .h(px(44.))
                .pt(px(16.))
                .px(px(12.))
                .gap(px(4.))
                .child(
                    icon_button("share-back", Lucide::ArrowLeft, palette)
                        .aria_label("Back to sharing")
                        .tooltip(|window, cx| Tooltip::new("Back to sharing").build(window, cx))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.history = false;
                            this.confirm_revoke = None;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(px(16.))
                        .line_height(px(24.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("History"),
                )
                .child(
                    icon_button("share-history-close", Lucide::X, palette)
                        .aria_label("Close")
                        .tooltip(|window, cx| Tooltip::new("Close  Esc").build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                ),
        );
        let people: Vec<Value> = match &self.access {
            Some(Ok(access)) => access["people"].as_array().cloned().unwrap_or_default(),
            _ => Vec::new(),
        };
        let waiting: Vec<&Value> = people.iter().filter(|p| word(p, "status") == "requested").collect();
        if !waiting.is_empty() {
            page = page.child(section("Waiting for you", palette));
            for person in waiting {
                let email = word(person, "email");
                let note = person["message"]
                    .as_str()
                    .unwrap_or("Asked to open this chat")
                    .to_string();
                page = page.child(
                    history_row(Lucide::User, format!("{email} asked to open this chat"), note, palette)
                        .child(self.access_button("deny", "Deny", false, &email, palette, cx))
                        .child(self.access_button("grant", "Approve", true, &email, palette, cx)),
                );
            }
        }
        if let Some(versions) = self.detail["artifacts"].as_array()
            && !versions.is_empty()
        {
            page = page.child(section("Versions", palette));
            for version in versions {
                let id = word(version, "id");
                let active = version["status"] == "active";
                let title = format!("Version {}", version["version"]);
                let detail = format!(
                    "{} · {}",
                    audience_words(&word(version, "audience"), &self.org),
                    word(version, "status")
                );
                if self.confirm_revoke.as_deref() == Some(id.as_str()) {
                    let endpoint = format!(
                        "/v1/artifacts/{}",
                        url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
                    );
                    page = page.child(
                        h_flex()
                            .mx(px(10.))
                            .p(px(10.))
                            .gap(px(12.))
                            .child(glyph(Lucide::Clock, palette.icon).size(px(16.)))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .child(format!("Revoke {}?", title.to_lowercase()))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .line_height(px(16.))
                                            .text_color(palette.muted)
                                            .child("The link stops working. Copies people already made stay theirs."),
                                    ),
                            )
                            .child(
                                secondary("share-keep-version", None, "Cancel", palette).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.confirm_revoke = None;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                danger("share-confirm-revoke", "Revoke", palette).when(!self.busy, |button| {
                                    button.on_click(cx.listener(move |this, _, _, cx| {
                                        this.change(Verb::Delete, endpoint.clone(), None, cx);
                                    }))
                                }),
                            ),
                    );
                    continue;
                }
                let version_url = format!(
                    "{}/a/{}",
                    self.gateway.site_url(),
                    url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
                );
                let group = SharedString::from(format!("share-version-{id}"));
                let mut row = history_row(Lucide::Clock, title, detail, palette)
                    .id(group.clone())
                    .group(group.clone())
                    .cursor_pointer()
                    .when(!active, |row| row.opacity(0.55))
                    .on_click(move |_, _, cx| crate::external_link::open(&version_url, cx));
                if active {
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("share-revoke-{id}")))
                            .h(px(28.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .text_color(palette.faint)
                            .group_hover(group, |style| style.text_color(palette.muted))
                            .hover(|style| style.text_color(palette.foreground))
                            .child("Revoke")
                            .when(!self.busy, |button| {
                                button.on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.confirm_revoke = Some(id.clone());
                                    cx.notify();
                                }))
                            }),
                    );
                }
                page = page.child(row);
            }
        }
        let readers: Vec<&Value> = people.iter().filter(|p| word(p, "status") == "granted").collect();
        if !readers.is_empty() || matches!(self.access, Some(Err(_))) {
            page = page.child(section("Readers", palette));
        }
        if let Some(Err(error)) = &self.access {
            page = page.child(div().px(px(20.)).text_color(palette.destructive).child(error.clone()));
        }
        for person in readers {
            let email = word(person, "email");
            page = page.child(
                history_row(Lucide::User, email.clone(), "Can open this chat".into(), palette)
                    .child(self.access_button("remove", "Remove", false, &email, palette, cx)),
            );
        }
        if let Some(problem) = &self.problem {
            page = page.child(
                div()
                    .px(px(20.))
                    .pt(px(8.))
                    .text_color(palette.destructive)
                    .child(problem.clone()),
            );
        }
        page
    }
}

/// Who a version reaches, in the sheet's words.
fn audience_words(kind: &str, org: &str) -> String {
    match Audience::from_key(kind) {
        Audience::Private => "Only you".into(),
        Audience::Org => format!("Anyone at {org}"),
        Audience::Teams => "Teams".into(),
        Audience::People => format!("People at {org}"),
        Audience::External => format!("People outside {org}"),
        Audience::Public => "Anyone with the link".into(),
    }
}

fn danger(id: &'static str, label: &'static str, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .bg(palette.destructive)
        .text_color(white())
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .child(label)
}

fn switch_row(label: &'static str, palette: Palette) -> Div {
    h_flex()
        .h(px(44.))
        .px(px(20.))
        .gap(px(12.))
        .child(div().flex_1().text_color(palette.foreground).child(label))
}

fn section(label: &'static str, palette: Palette) -> impl IntoElement {
    div()
        .pt(px(12.))
        .px(px(20.))
        .pb(px(4.))
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(palette.faint)
        .child(label)
}

fn history_row(icon: Lucide, title: String, detail: String, palette: Palette) -> Div {
    h_flex()
        .min_h(px(48.))
        .mx(px(10.))
        .px(px(10.))
        .gap(px(12.))
        .rounded(px(6.))
        .hover(|style| style.bg(palette.menu_hover))
        .child(
            div()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .child(glyph(icon, palette.icon).size(px(16.))),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .child(div().truncate().line_height(px(18.)).child(title))
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(palette.muted)
                        .child(detail),
                ),
        )
}
