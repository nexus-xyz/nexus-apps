//! The app window's last place, opened again at launch, as the Nexus app's last page (`router.tsx`,
//! `startPath` in `main.tsx`).
//!
//! Every place an app window visits is kept as a Nexus UI path, `/app/<org>/<page>` (`last_place` in this build's
//! settings file, `paths.rs`). The first app window of a launch opens it when it is in the organization signed in to
//! (`nexus_desktop_core::last_path::start_page`); otherwise it opens New session, the home. A window opened later
//! starts at the home, and the panel's places are never kept, as in the Nexus app. A sign-in to another Gateway or
//! organization lands the same way, unless the person opened something meanwhile ([`landing`]). Signing out forgets
//! the place: the Nexus app's window goes to the sign-in page then, which is in no organization. A Code session of
//! this computer that is gone opens the home, once the store's first read says so ([`resume`]): the account often
//! arrives before that read, and an unread store is not a session gone. The Nexus app shows "This session isn't
//! here" there, with a button to the home. A session on a server or a cloud machine opens, since its machine may be
//! read only later.
//!
//! The Nexus app keeps its path in its webview's local storage, which no other process reads, so the two apps keep
//! their own (as `cloneParent`).

use super::{Place, SettingsSection, Workspace};
use crate::model::SessionSummary;
use crate::pages::PageId;
use gpui_kit::*;

/// Settings' sections drawn by the workspace itself, by their page.
const SECTIONS: [(SettingsSection, &str); 6] = [
    (SettingsSection::Appearance, "/settings/appearance"),
    (SettingsSection::Notifications, "/settings/notifications"),
    (SettingsSection::ApiKeys, "/settings/api-keys"),
    (SettingsSection::Shortcuts, "/settings/shortcuts"),
    (SettingsSection::Account, "/settings/account"),
    (SettingsSection::LocalProviders, "/settings/local-providers"),
];

/// The page of `place` under its organization: Nexus UI's path where it has one.
pub(super) fn page_of(place: &Place) -> String {
    match place {
        Place::Chat(None) => "/".into(),
        Place::Chat(Some(session)) => format!("/sessions/{}", session.id),
        Place::Code(id) => format!("/code/{id}"),
        Place::NewCode => "/code".into(),
        Place::Library => "/library".into(),
        Place::Inbox => "/notifications".into(),
        Place::Docs(path) => format!("/docs/{}", path.trim_start_matches('/')),
        Place::Issues => "/issues".into(),
        Place::SettingsField(section, anchor) => format!("/settings/{}#{anchor}", section.dest().slug()),
        Place::Settings(SettingsSection::Page(id)) | Place::Page(id) => format!("/{}", id.web_path()),
        Place::Settings(SettingsSection::GatewayLink(dest)) => format!("/settings/{}", dest.slug()),
        Place::Settings(section) => SECTIONS
            .iter()
            .find(|(known, _)| known == section)
            .map_or("/settings/appearance", |(_, page)| page)
            .into(),
        Place::Models => "/models".into(),
        Place::Pull(id) => format!("/code/{id}/pull"),
    }
}

/// The place at `page` ([`page_of`]'s), with the chat's `title`. None for a page this build does not know.
pub(super) fn place_of(page: &str, title: Option<&str>) -> Option<Place> {
    if let Some((section, anchor)) = super::settings_registry::resolve_field(page) {
        return Some(Place::SettingsField(section, anchor));
    }
    if let Some(section) = super::settings_registry::resolve_local_gateway_link(page) {
        let field = page.split_once('#').and_then(|(_, anchor)| match anchor {
            "vault" => Some("vault"),
            "privacy" => Some("privacy"),
            _ => None,
        });
        return Some(match field {
            Some(anchor) => Place::SettingsField(section, anchor),
            None => Place::Settings(section),
        });
    }
    let page = page.split(['?', '#']).next().unwrap_or_default();
    let place = match page {
        "/" => Place::Chat(None),
        "/code" => Place::NewCode,
        "/library" => Place::Library,
        "/notifications" => Place::Inbox,
        "/models" => Place::Models,
        "/issues" => Place::Issues,
        // The docs' home: the trailing `/` goes when the path is read (`split_org_path`).
        "/docs" => Place::Docs(String::new()),
        _ => {
            if let Some(id) = page.strip_prefix("/sessions/").filter(|id| one_segment(id)) {
                Place::Chat(Some(SessionSummary {
                    id: id.to_string(),
                    title: title.unwrap_or("Chat").to_string(),
                    day: None,
                    at_ms: None,
                }))
            } else if let Some(rest) = page.strip_prefix("/code/") {
                match rest.strip_suffix("/pull") {
                    Some(id) if one_segment(id) => Place::Pull(id.to_string()),
                    _ if one_segment(rest) => Place::Code(rest.to_string()),
                    _ => return None,
                }
            } else if let Some(path) = page.strip_prefix("/docs/") {
                Place::Docs(path.to_string())
            } else if let Some((section, _)) = SECTIONS.iter().find(|(_, known)| *known == page) {
                Place::Settings(*section)
            } else if let Some((section, anchor)) = super::settings_registry::resolve_field(page) {
                Place::SettingsField(section, anchor)
            } else if let Some(section) = super::settings_registry::resolve(page) {
                // The shared map's addresses: `/settings/computer`, `/settings/integrations`, `/keys`…
                Place::Settings(section)
            } else {
                let id = PageId::from_web_path(page)?;
                if id.is_place() {
                    Place::Page(id)
                } else {
                    Place::Settings(SettingsSection::Page(id))
                }
            }
        }
    };
    Some(place)
}

