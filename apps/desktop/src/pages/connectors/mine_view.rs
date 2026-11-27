//! Settings › Connections: the person's own accounts. A row opens on its action (Connect, Reconnect, Disconnect),
//! then what agents can do, then one line on what the organization decided.

use super::*;

impl ConnectorsPage {
    pub(super) fn render_mine_page(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let mut page = v_flex().w_full().child(ui::lead(
            "Your own accounts on other services. Agents act as you through them.",
            palette,
        ));
        let rights = match &self.me {
            Load::Loading => return page.child(ui::skeleton(5, palette).mt(px(16.))),
            Load::Failed(error) => return page.child(ui::failed(error.clone(), palette)),
            Load::Ready(rights) => *rights,
        };
        let link = rights.manage.then_some((ORG_RULES_LINK, ORG_ADDRESS));
        page = page
            .child(self.render_scope(Lucide::User, "Only you see and change these".into(), link, palette))
            .child(self.render_toolbar(palette, cx));
        if !self.refresh_errors.is_empty() {
            page = page.child(self.refresh_banner(palette));
        }
        if let Load::Failed(error) = &self.connections {
            page = page
                .child(ui::error_line(format!("Your connections could not be loaded: {error}"), palette).mt(px(12.)));
        }
        let connectors = match &self.catalog {
            Load::Loading => return page.child(ui::skeleton(5, palette).mt(px(24.))),
            Load::Failed(error) => {
                return page.child(ui::failed(
                    format!("The connectors could not be loaded: {error}"),
                    palette,
                ));
            }
            Load::Ready(_) if matches!(self.connections, Load::Loading) => {
                return page.child(ui::skeleton(5, palette).mt(px(24.)));
            }
            Load::Ready(connectors) => connectors,
        };
        let connections: &[Connection] = match &self.connections {
            Load::Ready(list) => list,
            _ => &[],
        };
        let shelves = mine_shelves(connectors, connections, rights);
        if shelves.is_empty() {
            return page.child(ui::empty(
                "No connectors are offered on this Gateway",
                "Ask your administrator to make a connector available, then refresh this page.",
                palette,
            ));
        }
        let query = self.filter.read(cx).value().to_string();
        let mut shown = 0;
        for (shelf, cards) in &shelves {
            let cards: Vec<&Card> = cards.iter().filter(|card| matches_filter(card, &query)).collect();
            if cards.is_empty() {
                continue;
            }
            shown += cards.len();
            // Not available to you: a lock, the reason and Ask in place of the footnote, under the open group.
            let unavailable = *shelf == Shelf::Unavailable;
            page = self.render_group(
                page,
                shelf.title(),
                shelf.footnote(),
                shelf.folds(),
                &cards,
                rights,
                palette,
                cx,
            );
            let folded = shelf.folds() && query.trim().is_empty() && !self.unfolded.contains(&shelf.title());
            if unavailable && !folded {
                let line = self.turned_off(
                    "connectors:unavailable".to_string(),
                    format!("{} hasn't turned these on for you", self.org_words()),
                    "Could you turn on a connector for me in Nexus?".to_string(),
                );
                page = page.child(div().mt(px(8.)).child(line.render(&self.ctx.fixer, palette, cx)));
            }
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

    pub(super) fn render_mine_details(
        &self,
        card: &Card,
        rights: Rights,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let connector = &card.connector;
        let mut details = self.well(card, palette);
        if !connector.offered {
            details = details.child(ui::note(
                format!(
                    "In preview: this Gateway does not offer {} yet. Your accounts are kept, and no call uses them.",
                    card.name
                ),
                palette,
            ));
        }
        let connects_own = rights.own && connector.connects.iter().any(|scope| scope == "user");
        let usable = connector.usable == Access::Usable;
        // The action first: the person's accounts and Connect, or why there is nothing to connect.
        details = match (connects_own, connector.usable) {
            (_, Access::Unavailable) => details.child(
                self.turned_off(
                    format!("connector:{}", connector.name),
                    format!("{} hasn't turned this on for you", self.org_words()),
                    format!("Could you turn on {} for me in Nexus?", card.name),
                )
                .render(&self.ctx.fixer, palette, cx),
            ),
            (_, Access::Unknown) => details.child(ui::note(
                format!(
                    "Could not check whether {} is available to you. Press Refresh to try again.",
                    card.name
                ),
                palette,
            )),
            (true, Access::Usable) => details.child(block(
                "Your account",
                None,
                self.render_holder(card, "user", &card.mine, true, connector.offered, palette, cx),
                palette,
            )),
            (false, Access::Usable) => details.child(ui::note(
                "Agents use your organization's setup for this. You have nothing to connect.",
                palette,
            )),
        };
        // Accounts held where access was refused or unknown can still be disconnected.
        if !usable && !card.mine.is_empty() && rights.own {
            details = details.child(block(
                "Your account",
                None,
                self.render_holder(card, "user", &card.mine, true, false, palette, cx),
                palette,
            ));
        }
        if let Some(abilities) = self.render_abilities(
            card,
            "Changes run only where your organization allows them.",
            palette,
            cx,
        ) {
            details = details.child(abilities);
        }
        if usable && rights.manage {
            let navigate = self.ctx.navigate_settings.clone();
            details = details.child(
                h_flex().child(
                    ui::ghost_button(
                        SharedString::from(format!("connector-rules-{}", connector.name)),
                        "Change who can use it, in Integrations",
                        palette,
                    )
                    .on_click(move |_, window, cx| navigate(ORG_ADDRESS.to_string(), window, cx)),
                ),
            );
        }
        details
    }

    /// A connector the organization has not turned on for this person: owners and admins turn it on in Integrations,
    /// anyone else asks them.
    fn turned_off(&self, key: String, reason: String, message: String) -> crate::setup::Locked {
        crate::setup::Locked::new(key, reason)
            .fix(
                "Turn on",
                crate::setup::Fix::open(crate::pages::PageId::ConnectorRules).needs("policy.manage"),
            )
            .ask(message)
    }

    /// The page's one banner: a refresh that failed pauses every change.
    pub(super) fn refresh_banner(&self, palette: Palette) -> Div {
        let mut errors: Vec<_> = self
            .refresh_errors
            .iter()
            .map(|(part, error)| format!("{part}: {error}"))
            .collect();
        errors.sort();
        ui::settings_notice(
            "Could not refresh",
            format!(
                "Showing the last loaded snapshot where available. Editing is paused. Retry with Refresh. {}",
                errors.join(" · ")
            ),
            palette,
        )
        .mt(px(12.))
    }
}

/// The link from a person's page to the organization's, for owners and admins.
const ORG_RULES_LINK: &str = "Who can use each one is in Integrations";
