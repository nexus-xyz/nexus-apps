//! Native phase-1 watch-only sessions. The shared workbench owns keys and decryption.
//! Team decryption depends on the team key; its reader contract is unchanged.
#[cfg(test)]
mod lifecycle_tests;
mod model;
mod view;
mod watchers;

use super::{View, Workspace, team_presence::Scope};
use crate::{
    gateway::{Verb, team_sessions::News},
    palette::Palette,
};
use futures::StreamExt;
use gpui_kit::{prelude::*, *};
use model::{Connection, Page, Session, Transcript};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

#[derive(Default)]
pub(super) struct Viewer {
    transcript: Transcript,
    scope: Option<Scope>,
    stream_live: bool,
    retry_cursor: Option<i64>,
    person: Option<String>,
    sessions: Vec<Session>,
    loaded: bool,
    list_note: Option<&'static str>,
    poll: Option<Task<()>>,
    stream: Option<Task<()>>,
    read: Option<Task<()>>,
    older: Option<Task<()>>,
    following: Option<Task<()>>,
    watchers: watchers::Watchers,
}

impl Viewer {
    fn apply_page(&mut self, ticket: &model::Ticket, page: Page, person: &str, older: bool) -> bool {
        let applied = self.transcript.apply(ticket, page, person, older);
        if applied && self.stream_live {
            self.transcript.connection = Connection::Live;
        }
        applied
    }
    /// Suspend this exact read incarnation. Old callbacks cannot clear a replacement's task slots.
    fn suspend(&mut self, scope: &Option<Scope>, incarnation: Uuid) -> bool {
        if self.scope != *scope || self.transcript.incarnation != incarnation {
            return false;
        }
        self.transcript.incarnation = Uuid::new_v4();
        self.poll = None;
        self.stream = None;
        self.read = None;
        self.older = None;
        self.following = None;
        self.stream_live = false;
        self.watchers = Default::default();
        self.transcript.connection = Connection::Reconnecting;
        true
    }
}

#[derive(Deserialize)]
struct List {
    threads: Vec<Session>,
}

fn refused(error: &nexus_client::Error) -> bool {
    matches!(error, nexus_client::Error::Core(core) if matches!(core.status,401|403|404|410))
}
// The shared workbench's content-free string errors retain the HTTP status (`gateway::refusal`).
fn reader_refused(error: &str) -> bool {
    ["(401", "(403", "(404", "(410"]
        .iter()
        .any(|status| error.starts_with(&format!("session sync: Gateway refused the call {status}")))
        || error == "Account changed."
}

impl Workspace {
    fn viewer_active(&self, scope: &Option<Scope>, incarnation: Uuid) -> bool {
        self.team_scope() == *scope
            && scope.is_some()
            && self.team_presence.sessions
            && !self.team_presence.disabled
            && matches!(self.view, View::TeamSessions)
            && self.team_presence.viewer.transcript.incarnation == incarnation
    }

    fn team_viewer_visible(
        &mut self,
        scope: &Option<Scope>,
        incarnation: Uuid,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.viewer_active(scope, incarnation) {
            return false;
        }
        if visible {
            return true;
        }
        self.leave_team_session(cx);
        if self.team_presence.viewer.suspend(scope, incarnation) {
            self.team_presence.refresh_following();
            cx.notify();
        }
        false
    }

    pub(super) fn open_team_sessions(&mut self, person: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.team_scope().is_none() || !self.team_presence.sessions || self.team_presence.disabled {
            return;
        }
        self.leave_team_session(cx);
        self.team_presence.viewer = Viewer {
            person,
            scope: self.team_scope(),
            ..Default::default()
        };
        self.view = View::TeamSessions;
        self.sync_team_sessions(window, cx);
        cx.notify();
    }

