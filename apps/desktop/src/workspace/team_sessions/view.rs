//! A native read-only transcript. No composer, approval, co-driving, or handoff controls.
use super::{
    Connection, Palette, Session, Workspace,
    model::{MAX_EVENTS, Opened},
};
use gpui_kit::{
    component::{h_flex, v_flex},
    prelude::*,
    *,
};
use nexus_desktop_core::presence;
use nexus_workbench::event::WbEvent;

/// This computer runs no sessions, so it can't read a teammate's: the note links to Devices.
pub(super) const NOT_CONNECTED: &str = "This computer isn't connected.";
/// Devices' This device anchor (`THIS_DEVICE` in `pages/devices/view.rs`).
const THIS_DEVICE: &str = "this-device";

/// Why Team sessions show nothing here, in one quiet line, when they can't: nobody in the app can change it.
fn off_note(team: bool, gateway_lacks_presence: bool, sessions_on: bool) -> Option<&'static str> {
    if !team {
        Some("Team sessions are for organization accounts.")
    } else if gateway_lacks_presence {
        Some("Team sessions need a newer Nexus Gateway.")
    } else if !sessions_on {
        Some("Your organization keeps team sessions off.")
    } else {
        None
    }
}

fn action(id: &'static str, label: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .px(px(10.))
        .py(px(6.))
        .rounded(px(6.))
        .text_sm()
        .text_color(palette.foreground)
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .child(label)
}
fn event_text(event: &Opened) -> Option<(&'static str, String)> {
    match event {
        Opened::Open { event, .. } => match event {
            WbEvent::User { text, .. } => Some(("Person", text.clone())),
            WbEvent::Text { text, .. } => Some(("Assistant", text.clone())),
            WbEvent::Thinking { text, .. } => Some(("Thinking", text.clone())),
            WbEvent::Notice { text } => Some(("Notice", text.clone())),
            WbEvent::Error { text, .. } => Some(("Error", text.clone())),
            WbEvent::Task { description, .. } => Some(("Task", description.clone())),
            WbEvent::QueueEdit { text, .. } => Some(("Message edited", text.clone())),
            WbEvent::Tool { name, input, .. } => Some(("Tool", format!("{name}\n{input}"))),
            WbEvent::ToolResult { output, is_error, .. } => {
                Some((if *is_error { "Tool error" } else { "Tool result" }, output.clone()))
            }
            _ => None,
        },
        _ => None,
    }
}
impl Workspace {
    fn team_session_owner(&self, session: &Session) -> String {
        self.team_presence
            .own
            .iter()
            .chain(self.team_presence.members.iter())
            .find(|member| member.user_id == session.person_id)
            .map(|member| presence::display_name(member).to_string())
            .unwrap_or_else(|| "A teammate".into())
    }
    pub(in crate::workspace) fn render_team_sessions(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if let Some(why) = off_note(
            self.team_scope().is_some(),
            self.team_presence.disabled,
            self.team_presence.sessions,
        ) {
            return div()
                .p(px(24.))
                .child(crate::setup::quiet::info_line(
                    "team-sessions:off",
                    gpui_kit::assets::IconName::Info,
                    why,
                    palette,
                    cx,
                ))
                .into_any_element();
        }
        let viewer = &self.team_presence.viewer;
        let transcript = &viewer.transcript;
        let mut head = h_flex()
            .gap(px(12.))
            .p(px(16.))
            .border_b_1()
            .border_color(palette.border)
            .child(
                action("team-session-list", "Team sessions", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.open_team_sessions(None, window, cx))),
            );
        let mut body = v_flex()
            .id("native-team-session-body")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .p(px(24.))
            .gap(px(16.));
        if transcript.selected.is_some() {
            let title = transcript
                .title()
                .and_then(|title| title.title.as_deref())
                .unwrap_or("Teammate session")
                .to_string();
            head = head.child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(palette.foreground)
                    .child(title),
            );
            let self_id = self
                .account
                .as_ref()
                .and_then(|account| account.user_id.as_deref())
                .unwrap_or_default();
            if let Some(session) = &transcript.session {
                let state = if session.ended() {
                    "Ended"
                } else {
                    match transcript.connection {
                        Connection::Connecting => "Connecting…",
                        Connection::Live => "Live",
                        Connection::Reconnecting => "Disconnected · reconnecting…",
                        Connection::Unavailable => "Unavailable",
                    }
                };
                head = head.child(div().text_sm().text_color(palette.faint).child(state));
                if session.person_id != self_id && transcript.connection != Connection::Unavailable {
                    let following = session.followers.iter().any(|id| id == self_id);
                    head = head.child(
                        action(
                            "team-session-follow",
                            if viewer.following.is_some() {
                                "Saving…"
                            } else if following {
                                "Unfollow"
                            } else {
                                "Follow"
                            },
                            palette,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_team_follow(window, cx))),
                    );
                }
                let owner = self.team_session_owner(session);
                let agent = session.agent.map(|agent| agent.as_str()).unwrap_or("agent");
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(palette.faint)
                        .child(format!("{owner} · {agent} · Read only")),
                );
                let watchers = viewer.watchers.names(self_id);
                let watching = if watchers.is_empty() {
                    "No other viewers are visible".to_string()
                } else {
                    format!("Watching now: {}", watchers.join(", "))
                };
                body = body.child(div().text_sm().text_color(palette.faint).child(watching));
            }
            if let Some(project) = transcript.title().and_then(|title| title.project_name.as_deref()) {
                body = body.child(div().text_sm().text_color(palette.faint).child(project.to_string()));
            }
            if let Some(note) = transcript.note {
                // This computer runs no sessions: "Connect it ›" opens Devices at This device.
                let connect = (note == NOT_CONNECTED).then(|| {
                    crate::setup::quiet::link("team-session-connect", "Connect it", palette).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.run_setup_fix(
                                "team-sessions:connect".into(),
                                crate::setup::Fix::open_at(crate::pages::PageId::Devices, THIS_DEVICE),
                                window,
                                cx,
                            )
                        },
                    ))
                });
                body = body.child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .text_sm()
                        .text_color(palette.faint)
                        .child(note)
                        .children(connect),
                );
            }
            let local = transcript
                .events
                .values()
                .any(|event| matches!(event, Opened::Local { .. }));
            let locked = transcript
                .events
                .values()
                .any(|event| matches!(event, Opened::NoKey { .. }));
            let unreadable = transcript
                .events
                .values()
                .any(|event| matches!(event, Opened::Unreadable { .. }));
            if local {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(palette.faint)
                        .child("This organization keeps session content on its source computer."),
                );
            }
            if locked {
                body=body.child(div().text_sm().text_color(palette.faint).child("Waiting for a trusted key on this device. Teammate sessions open when the team key reaches this computer."));
            }
            if unreadable {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(palette.faint)
                        .child("Some events could not be decrypted or read. Retrying."),
                );
            }
            if transcript.more && transcript.events.len() < MAX_EVENTS {
                body = body.child(
                    action(
                        "team-session-older",
                        if viewer.older.is_some() {
                            "Loading…"
                        } else {
                            "Load earlier events"
                        },
                        palette,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        let before = this
                            .team_presence
                            .viewer
                            .transcript
                            .events
                            .first_key_value()
                            .map(|(seq, _)| *seq);
                        if let Some(before) = before {
                            this.read_team_session(Some(before), window, cx);
                        }
                    })),
                );
            }
            if transcript.events.len() >= MAX_EVENTS {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(palette.faint)
                        .child("Showing up to 1,000 events. Reopen the session to return to its latest events."),
                );
            }
            if transcript.events.is_empty() && transcript.note.is_none() {
                body = body.child(div().text_color(palette.faint).child(if viewer.read.is_some() {
                    "Reading encrypted session…"
                } else {
                    "No session events yet."
                }));
            }
            for event in transcript.events.values() {
                if let Some((role, text)) = event_text(event) {
                    body = body.child(
                        v_flex()
                            .gap(px(6.))
                            .p(px(12.))
                            .rounded(px(8.))
                            .bg(palette.raised)
                            .child(div().text_xs().text_color(palette.faint).child(role))
                            .child(div().text_sm().text_color(palette.foreground).child(text)),
                    );
                }
            }
        } else {
            head = head.child(div().flex_1()).child(
                div()
                    .text_sm()
                    .text_color(palette.faint)
                    .child("50 most recent team-visible sessions"),
            );
            if let Some(note) = viewer.list_note {
                body = body.child(div().text_color(palette.faint).child(note));
            }
            if !viewer.loaded {
                body = body.child(div().text_color(palette.faint).child("Loading team sessions…"));
            } else if viewer.sessions.is_empty() {
                body = body.child(div().text_color(palette.faint).child("No team-visible sessions yet."));
            }
            if viewer.person.is_some() {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(palette.faint)
                        .child("Showing this teammate’s sessions. Choose Team sessions to see everyone."),
                );
            }
            for session in &viewer.sessions {
                let thread = session.thread_id.clone();
                let owner = self.team_session_owner(session);
                let agent = session.agent.map(|agent| agent.as_str()).unwrap_or("agent");
                let status = if session.ended() { "ended" } else { "running" };
                body = body.child(
                    h_flex()
                        .id(SharedString::from(format!("team-session-{thread}")))
                        .p(px(12.))
                        .gap(px(12.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.hover))
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.open_team_session(thread.clone(), window, cx)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_color(palette.foreground)
                                .child(format!("{owner}'s session")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(palette.faint)
                                .child(format!("{agent} · {status}")),
                        ),
                );
            }
        }
        v_flex().size_full().child(head).child(body).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::off_note;
    use core::prelude::v1::test;

    #[test]
    fn team_sessions_say_why_they_are_off_in_one_quiet_line() {
        assert_eq!(
            off_note(false, false, true),
            Some("Team sessions are for organization accounts.")
        );
        assert_eq!(
            off_note(true, true, true),
            Some("Team sessions need a newer Nexus Gateway.")
        );
        assert_eq!(
            off_note(true, false, false),
            Some("Your organization keeps team sessions off.")
        );
        assert_eq!(off_note(true, false, true), None);
    }
}
