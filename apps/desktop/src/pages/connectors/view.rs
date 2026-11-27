use super::*;

impl ConnectorsPage {
    // -------------------------------------------------------------- Drawing, shared by both sides

    /// The organization's name in a sentence, or "your organization" before it is known.
    pub(super) fn org_words(&self) -> String {
        match self.ctx.org_name.trim() {
            "" => "your organization".into(),
            name => name.to_string(),
        }
    }

    /// A group's heading: its title, how many it holds, and Show or Hide when it folds.
    pub(super) fn render_group_head(
        &self,
        title: &'static str,
        count: usize,
        folds: bool,
        folded: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let head = ui::group_head(title.to_string(), palette).child(
            div()
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(count.to_string()),
        );
        if !folds || !self.filter.read(cx).value().trim().is_empty() {
            return head;
        }
        let toggle = ui::focusable(
            h_flex().id(SharedString::from(format!("connectors-fold-{title}"))),
            palette,
        )
        .flex_none()
        .gap(px(4.))
        .px(px(6.))
        .h(px(22.))
        .rounded(px(6.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.muted)
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover).text_color(palette.strong))
        .active(|style| style.opacity(0.8))
        .child(if folded { "Show" } else { "Hide" })
        .child(ui::icon(
            if folded { Lucide::ChevronDown } else { Lucide::ChevronUp },
            palette.icon,
        ))
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(at) = this.unfolded.iter().position(|item| *item == title) {
                this.unfolded.remove(at);
            } else {
                this.unfolded.push(title);
            }
            cx.notify();
        }));
        head.child(toggle)
    }

    /// One group of rows: its heading, then its card and footnote unless it is folded.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_group(
        &self,
        page: Div,
        title: &'static str,
        footnote: &'static str,
        folds: bool,
        cards: &[&Card],
        rights: Rights,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let filtering = !self.filter.read(cx).value().trim().is_empty();
        // A filter opens every group: a match is never hidden behind Show. So does a setup fix that opens on one of its
        // rows (`row_anchor`).
        let pending = self.ctx.settings_fields.pending();
        let asked_for = cards
            .iter()
            .any(|card| pending.is_some() && super::row_anchor(&card.connector.name) == pending);
        let folded = folds && !filtering && !asked_for && !self.unfolded.contains(&title);
        let page = page.child(self.render_group_head(title, cards.len(), folds, folded, palette, cx));
        if folded {
            return page;
        }
        let rows: Vec<Option<AnyElement>> = cards
            .iter()
            .map(|card| Some(self.render_card(card, rights, palette, cx).into_any_element()))
            .collect();
        let page = page.child(ui::group(rows, palette));
        if footnote.is_empty() {
            page
        } else {
            page.child(ui::footnote(footnote, palette))
        }
    }

    /// One connector inside its group's card: the row, and under it the open row's well.
    fn render_card(&self, card: &Card, rights: Rights, palette: Palette, cx: &mut Context<Self>) -> Div {
        let name = card.connector.name.clone();
        let open = self.open.as_deref() == Some(name.as_str());
        let toggle_card = card.clone();
        let dim = !card.connector.offered
            || match self.side {
                Side::Mine => card.connector.usable != Access::Usable,
                Side::Org => matches!(card.group(), Group::Unavailable | Group::Off),
            };
        let end: AnyElement = if self.side == Side::Mine && quick_connect(card, rights) {
            let busy = self.busy.is_some() || !self.refresh_errors.is_empty();
            let in_flight = self.busy.as_deref() == Some(format!("{name}|user|connect").as_str());
            let id = SharedString::from(format!("connector-quick-{name}"));
            let label = if in_flight { "Connecting…" } else { "Connect" };
            if busy {
                ui::disabled_button(id, label, palette).into_any_element()
            } else {
                let target = name.clone();
                let connect_card = card.clone();
                ui::button(id, label, palette)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        // The row's own press would toggle it: this one connects, and opens the row so its line
                        // ("Approve it at … in your browser") shows.
                        cx.stop_propagation();
                        if this.open.as_deref() != Some(target.as_str()) {
                            this.toggle(&connect_card, window, cx);
                        }
                        this.start_oauth(target.clone(), "user", cx);
                    }))
                    .into_any_element()
            }
        } else {
            let state = match self.side {
                Side::Mine => row_status(card, rights),
                Side::Org => org_status(card),
            };
            match state {
                Some(state) => ui::status(state.label, tone_color(state.tone, palette), palette).into_any_element(),
                None => div().into_any_element(),
            }
        };
        let head = ui::focusable(
            ui::card_row().id(SharedString::from(format!("connector-{name}"))),
            palette,
        )
        .cursor_pointer()
        .when(open, |row| row.bg(ui::inset(palette)))
        .when(!open, |row| row.hover(|style| style.bg(palette.hover)))
        .active(|style| style.opacity(0.8))
        .on_click(cx.listener(move |this, _, window, cx| this.toggle(&toggle_card, window, cx)))
        .child(service_tile(&card.name, palette))
        .child(
            ui::two_lines(card.name.clone(), card.line.clone(), palette)
                .flex_1()
                .min_w_0()
                .when(dim, |lines| lines.opacity(0.6)),
        )
        .child(end)
        .child(ui::icon(
            if open {
                Lucide::ChevronDown
            } else {
                Lucide::ChevronRight
            },
            palette.icon,
        ));
        let mut block = v_flex().child(head);
        if open {
            let details = match self.side {
                Side::Mine => self.render_mine_details(card, rights, palette, cx),
                Side::Org => self.render_org_details(card, rights, palette, cx),
            };
            block = block.child(details);
        }
        // A setup line opens Settings on this row (`Fix::open_at`).
        match super::row_anchor(&name) {
            Some(anchor) => self.ctx.settings_fields.field(anchor, block, None, cx),
            None => block,
        }
    }

    /// An open row's well, lined up with the row's text: the card's 16px inset, the 28px tile and the row's 16px gap.
    pub(super) fn well(&self, card: &Card, palette: Palette) -> Div {
        let mut details = v_flex()
            .pl(px(60.))
            .pr(px(16.))
            .pt(px(2.))
            .pb(px(16.))
            .gap(px(16.))
            .bg(ui::inset(palette));
        if let Some((key, result)) = &self.notice
            && *key == card.connector.name
        {
            details = details.child(notice_line(result, palette));
        }
        details
    }

    /// What agents can do with a connector: the counts folded, the list one click away. None when it lists nothing.
    pub(super) fn render_abilities(
        &self,
        card: &Card,
        note: &str,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let name = card.connector.name.clone();
        if card.connector.operations.is_empty() {
            return None;
        }
        let body = match self.abilities.get(&name) {
            None | Some(Load::Loading) => v_flex().child(ui::skeleton(1, palette)),
            Some(Load::Failed(error)) => v_flex().child(ui::error_line(
                format!("What agents can do could not be loaded: {error}"),
                palette,
            )),
            Some(Load::Ready(abilities)) => {
                let open = self.abilities_open.as_deref() == Some(name.as_str());
                let toggle = ui::ghost_button(
                    SharedString::from(format!("abilities-{name}")),
                    if open { "Hide" } else { "Show all" },
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_abilities(&name, cx)));
                let changes = abilities.iter().any(|item| item.changes);
                let summary = if changes {
                    format!("{} {note}", abilities_summary(abilities))
                } else {
                    abilities_summary(abilities)
                };
                let mut body = v_flex().gap(px(8.)).child(
                    h_flex()
                        .gap(px(8.))
                        .child(div().flex_1().min_w_0().child(ui::note(summary, palette)))
                        .child(toggle),
                );
                if open {
                    for (title, changes) in [("Read", false), ("Change", true)] {
                        let items: Vec<&Ability> = abilities.iter().filter(|item| item.changes == changes).collect();
                        if items.is_empty() {
                            continue;
                        }
                        body = body.child(
                            v_flex()
                                .gap(px(6.))
                                .child(ui::label(title, palette))
                                .children(items.into_iter().map(|item| ui::note(item.text.clone(), palette))),
                        );
                    }
                }
                body
            }
        };
        Some(block("What agents can do", None, body, palette))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_holder(
        &self,
        card: &Card,
        scope: &'static str,
        accounts: &[Connection],
        can_change: bool,
        can_connect: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let connector = &card.connector;
        let busy = self.busy.is_some() || !self.refresh_errors.is_empty();
        let mut holder = v_flex().gap(px(8.));
        for (ix, item) in accounts.iter().enumerate() {
            let mut sub: Vec<String> = Vec::new();
            sub.extend(item.identity.clone());
            sub.extend(item.last4.as_ref().map(|last4| format!("••••{last4}")));
            if !item.created_at.is_empty() {
                sub.push(format!("connected {}", ui::day(&item.created_at)));
            }
            let state = if item.suspended {
                status("Suspended", Tone::Off)
            } else if item.needs_reconnect {
                status("Needs reconnect", Tone::Warn)
            } else {
                status("Active", Tone::On)
            };
            let asking = can_change
                && self.confirm.as_ref() == Some(&(item.connector.clone(), item.account.clone(), item.scope.clone()));
            // An asked Disconnect turns the row into its question: what happens, then Cancel and the red action, so
            // the button keeps its width and the consequence sits beside it.
            let lines = if asking {
                v_flex()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.strong)
                            .truncate()
                            .child(format!("Disconnect {}?", item.name)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.destructive)
                            .child("Calls stop using it at once."),
                    )
            } else {
                ui::two_lines(item.name.clone(), sub.join(" · "), palette)
            };
            let mut row = h_flex()
                .min_h(px(48.))
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(10.))
                .bg(palette.raised)
                .when(asking, |row| row.border_1().border_color(palette.destructive))
                .child(lines.flex_1())
                .when(!asking, |row| {
                    row.child(ui::status(
                        state.label.clone(),
                        tone_color(state.tone, palette),
                        palette,
                    ))
                });
            if can_change {
                if !asking && can_connect && item.needs_reconnect && connector.oauth {
                    let target = connector.name.clone();
                    row = row.child(if busy {
                        ui::disabled_button(("reconnect", ix), "Reconnect", palette)
                    } else {
                        ui::primary_button(("reconnect", ix), "Reconnect", palette)
                            .on_click(cx.listener(move |this, _, _, cx| this.start_oauth(target.clone(), scope, cx)))
                    });
                }
                if asking {
                    row = row.child(
                        ui::ghost_button(SharedString::from(format!("keep-{scope}-{ix}")), "Cancel", palette).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.confirm = None;
                                cx.notify();
                            }),
                        ),
                    );
                }
                let id = SharedString::from(format!("disconnect-{scope}-{ix}"));
                let target = item.clone();
                row = row.child(if busy {
                    ui::disabled_button(id, "Disconnect", palette)
                } else if asking {
                    ui::danger_button(id, "Disconnect", palette)
                        .on_click(cx.listener(move |this, _, _, cx| this.disconnect(&target, cx)))
                } else {
                    ui::ghost_button(id, "Disconnect", palette)
                        .on_click(cx.listener(move |this, _, _, cx| this.disconnect(&target, cx)))
                });
            }
            holder = holder.child(row);
        }
        if can_connect {
            holder = holder.child(self.render_connect(card, scope, accounts, palette, cx));
        }
        holder
    }

    fn render_connect(
        &self,
        card: &Card,
        scope: &'static str,
        accounts: &[Connection],
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let connector = &card.connector;
        let another = !accounts.is_empty();
        let label = match (scope, another) {
            ("org", true) => "Add another organization account".to_string(),
            ("org", false) => format!("Connect the organization's {} account", card.name),
            (_, true) => "Add another account".to_string(),
            (_, false) => format!("Connect {}", card.name),
        };
        let mut form = v_flex().gap(px(8.)).pt(px(4.));
        if another {
            form = form.child(ui::note(
                format!(
                    "With a second {} account, agents ask which one to use each time.",
                    card.name
                ),
                palette,
            ));
        }
        if !connector.keyed && !connector.oauth {
            return form.child(ui::note(
                format!(
                    "This Gateway has no way to connect {} yet: ask whoever runs it.",
                    card.name
                ),
                palette,
            ));
        }
        let busy = self.busy.is_some() || !self.refresh_errors.is_empty();
        let id = SharedString::from(format!("connect-{scope}"));
        let in_flight = self.busy.as_deref() == Some(format!("{}|{scope}|connect", connector.name).as_str());
        let text = if in_flight { "Connecting…".to_string() } else { label };
        let key_open = self.key_form.as_deref() == Some(connector.name.as_str());
        // A second account is a quiet action; the first is the row's one primary.
        let button = |id: SharedString, text: String| {
            if another {
                ui::button(id, text, palette)
            } else {
                ui::primary_button(id, text, palette)
            }
        };
        if connector.oauth && !key_open {
            let target = connector.name.clone();
            let button = if busy {
                ui::disabled_button(id, text, palette)
            } else {
                button(id, text)
                    .on_click(cx.listener(move |this, _, _, cx| this.start_oauth(target.clone(), scope, cx)))
            };
            form = form.child(h_flex().gap(px(10.)).child(button).child(ui::note(
                format!("You approve it at {} in your browser.", card.name),
                palette,
            )));
            if connector.keyed {
                let target = connector.name.clone();
                form = form.child(
                    h_flex().child(
                        ui::ghost_button(
                            SharedString::from(format!("use-key-{scope}")),
                            "Use an API key instead",
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.key_form = Some(target.clone());
                            cx.notify();
                        })),
                    ),
                );
            }
            return form;
        }
        for field in &connector.fields {
            let Some(input) = self.inputs.get(&format!("{}|{scope}|{}", connector.name, field.name)) else {
                continue;
            };
            let title = if field.required {
                field.label.clone()
            } else {
                format!("{} (optional)", field.label)
            };
            form = form.child(
                v_flex()
                    .gap(px(4.))
                    .child(ui::label(title, palette))
                    .child(field_box(input, Some(360.), busy, palette))
                    .children(field.description.clone().map(|text| ui::note(text, palette))),
            );
        }
        let target = connector.clone();
        form.child(h_flex().child(if busy {
            ui::disabled_button(id, text, palette)
        } else {
            button(id, text).on_click(cx.listener(move |this, _, window, cx| this.connect(&target, scope, window, cx)))
        }))
    }

    /// The line under the page's lead: whose settings these are, and a link to the other side's page.
    pub(super) fn render_scope(
        &self,
        icon: Lucide,
        whose: String,
        link: Option<(&'static str, &'static str)>,
        palette: Palette,
    ) -> Div {
        let mut row = h_flex()
            .gap(px(12.))
            .mb(px(14.))
            .child(ui::scope_tag(icon, whose, palette));
        if let Some((text, address)) = link {
            let navigate = self.ctx.navigate_settings.clone();
            row = row.child(
                ui::focusable(
                    h_flex().id(SharedString::from(format!("connectors-other-{address}"))),
                    palette,
                )
                .gap(px(4.))
                .text_size(px(12.))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.text_color(palette.strong))
                .child(text)
                .child(ui::icon(Lucide::ChevronRight, palette.icon))
                .on_click(move |_, window, cx| navigate(address.to_string(), window, cx)),
            );
        }
        row
    }

    /// The filter and Refresh, over the cards, so Refresh shows without scrolling.
    pub(super) fn render_toolbar(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let refresh =
            ui::ghost_button("connectors-refresh", "Refresh", palette).on_click(cx.listener(|this, _, _, cx| {
                this.notice = None;
                this.confirm = None;
                this.abilities.clear();
                this.load(cx)
            }));
        toolbar(&self.filter, refresh, false, palette)
    }
}

