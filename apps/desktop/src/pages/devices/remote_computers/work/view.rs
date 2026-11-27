use super::*;

impl RemoteComputers {
    pub(in super::super) fn render_work(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let card = ui::card_frame(p);
        let view = match &self.work.screen {
            Screen::Computers => self.render_computers(card, p, cx),
            Screen::Projects {
                computer,
                choices,
                selected,
            } => {
                let mut card = self.work_header(card, "Saved projects", computer.name.clone(), p, cx);
                if choices.is_empty() && self.work.pending.is_none() && self.work.message.is_none() {
                    card = card.child(ui::card_rule(p)).child(
                        ui::card_row().child(ui::footnote("No saved projects are available on this computer.", p)),
                    );
                }
                for project in choices {
                    let id = project.id.clone();
                    let picked = selected.as_ref() == Some(&id);
                    let button_id = SharedString::from(format!("remote-project-{id}"));
                    let button = if picked || self.work.pending.is_some() {
                        ui::disabled_button(button_id, if picked { "Selected" } else { "Select" }, p)
                    } else {
                        ui::button(button_id, "Select", p).on_click(cx.listener(move |this, _, _, cx| {
                            if let Screen::Projects { selected, .. } = &mut this.work.screen {
                                *selected = Some(id.clone());
                                this.work.message = None;
                                cx.notify();
                            }
                        }))
                    };
                    card = card.child(ui::card_rule(p)).child(
                        ui::card_row()
                            .justify_between()
                            .child(ui::two_lines(project.name.clone(), project.location_hint.clone(), p))
                            .child(button),
                    );
                }
                if self.work.project().is_some() {
                    card = card.child(ui::card_rule(p)).child(self.prompt_row(
                        ui::primary_button("remote-start", "Start session", p).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.start_session(cx);
                                if this.work.pending.is_some() {
                                    this.work.prompt.update(cx, |state, cx| state.set_value("", window, cx));
                                }
                            },
                        )),
                        "remote-start-disabled",
                        "Start session",
                        p,
                        cx,
                    ));
                }
                card
            }
            Screen::Session {
                computer,
                project,
                transcript,
                ..
            } => {
                let mut card = self
                    .work_header(card, project.clone(), format!("Session on {}", computer.name), p, cx)
                    .child(ui::card_rule(p));
                match transcript {
                    Ok(lines) if lines.is_empty() => {
                        card = card.child(ui::card_row().child(ui::footnote("Waiting for session updates…", p)));
                    }
                    Ok(lines) => {
                        for line in lines {
                            card = card.child(
                                ui::card_row()
                                    .child(ui::setting_lines(line.label, line.text.clone(), p))
                                    .when_some(line.approval_id.as_ref(), |row, id| {
                                        let mut actions = h_flex().gap(px(6.));
                                        for (label, decision) in
                                            [("Deny", Decision::Deny), ("Allow once", Decision::Allow)]
                                        {
                                            let button_id = SharedString::from(format!("remote-approval-{id}-{label}"));
                                            let request_id = id.clone();
                                            let button = if self.work.pending.is_some() {
                                                ui::disabled_button(button_id, label, p)
                                            } else {
                                                ui::button(button_id, label, p).on_click(cx.listener(
                                                    move |this, _, _, cx| {
                                                        this.session_command(
                                                            Action::Approve {
                                                                request_id: request_id.clone(),
                                                                decision,
                                                                message: None,
                                                                answers: None,
                                                            },
                                                            cx,
                                                        );
                                                    },
                                                ))
                                            };
                                            actions = actions.child(button);
                                        }
                                        row.child(actions)
                                    }),
                            );
                        }
                    }
                    Err(error) => {
                        card = card.child(ui::card_row().child(ui::setting_lines(
                            "Waiting for the session to sync",
                            error.clone(),
                            p,
                        )));
                    }
                }
                let send = ui::primary_button("remote-send", "Send", p).on_click(cx.listener(|this, _, window, cx| {
                    let text = this.work.prompt.read(cx).value().trim().to_string();
                    this.session_command(Action::Send { text }, cx);
                    if this.work.pending.is_some() {
                        this.work.prompt.update(cx, |state, cx| state.set_value("", window, cx));
                    }
                }));
                let stop = if self.work.pending.is_some() {
                    ui::disabled_button("remote-stop", "Stop", p)
                } else {
                    ui::button("remote-stop", "Stop", p)
                        .on_click(cx.listener(|this, _, _, cx| this.session_command(Action::Stop, cx)))
                };
                card.child(ui::card_rule(p))
                    .child(self.prompt_row(send, "remote-send-disabled", "Send", p, cx).child(stop))
            }
        };
        v_flex().child(ui::group_head("Remote work", p)).child(view.when(
            self.work.pending.is_some() || self.work.message.is_some(),
            |card| {
                let message = if self.work.request_running {
                    "Waiting for the other computer…"
                } else {
                    self.work.message.as_deref().unwrap_or_default()
                };
                card.child(ui::card_rule(p)).child(
                    ui::card_row().justify_between().child(ui::footnote(message, p)).when(
                        self.work.pending.is_some() && !self.work.request_running,
                        |row| {
                            row.child(
                                ui::button("remote-retry", "Check again", p)
                                    .on_click(cx.listener(|this, _, _, cx| this.run_request(cx))),
                            )
                        },
                    ),
                )
            },
        ))
    }

    fn render_computers(&self, card: Div, p: Palette, cx: &mut Context<Self>) -> Div {
        let detail = match &self.work.peers {
            None if self.identity.is_none() => "Remote work is unavailable in this workspace.",
            None => "Looking for your computers…",
            Some(Ok(peers)) if peers.is_empty() => "Connected computers with remote control enabled appear here.",
            Some(Ok(_)) => "Start a session in a project saved on another computer.",
            Some(Err(error)) => error.as_str(),
        };
        let mut card = card.child(
            ui::card_row()
                .justify_between()
                .child(ui::setting_lines("Your computers", detail, p))
                .when(self.identity.is_some(), |row| {
                    row.child(
                        ui::ghost_button("remote-refresh", "Refresh", p)
                            .on_click(cx.listener(|this, _, _, cx| this.load_peers(cx))),
                    )
                }),
        );
        if let Some(Ok(peers)) = &self.work.peers {
            for peer in peers {
                let id = SharedString::from(format!("remote-computer-{}", peer.computer_id));
                let online = peer.ready();
                let button = if online {
                    let peer = peer.clone();
                    ui::button(id, "Choose project", p).on_click(cx.listener(move |this, _, _, cx| {
                        this.open_computer(peer.clone(), cx);
                    }))
                } else {
                    ui::disabled_button(id, "Choose project", p)
                };
                card = card.child(ui::card_rule(p)).child(
                    ui::card_row()
                        .justify_between()
                        .child(ui::two_lines(
                            peer.name.clone(),
                            if online { "Available" } else { "Unavailable" },
                            p,
                        ))
                        .child(button),
                );
            }
        }
        card
    }

    fn work_header(
        &self,
        card: Div,
        title: impl Into<SharedString>,
        detail: String,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let back = if self.work.request_running {
            ui::disabled_button("remote-back", "Back", p)
        } else {
            ui::ghost_button("remote-back", "Back", p)
                .on_click(cx.listener(|this, _, window, cx| this.back_to_computers(window, cx)))
        };
        card.child(
            ui::card_row()
                .justify_between()
                .child(ui::setting_lines(title, detail, p))
                .child(back),
        )
    }

    fn prompt_row(
        &self,
        submit: Stateful<Div>,
        disabled_id: &'static str,
        label: &'static str,
        p: Palette,
        cx: &Context<Self>,
    ) -> Div {
        let input = self.work.prompt.read(cx).value();
        let disabled = self.work.pending.is_some() || input.trim().is_empty() || input.len() > MAX_PROMPT_BYTES;
        ui::card_row()
            .gap(px(8.))
            .child(text_field(&self.work.prompt, p))
            .child(if disabled {
                ui::disabled_button(disabled_id, label, p)
            } else {
                submit
            })
    }
}
