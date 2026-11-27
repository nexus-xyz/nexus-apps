//! The page's Share sheet: the open link on top, who can open it as the chat's audience list, then what readers can
//! do, the public link's expiry and the version they see. One primary at the bottom right.

use super::share::{editable, expiry_choices, link};
use super::*;
use crate::pages::ui;
use crate::palette::Palette;
use crate::workspace::glyph;
use crate::workspace::share_ui::{blocked_row_end, card_shadow, checkbox, secondary};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Input;
use gpui_kit::component::{h_flex, v_flex};

/// Where a row's picks and notes start: under its label, past the icon.
const INDENT: f32 = 38.;

impl LibraryDialog {
    pub(super) fn render_share(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut sheet = v_flex().child(self.header("Share Artifact", palette, cx)).child(
            v_flex()
                .px(px(20.))
                .pt(px(4.))
                .pb(px(12.))
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .line_height(px(20.))
                        .text_color(palette.muted)
                        .child(self.row.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(palette.faint)
                        .child("Readers open it in a sandbox. Its scripts can't reach your account."),
                ),
        );
        if !self.ready {
            return sheet.child(self.share_unready(palette, cx)).into_any_element();
        }
        let state = &self.share;
        let org = self.org();
        sheet = sheet.children(self.link_card(palette, cx));
        sheet = sheet.child(note("Who can open it", palette).pt(px(4.)));

        let mut list = v_flex().px(px(10.)).gap(px(2.));
        for kind in [
            Audience::Private,
            Audience::Org,
            Audience::Teams,
            Audience::People,
            Audience::External,
            Audience::Public,
        ] {
            if state.personal && kind == Audience::Org {
                continue;
            }
            list = list.child(self.audience_row(kind, &org, palette, cx));
            if kind == state.kind {
                list = list.children(self.audience_choices(palette, cx));
            }
        }
        sheet = sheet.child(list);

        if state.kind != Audience::Private {
            sheet = sheet.child(div().h(px(1.)).mx(px(20.)).mt(px(12.)).mb(px(4.)).bg(palette.border));
        }
        if editable(state.kind) {
            let edit = state.edit;
            let tabs = ui::tabs(palette)
                .child(
                    ui::tab("share-role-view", "View", !edit, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.set_edit(false, cx))),
                )
                .child(
                    ui::tab("share-role-edit", "Edit", edit, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.set_edit(true, cx))),
                );
            sheet = sheet.child(option("Readers can", None, palette).child(tabs));
        }
        if state.kind == Audience::Public {
            sheet = sheet.child(self.expiry_row(&org, palette, cx));
        }
        if state.kind != Audience::Private {
            let caption = if state.edit {
                "Editors always write the latest version.".to_string()
            } else if state.follow_latest {
                "Readers see each new version as you write it.".to_string()
            } else {
                let pinned = state.newest().and_then(|share| share["pageVersion"].as_u64());
                format!("Readers see version {} only.", pinned.unwrap_or(state.latest_version))
            };
            let locked = self.busy || state.edit;
            sheet = sheet.child(option("Always show the latest version", Some(caption), palette).child(
                ui::switch("share-latest", state.follow_latest || state.edit, locked, palette).when(
                    !locked,
                    |switch| {
                        switch.on_click(cx.listener(|this, _, _, cx| {
                            this.share.follow_latest = !this.share.follow_latest;
                            this.share.copied = false;
                            cx.notify();
                        }))
                    },
                ),
            ));
        }
        sheet = sheet.children(self.problem_line(palette));
        sheet.child(self.share_footer(palette, cx)).into_any_element()
    }

    fn org(&self) -> String {
        self.org_name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "your organization".into())
    }

    fn set_edit(&mut self, edit: bool, cx: &mut Context<Self>) {
        if !self.busy {
            self.share.edit = edit;
            self.share.copied = false;
            cx.notify();
        }
    }

    /// Loading rows, or the error with Retry.
    fn share_unready(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.busy {
            let mut rows = v_flex().px(px(10.)).pb(px(20.)).gap(px(2.));
            for width in [140., 120., 90., 130., 200., 150.] {
                rows = rows.child(
                    h_flex()
                        .h(px(36.))
                        .px(px(10.))
                        .gap(px(12.))
                        .child(div().size(px(16.)).rounded(px(4.)).bg(palette.menu_hover))
                        .child(div().h(px(12.)).w(px(width)).rounded(px(6.)).bg(palette.menu_hover)),
                );
            }
            return rows.into_any_element();
        }
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
                        self.problem
                            .clone()
                            .unwrap_or_else(|| "Couldn't load sharing for this Artifact. Nothing was changed.".into()),
                    ),
            )
            .child(
                secondary("share-retry", Some(Lucide::RotateCcw), "Retry", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
            )
            .into_any_element()
    }

    /// The open link with Copy link, and a line that says what it shares.
    fn link_card(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let share = self.share.newest()?;
        let url = link(share, &self.gateway.site_url())?;
        let mut status = vec![
            share["pageVersion"]
                .as_u64()
                .map_or("Latest version".to_string(), |version| format!("Version {version}")),
            audience_words(share, &self.org()),
        ];
        if share["pageRole"] == "edit" {
            status.push("can edit".into());
        }
        if let Some(at) = share["expiresAt"].as_str() {
            status.push(format!("expires {}", at.get(..10).unwrap_or(at)));
        }
        let copied = self.share.copied;
        Some(
            v_flex()
                .pb(px(4.))
                .child(
                    h_flex()
                        .mx(px(20.))
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
                                .font_family(ui::MONO)
                                .text_size(px(12.))
                                .child(url.clone()),
                        )
                        .child(
                            secondary(
                                "share-copy",
                                Some(if copied { Lucide::Check } else { Lucide::Copy }),
                                if copied { "Copied" } else { "Copy link" },
                                palette,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                this.share.copied = true;
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    div()
                        .h(px(28.))
                        .px(px(20.))
                        .flex()
                        .items_center()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(status.join(" · ")),
                )
                .into_any_element(),
        )
    }

    fn audience_row(&self, kind: Audience, org: &str, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let (icon, label) = match kind {
            Audience::Private => (Lucide::Lock, "Only you".to_string()),
            Audience::Org => (Lucide::Building2, format!("Anyone at {org}")),
            Audience::Teams => (Lucide::Users, "Teams".to_string()),
            Audience::People => (Lucide::User, format!("People at {org}")),
            Audience::External => (Lucide::Mail, format!("People outside {org} by email")),
            Audience::Public => (Lucide::Link, "Anyone with the link".to_string()),
        };
        let blocked = self.share.blocked(kind);
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
            .when(kind == self.share.kind, |row| {
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
                org,
                close,
                &self.fixer,
                palette,
                cx,
            ));
        } else if !self.busy {
            row = row
                .cursor_pointer()
                .hover(|style| style.bg(palette.menu_hover))
                .on_click(cx.listener(move |this, _, window, cx| this.pick_audience(kind, window, cx)));
        }
        row.into_any_element()
    }

    /// Under the chosen audience: the teams or people to tick, the addresses to type, or what a public link means.
    fn audience_choices(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = &self.share;
        let hint = |text: &'static str| {
            div()
                .pt(px(6.))
                .pr(px(10.))
                .pb(px(4.))
                .pl(px(INDENT))
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(palette.faint)
                .child(text)
        };
        let picks = match state.kind {
            Audience::Public => {
                return Some(hint("No sign-in needed. Search engines don't list it.").into_any_element());
            }
            Audience::External => {
                return Some(
                    v_flex()
                        .child(
                            div()
                                .ml(px(INDENT))
                                .mr(px(10.))
                                .min_h(px(32.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .bg(palette.menu_hover)
                                .child(Input::new(&state.outside).appearance(false).disabled(self.busy)),
                        )
                        .child(hint(
                            "They sign in with these addresses to open it. They see your organization's name, not yours.",
                        ))
                        .into_any_element(),
                );
            }
            Audience::Teams => &state.teams,
            Audience::People => &state.people,
            _ => return None,
        };
        let mut rows = v_flex();
        match picks {
            Err(error) => {
                rows = rows.child(
                    div()
                        .pl(px(INDENT))
                        .py(px(6.))
                        .text_color(palette.destructive)
                        .child(error.clone()),
                )
            }
            Ok(picks) if picks.is_empty() => {
                let what = if state.kind == Audience::Teams {
                    "teams"
                } else {
                    "other people"
                };
                rows = rows.child(
                    h_flex()
                        .h(px(32.))
                        .pl(px(INDENT))
                        .text_color(palette.faint)
                        .child(format!("No {what} at {} yet", self.org())),
                )
            }
            Ok(picks) => {
                for (id, label) in picks {
                    let checked = state.picks.contains(id);
                    let id = id.clone();
                    rows = rows.child(
                        h_flex()
                            .id(SharedString::from(format!("share-pick-{id}")))
                            .h(px(32.))
                            .pl(px(INDENT))
                            .pr(px(10.))
                            .gap(px(10.))
                            .rounded(px(6.))
                            .child(checkbox(checked, palette))
                            .child(div().flex_1().min_w(px(0.)).truncate().child(label.clone()))
                            .when(label != &id, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .font_family(ui::MONO)
                                        .text_size(px(12.))
                                        .text_color(palette.faint)
                                        .child(id.clone()),
                                )
                            })
                            .when(!self.busy, |row| {
                                row.cursor_pointer()
                                    .hover(|style| style.bg(palette.menu_hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let picks = &mut this.share.picks;
                                        if picks.contains(&id) {
                                            picks.retain(|picked| picked != &id);
                                        } else {
                                            picks.push(id.clone());
                                        }
                                        this.share.copied = false;
                                        this.problem = None;
                                        cx.notify();
                                    }))
                            }),
                    );
                }
                if state.kind == Audience::Teams {
                    rows = rows.child(hint("Members of a team and of the teams under it."));
                }
            }
        }
        Some(rows.into_any_element())
    }

    /// The public link's expiry: the organization's choices, and the open link's own expiry while it is kept.
    fn expiry_row(&self, org: &str, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.share;
        let mut choices = expiry_choices(&state.policy);
        if let Some(at) = state.newest().and_then(|share| share["expiresAt"].as_str()) {
            choices.insert(0, ("current".into(), format!("Until {}", at.get(..10).unwrap_or(at))));
        }
        let mut tabs = ui::tabs(palette).flex_wrap();
        for (value, label) in choices {
            let picked = state.expiry == value;
            tabs = tabs.child(
                ui::tab(
                    SharedString::from(format!("share-expiry-{value}")),
                    label,
                    picked,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.busy {
                        this.share.expiry = value.clone();
                        this.share.copied = false;
                        cx.notify();
                    }
                })),
            );
        }
        let caption = state.policy["artifacts.external.expiry"]["maxDays"]
            .as_u64()
            .map(|days| format!("{org} allows at most {days} days."));
        option("Link expires", caption, palette).child(tabs).into_any_element()
    }

    /// Stop sharing on the left once shared; the one primary on the right.
    fn share_footer(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.share;
        let shared = state.newest().is_some();
        let missing = match state.kind {
            Audience::Teams | Audience::People | Audience::External => state.chosen(cx).is_empty(),
            Audience::Private => !shared,
            _ => false,
        };
        let label = if shared { "Save" } else { "Share and copy link" };
        let primary = if self.busy {
            ui::disabled_button("share-save", "Saving…", palette).into_any_element()
        } else if missing || (shared && !state.changed(cx)) {
            ui::disabled_button("share-save", label, palette).into_any_element()
        } else {
            ui::primary_button("share-save", label, palette)
                .on_click(cx.listener(|this, _, _, cx| this.save_share(cx)))
                .into_any_element()
        };
        h_flex()
            .pt(px(16.))
            .px(px(20.))
            .pb(px(20.))
            .gap(px(8.))
            .when(shared, |footer| {
                footer.child(
                    secondary("share-stop", None, "Stop sharing", palette)
                        .h(px(32.))
                        .when(!self.busy, |button| {
                            button.on_click(cx.listener(|this, _, _, cx| this.stop_sharing(cx)))
                        }),
                )
            })
            .child(div().flex_1())
            .child(primary)
            .into_any_element()
    }
}

