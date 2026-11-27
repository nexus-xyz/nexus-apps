use super::*;

fn create_team_button(palette: Palette, enabled: bool) -> Stateful<Div> {
    if enabled {
        ui::primary_button("teams-create-open", "Create team", palette)
    } else {
        ui::disabled_button("teams-create-open", "Create team", palette)
    }
}

pub(in crate::pages) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        v_flex().child(create_team_button(palette, false)).into_any_element()
    })
}

impl Render for TeamsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let me = match &self.me {
            Load::Ready(me) => me.clone(),
            Load::Loading => return v_flex().w_full().child(ui::skeleton(6, palette)),
            Load::Failed(message) => return v_flex().w_full().child(self.failed(message.clone(), palette, cx)),
        };
        if me.personal {
            return v_flex().w_full().child(ui::lead("Teams group the people of an organization.", palette)).child(
                ui::note(
                    "A personal organization is just you, so it has no teams. To work with others, create an organization from the switcher.",
                    palette,
                ),
            );
        }
        if self.open.is_some() {
            return self.render_team(&me, palette, cx);
        }
        self.render_tree(&me, palette, cx)
    }
}

impl TeamsPage {
    fn failed(&self, message: SharedString, palette: Palette, cx: &mut Context<Self>) -> Div {
        ui::failed(message, palette).child(ui::button("teams-retry", "Try again", palette).mt(px(8.)).on_click(
            cx.listener(|this, _, window, cx| {
                match this.open.clone() {
                    Some(slug) => this.open_team(slug, window, cx),
                    None => this.load(cx),
                }
                cx.notify();
            }),
        ))
    }

    fn render_tree(&self, me: &Me, palette: Palette, cx: &mut Context<Self>) -> Div {
        let lead = h_flex()
            .justify_between()
            .items_start()
            .gap(px(12.))
            .child(ui::lead(
                "Spend goes to the team a caller names, else to their default team.",
                palette,
            ))
            .when(me.manage && !self.creating, |row| {
                row.child(
                    create_team_button(palette, true).on_click(cx.listener(|this, _, window, cx| {
                        this.creating = true;
                        this.create_outcome = None;
                        this.create.name.update(cx, |state, cx| state.focus(window, cx));
                        cx.notify();
                    })),
                )
            });
        let mut page = v_flex().w_full().child(lead);
        if self.creating {
            page = page.child(self.render_create(palette, cx));
        }
        let rows = match &self.tree {
            Load::Loading => return page.child(ui::skeleton(6, palette)),
            Load::Failed(message) => return page.child(self.failed(message.clone(), palette, cx)),
            Load::Ready(rows) => rows,
        };
        let deepest = rows.iter().map(|row| row.depth).max().unwrap_or(0) as u64;
        let people: u64 = rows.iter().filter(|row| row.depth == 1).map(|row| row.headcount).sum();
        let meta = format!(
            "{} · {} · {}",
            plural(rows.len() as u64, "team", "teams"),
            plural(deepest, "level", "levels"),
            plural(people, "person", "people")
        );
        page = page.child(
            h_flex()
                .justify_between()
                .items_end()
                .child(ui::section_head("All teams", palette))
                .child(ui::label(meta, palette).mb(px(10.))),
        );
        if rows.is_empty() {
            page = page.child(ui::empty(
                "No teams yet",
                if me.manage {
                    "Create the first one."
                } else {
                    "An owner or admin of your organization creates them."
                },
                palette,
            ));
        } else {
            page = page
                .child(
                    ui::head_row(palette)
                        .child(div().flex_1().child("Team"))
                        .child(div().w(px(200.)).flex_none().child("Leads"))
                        .child(div().w(px(110.)).flex_none().text_right().child("People"))
                        .child(div().w(px(80.)).flex_none().child("You")),
                )
                .children(self.blocks.iter().enumerate().map(|(ix, block)| {
                    match block {
                        Block::Band(line) => self
                            .render_band(line, &rows[line.row], ix > 0, palette, cx)
                            .into_any_element(),
                        Block::Card(lines) => v_flex()
                            .mx(px(-ui::ROW_BLEED))
                            .mb(px(10.))
                            .rounded(px(12.))
                            .bg(palette.raised)
                            .border_1()
                            .border_color(palette.border)
                            .shadow(ui::lift(palette))
                            .overflow_hidden()
                            .children(
                                lines
                                    .iter()
                                    .enumerate()
                                    .map(|(at, line)| self.render_line(at > 0, line, &rows[line.row], palette, cx)),
                            )
                            .into_any_element(),
                    }
                }))
                .child(
                    ui::note(
                        "People: everyone in a team and the teams under it, once each, pending included.",
                        palette,
                    )
                    .mt(px(8.)),
                );
        }
        page = page.children(
            self.outcome
                .clone()
                .map(|outcome| outcome_line(outcome, palette).mt(px(10.))),
        );
        if me.manage {
            page = page.child(self.render_pending(palette, cx));
        }
        page.child(ui::note("From a terminal: nexus team tree, nexus team use <slug>.", palette).mt(px(18.)))
    }

