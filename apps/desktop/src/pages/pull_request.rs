//! The pull request page, as the Nexus app's `workbench/pull-page.tsx`: a Code session's pull request,
//! full canvas, opened from the session's topbar link or a pull request notification. Back or Esc returns to the
//! session. It shows the merge, the checks with links to their logs, the reviews, and the review threads with a reply
//! box and Resolve or Reopen. A merge asks first, and the core refuses it if the head moved since. A session with several
//! pull requests has a strip under the head that goes to each, its stacks bottom first in the order they merge, and
//! the merge card says when one under it must merge first. A pull request in a stack has a stack card under the merge
//! card that rebases the stack or merges it (`stack_card`), asking first as the merge does.
//!
//! The reads and writes go through `gh` (`crate::pull_page`), off the UI thread. The page reads again after each write,
//! on Refresh, and when the pull request watch reads the pull request. GitHub only: a GitLab session's page says so.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_pull_requests::badge::Badge;
use nexus_pull_requests::pull_page::{ErrorKind, MergeAction, PullError, PullPage, ReviewThread, Session};
use nexus_pull_requests::pull_request::PullRequestStatus;

use super::{palette, ui};
use crate::github_cli::Block;
use crate::palette::Palette;
use crate::pull_page as model;
use crate::setup::{FixKey, Fixer};

mod gh_problem;
mod stack_card;

/// How often the page looks whether the watch read the pull request again.
const WATCH_POLL: Duration = Duration::from_secs(2);
/// The page's column, as the Nexus app's (`.wb-pr-body`).
const COLUMN: f32 = 820.;

/// Where the page reads and writes, asked again at each read: the app may take the store over meanwhile.
pub type Locate = Rc<dyn Fn(&mut App) -> Result<Session, PullError>>;

/// Opens the page of another of the session's pull requests, by its address.
pub type OpenOther = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// What Esc did on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escape {
    /// It closed the merge's confirmation.
    Handled,
    /// It leaves the page for the session.
    Back,
    /// A reply box has it.
    Pass,
}

/// A thread's reply box and its write.
struct ThreadState {
    reply: Entity<TextareaState>,
    _enter: Subscription,
}

pub struct PullRequestPage {
    thread_id: String,
    locate: Locate,
    on_back: Rc<dyn Fn(&mut Window, &mut App)>,
    open_other: OpenOther,
    page: Option<PullPage>,
    problem: Option<PullError>,
    /// The read's failure, when setup fixes it: the GitHub CLI is missing or signed out (`gh_problem.rs`).
    block: Option<Block>,
    /// The fix the page reads again after, once registered (`setup::on_done`).
    armed: Option<FixKey>,
    /// Runs a setup line's fix in the page's window.
    fixer: Fixer,
    loading: bool,
    /// The watch's last read the page followed.
    seen_read: Option<i64>,
    /// The merge method the person picked; the core's choice until then.
    method: Option<String>,
    confirming: Option<MergeAction>,
    merge_busy: bool,
    merge_problem: Option<PullError>,
    /// The stack card's state: its confirmation, its write and what came of it.
    stack: stack_card::StackCard,
    threads: HashMap<String, ThreadState>,
    thread_busy: HashSet<String>,
    thread_problems: HashMap<String, PullError>,
    /// Threads the person folded or unfolded: open by default unless resolved.
    folded: HashMap<String, bool>,
    /// Threads whose reply went: their box empties at the next draw, which has the window.
    sent: HashSet<String>,
    _watch: Task<()>,
}