fn one_segment(id: &str) -> bool {
    !id.is_empty() && !id.contains('/')
}

/// Where a window goes when an account arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Landing {
    /// The last place in this organization, else New session.
    LastPlace,
    /// New session, with the remembered project.
    NewSession,
}

/// Where a window lands when the account for `org` arrives, `before` being the organization it showed (None after a
/// Gateway switch, a sign-out or an organization switch). It lands only on a sign-in to another Gateway or organization,
/// and only while it shows the empty chat (`idle`), so it never replaces what the person opened meanwhile. A launch's
/// first app window, or a window signed in before, opens the last place (`resume`); a later window starts at the home.
pub(super) fn landing(before: Option<&str>, org: &str, idle: bool, resume: bool) -> Option<Landing> {
    if before == Some(org) || !idle {
        return None;
    }
    Some(if resume {
        Landing::LastPlace
    } else {
        Landing::NewSession
    })
}

/// What the launch does with the last place it read.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Resume {
    Open(Place),
    /// The home: the place is a Code session of this computer the store no longer has.
    Home,
    /// A Code session of this computer, before the store's first read: whether it is gone is not known yet.
    Wait(Place),
}

/// What the launch does with `place`, kept with `machine` (a session on a server or a cloud machine). `loaded`: the
/// store's first read in this organization came back; `known`: the store has this session.
pub(super) fn resume(place: Place, machine: bool, loaded: bool, known: impl Fn(&str) -> bool) -> Resume {
    let id = match &place {
        Place::Code(id) | Place::Pull(id) if !machine => id,
        _ => return Resume::Open(place),
    };
    if known(id) {
        Resume::Open(place)
    } else if loaded {
        Resume::Home
    } else {
        Resume::Wait(place)
    }
}

/// A last place waiting for the store's first read in `scope` (`Workspace::code_scope`).
pub(super) struct Waiting {
    scope: Option<String>,
    place: Place,
}

impl Workspace {
    /// Keep `place` as the next launch's (an app window's, once the organization is known).
    pub(super) fn remember_place(&self, place: &Place) {
        if self.panel {
            return;
        }
        let Some(account) = &self.account else {
            return;
        };
        let (title, machine) = match place {
            Place::Chat(Some(session)) => (Some(session.title.clone()), false),
            Place::Code(id) | Place::Pull(id) => (
                None,
                self.session_by_id(id).is_some_and(|session| session.machine.is_some()),
            ),
            _ => (None, false),
        };
        let place = crate::paths::LastPlace {
            path: nexus_desktop_core::last_path::app_path(&account.org, &page_of(place)),
            title,
            machine,
        };
        #[cfg(test)]
        if let Some(sink) = &self.navigation_place_sink {
            sink.borrow_mut().push(place);
            return;
        }
        crate::paths::save_last_place(&place);
    }

    /// At launch, open the last place when it is in this organization. False when there is none to open: the caller
    /// opens the home.
    pub(super) fn open_last_place(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(org) = self.account.as_ref().map(|account| account.org.clone()) else {
            return false;
        };
        let Some(last) = crate::paths::last_place() else {
            return false;
        };
        let Some(page) = nexus_desktop_core::last_path::start_page(&last.path, &org) else {
            return false;
        };
        let Some(place) = place_of(&page, last.title.as_deref()) else {
            return false;
        };
        self.resume_last_place(place, last.machine, window, cx)
    }

