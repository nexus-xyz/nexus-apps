//! The Devices page's view: its cards, rows and the codes people compare. `devices.rs` holds the state, the
//! requests and the keys; this file only draws them.

use super::*;

/// This device's group, as a fix opens it: Team sessions' "Connect it ›" (`workspace/team_sessions/view.rs`).
const THIS_DEVICE: &str = "this-device";

fn phone_code_button(palette: Palette, enabled: bool) -> Stateful<Div> {
    if enabled {
        ui::button("phone-link-begin", "Show connection code", palette)
    } else {
        ui::disabled_button("phone-link-begin", "Show connection code", palette)
    }
}

pub(in crate::pages) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        v_flex()
            .child(ui::section_head("Phone control", palette))
            .child(phone_code_button(palette, false))
            .into_any_element()
    })
}

impl DevicesPage {
    /// The device invitation, control switch and connection result.
    fn render_phone(&self, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let state = self.phone_connection();
        if !self.parts().connection_code {
            return v_flex()
                .child(ui::group_head("Your devices", p))
                .child(ui::card_frame(p).child(self.render_control(state, cx)));
        }
        v_flex()
            .child(ui::group_head("Connect a device", p))
            .child(
                ui::card_frame(p)
                    .child(self.render_phone_link(state, cx))
                    .child(deep_rule(p))
                    .child(self.render_control(state, cx)),
            )
            .when_some(self.link_outcome.as_ref(), |column, (text, error)| {
                column.child(outcome_line(text, *error, p))
            })
    }