pub(super) fn locked_toolbar(input: &Entity<InputState>, palette: Palette) -> Div {
    toolbar(
        input,
        ui::disabled_button("connectors-refresh", "Refresh", palette),
        true,
        palette,
    )
}

fn toolbar(input: &Entity<InputState>, refresh: Stateful<Div>, locked: bool, palette: Palette) -> Div {
    h_flex()
        .gap(px(8.))
        .child(
            h_flex()
                .flex_1()
                .min_w(px(0.))
                .h(px(ui::CONTROL_HEIGHT))
                .gap(px(8.))
                .px(px(10.))
                .rounded(px(8.))
                .bg(palette.hover)
                .text_size(px(13.))
                .child(ui::icon(Lucide::Search, palette.icon))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(Input::new(input).appearance(false).disabled(locked)),
                ),
        )
        .child(refresh)
}

/// A connector's logo on its tile, the marks the conversation's connector calls use (`assets::connector`), or its first
/// letter where the app has no mark.
fn service_tile(name: &str, palette: Palette) -> Div {
    match crate::assets::connector(name) {
        Some((path, color)) => ui::mark_tile(path, color, palette),
        None => ui::letter_tile(name, palette),
    }
}

/// A block of an open row: its title, an optional note, then its body.
pub(super) fn block(title: &str, note: Option<&str>, body: impl IntoElement, palette: Palette) -> Div {
    v_flex()
        .gap(px(6.))
        .child(ui::label(title.to_string(), palette))
        .children(note.map(|text| ui::note(text.to_string(), palette)))
        .child(body)
}

/// The last action's result: a plain line, or the refusal in the error color.
pub(super) fn notice_line(result: &Result<String, String>, palette: Palette) -> Div {
    match result {
        Ok(text) => div()
            .text_size(px(13.))
            .text_color(palette.foreground)
            .child(text.clone()),
        Err(text) => ui::error_line(text.clone(), palette),
    }
}

/// A text field of an open row: raised on the row's well, so it reads on both themes' wells.
pub(super) fn field_box(input: &Entity<InputState>, width: Option<f32>, disabled: bool, palette: Palette) -> Div {
    h_flex()
        .h(px(ui::CONTROL_HEIGHT))
        .px(px(10.))
        .rounded(px(8.))
        .bg(palette.raised)
        .text_size(px(13.))
        .text_color(palette.foreground)
        .map(|field| match width {
            Some(width) => field.w(px(width)),
            None => field.flex_1().min_w(px(0.)),
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .child(Input::new(input).appearance(false).disabled(disabled)),
        )
}

impl Render for ConnectorsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        match self.side {
            Side::Mine => self.render_mine_page(palette, cx),
            Side::Org => self.render_org_page(palette, cx),
        }
    }
}
