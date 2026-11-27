use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::computer::{JoinState, JoinStatus};

use super::view::text_field;
use super::{Ctx, palette, ui};
use crate::code_run::{Answer, Projects};
use crate::palette::Palette;

mod work;
use work::Work;

const POLL: Duration = Duration::from_secs(2);
type Identity = (String, String);

pub(super) struct RemoteComputers {
    ctx: Ctx,
    identity: Option<Identity>,
    link: Entity<InputState>,
    join: JoinFlow,
    join_poll: Option<Task<()>>,
    control: Control,
    work: Work,
    _subscriptions: Vec<Subscription>,
}

#[derive(Default)]
enum JoinFlow {
    #[default]
    Idle,
    Starting,
    Waiting {
        status: JoinStatus,
        error: Option<String>,
    },
    Approved(JoinStatus),
    Failed(String),
}

#[derive(Default)]
enum Control {
    #[default]
    Check,
    Saving,
    On,
    Failed(String),
}

async fn answer<T>(request: Answer<T>) -> Result<T, String> {
    request
        .await
        .unwrap_or_else(|_| Err("Code sessions did not answer. Try again.".into()))
}

impl RemoteComputers {
    pub(super) fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let link = cx.new(|cx| InputState::new(window, cx).placeholder("Connection link"));
        let work = Work::new(window, cx);
        let subscriptions = vec![
            cx.observe(&link, |_, _, cx| cx.notify()),
            cx.subscribe_in(&link, window, |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.join(cx);
                }
            }),
            cx.observe(&work.prompt, |_, _, cx| cx.notify()),
        ];
        Self {
            ctx,
            identity: None,
            link,
            join: JoinFlow::Idle,
            join_poll: None,
            control: Control::Check,
            work,
            _subscriptions: subscriptions,
        }
    }

    fn access(&self, cx: &App) -> Option<Projects> {
        let commands = (self.ctx.code)(cx).projects?;
        (commands.computer_scope().as_ref() == self.identity.as_ref() && self.identity.is_some()).then_some(commands)
    }

    fn join(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.join, JoinFlow::Idle | JoinFlow::Failed(_)) {
            return;
        }
        let link = self.link.read(cx).value().trim().to_string();
        if link.is_empty() {
            return;
        }
        let Some(commands) = self.access(cx) else { return };
        self.join = JoinFlow::Starting;
        cx.notify();
        let request = commands.computer_join_begin(link);
        self.join_poll = Some(cx.spawn(async move |this, cx| {
            let response = answer(request).await;
            let _ = this.update(cx, |this, cx| {
                if this.access(cx).is_none() {
                    return;
                }
                match response {
                    Ok(status) => {
                        let invite_id = status.invite_id.clone();
                        if this.join_result(status, cx) {
                            this.wait_for_approval(invite_id, cx);
                        }
                    }
                    Err(error) => this.join = JoinFlow::Failed(error),
                }
                cx.notify();
            });
        }));
    }

    fn join_result(&mut self, status: JoinStatus, cx: &mut Context<Self>) -> bool {
        let pending = status.state == JoinState::Pending;
        self.join = match status.state {
            JoinState::Pending => JoinFlow::Waiting { status, error: None },
            JoinState::Approved => {
                self.load_peers(cx);
                JoinFlow::Approved(status)
            }
            JoinState::Cancelled => {
                JoinFlow::Failed("The other computer cancelled this connection. Ask for a new link.".into())
            }
            JoinState::Expired => {
                JoinFlow::Failed("This connection link expired. Ask the other computer for a new link.".into())
            }
        };
        if matches!(self.join, JoinFlow::Approved(_)) {
            self.allow_control(cx);
        }
        pending
    }

    fn wait_for_approval(&mut self, invite_id: String, cx: &mut Context<Self>) {
        self.join_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let Ok(Some(commands)) = this.update(cx, |this, cx| this.access(cx)) else {
                    return;
                };
                let response = answer(commands.computer_join_poll(invite_id.clone())).await;
                let Ok(pending) = this.update(cx, |this, cx| {
                    if this.access(cx).is_none() {
                        return false;
                    }
                    let pending = match response {
                        Ok(status) if status.invite_id == invite_id => this.join_result(status, cx),
                        Ok(_) => {
                            this.join = JoinFlow::Failed("This approval was for a different connection.".into());
                            false
                        }
                        Err(message) => {
                            if let JoinFlow::Waiting { error, .. } = &mut this.join {
                                *error = Some(message);
                            }
                            true
                        }
                    };
                    cx.notify();
                    pending
                }) else {
                    return;
                };
                if !pending {
                    return;
                }
            }
        }));
    }

    fn allow_control(&mut self, cx: &mut Context<Self>) {
        let JoinFlow::Approved(status) = &self.join else { return };
        if matches!(self.control, Control::Saving | Control::On) {
            return;
        }
        let Some(commands) = self.access(cx) else { return };
        self.control = Control::Saving;
        cx.notify();
        let invite_id = status.invite_id.clone();
        let request = commands.computer_join_allow(invite_id.clone());
        cx.spawn(async move |this, cx| {
            let response = answer(request).await;
            let _ = this.update(cx, |this, cx| {
                if this.access(cx).is_none()
                    || !matches!(&this.join, JoinFlow::Approved(status) if status.invite_id == invite_id)
                {
                    return;
                }
                this.control = match response {
                    Ok(settings) if settings.enabled => Control::On,
                    Ok(_) => Control::Failed("Set up this computer for encrypted session sync, then try again.".into()),
                    Err(error) => Control::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn reset_join(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.join_poll = None;
        self.join = JoinFlow::Idle;
        self.control = Control::Check;
        self.link.update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    fn render_join(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let card = ui::card_frame(p);
        match &self.join {
            JoinFlow::Waiting { status, error } => card.child(
                ui::card_row()
                    .justify_between()
                    .child(ui::setting_lines(
                        format!("Waiting for {}", status.computer_name),
                        error
                            .as_deref()
                            .unwrap_or("Select Approve device on that computer to finish connecting."),
                        p,
                    ))
                    .child(
                        ui::ghost_button("remote-use-new-link", "Use another link", p)
                            .on_click(cx.listener(|this, _, window, cx| this.reset_join(window, cx))),
                    ),
            ),
            JoinFlow::Approved(status) => {
                let (description, action) = match &self.control {
                    Control::On => (
                        format!("{} can also control this computer.", status.computer_name),
                        None,
                    ),
                    Control::Saving => ("Turning on remote control for this computer…".into(), None),
                    Control::Check => (
                        format!(
                            "Allow {} to start and continue work on this computer, too.",
                            status.computer_name
                        ),
                        Some("Allow remote control"),
                    ),
                    Control::Failed(error) => (error.clone(), Some("Try again")),
                };
                card.child(
                    ui::card_row()
                        .justify_between()
                        .child(ui::setting_lines(
                            format!("Connected to {}", status.computer_name),
                            description,
                            p,
                        ))
                        .when_some(action, |row, label| {
                            row.child(
                                ui::button("remote-allow-control", label, p)
                                    .on_click(cx.listener(|this, _, _, cx| this.allow_control(cx))),
                            )
                        }),
                )
                .child(ui::card_rule(p))
                .child(
                    ui::card_row().child(
                        ui::ghost_button("remote-use-new-link", "Connect another computer", p)
                            .on_click(cx.listener(|this, _, window, cx| this.reset_join(window, cx))),
                    ),
                )
            }
            JoinFlow::Idle | JoinFlow::Starting | JoinFlow::Failed(_) => {
                let starting = matches!(self.join, JoinFlow::Starting);
                let detail = match &self.join {
                    JoinFlow::Failed(error) => error.as_str(),
                    _ => {
                        "Paste the link from the other computer’s Devices settings. Linking allows each computer to start and continue work on the other."
                    }
                };
                card.child(
                    v_flex()
                        .min_w(px(0.))
                        .gap(px(8.))
                        .p(px(16.))
                        .child(ui::setting_lines("Join with a connection link", detail, p))
                        .child(h_flex().gap(px(8.)).child(text_field(&self.link, p)).child(
                            if starting || self.identity.is_none() || self.link.read(cx).value().trim().is_empty() {
                                ui::disabled_button(
                                    "remote-join",
                                    if starting { "Connecting…" } else { "Link computers" },
                                    p,
                                )
                            } else {
                                ui::primary_button("remote-join", "Link computers", p)
                                    .on_click(cx.listener(|this, _, _, cx| this.join(cx)))
                            },
                        )),
                )
            }
        }
    }
}

impl Render for RemoteComputers {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let identity = (self.ctx.code)(cx)
            .projects
            .and_then(|commands| commands.computer_scope());
        if self.identity != identity {
            self.identity = identity;
            self.reset_join(window, cx);
            self.work.prompt.update(cx, |state, cx| state.set_value("", window, cx));
            self.work.reset();
            self.load_peers(cx);
        }
        let p = palette(cx);
        v_flex()
            .child(ui::group_head("Other computers", p))
            .child(self.render_join(p, cx))
            .child(self.render_work(p, cx))
    }
}