    /// Show the invitation or approve the device that opened it.
    fn render_phone_link(&self, state: PhoneConnection, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let tile = || ui::icon_tile(Lucide::Smartphone, p);
        if !state.can_invite()
            && (self.link_invitation.is_none()
                || matches!(
                    state,
                    PhoneConnection::Managed | PhoneConnection::Expired | PhoneConnection::Revoked
                ))
        {
            let message: SharedString = match (&self.phone_control, state) {
                (Load::Failed(error), PhoneConnection::Unavailable) => error.clone(),
                _ => state.message().into(),
            };
            let control = if state == PhoneConnection::Unavailable {
                let mut actions = h_flex().flex_none().gap(px(12.));
                if matches!(self.phone_control, Load::Ready(_)) {
                    let ctx = self.ctx.clone();
                    actions = actions.child(
                        ui::web_link("phone-sync-settings", "Manage session sync", p)
                            .on_click(move |_, _, cx| ctx.open_web("settings/general#synced-sessions", cx)),
                    );
                }
                actions
                    .child(
                        ui::button("phone-sync-retry", "Try again", p)
                            .on_click(cx.listener(|this, _, _, cx| this.load_control(cx))),
                    )
                    .into_any_element()
            } else {
                ui::disabled_button("phone-link-begin", "Show connection code", p).into_any_element()
            };
            return ui::card_row()
                .child(tile())
                .child(ui::setting_lines("Link another device", message, p).flex_1())
                .child(control);
        }
        if let Some(candidate) = &self.link_candidate {
            let name = candidate.name.as_deref().unwrap_or("this device");
            let preview = self.remote_computers().is_some();
            let approve_label = if preview { "Approve device" } else { "Approve phone" };
            let approve = if self.link_busy {
                ui::disabled_button("phone-link-approve", "Approving…", p).into_any_element()
            } else {
                ui::primary_button("phone-link-approve", approve_label, p)
                    .on_click(cx.listener(|this, _, _, cx| this.approve_phone_link(cx)))
                    .into_any_element()
            };
            return v_flex()
                .child(
                    ui::card_row().child(tile()).child(
                        ui::two_lines(
                            format!("Connect {name}?"),
                            if preview {
                                "Approve the device that opened this connection code."
                            } else {
                                "Check that the phone shows the same code."
                            },
                            p,
                        )
                        .flex_1(),
                    ),
                )
                .when_some(candidate.code.as_deref(), |card, code| {
                    card.child(div().px(px(16.)).pb(px(8.)).child(ui::setting_detail(
                        format!("Connection code · {}", format_code(code)),
                        p,
                    )))
                })
                .child(actions_footer(
                    if preview {
                        "Allow this device to follow and control coding sessions on this computer."
                    } else {
                        "Use Remote control to let this phone send commands after pairing."
                    },
                    h_flex()
                        .gap(px(8.))
                        .child(if self.link_busy {
                            ui::disabled_button("phone-link-cancel", "Cancel", p).into_any_element()
                        } else {
                            ui::ghost_button("phone-link-cancel", "Cancel", p)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_phone_link(cx)))
                                .into_any_element()
                        })
                        .child(approve),
                    p,
                ));
        }
        if let Some(link) = &self.link_invitation {
            let copy_label = if self.copied_at.is_some_and(|(kind, _)| kind == Copied::PhoneLink) {
                "Link copied"
            } else {
                "Copy connection link"
            };
            return v_flex()
                .child(
                    ui::card_row()
                        .child(tile())
                        .child(
                            ui::two_lines(
                                "Connect to this computer",
                                if self.remote_computers().is_some() {
                                    "On a phone, scan this code. On another computer, paste the connection link in Settings → Devices."
                                } else {
                                    "Scan this code in Nexus on your phone."
                                },
                                p,
                            )
                            .flex_1(),
                        )
                        .child(ui::status(
                            time_left(link.invitation.expires_at, now_ms()),
                            p.waiting(),
                            p,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .px(px(16.))
                        .pb(px(16.))
                        .child(render_qr(link.qr.clone(), PHONE_QR_SIDE)),
                )
                .child(actions_footer(
                    "Approve the other device here when it asks to connect.",
                    h_flex()
                        .gap(px(8.))
                        .child(
                            ui::ghost_button("phone-link-cancel", "Cancel", p)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_phone_link(cx))),
                        )
                        .child(
                            ui::ghost_button("phone-link-renew", "New QR code", p)
                                .on_click(cx.listener(|this, _, _, cx| this.renew_phone_link(cx))),
                        )
                        .child(
                            ui::button("phone-link-copy", copy_label, p).on_click(cx.listener(|this, _, _, cx| {
                                if let Some(link) = &this.link_invitation {
                                    this.copy_fingerprint(link.link.clone(), Copied::PhoneLink, cx);
                                }
                            })),
                        ),
                    p,
                ));
        }
        let begin = if self.link_busy {
            ui::disabled_button("phone-link-begin", "Preparing…", p).into_any_element()
        } else {
            phone_code_button(p, true)
                .on_click(cx.listener(|this, _, _, cx| this.connect_phone(cx)))
                .into_any_element()
        };
        let detail = if state == PhoneConnection::Approver {
            "Use the device that approved this computer."
        } else {
            if self.remote_computers().is_some() {
                "Link a phone or another computer to work on this computer remotely."
            } else {
                "Link a phone to work on this computer remotely."
            }
        };
        ui::card_row()
            .child(tile())
            .child(ui::setting_lines("Link another device", detail, p).flex_1())
            .child(begin)
    }

    /// Remote control stays locked until encrypted session sync is available.
    fn render_control(&self, state: PhoneConnection, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let detail = if self.trust == DeviceTrust::Gateway {
            "Devices signed in to your account connect automatically. Turn off to stop them from starting and \
             continuing sessions on this computer."
        } else if self.remote_computers().is_some() {
            "Pairing allows the device you approve. Turning this switch on allows all your trusted devices."
        } else {
            "Turn on to let all your trusted devices start and continue sessions on this computer. Commands stay encrypted."
        };
        let row = |detail: SharedString, control: AnyElement| {
            ui::card_row()
                .child(ui::icon_tile(Lucide::ShieldCheck, p))
                .child(ui::setting_lines("Remote control", detail, p).flex_1())
                .child(control)
        };
        let locked = || ui::switch("phone-control", false, true, p).into_any_element();
        match (state, &self.phone_control) {
            (PhoneConnection::Ready, Load::Ready(settings)) if settings.available => {
                let enabled = settings.enabled;
                let busy = self.control_busy;
                row(
                    detail.into(),
                    ui::switch("phone-control", enabled, busy, p)
                        .when(!busy, |switch| {
                            switch.on_click(cx.listener(move |this, _, _, cx| this.set_control(!enabled, cx)))
                        })
                        .into_any_element(),
                )
            }
            (PhoneConnection::Ready | PhoneConnection::Unavailable, _) => {
                row("Remote control needs encrypted session sync.".into(), locked())
            }
            (state, _) => row(state.message_for(self.trust).into(), locked()),
        }
    }

    fn render_this_device(&self, devices: &[Device], cx: &mut Context<Self>) -> Div {
        if !self.parts().pairing {
            return self.render_this_device_by_account(cx);
        }
        let p = palette(cx);
        let now = now_ms();
        let Some(me) = self.me() else {
            let first = !devices.iter().any(|device| device.state != "pending");
            let label = if first {
                "Trust this computer"
            } else {
                "Request approval"
            };
            let setup = if self.busy {
                ui::disabled_button("devices-setup", "Setting up…", p).into_any_element()
            } else {
                ui::primary_button("devices-setup", label, p)
                    .on_click(cx.listener(|this, _, _, cx| this.register(None, cx)))
                    .into_any_element()
            };
            return ui::card_frame(p).child(
                v_flex()
                    .gap(px(14.))
                    .p(px(16.))
                    .child(
                        h_flex()
                            .items_start()
                            .gap(px(16.))
                            .child(ui::icon_tile(Lucide::KeyRound, p))
                            .child(
                                ui::setting_lines(
                                    "Trust this computer for encrypted sync",
                                    if first {
                                        "As your first trusted device, this computer can approve your other devices."
                                    } else {
                                        "A trusted phone or computer needs to approve this computer."
                                    },
                                    p,
                                )
                                .flex_1(),
                            ),
                    )
                    .child(
                        v_flex()
                            .pl(px(ROW_TEXT_INSET - 16.))
                            .gap(px(4.))
                            .child(ui::label("Device name", p))
                            .child(h_flex().gap(px(8.)).child(text_field(&self.name, p)).child(setup)),
                    ),
            );
        };
        let code = device_code(me);
        match status(me, now) {
            Status::Revoked => ui::card_frame(p).child(
                ui::card_row()
                    .child(device_tile(&me.name, p))
                    .child(
                        ui::setting_lines(
                            me.name.clone(),
                            format!(
                                "Revoked {}. It gets no new keys. To use it again, make a new key and pair it from a \
                                 trusted device.",
                                ui::day(me.revoked_at.as_deref().unwrap_or(&me.created_at))
                            ),
                            p,
                        )
                        .flex_1(),
                    )
                    .child(if self.busy {
                        ui::disabled_button("devices-new-key", "Making a new key…", p).into_any_element()
                    } else {
                        ui::button("devices-new-key", "Make a new key", p)
                            .on_click(cx.listener(|this, _, _, cx| this.new_key(cx)))
                            .into_any_element()
                    }),
            ),
            // Gateway answers an expired pairing's keys with the same expired row: only a new key starts a new pairing.
            Status::Expired => ui::card_frame(p).child(
                ui::card_row()
                    .child(device_tile(&me.name, p))
                    .child(
                        ui::setting_lines(
                            me.name.clone(),
                            "This pairing waited more than 15 minutes, so it expired. Start a new one and approve it \
                             sooner.",
                            p,
                        )
                        .flex_1(),
                    )
                    .child(if self.busy {
                        ui::disabled_button("devices-again", "Starting again…", p).into_any_element()
                    } else {
                        ui::primary_button("devices-again", "Start again", p)
                            .debug_selector(|| "devices-again".into())
                            .on_click(cx.listener(|this, _, _, cx| this.new_key(cx)))
                            .into_any_element()
                    }),
            ),
            Status::Pending => {
                let until = iso_ms(&me.created_at).unwrap_or(now) + PAIRING_TTL_MS;
                let share = ((until - now) as f32 / PAIRING_TTL_MS as f32).clamp(0., 1.);
                let trusted: Vec<&Device> = devices.iter().filter(|device| device.state == "trusted").collect();
                ui::card_frame(p)
                    .child(
                        ui::card_row()
                            .child(device_tile(&me.name, p))
                            .child(
                                ui::two_lines(me.name.clone(), "Approve this computer for encrypted sync.", p)
                                    .flex_1(),
                            )
                            .child(ui::status("Waiting for approval", p.waiting(), p)),
                    )
                    .child(
                        h_flex()
                            .items_start()
                            .gap(px(24.))
                            .px(px(16.))
                            .pb(px(16.))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .gap(px(12.))
                                    .pt(px(4.))
                                    // The only QR code here connects a phone: Link another device shows it, and a
                                    // trusted phone that scans it approves this computer too.
                                    .child(div().text_size(px(13.)).text_color(p.muted).child(
                                        "On a trusted phone, press Show connection code under Link another device, \
                                         then scan it in Nexus on the phone.",
                                    ))
                                    .child(ui::setting_detail(
                                        "On a trusted computer, enter the code under Settings → Devices → Add a device.",
                                        p,
                                    ))
                                    .child(code_groups(&code, 22., p))
                                    .child(
                                        v_flex()
                                            .gap(px(6.))
                                            .child(countdown_bar(share, p))
                                            .child(ui::setting_detail(time_left(until, now), p)),
                                    ),
                            ),
                    )
                    .when(!trusted.is_empty(), |card| {
                        card.child(
                            v_flex()
                                .gap(px(6.))
                                .px(px(16.))
                                .pb(px(16.))
                                .child(ui::setting_detail(
                                    "After approval, compare the approving device’s fingerprint and confirm it on this computer.",
                                    p,
                                ))
                                .child(well(p).children(trusted.into_iter().enumerate().map(|(ix, device)| {
                                    h_flex()
                                        .justify_between()
                                        .gap(px(12.))
                                        .px(px(12.))
                                        .py(px(8.))
                                        .text_size(px(12.))
                                        .when(ix > 0, |row| row.border_t_1().border_color(p.border))
                                        .child(
                                            div()
                                                .min_w(px(0.))
                                                .truncate()
                                                .text_color(p.foreground)
                                                .child(device.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .font_family(ui::MONO)
                                                .text_color(p.strong)
                                                .child(format_code(&device_code(device))),
                                        )
                                }))),
                        )
                    })
            }
            Status::Trusted => {
                let approver = me
                    .approved_by
                    .as_ref()
                    .and_then(|id| devices.iter().find(|device| device.id == *id));
                let detail = match approver {
                    Some(approver) => format!(
                        "Approved by {} · {}",
                        approver.name,
                        ui::day(me.approved_at.as_deref().unwrap_or(&me.created_at))
                    ),
                    None => "Trusted since you set it up".into(),
                };
                let roots = devices
                    .iter()
                    .filter(|device| trust(device, devices) == Some(Trust::Root))
                    .count();
                let mine = trust(me, devices);
                let confirm = approver::unconfirmed(&self.approver)
                    .cloned()
                    .map(|shown| self.render_confirm(&shown, cx));
                let (mark, dot) = if mine == Some(Trust::Unverified) {
                    ("Not verified", p.destructive)
                } else if confirm.is_some() {
                    ("Confirm approver", p.waiting())
                } else {
                    ("Trusted", Palette::success())
                };
                let copy = self.copy_button("devices-copy-fingerprint", Copied::Device, format_code(&code), cx);
                v_flex()
                    .when(mine == Some(Trust::Unverified), |column| {
                        column.child(
                            warning_banner(
                                "This device's approval does not verify",
                                "Do not trust it until you pair it again.",
                                p,
                            )
                            .mb(px(8.)),
                        )
                    })
                    .child(
                        ui::card_frame(p)
                            .child(
                                ui::card_row()
                                    .child(device_tile(&me.name, p))
                                    .child(ui::two_lines(me.name.clone(), detail, p).flex_1())
                                    .child(ui::status(mark, dot, p)),
                            )
                            .child(
                                h_flex()
                                    .justify_between()
                                    .gap(px(12.))
                                    .mx(px(16.))
                                    .mb(px(14.))
                                    .px(px(12.))
                                    .py(px(10.))
                                    .rounded(px(8.))
                                    .bg(ui::inset(p))
                                    .child(
                                        v_flex()
                                            .gap(px(2.))
                                            .child(ui::label("Fingerprint", p))
                                            .child(code_groups(&code, 15., p)),
                                    )
                                    .child(copy),
                            )
                            .when_some(confirm, |card, section| card.child(section)),
                    )
                    .child(ui::footnote(
                        if mine == Some(Trust::Root) && roots == 1 {
                            "This is your first trusted device. You add every other device from here. If you lose \
                             every trusted device, no one can read the sessions synced so far, Nexus included."
                        } else {
                            "When you approve a new device from here, it shows this fingerprint. Compare all 16 \
                             characters."
                        },
                        p,
                    ))
            }
        }
    }

    /// The button that copies a fingerprint, with a check for a moment after.
    fn copy_button(&self, id: &'static str, what: Copied, text: String, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = palette(cx);
        let copied = self.copied_at.is_some_and(|(last, _)| last == what);
        ui::focusable(div().id(id), p)
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .cursor_pointer()
            .hover(move |style| style.bg(p.selected))
            .active(|style| style.opacity(0.8))
            .child(ui::icon(
                if copied { Lucide::Check } else { Lucide::Copy },
                if copied { Palette::success() } else { p.icon },
            ))
            .on_click(cx.listener(move |this, _, _, cx| this.copy_fingerprint(text.clone(), what, cx)))
    }

    /// The org's recovery key, when it has one: laid out as This device, with its fingerprint in a
    /// well, and Confirm while this computer has not pinned the key Gateway serves. A changed key is a calm state, not
    /// an alarm: replacing the key is something admins do.
    pub(super) fn render_org_key(&self, cx: &mut Context<Self>) -> Option<Div> {
        use nexus_workbench::session_sync::org_key::{OrgKeyState, copy, grouped};
        let p = palette(cx);
        let Load::Ready((_, _, _, state)) = &self.org_key else {
            return None;
        };
        let (shown, mark, dot, detail) = match state {
            OrgKeyState::None => return None,
            OrgKeyState::Confirmed(shown) => (shown, "Confirmed", Palette::success(), copy::CONFIRMED),
            OrgKeyState::Unconfirmed(shown) => (shown, "Not confirmed", p.waiting(), copy::ABOUT),
            OrgKeyState::Changed { served, .. } => (served, "Key changed", p.waiting(), copy::CHANGED),
        };
        let pending = !matches!(state, OrgKeyState::Confirmed(_));
        let fingerprint = h_flex()
            .justify_between()
            .items_start()
            .gap(px(12.))
            .mx(px(16.))
            .mb(px(14.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(8.))
            .bg(ui::inset(p))
            .child(
                v_flex()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(ui::label("Fingerprint", p))
                    .child(code_groups(&shown.fingerprint, 13., p)),
            )
            .child(self.copy_button("devices-copy-org-key", Copied::OrgKey, grouped(&shown.fingerprint), cx));
        let confirm = if self.busy {
            ui::disabled_button("devices-confirm-org-key", "Confirming…", p).into_any_element()
        } else {
            ui::primary_button("devices-confirm-org-key", "Confirm", p)
                .on_click(cx.listener(|this, _, _, cx| this.confirm_org_key(cx)))
                .into_any_element()
        };
        Some(
            v_flex().child(ui::group_head(copy::TITLE, p)).child(
                ui::card_frame(p)
                    .child(
                        ui::card_row()
                            .child(ui::icon_tile(Lucide::KeyRound, p))
                            .child(ui::two_lines("Org recovery key", detail, p).flex_1())
                            .child(ui::status(mark, dot, p)),
                    )
                    .child(fingerprint)
                    .when(pending, |card| {
                        card.child(actions_footer(copy::COMPARE, h_flex().child(confirm), p))
                    }),
            ),
        )
    }

    /// Gateway offers to vouch for every device the person signs in on, and this computer relied on approvals: it keeps
    /// them until the person opts in here (`trust_pin.rs`).
    pub(super) fn render_trust_gateway(&self, cx: &mut Context<Self>) -> Option<Div> {
        self.gateway_waits.as_ref()?;
        let p = palette(cx);
        let accept = if self.busy {
            ui::disabled_button("devices-trust-gateway", "Trusting…", p).into_any_element()
        } else {
            ui::primary_button("devices-trust-gateway", "Trust Gateway's list", p)
                .on_click(cx.listener(|this, _, _, cx| this.trust_gateway(cx)))
                .into_any_element()
        };
        Some(
            v_flex().child(ui::group_head("Device trust", p)).child(
                ui::card_frame(p)
                    .child(
                        ui::card_row()
                            .child(ui::icon_tile(Lucide::ShieldCheck, p))
                            .child(
                                ui::two_lines(
                                    "Gateway offers to trust your devices",
                                    "Nexus Gateway now trusts every device you sign in on, with no approval. This                                      computer keeps asking for approvals until you choose otherwise here.",
                                    p,
                                )
                                .flex_1(),
                            ),
                    )
                    .child(actions_footer(
                        "Then every device Gateway lists can read your synced sessions and control this computer.",
                        h_flex().child(accept),
                        p,
                    )),
            ),
        )
    }

    /// This device's approver as Gateway names it, its code and Confirm. Session sync waits for it.
    fn render_confirm(&self, shown: &nexus_workbench::session_sync::approver::Approver, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let confirm = if self.busy {
            ui::disabled_button("devices-confirm-approver", "Confirming…", p).into_any_element()
        } else {
            ui::primary_button("devices-confirm-approver", "Confirm", p)
                .on_click(cx.listener(|this, _, _, cx| this.confirm_approver(cx)))
                .into_any_element()
        };
        v_flex()
            .border_t_1()
            .border_color(p.border)
            .child(
                v_flex()
                    .gap(px(6.))
                    .px(px(16.))
                    .pt(px(14.))
                    .pb(px(16.))
                    .child(div().text_size(px(13.)).text_color(p.muted).child(format!(
                        "Compare this fingerprint with the one on {}, then click Confirm to allow encrypted sync.",
                        shown.name
                    )))
                    .child(
                        well(p)
                            .py(px(14.))
                            .items_center()
                            .child(code_groups(&shown.code, 22., p)),
                    ),
            )
            .child(actions_footer(
                "If it shows another code, revoke this device from a device you trust.",
                h_flex().child(confirm),
                p,
            ))
    }

    fn render_approve(&self, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let my_code = self.me().map(device_code).unwrap_or_default();
        let card = match &self.found {
            Some((device, input)) => {
                let approve = if self.busy {
                    ui::disabled_button("devices-approve", "Approving…", p).into_any_element()
                } else {
                    ui::primary_button("devices-approve", "Approve", p)
                        .on_click(cx.listener(|this, _, window, cx| this.approve(window, cx)))
                        .into_any_element()
                };
                ui::card_frame(p)
                    .child(
                        ui::card_row()
                            .child(device_tile(&device.name, p))
                            .child(device_lines(
                                &device.name,
                                Some("New"),
                                div().truncate().child(format!(
                                    "Registered {} · Its keys match {}",
                                    ui::day(&device.created_at),
                                    format_code(&input.code)
                                )),
                                p,
                            ))
                            .child(ui::status("Waiting", p.waiting(), p)),
                    )
                    .child(
                        v_flex()
                            .gap(px(6.))
                            .px(px(16.))
                            .pb(px(16.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(p.muted)
                                    .child(format!("Check that {} shows this device's fingerprint:", device.name)),
                            )
                            .child(well(p).py(px(14.)).items_center().child(code_groups(&my_code, 22., p))),
                    )
                    .child(actions_footer(
                        "Once approved, it can read your synced sessions.",
                        h_flex()
                            .gap(px(8.))
                            .child(ui::ghost_button("devices-cancel", "Cancel", p).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.found = None;
                                    cx.notify();
                                },
                            )))
                            .child(approve),
                        p,
                    ))
            }
            None => {
                let empty = self.code.read(cx).value().trim().is_empty();
                let find = if self.busy || empty {
                    ui::disabled_button("devices-find", if self.busy { "Checking…" } else { "Find device" }, p)
                        .into_any_element()
                } else {
                    ui::primary_button("devices-find", "Find device", p)
                        .on_click(cx.listener(|this, _, _, cx| this.find(cx)))
                        .into_any_element()
                };
                ui::card_frame(p).child(
                    v_flex()
                        .gap(px(10.))
                        .px(px(16.))
                        .py(px(14.))
                        .child(div().text_size(px(13.)).text_color(p.muted).child(
                            "On the new device, open Settings, then Devices, and set it up. Type the code it shows.",
                        ))
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(text_field(&self.code, p).font_family(ui::MONO))
                                .child(find),
                        ),
                )
            }
        };
        v_flex()
            .child(card)
            .when_some(self.approve_outcome.as_ref(), |column, (text, error)| {
                column.child(outcome_line(text, *error, p))
            })
    }

    /// The last action's outcome line, when it answers `part`: it shows under that part of the page.
    fn outcome_under(&self, part: Part, p: Palette) -> Option<Div> {
        let (at, text, error) = self.outcome.as_ref()?;
        (*at == part).then(|| outcome_line(text, *error, p))
    }

    fn render_table(&self, devices: &[Device], can_revoke: bool, cx: &mut Context<Self>) -> Div {
        let p = palette(cx);
        let now = now_ms();
        let roots: Vec<&Device> = devices
            .iter()
            .filter(|device| trust(device, devices) == Some(Trust::Root))
            .collect();
        // The first device is the oldest one trusted without an approval; any later one came after every device was
        // revoked, or from someone with the person's sign-in.
        let first = roots
            .iter()
            .min_by_key(|device| iso_ms(&device.created_at).unwrap_or(i64::MAX))
            .map(|device| device.id.clone());
        let [active, expired, revoked] = table_groups(devices, now);
        let mut card = ui::card_frame(p);
        for (ix, device) in active.iter().enumerate() {
            if ix > 0 {
                card = card.child(deep_rule(p));
            }
            card = card.child(self.device_row(device, devices, first.as_deref(), can_revoke, now, cx));
        }
        let mut above = !active.is_empty();
        for (label, rows) in [("Expired pairings", &expired), ("Revoked", &revoked)] {
            if rows.is_empty() {
                continue;
            }
            card = card.child(
                div()
                    .px(px(16.))
                    .pt(px(8.))
                    .pb(px(6.))
                    .when(above, |head| head.border_t_1().border_color(p.border))
                    .text_size(px(11.))
                    .text_color(p.faint)
                    .child(format!("{label} · {}", rows.len())),
            );
            for (ix, device) in rows.iter().enumerate() {
                if ix > 0 {
                    card = card.child(deep_rule(p));
                }
                card = card.child(self.device_row(device, devices, first.as_deref(), can_revoke, now, cx));
            }
            above = true;
        }
        v_flex()
            .child(
                ui::group_head("All devices", p).child(
                    div()
                        .text_size(px(12.))
                        .text_color(p.faint)
                        .child(format!("{} active", active.len())),
                ),
            )
            // In gateway mode every device is trusted without an approval: that is the mode, not a warning.
            .when(roots.len() > 1 && self.trust == DeviceTrust::Approvals, |column| {
                column.child(
                    warning_banner(
                        format!("{} devices were trusted without an approval", roots.len()),
                        "One is your first device. Each other came after every device was revoked, or from someone \
                         with your sign-in. Revoke any you do not know.",
                        p,
                    )
                    .mb(px(8.)),
                )
            })
            .child(card)
    }

    /// One device in All devices: its name, a line of detail, its state and Revoke. After the first press of Revoke,
    /// the row asks to confirm, with Revoke where Cancel was not, so a double press lands on Cancel.
    fn device_row(
        &self,
        device: &Device,
        devices: &[Device],
        first: Option<&str>,
        can_revoke: bool,
        now: i64,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = palette(cx);
        let mine = self.me().is_some_and(|me| me.id == device.id);
        let id = device.id.clone();
        if self.confirm_revoke.as_deref() == Some(device.id.as_str()) {
            let (title, detail) = if mine {
                (
                    "Revoke this device?".to_string(),
                    "It stops getting keys, and you cannot approve or revoke from it again.",
                )
            } else {
                (
                    format!("Revoke {}?", device.name),
                    "It gets no new keys. What it already read stays on it.",
                )
            };
            return ui::card_row()
                .relative()
                .child(motion::fade_in(
                    div().absolute().inset_0().bg(danger_tint(p)),
                    SharedString::from(format!("devices-revoke-tint-{}", device.id)),
                ))
                .child(device_tile(&device.name, p))
                .child(ui::setting_lines(title, detail, p).flex_1())
                .child(
                    ui::primary_button(SharedString::from(format!("devices-revoke-{}", device.id)), "Revoke", p)
                        .bg(p.destructive)
                        .text_color(white())
                        .on_click(cx.listener(move |this, _, _, cx| this.revoke(id.clone(), cx))),
                )
                .child(
                    ui::ghost_button("devices-revoke-cancel", "Cancel", p)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_revoke(cx))),
                );
        }
        let state = status(device, now);
        let trust = trust(device, devices);
        // In gateway mode Gateway vouches for each device: none needed an approval.
        let by_account = self.trust == DeviceTrust::Gateway;
        let first_device = by_account || first == Some(device.id.as_str());
        let (mark, dot) = match (state, trust) {
            (Status::Trusted, Some(Trust::Unverified)) => ("Not verified", p.destructive),
            (Status::Trusted, Some(Trust::Root)) if !first_device => ("No approval", p.waiting()),
            (Status::Trusted, _) => ("Trusted", Palette::success()),
            (Status::Pending, _) => (status_label(state), p.waiting()),
            (Status::Expired | Status::Revoked, _) => (status_label(state), p.steel()),
        };
        let how = match (state, trust) {
            (Status::Revoked, _) => format!(
                "Revoked {}",
                ui::day(device.revoked_at.as_deref().unwrap_or(&device.created_at))
            ),
            (_, Some(Trust::Root)) if by_account => "Signed in to your account".into(),
            (_, Some(Trust::Root)) if first_device => "First device".into(),
            (_, Some(Trust::Root)) => "Trusted without an approval".into(),
            (_, Some(Trust::Unverified)) => "Approval does not verify".into(),
            (_, Some(Trust::Approved)) => device
                .approved_by
                .as_ref()
                .and_then(|id| devices.iter().find(|each| each.id == *id))
                .map(|approver| format!("Approved by {}", approver.name))
                .unwrap_or_else(|| "Approved".into()),
            (_, None) => "Not approved yet".into(),
        };
        let added = if state == Status::Revoked {
            String::new()
        } else {
            format!("Added {} · ", ui::day(&device.created_at))
        };
        ui::card_row()
            .when(state == Status::Revoked, |row| row.opacity(0.6))
            .child(device_tile(&device.name, p))
            .child(device_lines(
                &device.name,
                mine.then_some("This device"),
                h_flex()
                    .min_w(px(0.))
                    .child(
                        div()
                            .flex_none()
                            .font_family(ui::MONO)
                            .child(short_code(&device_code(device))),
                    )
                    .child(div().min_w(px(0.)).truncate().child(format!(" · {added}{how}"))),
                p,
            ))
            .child(ui::status(mark, dot, p))
            .when(state != Status::Revoked && can_revoke, |row| {
                row.child(
                    ui::ghost_button(SharedString::from(format!("devices-revoke-{}", device.id)), "Revoke", p)
                        .on_click(cx.listener(move |this, _, _, cx| this.revoke(id.clone(), cx))),
                )
            })
    }
}

