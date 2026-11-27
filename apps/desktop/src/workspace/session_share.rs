//! Code session sharing uses the full host log and Gateway's existing artifact policy.

use super::Workspace;
use crate::pages::ui;
use crate::palette::Palette;
use crate::workspace::glass::FloatingGround;
use crate::workspace::overlay::OverlaySlot;
use futures::FutureExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::event::StoredEvent;
use nexus_workbench::session_share::{Metadata, snapshot_metadata, trim_note};
use serde_json::{Value, json};

/// This Gateway has no session artifacts: nobody in the app can change that.
const NO_SHARING: &str = "This Gateway can't share sessions yet.";

const AUDIENCES: [(&str, &str); 5] = [
    ("org", "Anyone in the organization"),
    ("teams", "Teams"),
    ("people", "People in the organization"),
    ("external", "People outside, by email"),
    ("public", "Anyone with the link"),
];

pub(super) struct SessionShare {
    id: String,
    scope: Option<String>,
    artifacts: Option<std::sync::Arc<dyn crate::backend::SessionArtifacts>>,
    title: String,
    audience: usize,
    names: Entity<InputState>,
    thinking: bool,
    /// Keep updated: each turn's end shares the session again over the same link.
    follow: bool,
    busy: bool,
    problem: Option<String>,
    shared: Option<Value>,
    note: Option<String>,
    stopped: bool,
    copied: bool,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    _subscription: Subscription,
}

/// A session shared with Keep updated: what each turn's end sends over the share again.
pub(super) struct Followed {
    /// The shared artifact's id: every update must answer with it ([`same_share`]).
    artifact: String,
    audience: &'static str,
    thinking: bool,
    /// An update is on its way. `again`: a turn ended meanwhile, so one more follows it.
    busy: bool,
    again: bool,
}

/// The artifact, and a note when the snapshot was cut to fit.
type Posted = futures::future::LocalBoxFuture<'static, Result<(Value, Option<String>), String>>;