    pub(super) fn open_team_session(&mut self, thread: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.team_scope().is_none() || !self.team_presence.sessions || self.team_presence.disabled {
            return;
        }
        self.leave_team_session(cx);
        let scope = self.team_scope();
        let viewer = &mut self.team_presence.viewer;
        viewer.scope = scope;
        viewer.poll = None;
        viewer.stream = None;
        viewer.read = None;
        viewer.older = None;
        viewer.following = None;
        viewer.stream_live = false;
        viewer.retry_cursor = None;
        viewer.watchers = Default::default();
        viewer.transcript.open(thread);
        self.view = View::TeamSessions;
        self.sync_team_sessions(window, cx);
        cx.notify();
    }

    fn leave_team_session(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let viewer = &self.team_presence.viewer;
        if self.team_scope() != viewer.scope {
            return;
        }
        if let Some(thread) = &viewer.transcript.selected {
            // Only announce leave with the same account that opened it. Account resets use server expiry.
            let call = account_gateway.request_detailed(
                Verb::Post,
                format!("/v1/threads/{}/presence", nexus_client::url::enc(thread)),
                Some(json!({"client":viewer.transcript.incarnation.to_string(),"leave":true})),
            );
            cx.spawn(async move |_, _| {
                let _ = call.await;
            })
            .detach();
        }
    }