/// A row's text starts here in a card: the row's 16px padding, the 28px icon tile and the 16px gap.
const ROW_TEXT_INSET: f32 = 60.;

/// The hairline between two rows that start with an icon tile: it starts at their text, as on This computer.
fn deep_rule(palette: Palette) -> Div {
    div().h(px(1.)).ml(px(ROW_TEXT_INSET)).bg(palette.border)
}

/// A recessed panel inside a card, for a code someone compares.
fn well(palette: Palette) -> Div {
    v_flex().rounded(px(8.)).bg(ui::inset(palette))
}

/// The red fill behind a row that asks to confirm a revoke, and behind a trust warning.
fn danger_tint(palette: Palette) -> Hsla {
    palette.destructive.opacity(if palette.dark { 0.10 } else { 0.07 })
}

/// A phone's icon for a device whose name says it is one; a laptop's for every other. Gateway lists no device kind.
fn device_icon(name: &str) -> Lucide {
    let name = name.to_lowercase();
    if ["phone", "ipad", "android", "pixel"]
        .iter()
        .any(|word| name.contains(word))
    {
        Lucide::Smartphone
    } else {
        Lucide::Laptop
    }
}

pub(super) fn device_tile(name: &str, palette: Palette) -> Div {
    ui::icon_tile(device_icon(name), palette)
}

