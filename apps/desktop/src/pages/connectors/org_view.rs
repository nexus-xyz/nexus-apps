//! Settings › Integrations: the organization's rules, for owners and admins. A row opens on who can use it, then the
//! organization's own account, then what agents can do. No person's own account shows here.

use super::*;

impl ConnectorsPage {
    pub(super) fn render_org_page(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let org = self.org_words();
        let mut page = v_flex().w_full().child(ui::lead(
            format!("Who in {org} can use each connector, whose account it uses, and what agents may change."),
            palette,
        ));
        let rights = match &self.me {
            Load::Loading => return page.child(ui::skeleton(5, palette).mt(px(16.))),
            Load::Failed(error) => return page.child(ui::failed(error.clone(), palette)),
            Load::Ready(rights) => *rights,
        };
        if !rights.manage {
            return page.child(ui::empty(
                "Only owners and admins change these",
                "Your own accounts are in Connections.",
                palette,
            ));
        }
        page = page
            .child(self.render_scope(
                Lucide::Building2,
                format!("Applies to everyone in {org}"),
                Some(("Your own accounts are in Connections", MINE_ADDRESS)),
                palette,
            ))
            .child(self.render_toolbar(palette, cx));
        if !self.refresh_errors.is_empty() {
            page = page.child(self.refresh_banner(palette));
        }
        if let Some(Load::Failed(error)) = &self.availability {
            page = page.child(
                ui::error_line(
                    format!("Who can use each connector could not be loaded: {error}"),
                    palette,
                )
                .mt(px(12.)),
            );
        }
        page = page.child(self.render_writes(palette, cx));
        let waiting = matches!(self.connections, Load::Loading) || matches!(self.availability, Some(Load::Loading));
        let connectors = match &self.catalog {
            Load::Loading => return page.child(ui::skeleton(5, palette).mt(px(24.))),
            Load::Failed(error) => {
                return page.child(ui::failed(
                    format!("The connectors could not be loaded: {error}"),
                    palette,
                ));
            }
            Load::Ready(_) if waiting => return page.child(ui::skeleton(5, palette).mt(px(24.))),
            Load::Ready(connectors) => connectors,
        };
        // The organization's accounts only: a person's own never shows on this page.
        let connections: Vec<Connection> = match &self.connections {
            Load::Ready(list) => list.iter().filter(|item| item.scope == "org").cloned().collect(),
            _ => Vec::new(),
        };
        let availability = match &self.availability {
            Some(Load::Ready(list)) => Some(list.as_slice()),
            _ => None,
        };
        let groups = group_cards(connectors, &connections, availability, rights);
        if groups.is_empty() {
            return page.child(ui::empty(
                "No connectors are offered on this Gateway",
                "Ask whoever runs it to offer a connector, then refresh this page.",
                palette,
            ));
        }
        let query = self.filter.read(cx).value().to_string();
        let mut shown = 0;
        for (group, cards) in &groups {
            let cards: Vec<&Card> = cards.iter().filter(|card| matches_filter(card, &query)).collect();
            if cards.is_empty() {
                continue;
            }
            shown += cards.len();
            let (_, title, footnote) = GROUPS
                .iter()
                .find(|(id, _, _)| id == group)
                .copied()
                .unwrap_or((*group, "", ""));
            page = self.render_group(page, title, footnote, group.folds(), &cards, rights, palette, cx);
        }
        if shown == 0 {
            page = page.child(ui::empty(
                "No connector matches",
                format!("Nothing here is called \"{}\". Try another name.", query.trim()),
                palette,
            ));
        }
        page
    }

    pub(super) fn render_org_details(
        &self,
        card: &Card,
        rights: Rights,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let connector = &card.connector;
        let name = connector.name.clone();
        let mut details = self.well(card, palette);
        match (&card.availability, connector.offered) {
            (Some(availability), true) => {
                details = details.child(self.render_access(card, availability, palette, cx));
            }
            (Some(availability), false) => {
                let mut row = h_flex().gap(px(10.)).child(ui::note(
                    format!(
                        "In preview: this Gateway does not offer {} yet. Chosen here: {}.",
                        card.name,
                        level_label(&availability.level)
                    ),
                    palette,
                ));
                if availability.level != "disabled" {
                    let busy = self.busy.is_some() || !self.refresh_errors.is_empty();
                    let target = name.clone();
                    row = row.child(if busy {
                        ui::disabled_button(("turn-off", 0usize), "Turn off", palette)
                    } else {
                        ui::button(("turn-off", 0usize), "Turn off", palette).on_click(
                            cx.listener(move |this, _, _, cx| this.set_availability(target.clone(), "disabled", cx)),
                        )
                    });
                }
                details = details.child(block("Who can use it", None, row, palette));
            }
            (None, _) => {}
        }
        let takes_org = connector.connects.iter().any(|scope| scope == "org");
        if takes_org || !card.org.is_empty() {
            details = details.child(block(
                &format!("{}'s account", self.org_words()),
                Some("Agents use it for people without their own account while the connector is on for everyone."),
                self.render_holder(
                    card,
                    "org",
                    &card.org,
                    rights.manage && takes_org,
                    rights.manage && takes_org && connector.offered,
                    palette,
                    cx,
                ),
                palette,
            ));
        } else if connector.connects.is_empty() {
            details = details.child(ui::note(
                "This connector uses your organization's own setup. No account is added here.",
                palette,
            ));
        }
        if let Some(abilities) = self.render_abilities(
            card,
            "Changes run only with a write grant, set per module.",
            palette,
            cx,
        ) {
            details = details.child(abilities);
        }
        if connector.offered {
            let open_web = self.ctx.clone();
            let path = format!("settings/connectors?open={}", segment(&name));
            details = details.child(
                ui::web_link(
                    SharedString::from(format!("writes-{name}")),
                    "Write grants per module, in the browser",
                    palette,
                )
                .on_click(move |_, _, cx| open_web.open_web(&path, cx)),
            );
        }
        details
    }