    /// Open `place`, the launch's last place, or wait for the store's first read to know whether its session is gone.
    /// False when there is none to open: the caller opens the home.
    pub(super) fn resume_last_place(
        &mut self,
        place: Place,
        machine: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match resume(place, machine, self.code_overview_loaded, |id| {
            self.session_by_id(id).is_some()
        }) {
            Resume::Open(place) => {
                self.open_place(place, window, cx);
                true
            }
            Resume::Home => false,
            Resume::Wait(place) => {
                self.last_place_waiting = Some(Waiting {
                    scope: self.code_scope.clone(),
                    place,
                });
                true
            }
        }
    }

    /// Once the store's first read is in, the waiting last place opens, or the home when its session is gone. Only
    /// while the window still shows the empty chat and its scope is the same: what the person opened meanwhile stays.
    pub(super) fn settle_last_place(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.last_place_waiting.is_none() || !self.code_overview_loaded {
            return;
        }
        let Some(waiting) = self.last_place_waiting.take() else {
            return;
        };
        let idle = matches!(self.view, super::View::Chat)
            && self.current.is_none()
            && self.composer.read(cx).value().trim().is_empty();
        if waiting.scope != self.code_scope || !idle {
            return;
        }
        cx.defer_in(window, move |this, window, cx| {
            if !this.resume_last_place(waiting.place, false, window, cx) {
                this.start_new_session(window, cx);
            }
            cx.notify();
        });
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn every_place_comes_back_from_its_page() {
        let chat = SessionSummary {
            id: "c_1".into(),
            title: "Fix the login".into(),
            day: None,
            at_ms: None,
        };
        let places = [
            Place::Chat(None),
            Place::Chat(Some(chat)),
            Place::Code("t_1".into()),
            Place::NewCode,
            Place::Library,
            Place::Inbox,
            Place::Docs("guides/quickstart".into()),
            Place::Docs(String::new()),
            Place::Issues,
            Place::SettingsField(SettingsSection::Appearance, "text-size"),
            Place::SettingsField(SettingsSection::Page(PageId::Worktrees), "branch-prefix"),
            Place::Settings(SettingsSection::Appearance),
            Place::Settings(SettingsSection::Notifications),
            Place::Settings(SettingsSection::ApiKeys),
            Place::Settings(SettingsSection::Shortcuts),
            Place::Settings(SettingsSection::Account),
            Place::Settings(SettingsSection::Page(PageId::Devices)),
            Place::Models,
            Place::Page(PageId::Usage),
            Place::Pull("t_1".into()),
        ];
        for place in places {
            let page = page_of(&place);
            assert_eq!(place_of(&page, Some("Fix the login")), Some(place), "{page}");
        }
    }

    #[test]
    fn a_sign_in_to_another_gateway_or_organization_lands_again() {
        // A launch's first account, and the first after a Gateway switch, a sign-out or an organization switch.
        assert_eq!(landing(None, "acme", true, true), Some(Landing::LastPlace));
        // An organization moved to without clearing the account (a pinned organization).
        assert_eq!(landing(Some("solo"), "acme", true, true), Some(Landing::LastPlace));
        // A window opened later, or the panel, starts at the home.
        assert_eq!(landing(None, "acme", true, false), Some(Landing::NewSession));
        // The same organization read again stays where it is.
        assert_eq!(landing(Some("acme"), "acme", true, true), None);
        // Something the person opened meanwhile is never replaced.
        assert_eq!(landing(None, "acme", false, true), None);
    }

    #[test]
    fn a_page_this_build_does_not_draw_opens_the_home() {
        assert_eq!(place_of("/spend/calls/x", None), None);
        assert_eq!(place_of("/code/a/b", None), None);
        assert_eq!(place_of("/sessions/", None), None);
        // A chat kept without its title still opens.
        assert!(matches!(place_of("/sessions/c_9?x=1", None), Some(Place::Chat(Some(chat))) if chat.id == "c_9"));
    }

    #[test]
    fn a_code_session_waits_for_the_stores_first_read() {
        let code = || Place::Code("t_1".into());
        // The account came before the store's read: not known yet is not gone.
        assert_eq!(resume(code(), false, false, |_| false), Resume::Wait(code()));
        assert_eq!(
            resume(Place::Pull("t_1".into()), false, false, |_| false),
            Resume::Wait(Place::Pull("t_1".into()))
        );
        // Read, and there: it opens. Read, and not there: the home.
        assert_eq!(resume(code(), false, true, |id| id == "t_1"), Resume::Open(code()));
        assert_eq!(resume(code(), false, true, |_| false), Resume::Home);
        // Known before the read finished (an earlier read in the same organization): no wait.
        assert_eq!(resume(code(), false, false, |id| id == "t_1"), Resume::Open(code()));
        // A server's or a cloud machine's session, and every other place, open at once.
        assert_eq!(resume(code(), true, false, |_| false), Resume::Open(code()));
        assert_eq!(
            resume(Place::Library, false, false, |_| false),
            Resume::Open(Place::Library)
        );
    }

    fn session(id: &str) -> crate::code::CodeSession {
        crate::code::CodeSession {
            saved_provider: false,
            linked: None,
            created_at: 0,
            acted_at: 0,
            account: None,
            connection: None,
            interrupted: false,
            live: false,
            parent: None,
            background: false,
            background_busy: false,
            background_work: vec![],
            background_total: 0,
            wake: None,
            waits_on: vec![],
            has_worktree: false,
            side_chat: false,
            compact: false,
            orchestration: false,
            at_depth_limit: false,
            agent_started: false,
            pending: None,
            source: None,
            crew: Default::default(),
            start_choices: Vec::new(),
            id: id.into(),
            title: id.into(),
            name: None,
            status: crate::code::Status::Idle,
            activity: nexus_workbench::attention::Activity {
                status: nexus_workbench::event::ThreadStatus::Idle,
                updated_at: 0,
                listed_at: None,
            },
            limited: None,
            updated_at: 0,
            agent: nexus_workbench::event::SessionAgent::Claude,
            model: None,
            mode: nexus_workbench::event::PermissionMode::Default,
            effort: None,
            fast: false,
            via: nexus_workbench::event::Via::Subscription,
            pinned: false,
            branch: None,
            snoozed_until: None,
            settled: false,
            archived: false,
            machine: None,
            project_id: "project".into(),
            cost_usd: 0.0,
            context: None,
            pull_request: None,
            review: false,
        }
    }

    fn fixture(cx: &mut gpui_kit::TestAppContext) -> (Entity<Workspace>, &mut gpui_kit::VisualTestContext) {
        cx.update(gpui_kit::init);
        let (gateway, _gateway_events) = crate::gateway::Gateway::start();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::with_machine_source(
                std::sync::Arc::new(gateway),
                events,
                arrivals,
                false,
                super::super::machine_source::MachineSource::isolated(),
                window,
                cx,
            );
            workspace._code_poll = gpui_kit::Task::ready(());
            workspace
        });
        workspace.update_in(cx, |workspace, _, _| {
            // No account: nothing is written to this computer's settings (`remember_place`).
            workspace.auth = super::super::Auth::SignedIn;
            workspace.view = super::super::View::Chat;
            workspace.code_scope = Some("gateway|acme".into());
            workspace.code_overview_loaded = false;
            workspace.projects.clear();
            workspace.archived_code.clear();
        });
        (workspace, cx)
    }

    fn project_with(id: &str) -> crate::code::CodeProject {
        crate::code::CodeProject {
            id: "project".into(),
            name: "project".into(),
            branch: None,
            pinned: false,
            remotes: Vec::new(),
            sessions: vec![session(id)],
            machine: None,
            added_at: 0,
        }
    }

    fn open_id(workspace: &Workspace) -> Option<String> {
        match &workspace.view {
            super::super::View::Code(view) => Some(view.id.clone()),
            _ => None,
        }
    }

    /// The launch's account came before the Code store's first read: the last session opens once the read is in,
    /// instead of the home (which then also became the next launch's last place).
    #[gpui_kit::test]
    fn the_last_code_session_opens_after_the_stores_first_read(cx: &mut gpui_kit::TestAppContext) {
        let (workspace, cx) = fixture(cx);
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.resume_last_place(Place::Code("t_1".into()), false, window, cx));
            assert_eq!(open_id(workspace), None, "nothing opens before the read");
        });
        cx.run_until_parked();
        workspace.update(cx, |workspace, cx| {
            workspace.projects = vec![project_with("t_1")];
            workspace.code_overview_loaded = true;
            cx.notify();
        });
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert_eq!(open_id(workspace).as_deref(), Some("t_1"));
            assert!(workspace.last_place_waiting.is_none());
        });
    }

    #[gpui_kit::test]
    fn a_waiting_last_place_leaves_what_the_person_opened_meanwhile(cx: &mut gpui_kit::TestAppContext) {
        let (workspace, cx) = fixture(cx);
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.resume_last_place(Place::Code("t_1".into()), false, window, cx));
            // The person opened the Library before the read came back.
            workspace.view = super::super::View::Library;
        });
        workspace.update(cx, |workspace, cx| {
            workspace.projects = vec![project_with("t_1")];
            workspace.code_overview_loaded = true;
            cx.notify();
        });
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert!(matches!(workspace.view, super::super::View::Library));
            assert!(workspace.last_place_waiting.is_none());
        });
    }
}