/// A device's name, with a pill after it ("This device"), over one line of detail.
fn device_lines(name: &str, pill: Option<&'static str>, detail: impl IntoElement, palette: Palette) -> Div {
    v_flex()
        .flex_1()
        .min_w(px(0.))
        .gap(px(2.))
        .child(
            h_flex()
                .min_w(px(0.))
                .gap(px(8.))
                .child(
                    div()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(13.))
                        .text_color(palette.strong)
                        .child(name.to_string()),
                )
                .when_some(pill, |row, text| row.child(ui::pill(text, palette))),
        )
        .child(
            div()
                .min_w(px(0.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(detail),
        )
}

/// A code in its groups of four, spaced so two screens are easy to compare.
fn code_groups(code: &str, size: f32, palette: Palette) -> Div {
    h_flex()
        .flex_wrap()
        .gap(px((size * 0.45).round()))
        .font_family(ui::MONO)
        .text_size(px(size))
        .line_height(px((size * 1.3).round()))
        .text_color(palette.strong)
        .children(
            code.as_bytes()
                .chunks(4)
                .map(|group| div().child(String::from_utf8_lossy(group).into_owned())),
        )
}

/// The first two groups of a code: enough to tell devices apart in a list.
fn short_code(code: &str) -> String {
    format_code(&code.chars().take(8).collect::<String>())
}

/// "12 minutes left", or "Under a minute left".
fn time_left(until: i64, now: i64) -> String {
    let minutes = ((until - now) as f64 / 60_000.).ceil().max(1.) as i64;
    if minutes == 1 {
        "Under a minute left".into()
    } else {
        format!("{minutes} minutes left")
    }
}

/// How much of a pairing window is left, as a thin bar that empties.
fn countdown_bar(share: f32, palette: Palette) -> Div {
    div()
        .h(px(3.))
        .w_full()
        .rounded(px(2.))
        .bg(palette.selected)
        .overflow_hidden()
        .child(div().h_full().w(relative(share)).bg(palette.waiting()))
}

/// A warning about trust: an icon, a title and what to do, on a red tint, so it does not rely on color alone.
fn warning_banner(title: impl Into<SharedString>, detail: impl Into<SharedString>, palette: Palette) -> Div {
    h_flex()
        .items_start()
        .gap(px(10.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(12.))
        .border_1()
        .border_color(palette.destructive.opacity(0.35))
        .bg(danger_tint(palette))
        .child(
            div()
                .flex_none()
                .pt(px(2.))
                .child(ui::icon(Lucide::TriangleAlert, palette.destructive)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.strong)
                        .child(title.into()),
                )
                .child(ui::setting_detail(detail, palette)),
        )
}

/// A card's last strip: a note on the left, its buttons on the right.
fn actions_footer(note: &'static str, buttons: Div, palette: Palette) -> Div {
    h_flex()
        .flex_wrap()
        .gap(px(12.))
        .px(px(16.))
        .py(px(12.))
        .border_t_1()
        .border_color(palette.border)
        .child(ui::setting_detail(note, palette).flex_1().min_w(px(160.)))
        .child(buttons.flex_none().ml_auto())
}

/// What an action did, under its card: red with an error, the body color otherwise.
fn outcome_line(text: &str, error: bool, palette: Palette) -> Div {
    if error {
        ui::error_line(text.to_string(), palette).mt(px(8.))
    } else {
        ui::footnote(text.to_string(), palette).text_color(palette.foreground)
    }
}

pub(super) fn text_field(state: &Entity<InputState>, palette: Palette) -> Div {
    h_flex()
        .flex_1()
        .min_w(px(0.))
        .h(px(ui::CONTROL_HEIGHT))
        .px(px(10.))
        .rounded(px(8.))
        .bg(ui::inset(palette))
        .border_1()
        .border_color(palette.border)
        .text_size(px(13.))
        .child(div().flex_1().min_w(px(0.)).child(Input::new(state).appearance(false)))
}

impl Render for DevicesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let parts = self.parts();
        let column = v_flex().child(ui::lead(
            if parts.pairing {
                "The devices that can read your synced sessions. Each holds its own key. Nexus holds none that opens \
                 them."
            } else {
                "The devices that can read your synced sessions. Devices signed in to your account connect \
                 automatically, with no code to compare. Each holds its own key."
            },
            p,
        ));
        let web = self.ctx.clone();
        let recovery = v_flex().child(ui::group_head("Recovery", p)).child(
            ui::card_frame(p).child(
                ui::nav_row(
                    "devices-recovery-browser",
                    Lucide::KeyRound,
                    "Recovery codes and passkeys",
                    if parts.pairing {
                        "Manage them in your browser. Pairing and approval stay here."
                    } else {
                        "Manage them in your browser."
                    },
                    true,
                    p,
                )
                .on_click(move |_, _, cx| web.open_web("settings/devices", cx)),
            ),
        );

        let failure = match (&self.key, &self.devices) {
            (Load::Failed(error), _) | (_, Load::Failed(error)) => Some(error.clone()),
            _ => None,
        };
        if let Some(error) = failure {
            return column
                .child(ui::failed(error, p))
                .child(
                    h_flex()
                        .justify_center()
                        .child(
                            ui::button("devices-retry", "Try again", p).on_click(cx.listener(|this, _, _, cx| {
                                if matches!(this.key, Load::Failed(_)) {
                                    this.retry_key(cx);
                                }
                                this.devices = Load::Loading;
                                this.load(cx);
                                cx.notify();
                            })),
                        ),
                )
                .child(recovery);
        }
        let devices = match (&self.key, &self.devices) {
            (Load::Ready(_), Load::Ready(devices)) => devices.clone(),
            _ => {
                return column.child(ui::group_head("This device", p)).child(ui::skeleton(2, p));
            }
        };
        let trusted_here = self.me().is_some_and(|me| status(me, now_ms()) == Status::Trusted);
        let mut column = column
            .child(self.render_phone(cx))
            .children(self.remote_computers().cloned());
        column = column.children(self.render_trust_gateway(cx));
        if self.link_invitation.is_none() {
            let this_device = v_flex()
                .child(ui::group_head("This device", p))
                .child(self.render_this_device(&devices, cx))
                .children(self.outcome_under(Part::ThisDevice, p));
            column = column.child(self.ctx.settings_fields.field(THIS_DEVICE, this_device, None, cx));
        }
        if trusted_here {
            if let Some(org_key) = self.render_org_key(cx) {
                column = column.child(org_key).children(self.outcome_under(Part::OrgKey, p));
            }
            if parts.add_device {
                column = column
                    .child(ui::group_head("Add a device", p))
                    .child(self.render_approve(cx));
            }
        }
        if !devices.is_empty() {
            column = column
                .child(self.render_table(&devices, trusted_here, cx))
                .children(self.outcome_under(Part::AllDevices, p));
        }
        column.child(recovery)
    }
}