impl PullRequestPage {
    pub fn new(
        thread_id: String,
        locate: Locate,
        on_back: Rc<dyn Fn(&mut Window, &mut App)>,
        open_other: OpenOther,
        fixer: Fixer,
        cx: &mut Context<Self>,
    ) -> Self {
        let watch = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_POLL).await;
                let alive = this.update(cx, |page, cx| {
                    let read = model::watch_read(&page.thread_id);
                    if read != page.seen_read {
                        page.seen_read = read;
                        page.load(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let page = PullRequestPage {
            seen_read: model::watch_read(&thread_id),
            thread_id,
            locate,
            on_back,
            open_other,
            page: None,
            problem: None,
            block: None,
            armed: None,
            fixer,
            loading: false,
            method: None,
            confirming: None,
            merge_busy: false,
            merge_problem: None,
            stack: Default::default(),
            threads: HashMap::new(),
            thread_busy: HashSet::new(),
            thread_problems: HashMap::new(),
            folded: HashMap::new(),
            sent: HashSet::new(),
            _watch: watch,
        };
        // The first read waits for the workspace that opens the page to finish its update: `locate` reads it.
        cx.spawn(async move |this, cx| this.update(cx, |page, cx| page.load(cx)).ok())
            .detach();
        page
    }

    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    /// Esc: cancel the merge's or the stack's confirmation, else leave the page ([`Escape::Back`], which the workspace does) unless
    /// a reply box has the focus.
    pub fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Escape {
        if self.confirming.take().is_some() || self.stack.cancel() {
            cx.notify();
            return Escape::Handled;
        }
        let typing = self
            .threads
            .values()
            .any(|thread| thread.reply.focus_handle(cx).is_focused(window));
        if typing { Escape::Pass } else { Escape::Back }
    }

    /// Reads the page again. A failed read keeps what was shown, under the failure.
    fn load(&mut self, cx: &mut Context<Self>) {
        let session = (self.locate)(cx);
        self.loading = true;
        cx.notify();
        // What setup fixes is found here too: a missing `gh` looks for the computer's package manager.
        let task = cx.background_executor().spawn(async move {
            session.and_then(|session| model::read(&session)).map_err(|problem| {
                let block = Block::of_pull(&problem);
                (problem, block)
            })
        });
        cx.spawn(async move |this, cx| {
            let read = task.await;
            this.update(cx, |page, cx| {
                page.loading = false;
                match read {
                    Ok(read) => {
                        page.page = Some(read);
                        page.problem = None;
                        page.block = None;
                    }
                    Err((problem, block)) => page.read_failed(problem, block, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs a write off the UI thread, then reads the page again. A failure shows by the control that made it; one
    /// that leaves the confirmed state out of date reads the page again too.
    fn write<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(Session) -> Result<T, PullError> + Send + 'static,
        done: impl FnOnce(&mut Self, Result<T, PullError>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let session = (self.locate)(cx);
        let task = cx.background_executor().spawn(async move { session.and_then(work) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |page, cx| {
                let reread = match &result {
                    Ok(_) => true,
                    Err(problem) => model::rereads_after(problem.kind),
                };
                // A sign-in that lapsed: the write's setup line signs in, then the page reads again.
                if matches!(&result, Err(problem) if problem.kind == ErrorKind::SignedOut) {
                    page.arm(&Block::SignedOut, cx);
                }
                done(page, result, cx);
                if reread {
                    page.load(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The method the merge uses: the person's pick while the repository allows it, else the core's choice.
    fn chosen_method(&self, page: &PullPage) -> Option<String> {
        self.method
            .clone()
            .filter(|method| page.details.methods.contains(method))
            .or_else(|| page.merge.method.clone())
    }

    fn merge(&mut self, action: MergeAction, cx: &mut Context<Self>) {
        let Some(page) = &self.page else { return };
        let Some(method) = self.chosen_method(page) else { return };
        let (head, thread_id) = (page.details.head.clone(), self.thread_id.clone());
        self.merge_busy = true;
        self.merge_problem = None;
        self.write(
            move |session| model::merge(&session, &thread_id, &head, &method, action),
            |page, result, _| {
                page.merge_busy = false;
                page.confirming = None;
                page.merge_problem = result.err();
            },
            cx,
        );
    }

    fn auto_off(&mut self, cx: &mut Context<Self>) {
        let thread_id = self.thread_id.clone();
        self.merge_busy = true;
        self.merge_problem = None;
        self.write(
            move |session| model::auto_off(&session, &thread_id),
            |page, result, _| {
                page.merge_busy = false;
                page.merge_problem = result.err();
            },
            cx,
        );
    }

    fn reply_box(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextareaState> {
        if let Some(thread) = self.threads.get(id) {
            return thread.reply.clone();
        }
        let reply = cx.new(|cx| TextareaState::new(window, cx).placeholder("Reply…").rows(2));
        let review = id.to_string();
        // Cmd+Enter (Ctrl+Enter) sends, as in the Nexus app.
        let enter = cx.subscribe_in(&reply, window, move |page, _, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = event {
                page.reply(&review, cx);
            }
        });
        self.threads.insert(
            id.to_string(),
            ThreadState {
                reply: reply.clone(),
                _enter: enter,
            },
        );
        reply
    }

    fn reply(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(state) = self.threads.get(id) else { return };
        let body = state.reply.read(cx).value().to_string();
        if body.trim().is_empty() || self.thread_busy.contains(id) {
            return;
        }
        let review = id.to_string();
        self.thread_busy.insert(review.clone());
        self.thread_problems.remove(&review);
        self.write(
            {
                let review = review.clone();
                move |session| model::reply(&session, &review, &body)
            },
            move |page, result, cx| {
                page.thread_busy.remove(&review);
                match result {
                    Ok(()) => {
                        page.sent.insert(review);
                    }
                    Err(problem) => {
                        page.thread_problems.insert(review, problem);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    fn resolve(&mut self, id: &str, resolved: bool, cx: &mut Context<Self>) {
        let review = id.to_string();
        self.thread_busy.insert(review.clone());
        self.thread_problems.remove(&review);
        self.write(
            {
                let review = review.clone();
                move |session| model::resolve(&session, &review, resolved)
            },
            move |page, result, _| {
                page.thread_busy.remove(&review);
                if let Err(problem) = result {
                    page.thread_problems.insert(review, problem);
                }
            },
            cx,
        );
    }

    fn render_head(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let pull = self.page.as_ref().map(|page| &page.status);
        h_flex()
            .flex_none()
            .w_full()
            .gap(px(8.))
            .px(px(16.))
            .py(px(10.))
            .border_b_1()
            .border_color(palette.border)
            .child(
                h_flex()
                    .id("pull-back")
                    .flex_none()
                    .h(px(28.))
                    .pl(px(8.))
                    .pr(px(10.))
                    .gap(px(6.))
                    .rounded(px(8.))
                    .bg(palette.hover)
                    .text_size(px(12.))
                    .text_color(palette.foreground)
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.selected))
                    .child(ui::icon(Lucide::ArrowLeft, palette.foreground))
                    .child("Back")
                    .child(
                        div()
                            .px(px(5.))
                            .rounded(px(4.))
                            .bg(palette.selected)
                            .text_size(px(11.))
                            .text_color(palette.muted)
                            .child("Esc"),
                    )
                    .on_click(cx.listener(|page, _, window, cx| (page.on_back)(window, cx))),
            )
            .children(pull.map(|pull| {
                let state = if pull.draft {
                    "Draft".to_string()
                } else {
                    pull.state.to_lowercase()
                };
                h_flex()
                    .min_w(px(0.))
                    .gap(px(6.))
                    .text_size(px(13.))
                    .child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.strong)
                            .child(format!("#{} · {}", pull.number, pull.title)),
                    )
                    .child(div().flex_none().text_color(palette.muted).child(state))
            }))
            .child(div().flex_1())
            .child(
                ui::button(
                    "pull-refresh",
                    if self.loading { "Reading…" } else { "Refresh" },
                    palette,
                )
                .on_click(cx.listener(|page, _, _, cx| page.load(cx))),
            )
            .children(pull.map(|pull| {
                let url = pull.url.clone();
                ui::button("pull-open", "Open on GitHub ↗", palette)
                    .on_click(move |_, _, cx| crate::external_link::open(&url, cx))
            }))
    }

    fn render_merge(&self, page: &PullPage, palette: Palette, cx: &mut Context<Self>) -> Div {
        let action = page.merge.action;
        let chosen = self.chosen_method(page);
        let busy = self.merge_busy;
        let open = page.status.state == "OPEN";
        let mut card = card("Merge", "", palette);
        if let Some(note) = model::merge_note(page) {
            card = card.child(ui::note(note, palette));
        }
        let pulls = self.session_pulls();
        if let Some(note) = nexus_pull_requests::stack::stack_note(&page.status, &pulls) {
            let warn = crate::workspace::pull_color(Badge::Changes, palette);
            card = card.child(
                h_flex()
                    .items_start()
                    .gap(px(8.))
                    .p(px(8.))
                    .rounded(px(8.))
                    .bg(warn.opacity(0.1))
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child(ui::icon(Lucide::Layers, warn).mt(px(2.)))
                    .child(div().flex_1().min_w(px(0.)).child(note)),
            );
        }
        if !page.merge.blocked.is_empty() {
            card = card.child(v_flex().gap(px(2.)).children(page.merge.blocked.iter().map(|reason| {
                h_flex()
                    .gap(px(6.))
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .child("•")
                    .child(reason.clone())
            })));
        }
        if open && !page.details.in_queue && page.details.auto_merge.is_none() {
            let label = model::merge_label(action.unwrap_or(MergeAction::Merge), chosen.as_deref().unwrap_or(""));
            let can = action.is_some() && chosen.is_some() && !busy && self.confirming.is_none();
            let button = if can {
                ui::primary_button("pull-merge", label, palette).on_click(cx.listener(move |page, _, _, cx| {
                    page.confirming = action;
                    page.merge_problem = None;
                    cx.notify();
                }))
            } else {
                let tip: SharedString = page.merge.blocked.join(" ").into();
                ui::disabled_button("pull-merge", label, palette).when(!tip.is_empty(), |button| {
                    button.tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                })
            };
            let methods = (page.details.methods.len() > 1).then(|| {
                h_flex()
                    .gap(px(6.))
                    .children(page.details.methods.iter().enumerate().map(|(ix, method)| {
                        let picked = chosen.as_deref() == Some(method.as_str());
                        let method = method.clone();
                        ui::chip(
                            ("pull-method", ix),
                            model::method_label(&method).to_string(),
                            picked,
                            palette,
                        )
                        .when(!busy && self.confirming.is_none(), |chip| {
                            chip.on_click(cx.listener(move |page, _, _, cx| {
                                page.method = Some(method.clone());
                                cx.notify();
                            }))
                        })
                    }))
            });
            card = card.child(h_flex().mt(px(4.)).gap(px(8.)).children(methods).child(button));
        }
        if page.details.auto_merge.is_some() && open {
            let off = if busy {
                ui::disabled_button("pull-auto-off", "Turn off auto-merge", palette)
            } else {
                ui::button("pull-auto-off", "Turn off auto-merge", palette)
                    .on_click(cx.listener(|page, _, _, cx| page.auto_off(cx)))
            };
            card = card.child(h_flex().mt(px(4.)).child(off));
        }
        if let (Some(confirming), Some(method)) = (self.confirming, chosen) {
            let confirm = if busy {
                ui::disabled_button("pull-confirm", model::merge_label(confirming, &method), palette)
            } else {
                ui::primary_button("pull-confirm", model::merge_label(confirming, &method), palette)
                    .on_click(cx.listener(move |page, _, _, cx| page.merge(confirming, cx)))
            };
            let cancel = ui::button("pull-cancel", "Cancel", palette).on_click(cx.listener(|page, _, _, cx| {
                page.confirming = None;
                cx.notify();
            }));
            card = card.child(confirm_box(
                model::confirm_text(page, confirming, &method),
                confirm,
                cancel,
                palette,
            ));
        }
        card.children(
            self.merge_problem
                .as_ref()
                .map(|problem| self.render_problem(problem, palette, cx)),
        )
    }

    /// The session's pull requests as the watch last read them, with this page's own read in place of its entry.
    fn session_pulls(&self) -> Vec<PullRequestStatus> {
        let mut pulls = crate::pr_watch::known(&self.thread_id)
            .map(|watch| nexus_pull_requests::chip::session_pulls(&watch))
            .unwrap_or_default();
        if let Some(page) = &self.page {
            match pulls.iter_mut().find(|pull| pull.url == page.status.url) {
                Some(pull) => *pull = page.status.clone(),
                None => pulls.push(page.status.clone()),
            }
        }
        pulls
    }

    /// The strip under the head of a session with several: each stack bottom first, in the order it merges, the oldest
    /// on the left, this page's pull request on a fill. A click opens that one's page. None for one.
    fn render_others(&self, palette: Palette) -> Option<Stateful<Div>> {
        let here = self.page.as_ref()?.status.url.clone();
        let pulls = self.session_pulls();
        if pulls.len() < 2 {
            return None;
        }
        let groups = nexus_pull_requests::stack::groups(&pulls);
        let mut strip = h_flex()
            .id("pull-others")
            .flex_none()
            .w_full()
            .gap(px(4.))
            .px(px(16.))
            .py(px(6.))
            .border_b_1()
            .border_color(palette.border)
            .overflow_x_scroll()
            .text_size(px(12.))
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(6.))
                    .pr(px(6.))
                    .text_color(palette.faint)
                    .child(ui::icon(Lucide::Layers, palette.faint))
                    .children(nexus_pull_requests::stack::count_line(&pulls)),
            );
        let mut ix = 0usize;
        for (at, group) in groups.iter().rev().enumerate() {
            if at > 0 {
                strip = strip.child(div().flex_none().mx(px(6.)).w(px(1.)).h(px(16.)).bg(palette.border));
            }
            for (step, row) in group.rows.iter().rev().enumerate() {
                if step > 0 {
                    strip = strip.child(ui::icon(Lucide::ChevronRight, palette.faint));
                }
                let pull = row.pull;
                let badge = Badge::of(pull);
                let color = crate::workspace::pull_color(badge, palette);
                let current = pull.url == here;
                let url = pull.url.clone();
                let open = self.open_other.clone();
                let tip: SharedString = pull.title.clone().into();
                strip = strip.child(
                    h_flex()
                        .id(("pull-other", ix))
                        .flex_none()
                        .h(px(24.))
                        .px(px(8.))
                        .gap(px(6.))
                        .rounded(px(6.))
                        .text_color(if current { palette.strong } else { palette.muted })
                        .when(current, |pill| {
                            pill.bg(palette.selected).font_weight(FontWeight::MEDIUM)
                        })
                        .when(!current, |pill| {
                            pill.cursor_pointer()
                                .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                                .active(|style| style.opacity(0.72))
                                .on_click(move |_, window, cx| open(url.clone(), window, cx))
                        })
                        .child(div().size(px(7.)).rounded_full().bg(color))
                        .child(pull.host.reference(pull.number))
                        .child(
                            div()
                                .text_color(palette.faint)
                                .child(nexus_pull_requests::chip::word(pull).0),
                        )
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                        }),
                );
                ix += 1;
            }
        }
        Some(strip)
    }

    fn render_checks(&self, page: &PullPage, palette: Palette) -> Div {
        let checks = &page.status.checks;
        let mut card = card("Checks", model::checks_summary(page), palette);
        if checks.is_empty() {
            return card.child(ui::note("No checks reported.", palette));
        }
        for (ix, check) in checks.iter().enumerate() {
            let required = page.details.required.contains(&check.name);
            let name = div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(13.))
                .text_color(palette.foreground)
                .child(check.name.clone());
            let name = match check.url.clone() {
                Some(url) => div().flex_1().min_w(px(0.)).child(
                    h_flex()
                        .id(("pull-check", ix))
                        .gap(px(4.))
                        .cursor_pointer()
                        .hover(|style| style.text_color(palette.strong))
                        .child(name)
                        .child(ui::icon(Lucide::ExternalLink, palette.icon))
                        .on_click(move |_, _, cx| crate::external_link::open(&url, cx)),
                ),
                None => name,
            };
            card = card.child(
                h_flex()
                    .gap(px(10.))
                    .py(px(4.))
                    .child(dot(&check.state, palette))
                    .child(name)
                    .when(required, |row| row.child(ui::badge("Required", palette)))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(palette.muted)
                            .child(check.state.clone()),
                    ),
            );
        }
        card
    }

    fn render_reviews(&self, page: &PullPage, palette: Palette) -> Option<Div> {
        let reviews: Vec<_> = page
            .status
            .reviews
            .iter()
            .filter(|review| review.state != "PENDING")
            .collect();
        if reviews.is_empty() {
            return None;
        }
        Some(card("Reviews", "", palette).children(reviews.into_iter().map(|review| {
            v_flex()
                .pl(px(10.))
                .py(px(2.))
                .gap(px(2.))
                .border_l_2()
                .border_color(if review.state == "CHANGES_REQUESTED" {
                    palette.destructive
                } else {
                    palette.border
                })
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.strong)
                        .child(format!("{} {}", review.author, model::review_word(&review.state))),
                )
                .when(!review.body.is_empty(), |quote| {
                    quote.child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.foreground)
                            .child(review.body.clone()),
                    )
                })
        })))
    }

    fn render_threads(
        &mut self,
        page: &PullPage,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut card = card("Review comments", model::threads_summary(page), palette);
        if page.details.threads.is_empty() {
            return card.child(ui::note("No review comments.", palette));
        }
        for (ix, thread) in model::sort_threads(&page.details.threads).into_iter().enumerate() {
            card = card.child(self.render_thread(ix, thread, palette, window, cx));
        }
        card
    }

    fn render_thread(
        &mut self,
        ix: usize,
        thread: &ReviewThread,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.folded.get(&thread.id).copied().unwrap_or(!thread.resolved);
        let busy = self.thread_busy.contains(&thread.id);
        let id = thread.id.clone();
        let summary = h_flex()
            .id(("pull-thread", ix))
            .gap(px(8.))
            .text_size(px(12.))
            .cursor_pointer()
            .child(ui::icon(
                if open {
                    Lucide::ChevronDown
                } else {
                    Lucide::ChevronRight
                },
                palette.icon,
            ))
            .child(
                div()
                    .font_family("monospace")
                    .text_color(palette.foreground)
                    .child(model::thread_place(thread)),
            )
            .when(thread.outdated, |row| row.child(ui::badge("Outdated", palette)))
            .when(thread.resolved, |row| row.child(ui::badge("Resolved", palette)))
            .on_click(cx.listener(move |page, _, _, cx| {
                page.folded.insert(id.clone(), !open);
                cx.notify();
            }));
        let mut body = v_flex()
            .mt(px(8.))
            .p(px(10.))
            .px(px(12.))
            .rounded(px(10.))
            .border_1()
            .border_color(palette.border)
            .when(thread.resolved, |body| body.opacity(0.75))
            .child(summary);
        if !open {
            return body;
        }
        for (cx_ix, comment) in thread.comments.iter().enumerate() {
            let link = comment.url.clone().map(|url| {
                div()
                    .id(("pull-comment-link", ix * 1000 + cx_ix))
                    .cursor_pointer()
                    .text_color(palette.muted)
                    .hover(|style| style.text_color(palette.strong))
                    .child("↗")
                    .on_click(move |_, _, cx| crate::external_link::open(&url, cx))
            });
            body = body.child(
                v_flex()
                    .mt(px(8.))
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .text_size(px(12.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(palette.strong)
                                    .child(comment.author.clone()),
                            )
                            .children(
                                comment
                                    .created_at
                                    .as_deref()
                                    .map(|at| div().text_color(palette.faint).child(model::comment_time(at))),
                            )
                            .children(link),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.foreground)
                            .child(comment.body.clone()),
                    ),
            );
        }
        if thread.can_reply {
            let reply = self.reply_box(&thread.id, window, cx);
            if self.sent.remove(&thread.id) {
                reply.update(cx, |state, cx| state.set_value("", window, cx));
            }
            let empty = reply.read(cx).value().trim().is_empty();
            let (review, resolved) = (thread.id.clone(), thread.resolved);
            let toggle = if resolved {
                thread.can_unresolve.then_some("Reopen")
            } else {
                thread.can_resolve.then_some("Resolve")
            }
            .map(|label| {
                if busy {
                    ui::disabled_button(("pull-resolve", ix), label, palette)
                } else {
                    let review = review.clone();
                    ui::button(("pull-resolve", ix), label, palette)
                        .on_click(cx.listener(move |page, _, _, cx| page.resolve(&review, !resolved, cx)))
                }
            });
            let send = if busy || empty {
                ui::disabled_button(("pull-reply", ix), "Reply", palette)
            } else {
                ui::primary_button(("pull-reply", ix), "Reply", palette)
                    .on_click(cx.listener(move |page, _, _, cx| page.reply(&review, cx)))
            };
            body = body
                .child(
                    div()
                        .mt(px(8.))
                        .p(px(8.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(palette.border)
                        .bg(palette.background)
                        .text_size(px(13.))
                        .child(Textarea::new(&reply).appearance(false).disabled(busy)),
                )
                .child(
                    h_flex()
                        .mt(px(8.))
                        .gap(px(8.))
                        .justify_end()
                        .children(toggle)
                        .child(send),
                );
        }
        body.children(
            self.thread_problems
                .get(&thread.id)
                .map(|problem| self.render_problem(problem, palette, cx)),
        )
    }
}

/// A section of the page: a bordered card with its heading and a faint count beside it.
fn card(title: &'static str, count: impl Into<SharedString>, palette: Palette) -> Div {
    ui::card(palette).child(
        h_flex()
            .gap(px(8.))
            .items_baseline()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child(title),
            )
            .child(div().text_size(px(12.)).text_color(palette.faint).child(count.into())),
    )
}

/// A confirmation under a card's button: what it will do, then its confirm and cancel buttons.
fn confirm_box(text: String, confirm: Stateful<Div>, cancel: Stateful<Div>, palette: Palette) -> Div {
    v_flex()
        .mt(px(6.))
        .p(px(10.))
        .gap(px(8.))
        .rounded(px(10.))
        .border_1()
        .border_color(palette.border)
        .child(div().text_size(px(13.)).text_color(palette.foreground).child(text))
        .child(h_flex().gap(px(8.)).child(confirm).child(cancel))
}

/// A failure: its kind's title and its message, with a red rule on the left.
fn problem_box(problem: &PullError, palette: Palette) -> Div {
    v_flex()
        .mt(px(8.))
        .pl(px(10.))
        .py(px(4.))
        .gap(px(2.))
        .border_l_2()
        .border_color(palette.destructive)
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.strong)
                .child(model::error_title(problem.kind)),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(palette.muted)
                .child(problem.message.clone()),
        )
}

/// A check's dot: filled for passed, red for failed, a ring while it runs.
pub(crate) fn dot(state: &str, palette: Palette) -> Div {
    let base = div().flex_none().size(px(8.)).rounded(px(4.));
    match state {
        "success" => base.bg(palette.strong),
        "failure" => base.bg(palette.destructive),
        "pending" => base.border_1().border_color(palette.muted),
        _ => base.bg(palette.border),
    }
}

impl Render for PullRequestPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let mut body = v_flex().w_full().max_w(px(COLUMN)).mx_auto().p(px(16.)).gap(px(12.));
        body = body.children(self.render_read_problem(palette, cx));
        match self.page.clone() {
            None if self.problem.is_none() => body = body.child(ui::note("Reading the pull request…", palette)),
            None => {}
            Some(page) => {
                body = body
                    .child(self.render_merge(&page, palette, cx))
                    .children(self.render_stack(&page, palette, cx))
                    .child(self.render_checks(&page, palette))
                    .children(self.render_reviews(&page, palette))
                    .child(self.render_threads(&page, palette, window, cx));
            }
        }
        v_flex()
            .size_full()
            .child(self.render_head(palette, cx))
            .children(self.render_others(palette))
            .child(
                div()
                    .id("pull-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}
