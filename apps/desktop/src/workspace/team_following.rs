//! The sidebar's Following: the team-visible sessions you follow, above Team, as Nexus UI's
//! `SidebarFollowing` (`synced/following.tsx`) draws them. It shows while the organization's `multiplayerSessions` is
//! on and you follow one. This build does not open a teammate's titles yet (it needs the team key), so each row
//! names whose session it is, as the web does until a title opens. A row opens the native read-only viewer.
use super::Workspace;
use super::sidebar::{ROW_TEXT, group_head};
use crate::palette::Palette;
use gpui_kit::{
    component::{h_flex, v_flex},
    prelude::*,
    *,
};
use nexus_desktop_core::presence::{self, Member};
use serde::Deserialize;
use serde_json::Value;

/// `GET /v1/threads?scope=following`, as Nexus UI's `followedThreads` asks it.
pub(super) const PATH: &str = "/v1/threads?scope=following&limit=20";

/// One followed session: only what its row shows.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Followed {
    thread_id: String,
    person_id: String,
    agent: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct List {
    threads: Vec<Followed>,
}

/// The followed sessions in a `/v1/threads` answer; None when it is not one.
pub(super) fn parse(value: Value) -> Option<Vec<Followed>> {
    serde_json::from_value::<List>(value).ok().map(|list| list.threads)
}

/// Whether `/v1/orgs/settings` says members watch each other's sessions.
pub(super) fn sessions_on(settings: &Value) -> bool {
    settings.get("multiplayerSessions").and_then(Value::as_bool) == Some(true)
}

/// The agent's name, as `agentName` in `synced/model.ts`: a session agent's label (session sync takes Claude Code,
/// Codex and Nexus), else the id as it came.
fn agent_name(agent: Option<&str>) -> &str {
    let Some(id) = agent else {
        return "—";
    };
    nexus_workbench::event::SessionAgent::parse(id).map_or(id, |agent| nexus_workbench::harness::harness(agent).label())
}

/// A row's two lines before its title opens: whose session, then its agent (`followingLabel`).
fn label(thread: &Followed, owner: &str) -> (String, String) {
    let agent = agent_name(thread.agent.as_deref());
    let doing = if thread.status.as_deref() == Some("running") {
        format!("{agent}, running")
    } else {
        agent.to_string()
    };
    (format!("{owner}'s session"), doing)
}

fn owner<'a>(members: impl Iterator<Item = &'a Member>, person: &str) -> String {
    members.into_iter().find(|member| member.user_id == person).map_or_else(
        || "A teammate".into(),
        |member| presence::display_name(member).to_string(),
    )
}

impl Workspace {
    pub(super) fn render_following(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let team = &self.team_presence;
        if self.team_scope().is_none() || !team.sessions || team.disabled || team.following.is_empty() {
            return None;
        }
        let members = || team.own.iter().chain(team.members.iter());
        Some(
            v_flex()
                .child(group_head("Following", palette))
                .children(team.following.iter().map(|thread| {
                    let (name, doing) = label(thread, &owner(members(), &thread.person_id));
                    let running = thread.status.as_deref() == Some("running");
                    let thread_id = thread.thread_id.clone();
                    h_flex()
                        .id(SharedString::from(format!("following-{}", thread.thread_id)))
                        .group("following-row")
                        .h(px(40.))
                        .pl(px(6.))
                        .pr(px(8.))
                        .text_size(px(ROW_TEXT))
                        .cursor_pointer()
                        .on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.open_team_session(thread_id.clone(), window, cx)
                            }),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w(px(0.))
                                .child(
                                    div()
                                        .line_height(px(18.))
                                        .truncate()
                                        .text_color(if running { palette.foreground } else { palette.faint })
                                        .group_hover("following-row", |style| style.underline())
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .line_height(px(16.))
                                        .text_color(palette.faint)
                                        .truncate()
                                        .child(doing),
                                ),
                        )
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use serde_json::json;

    #[test]
    fn a_followed_row_names_its_owner_and_agent_until_a_title_opens() {
        let threads = parse(json!({"threads":[
            {"threadId":"t1","personId":"ana","agent":"claude","status":"running","lastSeq":3},
            {"threadId":"t2","personId":"gone","agent":null,"status":"completed"}
        ],"nextBefore":null}))
        .unwrap();
        let ana = Member {
            user_id: "ana".into(),
            name: Some("Ada Byron".into()),
            email: None,
            online: true,
            activity: None,
        };
        let members = [ana];
        assert_eq!(
            label(&threads[0], &owner(members.iter(), &threads[0].person_id)),
            ("Ada Byron's session".into(), "Claude Code, running".into())
        );
        assert_eq!(
            label(&threads[1], &owner(members.iter(), &threads[1].person_id)),
            ("A teammate's session".into(), "—".into())
        );
        assert_eq!(parse(json!({"error":"no"})), None);
    }

    #[test]
    fn team_sessions_follow_the_organizations_setting_only_when_it_is_on() {
        assert!(sessions_on(&json!({"multiplayerSessions": true})));
        assert!(!sessions_on(&json!({"multiplayerSessions": false})));
        assert!(!sessions_on(&json!({})));
    }
}