    /// A top-level team with teams under it: a band with a rule above it (`rule`), over its cards.
    fn render_band(&self, line: &Line, row: &TreeRow, rule: bool, palette: Palette, cx: &mut Context<Self>) -> Div {
        let slug = row.slug.clone();
        let name = self.name_cell(line, row, palette, cx).child(ui::label(
            format!("{} under it", plural(line.below as u64, "team", "teams")),
            palette,
        ));
        let band = h_flex()
            .id(SharedString::from(format!("teams-row-{}", line.row)))
            .h(px(44.))
            .mx(px(-ui::ROW_BLEED))
            .px(px(ui::ROW_BLEED))
            .gap(px(12.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .on_click(cx.listener(move |this, _, window, cx| this.open_team(slug.clone(), window, cx)))
            .child(name)
            .children(tail_cells(row, palette));
        v_flex()
            .mb(px(6.))
            .when(rule, |wrap| {
                wrap.mt(px(14.)).pt(px(10.)).border_t_1().border_color(palette.border)
            })
            .child(band)
    }

    /// A row on a card. `divided` draws a hairline above a head or an area; leaves hang off their guide instead.
    fn render_line(
        &self,
        divided: bool,
        line: &Line,
        row: &TreeRow,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let slug = row.slug.clone();
        h_flex()
            .id(SharedString::from(format!("teams-row-{}", line.row)))
            .h(px(match line.kind {
                Kind::Band | Kind::Head => 44.,
                Kind::Area => 40.,
                Kind::Leaf => 36.,
            }))
            .px(px(ui::ROW_BLEED))
            .gap(px(12.))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .when(divided && line.kind != Kind::Leaf, |line| {
                line.border_t_1().border_color(palette.border)
            })
            .on_click(cx.listener(move |this, _, window, cx| this.open_team(slug.clone(), window, cx)))
            .child(self.name_cell(line, row, palette, cx))
            .children(tail_cells(row, palette))
    }

    /// The Team column: the fold control, the name in its level's weight, the level and the slug.
    fn name_cell(&self, line: &Line, row: &TreeRow, palette: Palette, cx: &mut Context<Self>) -> Div {
        let (size, weight, color) = match line.kind {
            Kind::Band => (16., FontWeight::SEMIBOLD, palette.strong),
            Kind::Head => (14., FontWeight::MEDIUM, palette.strong),
            Kind::Area => (13., FontWeight::MEDIUM, palette.strong),
            Kind::Leaf => (13., FontWeight::NORMAL, palette.foreground),
        };
        let fold: AnyElement = match line.fold {
            Some(open) => {
                let slug = row.slug.clone();
                h_flex()
                    .id(SharedString::from(format!("teams-fold-{}", line.row)))
                    .flex_none()
                    .size(px(18.))
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.selected))
                    .child(ui::icon(
                        if open {
                            gpui_kit::assets::IconName::ChevronDown
                        } else {
                            gpui_kit::assets::IconName::ChevronRight
                        },
                        palette.icon,
                    ))
                    // Folding is not opening the team: the row's own click stays out of it.
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_fold(slug.clone(), cx);
                    }))
                    .into_any_element()
            }
            None => div().flex_none().w(px(18.)).into_any_element(),
        };
        h_flex()
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .overflow_hidden()
            .gap(px(8.))
            .pr(px(12.))
            .when(line.kind == Kind::Leaf, |cell| {
                cell.pl(px(16. * line.indent as f32)).child(guide(line.last, palette))
            })
            .child(fold)
            .child(
                div()
                    .text_size(px(size))
                    .font_weight(weight)
                    .text_color(color)
                    .flex_none()
                    .child(row.name.clone()),
            )
            .when(!row.level.is_empty(), |cell| {
                cell.child(ui::badge(capitalize(&row.level), palette))
            })
            // The slug only when the name doesn't already say it (a child's slug is its parents' plus its name):
            // a long slug cut mid-word repeated the tree and crowded the row.
            .when(!row.slug.ends_with(&slug_of(&row.name)), |cell| {
                cell.child(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .font_family(ui::MONO)
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .child(row.slug.clone()),
                )
            })
    }

    fn render_create(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let teams: Vec<TreeRow> = match &self.tree {
            Load::Ready(rows) => rows.clone(),
            _ => Vec::new(),
        };
        let parent = self.create.parent.clone();
        let parents = parent_chips(
            "teams-create-parent",
            &teams,
            parent.as_deref(),
            &HashSet::new(),
            palette,
        )
        .into_iter()
        .map(|(chip, pick)| {
            chip.on_click(cx.listener(move |this, _, _, cx| {
                this.create.parent = pick.clone();
                cx.notify();
            }))
        });
        ui::card(palette)
            .mb(px(14.))
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child("Create a team"),
            )
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(labeled("Name", field(&self.create.name, palette), palette))
                    .child(labeled("Slug", field(&self.create.slug, palette), palette))
                    .child(labeled("Level", field(&self.create.level, palette), palette)),
            )
            .child(labeled(
                "Description",
                field(&self.create.description, palette),
                palette,
            ))
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(ui::label("Parent", palette))
                    .child(h_flex().flex_wrap().gap(px(6.)).children(parents)),
            )
            .child(ui::note(
                "Levels are your own labels; Nexus gives them no meaning. Add leads and members from the team's page.",
                palette,
            ))
            .children(
                self.create_outcome
                    .clone()
                    .map(|outcome| outcome_line(outcome, palette)),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .child(
                        ui::button("teams-create-cancel", "Cancel", palette).on_click(cx.listener(|this, _, _, cx| {
                            this.creating = false;
                            this.create_outcome = None;
                            cx.notify();
                        })),
                    )
                    .child(
                        ui::primary_button(
                            "teams-create-submit",
                            if self.working { "Creating…" } else { "Create team" },
                            palette,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.create_team(window, cx))),
                    ),
            )
    }

    fn render_pending(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let mut section = v_flex().child(ui::section_head("Pending", palette));
        let entries = match &self.pending {
            None | Some(Load::Loading) => return section.child(ui::skeleton(1, palette)),
            Some(Load::Failed(message)) => {
                return section.child(ui::error_line(
                    format!("Pending people could not be loaded: {message}"),
                    palette,
                ));
            }
            Some(Load::Ready(entries)) => entries,
        };
        let people = pending_people(entries);
        let org = self.ctx.clone();
        section = section.child(
            h_flex()
                .gap(px(4.))
                .mb(px(8.))
                .child(ui::note(
                    format!(
                        "{}They join these teams when they join the organization. Invite them from",
                        if people.is_empty() {
                            String::new()
                        } else {
                            format!("{}. ", plural(people.len() as u64, "person", "people"))
                        }
                    ),
                    palette,
                ))
                .child(
                    div()
                        .id("teams-pending-members")
                        .text_size(px(13.))
                        .text_color(palette.muted)
                        .cursor_pointer()
                        .hover(|style| style.text_color(palette.strong))
                        .child("Members.")
                        .on_click(move |_, window, cx| (org.navigate)(super::PageId::Members, window, cx)),
                ),
        );
        if people.is_empty() {
            return section.child(ui::note(
                "No one pending. People added to a team before they join show here.",
                palette,
            ));
        }
        section.children(people.into_iter().enumerate().map(|(ix, (email, list))| {
            let teams = list
                .iter()
                .map(|entry| format!("{}{}", entry.team, if entry.lead { " (lead)" } else { "" }))
                .collect::<Vec<_>>()
                .join(", ");
            let home = list
                .iter()
                .find(|entry| entry.home)
                .map(|entry| format!("default {}", entry.team))
                .unwrap_or_else(|| "no default team".into());
            let since = list
                .first()
                .map(|entry| ui::day(&entry.created_at).to_string())
                .unwrap_or_default();
            let detail = format!("{teams} · {home} · added {since}");
            let confirming = self.confirm == Some(Confirm::CancelPerson(email.clone()));
            let target = email.clone();
            ui::row(palette)
                .child(ui::two_lines(email.clone(), detail, palette).flex_1())
                .child(
                    ui::danger_button(
                        SharedString::from(format!("teams-pending-cancel-{ix}")),
                        if confirming { "Cancel in every team" } else { "Cancel" },
                        palette,
                    )
                    .when(confirming, |button| button.bg(palette.destructive.opacity(0.14)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.confirm == Some(Confirm::CancelPerson(target.clone())) {
                            this.cancel_pending(
                                format!("email={}", encode(&target)),
                                format!("Cancelled {target}."),
                                format!("{target} is still pending"),
                                cx,
                            );
                        } else {
                            this.confirm = Some(Confirm::CancelPerson(target.clone()));
                            cx.notify();
                        }
                    })),
                )
        }))
    }

    fn render_team(&self, me: &Me, palette: Palette, cx: &mut Context<Self>) -> Div {
        let slug = self.open.clone().unwrap_or_default();
        let mut crumbs = h_flex()
            .gap(px(6.))
            .mb(px(10.))
            .text_size(px(13.))
            .child(crumb("teams-back", "‹ Teams", palette).on_click(cx.listener(|this, _, _, cx| this.back(cx))));
        let team = match &self.team {
            Load::Loading => return v_flex().w_full().child(crumbs).child(ui::skeleton(6, palette)),
            Load::Failed(message) => {
                return v_flex()
                    .w_full()
                    .child(crumbs)
                    .child(ui::section_head(slug, palette))
                    .child(self.failed(format!("This team could not be loaded: {message}").into(), palette, cx));
            }
            Load::Ready(team) => team.clone(),
        };
        for (ix, (step, name)) in team.path.iter().enumerate() {
            let step = step.clone();
            crumbs = crumbs.child(div().text_color(palette.faint).child("›")).child(
                crumb(SharedString::from(format!("teams-crumb-{ix}")), name.clone(), palette)
                    .on_click(cx.listener(move |this, _, window, cx| this.open_team(step.clone(), window, cx))),
            );
        }
        let manage = me.manage;
        let title = h_flex()
            .justify_between()
            .items_start()
            .gap(px(12.))
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(palette.strong)
                                    .child(team.name.clone()),
                            )
                            .when(!team.level.is_empty(), |line| {
                                line.child(ui::badge(team.level.clone(), palette))
                            }),
                    )
                    .when(!team.description.is_empty(), |column| {
                        column.child(ui::note(team.description.clone(), palette))
                    }),
            )
            .when(!team.my_role.is_empty() && !team.is_default, |row| {
                let slug = team.slug.clone();
                let name = team.name.clone();
                row.child(
                    ui::button("teams-make-default", "Make my default", palette).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.change_team(
                                Verb::Put,
                                "/v1/people/me/default-team".into(),
                                Some(json!({ "team": slug })),
                                format!("{name} is now your default team."),
                                "Your default team didn't change".into(),
                                window,
                                cx,
                            )
                        },
                    )),
                )
            });
        let you = match team.my_role.as_str() {
            "lead" => "Lead",
            "member" => "Member",
            _ => "Not in it",
        };
        let you_note = if team.is_default {
            "Your default team"
        } else if !team.my_role.is_empty() {
            "Not your default"
        } else if manage {
            "You manage teams"
        } else {
            ""
        };
        let stats = h_flex()
            .gap(px(10.))
            .mt(px(14.))
            .child(ui::stat("Members", team.members.to_string(), palette).child(ui::label(
                if team.pending > 0 {
                    format!("And {} pending", team.pending)
                } else {
                    team.slug.clone()
                },
                palette,
            )))
            .child(ui::stat("You", you, palette).child(ui::label(you_note, palette)))
            .child(
                ui::stat("Child teams", team.children.len().to_string(), palette)
                    .child(ui::label(format!("Created {}", ui::day(&team.created_at)), palette)),
            )
            .child({
                let (value, note) = match &self.spend {
                    Some(spend) => (
                        spend.spent.map(spent_text).unwrap_or_else(|| "—".into()),
                        format!(
                            "{} calls{}",
                            ui::count(spend.calls),
                            if team.children.is_empty() {
                                ""
                            } else {
                                ", with the teams under it"
                            }
                        ),
                    ),
                    None => ("—".into(), "Visible to its members and admins".into()),
                };
                ui::stat("Spent, 30 days", value, palette).child(ui::label(note, palette))
            });

        let mut page = v_flex().w_full().child(crumbs).child(title).child(stats).children(
            self.team_outcome
                .clone()
                .map(|outcome| outcome_line(outcome, palette).mt(px(12.))),
        );

        if !team.children.is_empty() {
            page = page
                .child(ui::section_head("Child teams", palette))
                .child(
                    ui::head_row(palette)
                        .child(div().flex_1().child("Team"))
                        .child(div().w(px(120.)).flex_none().child("Level"))
                        .child(div().w(px(110.)).flex_none().text_right().child("Members"))
                        .child(div().w(px(90.)).flex_none().text_right().child("Child teams")),
                )
                .children(team.children.iter().enumerate().map(|(ix, child)| {
                    let slug = child.slug.clone();
                    ui::row(palette)
                        .id(SharedString::from(format!("teams-child-{ix}")))
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.hover))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_team(slug.clone(), window, cx)))
                        .child(ui::two_lines(child.name.clone(), child.slug.clone(), palette).flex_1())
                        .child(div().w(px(120.)).flex_none().when(!child.level.is_empty(), |cell| {
                            cell.child(ui::badge(child.level.clone(), palette))
                        }))
                        .child(div().w(px(110.)).flex_none().text_right().text_size(px(13.)).child(
                            if child.pending > 0 {
                                format!("{} +{} pending", child.members, child.pending)
                            } else {
                                child.members.to_string()
                            },
                        ))
                        .child(
                            div()
                                .w(px(90.))
                                .flex_none()
                                .text_right()
                                .text_size(px(13.))
                                .text_color(palette.faint)
                                .child(if child.child_count > 0 {
                                    child.child_count.to_string()
                                } else {
                                    "–".into()
                                }),
                        )
                }));
        }

        page = page.child(ui::section_head("Members", palette));
        if team.member_list.is_empty() && team.pending_list.is_empty() {
            page = page.child(ui::empty(
                "No one in this team yet",
                if manage {
                    "Add people from your organization below."
                } else {
                    "An owner or admin adds people."
                },
                palette,
            ));
        } else {
            page = page
                .child(
                    ui::head_row(palette)
                        .child(div().flex_1().child("Person"))
                        .child(div().w(px(80.)).flex_none().child("Role"))
                        .child(div().w(px(100.)).flex_none().child("Spent, 30 days"))
                        .child(div().w(px(150.)).flex_none().child("Default team"))
                        .child(div().w(px(230.)).flex_none()),
                )
                .children(
                    team.member_list
                        .iter()
                        .enumerate()
                        .map(|(ix, member)| self.render_member(ix, member, &team, me, palette, cx)),
                )
                .children(team.pending_list.iter().enumerate().map(|(ix, entry)| {
                    let confirming = self.confirm == Some(Confirm::CancelEntry(entry.id.clone()));
                    let id = entry.id.clone();
                    let email = entry.email.clone();
                    let name = team.name.clone();
                    ui::row(palette)
                        .child(ui::two_lines(entry.email.clone(), "Pending until they join", palette).flex_1())
                        .child(
                            div()
                                .w(px(80.))
                                .flex_none()
                                .child(ui::badge(if entry.lead { "Lead" } else { "Member" }, palette)),
                        )
                        .child(
                            div()
                                .w(px(150.))
                                .flex_none()
                                .text_size(px(13.))
                                .text_color(palette.faint)
                                .child(if entry.home { "This team, when they join" } else { "–" }),
                        )
                        .child(h_flex().w(px(230.)).flex_none().justify_end().when(manage, |cell| {
                            cell.child(
                                ui::danger_button(
                                    SharedString::from(format!("teams-entry-cancel-{ix}")),
                                    if confirming { "Cancel invite?" } else { "Cancel" },
                                    palette,
                                )
                                .when(confirming, |button| button.bg(palette.destructive.opacity(0.14)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.confirm == Some(Confirm::CancelEntry(id.clone())) {
                                        this.change_team(
                                            Verb::Delete,
                                            format!("/v1/teams/pending?id={}", encode(&id)),
                                            None,
                                            format!("Cancelled {email} in {name}."),
                                            format!("{email} is still pending"),
                                            window,
                                            cx,
                                        );
                                    } else {
                                        this.confirm = Some(Confirm::CancelEntry(id.clone()));
                                        cx.notify();
                                    }
                                })),
                            )
                        }))
                }));
        }
        if manage {
            page = page.child(self.render_add(palette, cx));
        }

        page = page
            .child(ui::section_head("Models", palette))
            .children(self.policy.as_ref().map(|(_, view)| view.clone()));
        if manage {
            page = page.child(self.render_settings(&team, palette, cx));
        }
        page
    }

    fn render_member(
        &self,
        ix: usize,
        member: &TeamMember,
        team: &Detail,
        me: &Me,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let own = member.user_id == me.user_id;
        let title = if own {
            format!("{} (you)", member.who())
        } else {
            member.who()
        };
        let detail = if !member.name.is_empty() {
            member.email.clone()
        } else {
            String::new()
        };
        let mut actions = h_flex().w(px(230.)).flex_none().justify_end().gap(px(6.));
        if own && !member.is_default {
            let slug = team.slug.clone();
            let name = team.name.clone();
            actions = actions.child(
                ui::button(
                    SharedString::from(format!("teams-default-{ix}")),
                    "Make default",
                    palette,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.change_team(
                        Verb::Put,
                        "/v1/people/me/default-team".into(),
                        Some(json!({ "team": slug })),
                        format!("{name} is now your default team."),
                        "Your default team didn't change".into(),
                        window,
                        cx,
                    )
                })),
            );
        }
        if me.manage {
            let confirming = self.confirm == Some(Confirm::RemoveMember(member.user_id.clone()));
            let user_id = member.user_id.clone();
            let who = member.who();
            let label = if member.email.is_empty() {
                member.user_id.clone()
            } else {
                member.email.clone()
            };
            let slug = team.slug.clone();
            let name = team.name.clone();
            actions = actions.child(
                ui::danger_button(
                    SharedString::from(format!("teams-remove-{ix}")),
                    if confirming { "Remove from team?" } else { "Remove" },
                    palette,
                )
                .when(confirming, |button| button.bg(palette.destructive.opacity(0.14)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.confirm == Some(Confirm::RemoveMember(user_id.clone())) {
                        this.change_team(
                            Verb::Delete,
                            format!("/v1/teams/{}/members/{}", encode(&slug), encode(&user_id)),
                            None,
                            format!("Removed {label} from {name}. They stay in the organization."),
                            format!("{who} wasn't removed"),
                            window,
                            cx,
                        );
                    } else {
                        this.confirm = Some(Confirm::RemoveMember(user_id.clone()));
                        cx.notify();
                    }
                })),
            );
        }
        ui::row(palette)
            .child(ui::two_lines(title, detail, palette).flex_1())
            .child(
                div()
                    .w(px(80.))
                    .flex_none()
                    .child(ui::badge(if member.lead { "Lead" } else { "Member" }, palette)),
            )
            .child(
                div()
                    .w(px(100.))
                    .flex_none()
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .child(
                        self.spend
                            .as_ref()
                            .and_then(|spend| spend.people.iter().find(|(id, _)| *id == member.user_id))
                            .map(|(_, dollars)| spent_text(*dollars))
                            .unwrap_or_else(|| "—".into()),
                    ),
            )
            .child(
                div()
                    .w(px(150.))
                    .flex_none()
                    .text_size(px(13.))
                    .text_color(if member.is_default {
                        palette.foreground
                    } else {
                        palette.faint
                    })
                    .child(if member.is_default { "This team" } else { "Another team" }),
            )
            .child(actions)
    }

    fn render_add(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let typed = self.add_email.read(cx).value().trim().to_lowercase();
        let known = self
            .org_members
            .as_ref()
            .is_some_and(|list| list.iter().any(|(_, email)| *email == typed));
        let role = |id: &'static str, text: &'static str, lead: bool| {
            ui::chip(id, text, self.add_lead == lead, palette).on_click(cx.listener(move |this, _, _, cx| {
                this.add_lead = lead;
                cx.notify();
            }))
        };
        v_flex()
            .gap(px(8.))
            .mt(px(12.))
            .child(ui::label("Add someone by email · pending until they join", palette))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(field(&self.add_email, palette))
                    .child(role("teams-add-member", "Member", false))
                    .child(role("teams-add-lead", "Lead", true))
                    .child(
                        h_flex()
                            .id("teams-add-home")
                            .gap(px(6.))
                            .cursor_pointer()
                            .child(ui::switch("teams-add-home-switch", self.add_home, false, palette))
                            .child(ui::label("Their default", palette))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.add_home = !this.add_home;
                                cx.notify();
                            })),
                    )
                    .child(
                        ui::primary_button(
                            "teams-add-submit",
                            if self.working {
                                "Adding…"
                            } else if known || typed.is_empty() {
                                "Add to team"
                            } else {
                                "Add as pending"
                            },
                            palette,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.add_member(window, cx))),
                    ),
            )
            .children(self.add_outcome.clone().map(|outcome| outcome_line(outcome, palette)))
    }

    fn render_settings(&self, team: &Detail, palette: Palette, cx: &mut Context<Self>) -> Div {
        let rows: Vec<TreeRow> = match &self.tree {
            Load::Ready(rows) => rows.clone(),
            _ => Vec::new(),
        };
        let blocked = subtree(&rows, &team.slug);
        let parents = parent_chips(
            "teams-edit-parent",
            &rows,
            self.edit.parent.as_deref(),
            &blocked,
            palette,
        )
        .into_iter()
        .map(|(chip, pick)| {
            chip.on_click(cx.listener(move |this, _, _, cx| {
                this.edit.parent = pick.clone();
                cx.notify();
            }))
        });
        let confirming = self.confirm == Some(Confirm::DeleteTeam);
        let children = team.children.len() as u64;
        let warning = if children > 0 {
            format!(
                "Deleting removes this team and every team under it ({} and theirs), with their memberships. Past spend keeps its name in Usage.",
                plural(children, "child team", "child teams")
            )
        } else {
            "Deleting removes the team and its memberships. Past spend keeps its name in Usage.".into()
        };
        let delete_label = if !confirming {
            "Delete team".to_string()
        } else if children > 0 {
            format!("Delete {} and the teams under it", team.slug)
        } else {
            format!("Delete {}", team.slug)
        };
        v_flex()
            .child(
                h_flex()
                    .justify_between()
                    .items_end()
                    .child(ui::section_head("Settings", palette))
                    .child(ui::label("Owners and admins", palette).mb(px(10.))),
            )
            .child(
                ui::card(palette)
                    .gap(px(10.))
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .child(labeled("Name", field(&self.edit.name, palette), palette))
                            .child(labeled("Slug", field(&self.edit.slug, palette), palette))
                            .child(labeled("Level", field(&self.edit.level, palette), palette)),
                    )
                    .child(labeled("Description", field(&self.edit.description, palette), palette))
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .child(ui::label("Inside · moving keeps its history", palette))
                            .child(h_flex().flex_wrap().gap(px(6.)).children(parents)),
                    )
                    .children(self.edit_outcome.clone().map(|outcome| outcome_line(outcome, palette)))
                    .child(
                        h_flex().justify_end().child(
                            ui::button("teams-edit-save", "Save changes", palette)
                                .on_click(cx.listener(|this, _, window, cx| this.save_team(window, cx))),
                        ),
                    )
                    .child(
                        h_flex()
                            .gap(px(12.))
                            .pt(px(10.))
                            .child(ui::note(warning, palette).flex_1())
                            .when(confirming, |row| {
                                row.child(
                                    ui::button("teams-delete-cancel", "Cancel", palette).on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.confirm = None;
                                            cx.notify();
                                        },
                                    )),
                                )
                            })
                            .child(
                                ui::danger_button("teams-delete", delete_label, palette)
                                    .when(confirming, |button| button.bg(palette.destructive.opacity(0.14)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.confirm == Some(Confirm::DeleteTeam) {
                                            this.delete_team(cx);
                                        } else {
                                            this.confirm = Some(Confirm::DeleteTeam);
                                            cx.notify();
                                        }
                                    })),
                            ),
                    ),
            )
    }
}

