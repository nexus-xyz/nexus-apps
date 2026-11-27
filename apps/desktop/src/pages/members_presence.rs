//! Settings › Members' live data: who is online, what each person is doing, and whether their sessions can be
//! watched. It reads the sidebar's presence poll (`workspace/team_presence.rs`), so the page opens no second timer.
//!
//! Gateway reports a teammate who hides their status as offline, so the page cannot tell the two apart for others.
//! Only your own row says "Hidden from teammates".

use nexus_desktop_core::presence::{self, Activity, Member};

/// The workspace's presence, as the Members page reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Team {
    /// The poll has answered at least once.
    pub loaded: bool,
    /// Gateway has no presence, or the organization is personal: the page draws without live data.
    pub unavailable: bool,
    /// Your own row from the poll.
    pub own: Option<Member>,
    /// Everyone else.
    pub members: Vec<Member>,
    /// Your "Show when I'm online" preference; `None` while unknown.
    pub show_online: Option<bool>,
    /// The organization's `multiplayerSessions`: members can watch each other's shared sessions.
    pub sessions: bool,
}

/// The dot on a person's avatar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Online and doing something.
    Active,
    /// Online, doing nothing the team can see.
    Idle,
    Offline,
    /// Your own status, hidden from teammates.
    Hidden,
}

/// One row's live data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub status: Status,
    /// The Activity column's text.
    pub text: String,
    /// The person runs agents now.
    pub running: bool,
    /// The row offers Watch: a teammate's running agents, with shared sessions on.
    pub watch: bool,
}

/// The list's filters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Online,
    Running,
}

impl Team {
    /// Whether the page shows live data at all.
    pub fn shown(&self) -> bool {
        !self.unavailable
    }

    /// The row's live data, or `None` before the first poll answers.
    pub fn live(&self, user_id: &str, own: bool) -> Option<Live> {
        if !self.loaded {
            return None;
        }
        if own {
            if self.show_online == Some(false) {
                return Some(Live {
                    status: Status::Hidden,
                    text: "Hidden from teammates".into(),
                    running: false,
                    watch: false,
                });
            }
            return Some(describe(self.own.as_ref(), false));
        }
        let member = self.members.iter().find(|member| member.user_id == user_id);
        Some(describe(member, self.sessions))
    }

    /// How many people are online, you included when teammates can see you.
    pub fn online(&self) -> usize {
        let own = self
            .own
            .as_ref()
            .is_some_and(|own| own.online && self.show_online == Some(true));
        self.members.iter().filter(|member| member.online).count() + usize::from(own)
    }

    /// Whether a row passes the filter.
    pub fn passes(&self, filter: Filter, user_id: &str, own: bool) -> bool {
        match (filter, self.live(user_id, own)) {
            (Filter::All, _) => true,
            (Filter::Online, Some(live)) => matches!(live.status, Status::Active | Status::Idle),
            (Filter::Running, Some(live)) => live.running,
            (_, None) => false,
        }
    }
}

fn describe(member: Option<&Member>, sessions: bool) -> Live {
    let offline = Live {
        status: Status::Offline,
        text: "Offline".into(),
        running: false,
        watch: false,
    };
    let Some(member) = member.filter(|member| member.online) else {
        return offline;
    };
    let running = matches!(member.activity, Some(Activity::Agents { count }) if count > 0);
    match presence::activity_label(member) {
        Some(label) => Live {
            status: Status::Active,
            text: capitalize(&label),
            running,
            watch: running && sessions,
        },
        None => Live {
            status: Status::Idle,
            text: "Idle".into(),
            running: false,
            watch: false,
        },
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, online: bool, activity: Option<Activity>) -> Member {
        Member {
            user_id: id.into(),
            name: Some(id.into()),
            email: None,
            online,
            activity,
        }
    }

    fn team(sessions: bool) -> Team {
        Team {
            loaded: true,
            unavailable: false,
            own: Some(member("me", true, Some(Activity::Chatting))),
            members: vec![
                member("a", true, Some(Activity::Agents { count: 3 })),
                member("b", true, Some(Activity::Chatting)),
                member("c", true, Some(Activity::Idle)),
                member("d", false, Some(Activity::Chatting)),
            ],
            show_online: Some(true),
            sessions,
        }
    }

    #[test]
    fn says_what_each_teammate_is_doing() {
        let team = team(true);
        let running = team.live("a", false).unwrap();
        assert_eq!(running.status, Status::Active);
        assert_eq!(running.text, "Running 3 agents");
        assert!(running.running && running.watch);
        assert_eq!(team.live("b", false).unwrap().text, "Chatting");
        assert_eq!(team.live("c", false).unwrap().status, Status::Idle);
        let offline = team.live("d", false).unwrap();
        assert_eq!((offline.status, offline.text.as_str()), (Status::Offline, "Offline"));
        // A member the poll does not list yet reads as offline.
        assert_eq!(team.live("new", false).unwrap().status, Status::Offline);
    }

    #[test]
    fn watch_needs_shared_sessions_and_is_never_on_your_own_row() {
        assert!(!team(false).live("a", false).unwrap().watch);
        let mut team = team(true);
        team.own = Some(member("me", true, Some(Activity::Agents { count: 1 })));
        let own = team.live("me", true).unwrap();
        assert!(own.running && !own.watch);
    }

    #[test]
    fn your_hidden_status_says_so_and_leaves_the_count() {
        let mut team = team(true);
        assert_eq!(team.online(), 4);
        team.show_online = Some(false);
        let own = team.live("me", true).unwrap();
        assert_eq!(
            (own.status, own.text.as_str()),
            (Status::Hidden, "Hidden from teammates")
        );
        assert_eq!(team.online(), 3);
    }

    #[test]
    fn filters_and_the_first_poll() {
        let team = team(true);
        assert!(team.passes(Filter::Online, "c", false));
        assert!(!team.passes(Filter::Online, "d", false));
        assert!(team.passes(Filter::Running, "a", false));
        assert!(!team.passes(Filter::Running, "b", false));
        let waiting = Team::default();
        assert_eq!(waiting.live("a", false), None);
        assert!(waiting.passes(Filter::All, "a", false));
        assert!(!waiting.passes(Filter::Online, "a", false));
    }
}