impl Workspace {
    pub(super) fn open_session_share(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(session) = self.session_by_id(id) else { return };
        let title = session.title.clone();
        let names = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe_in(&names, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.share_code_snapshot(cx);
            }
        });
        let focus = cx.focus_handle();
        let return_focus = window.focused(cx);
        window.focus(&focus, cx);
        self.session_share = Some(SessionShare {
            id: id.into(),
            scope: self.code_scope.clone(),
            artifacts: account_gateway.session_artifacts(),
            title,
            audience: 0,
            names,
            thinking: false,
            follow: false,
            busy: false,
            problem: None,
            shared: None,
            note: None,
            stopped: false,
            copied: false,
            focus,
            return_focus,
            _subscription: subscription,
        });
        self.overlay.close_session_menu();
        cx.notify();
    }

    fn close_session_share(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_share.as_ref().is_some_and(|sheet| sheet.busy) {
            return;
        }
        if let Some(sheet) = self.session_share.take()
            && restore
            && let Some(focus) = sheet.return_focus
        {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn share_code_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some(sheet) = &self.session_share else { return };
        if sheet.busy || sheet.shared.is_some() {
            return;
        }
        let id = sheet.id.clone();
        let kind = AUDIENCES[sheet.audience].0;
        let names: Vec<String> = sheet
            .names
            .read(cx)
            .value()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        if kind != "org" && kind != "public" && names.is_empty() {
            if let Some(sheet) = &mut self.session_share {
                sheet.problem = Some("Enter at least one team name or email.".into());
            }
            cx.notify();
            return;
        }
        let (thinking, follow) = (sheet.thinking, sheet.follow);
        let mut fields = json!({"audience": kind, "replace": true});
        if follow {
            fields["follow"] = json!(true);
        }
        if kind == "teams" {
            fields["teams"] = json!(names);
        } else if kind == "people" || kind == "external" {
            fields["people"] = json!(names);
        }
        let request = match self.post_snapshot(&id, thinking, fields, cx) {
            Ok(request) => request,
            Err(problem) => {
                if let Some(sheet) = &mut self.session_share {
                    sheet.problem = Some(problem);
                }
                cx.notify();
                return;
            }
        };
        let scope = self.code_scope.clone();
        if let Some(sheet) = &mut self.session_share {
            sheet.busy = true;
            sheet.problem = None;
        }
        cx.spawn(async move |this, cx| {
            let answer = request.await;
            let _ = this.update(cx, |this, cx| {
                if this.code_scope != scope {
                    return;
                }
                // This share replaced the earlier ones, so only it is followed, and only with Keep updated.
                if let Ok((artifact, _)) = &answer {
                    if follow {
                        let followed = Followed {
                            artifact: artifact["id"].as_str().unwrap_or_default().to_string(),
                            audience: kind,
                            thinking,
                            busy: false,
                            again: false,
                        };
                        this.followed_shares.insert(id.clone(), followed);
                    } else {
                        this.followed_shares.remove(&id);
                    }
                }
                if let Some(sheet) = &mut this.session_share
                    && sheet.id == id
                {
                    sheet.busy = false;
                    match answer {
                        Ok((artifact, note)) => {
                            sheet.shared = Some(artifact);
                            sheet.note = note;
                        }
                        Err(error) => sheet.problem = Some(error),
                    }
                }
                if let Some(account) = this.account_gateway() {
                    account.load_library(None);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Read session `id`'s full log, build its snapshot, and send it with `fields`: the audience with `replace` and
    /// maybe `follow` for a share, or `update` for Keep updated. Err: why it cannot start, for a person.
    fn post_snapshot(&self, id: &str, thinking: bool, fields: Value, cx: &mut Context<Self>) -> Result<Posted, String> {
        let session = self
            .session_by_id(id)
            .ok_or_else(|| "The session is no longer here.".to_string())?;
        let metadata = Metadata {
            id: id.into(),
            title: session.title.clone(),
            agent: session.agent,
            model: session.model.clone(),
            cost_usd: session.cost_usd,
            created_at: session.created_at,
        };
        let events: futures::future::LocalBoxFuture<'static, Result<Vec<StoredEvent>, String>> =
            if let Some(runs) = self.runs(id) {
                let read = runs.call("wb_thread_events", json!({"threadId":id,"afterSeq":0}));
                async move {
                    let value = read
                        .await
                        .map_err(|_| "The session log's answer was lost.".to_string())??;
                    serde_json::from_value(value).map_err(|error| format!("The session log could not be read: {error}"))
                }
                .boxed_local()
            } else if let Some(store) = &self.code {
                let reader = store.reader();
                let read_id = id.to_string();
                let task = cx
                    .background_executor()
                    .spawn(async move { reader.agent_events(&read_id, 0).map(|(events, _)| events) });
                async move { task.await }.boxed_local()
            } else {
                return Err("The session log is unavailable.".into());
            };
        let artifacts = self
            .account_gateway()
            .and_then(|gateway| gateway.session_artifacts())
            .ok_or_else(|| NO_SHARING.to_string())?;
        Ok(async move {
            let log = events.await?;
            let built = snapshot_metadata(&metadata, &log, thinking);
            let (session, images, trimmed) =
                nexus_client::session_share::fit_session_snapshot(&built.session, &built.images)
                    .map_err(|_| "This session is too large to share. Share a shorter session.".to_string())?;
            let mut body = fields;
            body["includeThinking"] = json!(thinking);
            body["session"] = session;
            body["images"] = Value::Array(images);
            let artifact = artifacts.share(body).await?;
            Ok((artifact, trim_note(trimmed)))
        }
        .boxed_local())
    }

    /// Session `id`'s turn ended: when this window shares it with Keep updated, send the new snapshot over that share,
    /// so the guest's link shows it. One update at a time; a turn that ends meanwhile sends one more after it. A failed
    /// update stops following (the share was stopped, replaced or expired, or Gateway was away), and the composer says
    /// so: the guest's page never goes stale without the sharer knowing.
    pub(super) fn update_followed_share(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(followed) = self.followed_shares.get_mut(id) else {
            return;
        };
        if followed.busy {
            followed.again = true;
            return;
        }
        let (audience, thinking, artifact) = (followed.audience, followed.thinking, followed.artifact.clone());
        let request = self.post_snapshot(id, thinking, json!({"audience": audience, "update": true}), cx);
        if let Some(followed) = self.followed_shares.get_mut(id) {
            followed.busy = true;
        }
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let answer = match request {
                Ok(request) => request.await.and_then(|(answer, _)| same_share(&artifact, &answer)),
                Err(problem) => Err(problem),
            };
            let _ = this.update(cx, |this, cx| {
                // Not busy: shared again or stopped meanwhile, and that answer decides, not this one.
                let Some(followed) = this.followed_shares.get_mut(&id).filter(|followed| followed.busy) else {
                    return;
                };
                followed.busy = false;
                match answer {
                    Ok(()) if std::mem::take(&mut followed.again) => this.update_followed_share(&id, cx),
                    Ok(()) => {}
                    Err(error) => {
                        this.followed_shares.remove(&id);
                        this.composer_note = Some(
                            format!("Keep updated stopped for a shared session: {error} Share it again to resume.")
                                .into(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn stop_code_sharing(&mut self, cx: &mut Context<Self>) {
        let Some(sheet) = &mut self.session_share else { return };
        if sheet.busy {
            return;
        }
        let Some(key) = sheet
            .shared
            .as_ref()
            .and_then(|artifact| artifact["sessionId"].as_str())
        else {
            sheet.problem =
                Some("The shared session identifier is missing. Reload the Library to manage this share.".into());
            cx.notify();
            return;
        };
        let Some(artifacts) = &sheet.artifacts else { return };
        let request = artifacts.stop(key);
        let id = sheet.id.clone();
        let scope = self.code_scope.clone();
        sheet.busy = true;
        sheet.problem = None;
        cx.spawn(async move |this, cx| {
            let answer = request.await;
            let _ = this.update(cx, |this, cx| {
                if this.code_scope != scope {
                    return;
                }
                if let Some(sheet) = &mut this.session_share
                    && sheet.id == id
                {
                    sheet.busy = false;
                    match &answer {
                        Ok(_) => sheet.stopped = true,
                        Err(error) => sheet.problem = Some(error.clone()),
                    }
                }
                if answer.is_ok() {
                    this.followed_shares.remove(&id);
                }
                if let Some(account) = this.account_gateway() {
                    account.load_library(None);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_session_share(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sheet = self.session_share.as_ref()?;
        if sheet.scope != self.code_scope {
            return None;
        }
        let busy = sheet.busy;
        let mut content = v_flex()
            .gap(px(10.))
            .text_size(px(13.))
            .text_color(palette.foreground)
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Share as artifact"),
                    )
                    .child(ui::button("code-share-close", "Close", palette).when(!busy, |button| {
                        button.on_click(cx.listener(|this, _, window, cx| this.close_session_share(true, window, cx)))
                    })),
            )
            .child(div().text_color(palette.muted).child(sheet.title.clone()));
        if let Some(artifact) = &sheet.shared {
            content = content.child(if sheet.stopped {
                "No longer shared. No version opens now."
            } else if self.followed_shares.contains_key(&sheet.id) {
                "This session is shared. The link shows each turn after it ends, while this window stays open."
            } else {
                "This session snapshot is shared."
            });
            if !sheet.stopped {
                if let Some(link) = artifact["publicUrl"]
                    .as_str()
                    .or_else(|| artifact["viewerUrl"].as_str())
                    .or_else(|| artifact["url"].as_str())
                {
                    let link = if link.starts_with('/') {
                        format!("{}{link}", self.gateway.site_url())
                    } else {
                        link.into()
                    };
                    let open = link.clone();
                    content = content
                        .child(
                            div()
                                .id("code-session-share-link")
                                .text_color(palette.muted)
                                .child(link.clone())
                                .cursor_pointer()
                                .on_click(move |_, _, cx| crate::external_link::open(&open, cx)),
                        )
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(
                                    ui::button(
                                        "code-share-copy",
                                        if sheet.copied { "Copied" } else { "Copy link" },
                                        palette,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                                            if let Some(sheet) = &mut this.session_share {
                                                sheet.copied = true;
                                            }
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    ui::button(
                                        "code-share-stop",
                                        if busy { "Stopping…" } else { "Stop sharing" },
                                        palette,
                                    )
                                    .when(!busy, |button| {
                                        button.on_click(cx.listener(|this, _, _, cx| this.stop_code_sharing(cx)))
                                    }),
                                ),
                        );
                } else {
                    content = content.child("The snapshot was saved. Open Library to find its link.");
                }
            }
        } else {
            content=content.child(div().text_color(palette.muted).child("Readers see prompts, answers, tools, approvals, images, and cost. Common key formats and home paths are hidden. Other tool-output secrets and image contents remain. Check the audience before sharing. Sharing again replaces the earlier version."))
                .child(div().child("Who can read it"));
            for (index, (_, label)) in AUDIENCES.iter().enumerate() {
                content = content.child(
                    ui::button(("code-share-audience", index), *label, palette)
                        .when(sheet.audience == index, |button| button.bg(palette.selected))
                        .when(!busy, |button| {
                            button.on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(sheet) = &mut this.session_share {
                                    sheet.audience = index;
                                    sheet.problem = None;
                                }
                                cx.notify();
                            }))
                        }),
                );
            }
            if !matches!(sheet.audience, 0 | 4) {
                content = content
                    .child(div().text_color(palette.muted).child(if sheet.audience == 1 {
                        "Team names, separated by commas"
                    } else {
                        "Email addresses, separated by commas"
                    }))
                    .child(Input::new(&sheet.names));
            }
            let caption = if sheet.follow {
                "Readers see each turn after it ends, on the same link, while this window stays open."
            } else {
                "Readers see the session as it is now."
            };
            content = content
                .child(
                    // The page share sheet's setting row: its name and meaning on the left, the switch on the right.
                    h_flex()
                        .gap(px(12.))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w(px(0.))
                                .child("Keep updated")
                                .child(div().text_size(px(12.)).text_color(palette.faint).child(caption)),
                        )
                        .child(
                            ui::switch("code-share-follow", sheet.follow, busy, palette).when(!busy, |switch| {
                                switch.on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(sheet) = &mut this.session_share {
                                        sheet.follow = !sheet.follow;
                                    }
                                    cx.notify();
                                }))
                            }),
                        ),
                )
                .child(
                    ui::button(
                        "code-share-thinking",
                        if sheet.thinking {
                            "Include thinking: On"
                        } else {
                            "Include thinking: Off"
                        },
                        palette,
                    )
                    .when(!busy, |button| {
                        button.on_click(cx.listener(|this, _, _, cx| {
                            if let Some(sheet) = &mut this.session_share {
                                sheet.thinking = !sheet.thinking;
                            }
                            cx.notify();
                        }))
                    }),
                )
                .child(
                    ui::button("code-share-submit", if busy { "Sharing…" } else { "Share" }, palette)
                        .when(!busy, |button| {
                            button.on_click(cx.listener(|this, _, _, cx| this.share_code_snapshot(cx)))
                        }),
                );
        }
        if let Some(note) = &sheet.note {
            content = content.child(div().text_color(palette.muted).child(note.clone()));
        }
        if let Some(error) = &sheet.problem {
            // A Gateway that can't share is no failure to try again: it says so quietly.
            content = content.child(if error.as_str() == NO_SHARING {
                crate::setup::quiet::info_line(
                    "share:unsupported",
                    gpui_kit::assets::IconName::Info,
                    error.clone(),
                    palette,
                    cx,
                )
                .into_any_element()
            } else {
                div()
                    .text_color(palette.destructive)
                    .child(error.clone())
                    .into_any_element()
            });
        }
        Some(
            deferred(
                div()
                    .absolute()
                    .inset_0()
                    .bg(palette.scrim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close_session_share(false, window, cx)),
                    )
                    .child(
                        div()
                            .id("code-session-share-dialog")
                            .track_focus(&sheet.focus)
                            .key_context("CodeSessionShare")
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                                if event.keystroke.key == "escape" {
                                    this.close_session_share(true, window, cx);
                                }
                                if event.keystroke.key == "enter"
                                    && (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
                                {
                                    this.share_code_snapshot(cx);
                                }
                                cx.stop_propagation();
                            }))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .w(px(520.))
                            .max_w(relative(0.94))
                            .max_h(relative(0.90))
                            .flex()
                            .flex_col()
                            .rounded(px(14.))
                            .floating_ground(14., palette.pop, false, &palette)
                            .shadow(palette.pop_shadow(palette.border))
                            // The content scrolls inside the card, so the card's glass stays under it.
                            .child(
                                div()
                                    .id("code-session-share-scroll")
                                    .min_h(px(0.))
                                    .overflow_y_scroll()
                                    .p(px(20.))
                                    .child(content),
                            ),
                    ),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

/// Whether an update's `answer` is the followed share itself. A Gateway older than Keep updated ignores `update` and
/// answers with a new version under a new id, a new link and a share counted against the limits: following stops then.
fn same_share(followed: &str, answer: &Value) -> Result<(), String> {
    if answer["id"].as_str() == Some(followed) {
        Ok(())
    } else {
        Err(
            "Nexus Gateway made a new version instead of updating the link; this Gateway cannot keep a share updated."
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::same_share;
    use serde_json::json;

    #[test]
    fn an_update_must_answer_with_the_followed_share() {
        assert!(same_share("art_1", &json!({"id": "art_1", "version": 1})).is_ok());
        // An older Gateway made the next version: another id.
        assert!(same_share("art_1", &json!({"id": "art_2", "version": 2})).is_err());
        assert!(same_share("art_1", &json!({})).is_err());
        // A share whose answer had no id is never matched by one that has none.
        assert!(same_share("", &json!({})).is_err());
    }
}