/// The Leads, People and You columns of a tree row. No lead and an empty team read faint.
fn tail_cells(row: &TreeRow, palette: Palette) -> [Div; 3] {
    let you = match row.my_role.as_str() {
        "lead" => "Lead",
        "member" => "Member",
        _ => "",
    };
    let you = if !you.is_empty() && row.is_default {
        format!("{you} · Default")
    } else {
        you.to_string()
    };
    let empty = row.headcount == 0 && row.pending == 0;
    [
        div()
            .w(px(200.))
            .flex_none()
            .truncate()
            .text_size(px(13.))
            .text_color(if row.leads.is_empty() {
                palette.faint
            } else {
                palette.foreground
            })
            .child(if row.leads.is_empty() {
                "—".to_string()
            } else {
                row.leads.join(", ")
            }),
        div()
            .w(px(110.))
            .flex_none()
            .text_right()
            .text_size(px(13.))
            .text_color(if empty { palette.faint } else { palette.foreground })
            .child(if row.pending > 0 {
                format!("{} +{} pending", row.headcount, row.pending)
            } else {
                row.headcount.to_string()
            }),
        div()
            .w(px(80.))
            .flex_none()
            .text_size(px(13.))
            .text_color(palette.muted)
            .child(you),
    ]
}

/// A leaf's tree guide: a line down from its parent and a tick to its name. The last leaf's line stops at the tick.
fn guide(last: bool, palette: Palette) -> Div {
    div()
        .relative()
        .flex_none()
        .w(px(14.))
        .h_full()
        .child(
            div()
                .absolute()
                .left(px(6.))
                .top_0()
                .w(px(1.))
                .bg(palette.border)
                .map(|line| if last { line.h(relative(0.5)) } else { line.bottom_0() }),
        )
        .child(
            div()
                .absolute()
                .left(px(6.))
                .top(relative(0.5))
                .w(px(8.))
                .h(px(1.))
                .bg(palette.border),
        )
}

