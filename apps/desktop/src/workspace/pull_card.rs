//! A Code session's pull request in the Workbench's Agents tab, as the Nexus app shows it (`workspace-context.tsx`'s
//! `PullRequestSection`, `pane-facts.tsx`'s `PullRequestCard`): its checks, the reviews that ask for changes, a
//! conflict, "Send to agent", the switch that sends failures to the agent on its own, and the agent's own watch with
//! Turn off. The section opens by itself when something needs fixing. A session with several lists them as stacks
//! (`pull_stack.rs`) above the card, and the card shows the one the person picked.
//!
//! The watch (`pr_watch.rs`) reads the pull request and pushes each change; this reads what it last pushed, and asks it
//! to read once when a session first shows. The words are `nexus_pull_requests::card`'s, shared with the Nexus app's
//! `pull-watch.ts`. A session on a person's own server shows here too: this computer's watch reads its pull request
//! (`machines/pull_sessions.rs`). A cloud session shows nothing here.

use std::collections::{HashMap, HashSet};

use futures::StreamExt;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_pull_requests::card;
use nexus_pull_requests::pull_request::PullRequestStatus;
use nexus_pull_requests::watch::PullWatch;
use nexus_pull_requests::{chip, stack};
use nexus_workbench::source_host::Host;

use super::pull_stack::{Density, Pick, pull_list, stack_callout};
use super::{View, Workspace};
use crate::pages::ui;
use crate::palette::Palette;

/// What the section keeps for the window: which session it asked the watch to read, and the person's own fold.
#[derive(Default)]
pub(super) struct PullCard {
    /// The session the watch was last asked to read for; a new session shown asks again.
    read_for: Option<String>,
    /// Why the watch could not read the shown session's pull request.
    read_problem: Option<String>,
    /// The person's fold for each session, and whether its pull request needed fixing then: it follows the pull
    /// request again when that changes, as the Nexus app's section remounts.
    open: HashMap<String, (bool, bool)>,
    /// The pull request the person picked in each session's list; the watch's own until then.
    picked: HashMap<String, String>,
    /// The reviews the person opened whole, by pull request URL and the review's place in it.
    whole_reviews: HashSet<String>,
    busy: bool,
    /// Why the last action failed, and for which session.
    problem: Option<(String, String)>,
    _changes: Option<Task<()>>,
}