/// All devices in three groups, each newest first: active (trusted, or waiting within the pairing window), expired
/// pairings, and revoked. Only the first group counts as active: an expired pairing can never be approved.
fn table_groups(devices: &[Device], now: i64) -> [Vec<&Device>; 3] {
    let mut rows: Vec<&Device> = devices.iter().collect();
    rows.sort_by_key(|device| std::cmp::Reverse(iso_ms(&device.created_at).unwrap_or(0)));
    let mut groups: [Vec<&Device>; 3] = Default::default();
    for device in rows {
        let group = match status(device, now) {
            Status::Revoked => 2,
            Status::Expired => 1,
            Status::Trusted | Status::Pending => 0,
        };
        groups[group].push(device);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::{Lucide, device_icon, short_code, table_groups, time_left};

    #[test]
    fn counts_down_in_whole_minutes() {
        let now = 1_000_000;
        assert_eq!(time_left(now + 12 * 60_000, now), "12 minutes left");
        assert_eq!(time_left(now + 11 * 60_000 + 1, now), "12 minutes left");
        assert_eq!(time_left(now + 30_000, now), "Under a minute left");
        assert_eq!(time_left(now - 5_000, now), "Under a minute left");
    }

    #[test]
    fn a_list_shows_the_first_two_groups_of_a_code() {
        assert_eq!(short_code("8C2F91AE33D07B1C"), "8C2F 91AE");
        assert_eq!(short_code(""), "");
    }

    #[test]
    fn a_phone_by_its_name_gets_a_phone_icon() {
        assert!(matches!(device_icon("iPhone 17"), Lucide::Smartphone));
        assert!(matches!(device_icon("Ana's Pixel 9"), Lucide::Smartphone));
        assert!(matches!(device_icon("Studio iMac"), Lucide::Laptop));
    }

    #[test]
    fn expired_pairings_are_not_active_and_list_apart() {
        let row = |id: &str, state: &str, at: &str| {
            serde_json::json!({ "id": id, "name": id, "encryptionKey": "e", "signingKey": "s", "state": state,
                "createdAt": at })
        };
        let devices = super::read_devices(&serde_json::json!({ "devices": [
            row("phone", "trusted", "2026-11-09T10:00:00Z"),
            row("old-mac", "pending", "2026-11-21T10:00:00Z"),
            row("this-mac", "pending", "2026-11-25T11:50:00Z"),
            row("newer-mac", "pending", "2026-11-23T10:00:00Z"),
            row("lost", "revoked", "2026-11-01T10:00:00Z"),
        ] }));
        let now = super::iso_ms("2026-11-25T12:00:00Z").expect("a time");
        let ids = |group: &Vec<&super::Device>| group.iter().map(|device| device.id.clone()).collect::<Vec<_>>();
        let [active, expired, revoked] = table_groups(&devices, now);
        assert_eq!(ids(&active), ["this-mac", "phone"], "10 minutes old still waits");
        assert_eq!(ids(&expired), ["newer-mac", "old-mac"], "newest first");
        assert_eq!(ids(&revoked), ["lost"]);
    }
}