/// The parent picker: Top level, then each team the team may sit in, indented by depth. Each chip with the slug it
/// picks (`None` for the top level).
fn parent_chips(
    id: &str,
    rows: &[TreeRow],
    picked: Option<&str>,
    blocked: &HashSet<String>,
    palette: Palette,
) -> Vec<(Stateful<Div>, Option<String>)> {
    let mut chips = vec![(
        ui::chip(
            SharedString::from(format!("{id}-top")),
            "Top level",
            picked.is_none(),
            palette,
        ),
        None,
    )];
    // Gateway refuses a team deeper than 10 levels.
    for (ix, row) in rows
        .iter()
        .enumerate()
        .filter(|(_, row)| !blocked.contains(&row.slug) && row.depth < 10)
    {
        let label = format!("{}{}", "· ".repeat(row.depth.saturating_sub(1)), row.name);
        chips.push((
            ui::chip(
                SharedString::from(format!("{id}-{ix}")),
                label,
                picked == Some(row.slug.as_str()),
                palette,
            ),
            Some(row.slug.clone()),
        ));
    }
    chips
}

/// A text field in the soft track Settings uses.
fn field(state: &Entity<InputState>, palette: Palette) -> Div {
    h_flex()
        .flex_1()
        .min_w(px(0.))
        .h(px(32.))
        .px(px(10.))
        .rounded(px(8.))
        .bg(palette.hover)
        .text_size(px(13.))
        .child(div().flex_1().min_w(px(0.)).child(Input::new(state).appearance(false)))
}

/// A field with its small label above.
fn labeled(text: &'static str, field: Div, palette: Palette) -> Div {
    v_flex()
        .flex_1()
        .min_w(px(0.))
        .gap(px(4.))
        .child(ui::label(text, palette))
        .child(h_flex().child(field))
}

/// A link in the path above a team.
fn crumb(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .text_color(palette.muted)
        .cursor_pointer()
        .hover(|style| style.text_color(palette.strong))
        .child(text.into())
}

fn outcome_line(outcome: Outcome, palette: Palette) -> Div {
    if outcome.error {
        ui::error_line(outcome.text, palette)
    } else {
        ui::note(outcome.text, palette)
    }
}