/// A small faint line above a group.
fn note(text: &'static str, palette: Palette) -> Div {
    div()
        .pt(px(8.))
        .px(px(20.))
        .pb(px(4.))
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(palette.faint)
        .child(text)
}

/// A setting under the audiences: its name and what it means on the left, its control on the right.
fn option(label: &'static str, caption: Option<String>, palette: Palette) -> Div {
    h_flex().min_h(px(44.)).px(px(20.)).py(px(6.)).gap(px(12.)).child(
        v_flex()
            .flex_1()
            .min_w(px(0.))
            .child(div().line_height(px(20.)).child(label))
            .children(caption.map(|caption| {
                div()
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(palette.faint)
                    .child(caption)
            })),
    )
}

/// Who a share reaches, in the sheet's words.
fn audience_words(share: &Value, org: &str) -> String {
    let audience = &share["audience"];
    let count = |key: &str| audience[key].as_array().map_or(0, Vec::len);
    match Audience::from_key(audience["kind"].as_str().unwrap_or_default()) {
        Audience::Private => "Only you".into(),
        Audience::Org => format!("Anyone at {org}"),
        Audience::Teams => plural(count("teams"), "team"),
        Audience::People => format!("{} at {org}", plural(count("people"), "person")),
        Audience::External => format!("{} outside {org}", plural(count("people"), "person")),
        Audience::Public => "Anyone with the link".into(),
    }
}

fn plural(count: usize, one: &str) -> String {
    match (count, one) {
        (1, _) => format!("1 {one}"),
        (_, "person") => format!("{count} people"),
        _ => format!("{count} {one}s"),
    }
}