    /// Who can use it: one choice of three, what it means in one sentence, and the teams it narrows to.
    fn render_access(&self, card: &Card, availability: &Availability, palette: Palette, cx: &mut Context<Self>) -> Div {
        let name = card.connector.name.clone();
        let busy = self.busy.is_some() || !self.refresh_errors.is_empty();
        // One choice of three: a segmented control, as the MCP editor's Local or Remote.
        let chips = h_flex().child(ui::tabs(palette).bg(ui::thumb(palette).opacity(0.5)).children(
            LEVELS.iter().enumerate().map(|(ix, (level, label))| {
                let picked = availability.level == *level;
                let target = name.clone();
                let level: &'static str = level;
                ui::tab(("level", ix), *label, picked, palette).when(!picked && !busy, |tab| {
                    tab.on_click(cx.listener(move |this, _, _, cx| this.set_availability(target.clone(), level, cx)))
                })
            }),
        ));
        let teams = (availability.level != "disabled")
            .then(|| self.inputs.get(&format!("{name}|teams")))
            .flatten()
            .map(|input| {
                let target = name.clone();
                let current = availability.level.clone();
                v_flex()
                    .gap(px(4.))
                    .child(ui::label("Only these teams", palette))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(field_box(input, Some(300.), busy, palette))
                            .child(if busy {
                                ui::disabled_button("teams-save", "Save", palette)
                            } else {
                                ui::button("teams-save", "Save", palette).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        let level = LEVELS
                                            .iter()
                                            .map(|(id, _)| *id)
                                            .find(|id| *id == current)
                                            .unwrap_or("disabled");
                                        this.set_availability(target.clone(), level, cx)
                                    },
                                ))
                            }),
                    )
            });
        block(
            "Who can use it",
            None,
            v_flex()
                .gap(px(8.))
                .child(chips)
                .child(ui::note(level_consequence(availability, &self.org_words()), palette))
                .children(teams),
            palette,
        )
    }

    /// The Writes card: chat's switch, and the browser row where the per-module write grants live.
    fn render_writes(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let chat = match self.chat.as_ref() {
            None => None,
            Some(Load::Loading) => Some(ui::card_row().child(ui::skeleton(1, palette).flex_1())),
            Some(Load::Failed(error)) => Some(ui::card_row().child(ui::error_line(
                format!("The organization's chat-write setting could not be loaded: {error}"),
                palette,
            ))),
            Some(Load::Ready(enabled)) => {
                let enabled = *enabled;
                let detail = if enabled {
                    "Agents in chat may change things. Each connector chooses which operations."
                } else {
                    "Agents in chat change nothing. Modules still write with their grants."
                };
                let busy = self.busy.as_deref() == Some("chat") || !self.refresh_errors.is_empty();
                let switch = ui::switch("chat-writes", enabled, busy, palette);
                let switch = if busy {
                    switch
                } else {
                    switch.on_click(cx.listener(move |this, _, _, cx| this.set_chat(!enabled, cx)))
                };
                Some(ui::setting_row("Changes from chat", detail, switch, palette))
            }
        };
        let web = self.ctx.clone();
        let permissions = ui::nav_row(
            "connectors-write-handoff",
            Lucide::PenLine,
            "Write grants per module",
            "Which modules may change what, connector by connector. Refresh here after a change.",
            true,
            palette,
        )
        .on_click(move |_, _, cx| web.open_web("settings/connectors", cx));
        let notice = match &self.notice {
            Some((key, result)) if key.is_empty() => Some(notice_line(result, palette).mt(px(8.))),
            _ => None,
        };
        v_flex()
            .child(ui::group_head("What agents may change", palette))
            .child(ui::group(
                [
                    chat.map(IntoElement::into_any_element),
                    Some(permissions.into_any_element()),
                ],
                palette,
            ))
            .children(notice)
    }
}