    pub(super) fn sync_team_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let eligible = self.team_scope().is_some()
            && self.team_presence.sessions
            && !self.team_presence.disabled
            && matches!(self.view, View::TeamSessions);
        if !eligible {
            if self.team_presence.viewer.poll.is_some() {
                self.leave_team_session(cx);
            }
            self.team_presence.viewer = Viewer::default();
            return;
        }
        if !window.is_visible() {
            let scope = self.team_scope();
            let incarnation = self.team_presence.viewer.transcript.incarnation;
            // A hidden render is optional: async wakes call the same guard below.
            if self.team_presence.viewer.poll.is_some()
                || self.team_presence.viewer.stream.is_some()
                || self.team_presence.viewer.read.is_some()
                || self.team_presence.viewer.older.is_some()
                || self.team_presence.viewer.following.is_some()
            {
                self.team_viewer_visible(&scope, incarnation, false, cx);
            }
            return;
        }
        let scope = self.team_scope();
        if self.team_presence.viewer.scope.is_none() {
            self.team_presence.viewer.scope = scope.clone();
        }
        let incarnation = self.team_presence.viewer.transcript.incarnation;
        if self.team_presence.viewer.poll.is_none() {
            self.team_presence.viewer.poll = Some(cx.spawn_in(window, async move |this, cx| {
                let mut tick = 0u64;
                loop {
                    let Ok(Some((list, settings, header, beat))) = this.update_in(cx, |this, window, cx| {
                        if !this.team_viewer_visible(&scope, incarnation, window.is_visible(), cx) { return None; }
                        let account = this.account_gateway()?;
                        let refresh = tick % 5 == 0;
                        tick = tick.wrapping_add(1);
                        if refresh { this.read_team_session(None, window, cx); }
                        let viewer = &this.team_presence.viewer;
                        let person = viewer.person.as_ref().map(|person|format!("&person={}",nexus_client::url::enc(person))).unwrap_or_default();
                        let path = format!("/v1/threads?scope=team&limit=50{person}");
                        let thread = viewer.transcript.selected.as_ref();
                        Some((refresh.then(||account.request_detailed(Verb::Get,path,None)),
                            refresh.then(||account.request_detailed(Verb::Get,"/v1/orgs/settings".into(),None)),
                            thread.filter(|_|refresh).map(|thread|account.request_detailed(Verb::Get,format!("/v1/threads/{}",nexus_client::url::enc(thread)),None)),
                            thread.filter(|_|viewer.transcript.connection != Connection::Unavailable).map(|thread|account.request_detailed(Verb::Post,
                                format!("/v1/threads/{}/presence",nexus_client::url::enc(thread)),
                                Some(json!({"client":incarnation.to_string(),"seq":viewer.transcript.events.last_key_value().map(|(seq,_)|*seq)}))))))
                    }) else { return; };
                    let optional = |request: Option<crate::backend::DetailedRequest>| async move { match request { Some(request)=>Some(request.await),None=>None } };
                    let (list,settings,header,beat)=futures::join!(optional(list),optional(settings),optional(header),optional(beat));
                    if this.update_in(cx,|this,window,cx| {
                        if !this.team_viewer_visible(&scope,incarnation,window.is_visible(),cx) { return; }
                        let stopped = match &settings {
                            Some(Ok(value)) => !super::team_following::sessions_on(value),
                            Some(Err(error)) if refused(error) => true,
                            Some(Err(_)) => false,
                            None => false,
                        };
                        if stopped {
                            this.team_presence.sessions=false;
                            this.team_presence.following.clear();
                            this.team_presence.viewer=Viewer::default();
                            cx.notify(); return;
                        }
                        let self_id=this.account.as_ref().and_then(|account|account.user_id.as_deref()).unwrap_or_default();
                        let viewer=&mut this.team_presence.viewer;
                        viewer.watchers.prune();
                        match list {
                            Some(Ok(value))=>match serde_json::from_value::<List>(value) {
                                Ok(list)=>{viewer.sessions=list.threads.into_iter().filter(|thread|!thread.private && viewer.person.as_ref().is_none_or(|person|person==&thread.person_id)).take(50).collect();viewer.loaded=true;viewer.list_note=None;},
                                Err(_)=>{viewer.sessions.clear();viewer.list_note=Some("The team session list could not be read.");}
                            },
                            Some(Err(error)) if refused(&error)=>{viewer.sessions.clear();viewer.transcript.deny();viewer.stream=None;viewer.watchers=Default::default();viewer.list_note=Some("Team sessions are unavailable to this account.");},
                            Some(Err(_))=>viewer.list_note=Some("Team sessions could not be refreshed. Retrying."),
                            None=>{},
                        }
                        if viewer.transcript.connection == Connection::Unavailable { cx.notify(); return; }
                        if let Some(header)=header {
                            match header {
                                Ok(value)=>match serde_json::from_value::<Session>(value) {
                                    Ok(header) if Some(header.thread_id.as_str())==viewer.transcript.selected.as_deref() && header.visible_to(self_id)=>viewer.transcript.session=Some(header),
                                    _=>{viewer.transcript.deny();viewer.stream=None;viewer.watchers=Default::default();}
                                },
                                Err(error) if refused(&error)=>{viewer.transcript.deny();viewer.stream=None;viewer.watchers=Default::default();},
                                Err(_)=>viewer.transcript.connection=Connection::Reconnecting,
                            }
                        }
                        if beat.as_ref().is_some_and(|answer|answer.as_ref().err().is_some_and(refused)) {
                            viewer.transcript.deny();viewer.stream=None;viewer.watchers=Default::default();
                        }
                        cx.notify();
                    }).is_err() {return;}
                    cx.background_executor().timer(Duration::from_secs(3)).await;
                }
            }));
        }
        self.start_team_stream(window, cx);
    }

    fn start_team_stream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewer = &self.team_presence.viewer;
        if viewer.stream.is_some() || viewer.transcript.connection == Connection::Unavailable {
            return;
        }
        let Some(thread) = viewer.transcript.selected.clone() else {
            return;
        };
        let scope = self.team_scope();
        let incarnation = viewer.transcript.incarnation;
        self.team_presence.viewer.stream = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(Some(Some(mut stream))) = this.update_in(cx, |this, window, cx| {
                    if !this.team_viewer_visible(&scope, incarnation, window.is_visible(), cx) {
                        return None;
                    }
                    let stream = this.account_gateway()?.session_stream(&thread);
                    if stream.is_none() {
                        this.team_presence.viewer.stream = None;
                    }
                    Some(stream)
                }) else {
                    return;
                };
                while let Some(news) = stream.events.next().await {
                    let Ok(keep) = this.update_in(cx, |this, window, cx| {
                        if !this.team_viewer_visible(&scope, incarnation, window.is_visible(), cx) {
                            return false;
                        }
                        let viewer = &mut this.team_presence.viewer;
                        match news {
                            News::Open => {
                                viewer.stream_live = true;
                                viewer.transcript.connection = Connection::Live;
                            }
                            News::Closed => {
                                viewer.stream_live = false;
                                viewer.transcript.connection = Connection::Reconnecting;
                                viewer.watchers = Default::default();
                            }
                            News::Denied => {
                                viewer.transcript.deny();
                                viewer.watchers = Default::default();
                                cx.notify();
                                return false;
                            }
                            News::Event(value) => {
                                if value["threadId"] != thread {
                                    return true;
                                }
                                viewer.stream_live = true;
                                viewer.transcript.connection = Connection::Live;
                                if value.get("from").is_some() {
                                    viewer.watchers.receive(value, incarnation);
                                    cx.notify();
                                    return true;
                                } else if let Some(last) = value["lastSeq"].as_i64() {
                                    if let Some(session) = &mut viewer.transcript.session {
                                        session.last_seq = Some(session.last_seq.map_or(last, |held| held.max(last)));
                                    }
                                } else {
                                    viewer.transcript.deny();
                                    viewer.watchers = Default::default();
                                    cx.notify();
                                    return false;
                                }
                            }
                        }
                        this.read_team_session(None, window, cx);
                        cx.notify();
                        true
                    }) else {
                        return;
                    };
                    if !keep {
                        return;
                    }
                }
                let Ok(keep) = this.update_in(cx, |this, window, cx| {
                    if !this.team_viewer_visible(&scope, incarnation, window.is_visible(), cx) {
                        return false;
                    }
                    this.team_presence.viewer.stream_live = false;
                    this.team_presence.viewer.transcript.connection = Connection::Reconnecting;
                    this.team_presence.viewer.watchers = Default::default();
                    cx.notify();
                    true
                }) else {
                    return;
                };
                if !keep {
                    return;
                }
                cx.background_executor().timer(Duration::from_secs(5)).await;
            }
        }));
    }

    fn read_team_session(&mut self, before: Option<i64>, window: &mut Window, cx: &mut Context<Self>) {
        let viewer = &self.team_presence.viewer;
        if viewer.transcript.connection == Connection::Unavailable
            || (if before.is_some() {
                viewer.older.is_some()
            } else {
                viewer.read.is_some()
            })
        {
            return;
        }
        let Some(ticket) = viewer.transcript.ticket() else {
            return;
        };
        let Some(thread) = viewer.transcript.selected.clone() else {
            return;
        };
        let Some(runner) = &self.runner else {
            self.team_presence.viewer.transcript.note = Some(view::NOT_CONNECTED);
            return;
        };
        // Re-read locked pages: a team key may arrive without changing the event sequence.
        let locked = viewer.transcript.next_locked(viewer.retry_cursor);
        let last = viewer.transcript.events.last_key_value().map(|(seq, _)| *seq);
        let advanced = viewer
            .transcript
            .session
            .as_ref()
            .and_then(|session| session.last_seq)
            .zip(last)
            .is_some_and(|(head, last)| head > last);
        let retry_locked = before.is_none() && locked.is_some() && !advanced;
        let read_before = before.or_else(|| {
            if retry_locked {
                locked
                    .zip(last)
                    .map(|(locked, last)| locked.saturating_add(100).min(last.saturating_add(1)))
            } else {
                None
            }
        });
        let after = if read_before.is_none() { last } else { None };
        let answer = runner.synced_thread_page(thread, read_before, after);
        let scope = self.team_scope();
        let incarnation = viewer.transcript.incarnation;
        if retry_locked {
            self.team_presence.viewer.retry_cursor = read_before;
        }
        let task=cx.spawn_in(window,async move |this,cx| {
            let answer=answer.await.unwrap_or_else(|_|Err("Reader disconnected.".into()));
            let _=this.update_in(cx,|this,window,cx| {
                if !this.team_viewer_visible(&scope,incarnation,window.is_visible(),cx)||!this.team_presence.viewer.transcript.current(&ticket){return;}
                let self_id=this.account.as_ref().and_then(|account|account.user_id.as_deref()).unwrap_or_default();
                let viewer=&mut this.team_presence.viewer;
                if before.is_some(){viewer.older=None;}else{viewer.read=None;}
                let mut catch_up = false;
                match answer {
                    Ok(value)=>match serde_json::from_value::<Page>(value) {
                        Ok(page)=>{let more = page.more; let applied = viewer.apply_page(&ticket,page,self_id,before.is_some());
                            catch_up = applied && after.is_some() && more;},
                        Err(_)=>{viewer.transcript.events.clear();viewer.transcript.note=Some("The session reader returned an unreadable page. Retrying.");}
                    },
                    Err(error) if reader_refused(&error)=>{viewer.transcript.deny();viewer.stream=None;viewer.watchers=Default::default();},
                    Err(_)=>{viewer.transcript.connection=Connection::Reconnecting;viewer.transcript.note=Some("The encrypted session could not be read. Check Settings › Devices and session sync. Retrying.");}
                }
                cx.notify();
                if catch_up { this.read_team_session(None,window,cx); }
            });
        });
        if before.is_some() {
            self.team_presence.viewer.older = Some(task);
        } else {
            self.team_presence.viewer.read = Some(task);
        }
    }

    fn toggle_team_follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let viewer = &self.team_presence.viewer;
        if viewer.following.is_some() || viewer.transcript.connection == Connection::Unavailable {
            return;
        }
        let Some(session) = &viewer.transcript.session else {
            return;
        };
        let Some(self_id) = self.account.as_ref().and_then(|account| account.user_id.as_ref()) else {
            return;
        };
        if session.person_id == *self_id {
            return;
        }
        let following = session.followers.contains(self_id);
        let request = account_gateway.request_detailed(
            if following { Verb::Delete } else { Verb::Put },
            format!("/v1/threads/{}/follow", nexus_client::url::enc(&session.thread_id)),
            None,
        );
        let scope = self.team_scope();
        let incarnation = viewer.transcript.incarnation;
        self.team_presence.viewer.following = Some(cx.spawn_in(window, async move |this, cx| {
            let answer = request.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.team_viewer_visible(&scope, incarnation, window.is_visible(), cx) {
                    return;
                }
                let viewer = &mut this.team_presence.viewer;
                viewer.following = None;
                match answer {
                    Ok(value) => {
                        if let Ok(session) = serde_json::from_value::<Session>(value)
                            && viewer.transcript.selected.as_deref() == Some(session.thread_id.as_str())
                            && !session.private
                        {
                            viewer.transcript.session = Some(session);
                        }
                    }
                    Err(error) if refused(&error) => {
                        viewer.transcript.deny();
                        viewer.stream = None;
                        viewer.watchers = Default::default();
                    }
                    Err(_) => viewer.transcript.note = Some("Following could not be changed. Try again."),
                }
                // Refresh the shared sidebar through its existing poll, without manufacturing a second list.
                this.team_presence.refresh_following();
                cx.notify();
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::reader_refused;
    #[test]
    fn shared_reader_refusals_stop_access_but_transport_failures_retry() {
        for status in [401, 403, 404, 410] {
            assert!(reader_refused(&format!(
                "session sync: Gateway refused the call ({status} unavailable)"
            )));
        }
        assert!(reader_refused("Account changed."));
        assert!(!reader_refused("session sync: Nexus Gateway is unreachable"));
        assert!(!reader_refused(
            "session sync: Gateway refused the call (503 unavailable)"
        ));
    }
}