impl PullCard {
    /// Draws the window again each time a session's watch changes: the section and the sidebar's dot read it.
    pub(super) fn new(cx: &mut Context<Workspace>) -> PullCard {
        let mut changes = crate::pr_watch::subscribe();
        let task = cx.spawn(async move |this, cx| {
            while changes.next().await.is_some() {
                // A burst of changes draws once.
                while changes.try_recv().is_ok() {}
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        PullCard {
            _changes: Some(task),
            ..PullCard::default()
        }
    }
}

/// The lines a review shows until the person opens it whole, so the fix's row stays in view.
const REVIEW_LINES: usize = 3;

/// Whether a review is long enough to fold: more than about three lines of the sidebar.
fn review_is_long(body: &str) -> bool {
    let body = body.trim();
    body.chars().count() > 200 || body.lines().count() > REVIEW_LINES
}

/// Whether the section starts open: when something needs fixing.
fn starts_open(fix: bool, kept: Option<(bool, bool)>) -> bool {
    match kept {
        Some((was_fix, open)) if was_fix == fix => open,
        _ => fix,
    }
}

impl Workspace {
    /// The session shown, on this computer or on an SSH server, whose pull request the section reads.
    fn pull_session(&self) -> Option<String> {
        let View::Code(view) = &self.view else {
            return None;
        };
        let session = self.session_by_id(&view.id)?;
        self.watches_pull(&session).then(|| session.id.clone())
    }

    /// Whether this computer's watch reads `session`'s pull request: one of its own, or an SSH server's. A cloud
    /// session's runs on its machine.
    pub(super) fn watches_pull(&self, session: &crate::code::CodeSession) -> bool {
        session.machine.is_none() || self.machine_source.ssh_session(&session.id).is_some()
    }

    /// Asks the watch to read the shown session's pull request, once each time another session shows (the Nexus app's
    /// `useReadPull`).
    pub(super) fn read_pull(&mut self, cx: &mut Context<Self>) {
        let shown = self.pull_session();
        if shown == self.pull_card.read_for {
            return;
        }
        self.pull_card.read_for = shown.clone();
        self.pull_card.read_problem = None;
        let Some(id) = shown else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let read = crate::pr_watch::read(id.clone()).await;
            this.update(cx, |this, cx| {
                if this.pull_card.read_for.as_deref() == Some(id.as_str()) {
                    this.pull_card.read_problem = read.err();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Whether the watch is still reading the session's pull request for the first time: the top bar holds its place
    /// then, rather than offering "Open PR" for a branch that may have one.
    pub(super) fn pull_pending(&self, id: &str) -> bool {
        self.pull_card.read_for.as_deref() == Some(id)
            && self.pull_card.read_problem.is_none()
            && crate::pr_watch::known(id).is_none()
    }

    /// Runs one of the section's actions; the watch it returns is drawn through `pr_watch`'s change signal.
    fn pull_action(&mut self, id: String, action: PullAction, cx: &mut Context<Self>) {
        if self.pull_card.busy {
            return;
        }
        self.pull_card.busy = true;
        self.pull_card.problem = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let done = match action {
                PullAction::Send => crate::pr_watch::send(id.clone()).await,
                PullAction::Auto(on) => crate::pr_watch::set_auto(&id, on),
                PullAction::AgentOff => crate::pr_watch::agent_off(&id),
            };
            this.update(cx, |this, cx| {
                this.pull_card.busy = false;
                this.pull_card.problem = done.err().map(|problem| (id, problem));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The "Pull request" section, once the watch has read one.
    pub(super) fn render_pull_section(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.pull_session()?;
        let watch = crate::pr_watch::known(&id)?;
        let shown = watch.status.clone()?;
        let pulls = chip::session_pulls(&watch);
        let pull = self
            .pull_card
            .picked
            .get(&id)
            .and_then(|url| pulls.iter().find(|pull| &pull.url == url))
            .cloned()
            .unwrap_or_else(|| shown.clone());
        let fix = card::needs_fix(Some(&shown));
        let open = starts_open(fix, self.pull_card.open.get(&id).copied());
        let host = pull.host;
        let summary = card::pull_summary(Some(&pull)).unwrap_or_default();
        let title = match stack::count_line(&pulls) {
            Some(count) => count,
            None => host.request_title().to_string(),
        };
        let toggle = {
            let id = id.clone();
            cx.listener(move |this, _, _, cx| {
                this.pull_card.open.insert(id.clone(), (fix, !open));
                cx.notify();
            })
        };
        let head = h_flex()
            .id("pull-section-head")
            .gap(px(8.))
            .cursor_pointer()
            .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_color(p.muted).child(summary))
            .child(ui::icon(
                if open {
                    Lucide::ChevronDown
                } else {
                    Lucide::ChevronRight
                },
                p.icon,
            ))
            .on_click(toggle);
        let mut section = v_flex().id("pull-section").gap(px(8.)).child(head);
        if open {
            if pulls.len() > 1 {
                let this = cx.entity().downgrade();
                let picked_for = id.clone();
                let pick: Pick = std::rc::Rc::new(move |url, _, cx| {
                    this.update(cx, |this, cx| {
                        this.pull_card.picked.insert(picked_for.clone(), url);
                        cx.notify();
                    })
                    .ok();
                });
                section = section.child(pull_list(&pulls, Some(&pull.url), Density::Card, p, Some(pick)).mx(px(-6.)));
            }
            section = section.child(self.render_pull_card(&id, &watch, &pull, &pulls, p, cx));
        }
        Some(section.into_any_element())
    }

    fn render_pull_card(
        &self,
        id: &str,
        watch: &PullWatch,
        pull: &PullRequestStatus,
        pulls: &[PullRequestStatus],
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let host = pull.host;
        let state = if pull.draft {
            "Draft".to_string()
        } else {
            pull.state.to_lowercase()
        };
        let mut body = v_flex().gap(px(6.)).child(
            h_flex()
                .gap(px(6.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(format!("{} · {}", host.reference(pull.number), pull.title)),
                )
                .child(div().flex_none().text_size(px(12.)).text_color(p.muted).child(state)),
        );
        if pulls.len() > 1
            && let Some(into) = stack::into_line(pull, pulls)
        {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .child(format!("Into {into}")),
            );
        }
        if let Some(note) = stack::stack_note(pull, pulls) {
            body = body.child(stack_callout(note, p));
        }
        for (ix, check) in pull.checks.iter().enumerate() {
            let name = div().flex_1().min_w(px(0.)).truncate().child(check.name.clone());
            let name = match check.url.clone() {
                Some(url) => div().flex_1().min_w(px(0.)).child(
                    h_flex()
                        .id(("pull-card-check", ix))
                        .cursor_pointer()
                        .hover(|style| style.text_color(p.strong))
                        .child(name)
                        .on_click(move |_, _, cx| crate::external_link::open(&url, cx)),
                ),
                None => name,
            };
            body = body.child(
                h_flex()
                    .gap(px(8.))
                    .child(crate::pages::pull_request::dot(&check.state, p))
                    .child(name)
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(p.muted)
                            .child(check.state.clone()),
                    ),
            );
        }
        if pull.checks.is_empty() {
            body = body.child(ui::note("No checks reported.", p));
        }
        let asked = pull.reviews.iter().filter(|review| review.state == "CHANGES_REQUESTED");
        for (ix, review) in asked.enumerate() {
            let key = format!("{}#{ix}", pull.url);
            let long = review_is_long(&review.body);
            let whole = self.pull_card.whole_reviews.contains(&key);
            let more = ui::ghost_button(
                ("pull-card-review-more", ix),
                if whole { "Show less" } else { "Show the whole review" },
                p,
            )
            .ml(px(-6.))
            .text_size(px(12.))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.pull_card.whole_reviews.remove(&key) {
                    this.pull_card.whole_reviews.insert(key.clone());
                }
                cx.notify();
            }));
            body = body.child(
                v_flex()
                    .pl(px(10.))
                    .gap(px(2.))
                    .border_l_2()
                    .border_color(p.destructive)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} asked for changes", review.author)),
                    )
                    .when(!review.body.trim().is_empty(), |quote| {
                        quote.child(
                            div()
                                .text_color(p.muted)
                                .when(long && !whole, |text| text.line_clamp(REVIEW_LINES))
                                .child(review.body.clone()),
                        )
                    })
                    .when(long, |quote| quote.child(h_flex().child(more))),
            );
        }
        if pull.mergeable == "CONFLICTING" {
            body = body.child(ui::note("The branch has merge conflicts.", p));
        }
        let checks_problem = self.pull_card.read_problem.clone().or_else(|| watch.problem.clone());
        if let Some(problem) = checks_problem {
            body = body.child(ui::note(format!("Checks unavailable: {problem}"), p));
        }
        let rule = || div().my(px(2.)).h(px(1.)).bg(p.border);
        if let Some(actions) = self.render_pull_actions(id, watch, host, p, cx) {
            body = body.child(rule()).child(actions);
        }
        // Where to go next, quieter than the fix above it: the app's page on the leading edge, the host's on the
        // trailing one.
        let mut links = h_flex().gap(px(8.)).mx(px(-6.));
        if host != Host::GitLab {
            let id = id.to_string();
            let shown = watch.status.as_ref().map(|status| status.url.as_str());
            let page_url = (shown != Some(pull.url.as_str())).then(|| pull.url.clone());
            links = links.child(
                ui::ghost_button("pull-card-page", "Checks, comments and merge →", p).on_click(
                    cx.listener(move |this, _, window, cx| this.open_pull_at(&id, page_url.clone(), window, cx)),
                ),
            );
        }
        let url = pull.url.clone();
        links = links.child(div().flex_1()).child(
            ui::ghost_button("pull-card-open", format!("Open on {} ↗", host.name()), p)
                .on_click(move |_, _, cx| crate::external_link::open(&url, cx)),
        );
        body.child(rule()).child(links)
    }

    /// The agent's own watch, then one row for the fix: the session's switch on the leading edge and "Send to agent"
    /// on the trailing one, as both send the agent the same message.
    fn render_pull_actions(
        &self,
        id: &str,
        watch: &PullWatch,
        host: Host,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let fix = card::needs_fix(watch.status.as_ref());
        if !watch.watching && !fix && !watch.agent_watch {
            return None;
        }
        let busy = self.pull_card.busy;
        let mut actions = v_flex().gap(px(6.));
        if watch.agent_watch {
            let id = id.to_string();
            actions =
                actions.child(
                    h_flex()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .text_color(p.muted)
                                .child(card::agent_watch_note(watch.wakes, host)),
                        )
                        .child(
                            ui::ghost_button("pull-card-agent-off", "Turn off", p).when(!busy, |button| {
                                button.on_click(cx.listener(move |this, _, _, cx| {
                                    this.pull_action(id.clone(), PullAction::AgentOff, cx)
                                }))
                            }),
                        ),
                );
        }
        let mut bar = h_flex().gap(px(8.));
        if watch.watching {
            let id = id.to_string();
            let on = watch.auto_wake;
            bar = bar.child(
                h_flex()
                    .id("pull-card-auto")
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(8.))
                    .when(!busy, |row| {
                        row.cursor_pointer().on_click(
                            cx.listener(move |this, _, _, cx| this.pull_action(id.clone(), PullAction::Auto(!on), cx)),
                        )
                    })
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "When a check fails, a review asks for changes or a conflict appears, the agent gets the \
                             same message on its own.",
                        )
                        .build(window, cx)
                    })
                    .child(ui::switch("pull-card-switch", on, busy, p))
                    .child(
                        h_flex()
                            .min_w(px(0.))
                            .flex_wrap()
                            .gap_x(px(6.))
                            .child("Send on its own")
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(p.muted)
                                    .child(card::wakes_left(watch.wakes)),
                            ),
                    ),
            );
        } else {
            bar = bar.child(div().flex_1());
        }
        if fix {
            let id = id.to_string();
            let send = if busy {
                ui::disabled_button("pull-card-send", "Send to agent", p)
            } else {
                ui::primary_button("pull-card-send", "Send to agent", p)
                    .on_click(cx.listener(move |this, _, _, cx| this.pull_action(id.clone(), PullAction::Send, cx)))
            };
            bar = bar.child(send);
        }
        if watch.watching || fix {
            actions = actions.child(bar);
        }
        if let Some((_, problem)) = self.pull_card.problem.as_ref().filter(|(at, _)| at == id) {
            actions = actions.child(ui::error_line(problem.clone(), p));
        }
        Some(actions)
    }
}

#[derive(Clone, Copy)]
enum PullAction {
    Send,
    Auto(bool),
    AgentOff,
}

#[cfg(test)]
mod tests {
    use super::{review_is_long, starts_open};

    #[test]
    fn a_review_folds_only_when_it_runs_past_three_lines() {
        assert!(!review_is_long("Use side-turn-3."));
        assert!(!review_is_long("One.\nTwo.\nThree."));
        assert!(review_is_long("One.\nTwo.\nThree.\nFour."));
        assert!(review_is_long(&"word ".repeat(60)));
    }

    #[test]
    fn the_section_opens_when_something_needs_fixing_and_keeps_the_persons_fold_until_that_changes() {
        assert!(starts_open(true, None));
        assert!(!starts_open(false, None));
        assert!(!starts_open(true, Some((true, false))));
        assert!(starts_open(false, Some((false, true))));
        // The pull request now needs fixing: it opens again, whatever the person folded before.
        assert!(starts_open(true, Some((false, false))));
        assert!(!starts_open(false, Some((true, true))));
    }
}
