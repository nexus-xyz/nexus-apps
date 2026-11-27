//! An agent's plan accounts on this computer, under its section of Settings › Agents and plans: each account with
//! what is left of its plan and its banked resets, the active one picked by a radio, a click on the row to show or hide
//! its limits, its other actions in a "…" menu, and the switch that moves sessions near a limit. With no account, the
//! card suggests connecting one. The state and the actions are `Plans`' (`subscriptions.rs`).
//!
//! A login being removed dims and says "Removing…" in place of its controls. Once removed, its row folds away and a
//! note under the card says what changed. A removal that failed leaves the row as it was, with why under it.

use std::collections::{HashMap, HashSet};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_client::agents::Route;
use nexus_workbench::event::SessionAgent;
use nexus_workbench::plans::{Limit, PlanUsage};

use super::super::subscriptions::{
    ACTIVE_NOTE, Group, LOW_LEFT, fold_key, folds, groups, limit_label, login_name, plan_name, resets_in, switches,
    tightest, used_pct, vendor, window_label,
};
use super::super::{Load, palette, row, ui};
use super::sign_in_card::{self, Shown};
use super::{Access, AgentEntry, AgentsPage, plan_locked};
use crate::motion;
use crate::palette::Palette;
use crate::workspace::MenuTrigger;

/// Whether Nexus keeps plan accounts for `agent` (Claude Code, Codex): it adds accounts, signs each in and out, and
/// rotates between them. An agent without them (agy) keeps the one sign-in it makes on its own first start.
fn keeps_accounts(agent: SessionAgent) -> bool {
    nexus_workbench::harness::harness(agent).accounts().is_some()
}

/// Whether a running session of `agent` moves to another account near a limit; else only new sessions start on it.
fn moves_running_session(agent: SessionAgent) -> bool {
    nexus_workbench::harness::harness(agent)
        .accounts()
        .is_none_or(|accounts| accounts.moves_running_session)
}

/// What the card needs of `Plans`, read once so the page can draw while it holds `cx`.
struct Seen {
    plans: Load<Vec<PlanUsage>>,
    waiting: Vec<SessionAgent>,
    /// A check runs; Check again waits for it.
    checking: bool,
    busy: bool,
    /// The sign-in Nexus runs, or its card's last state.
    sign_in: Option<Shown>,
    /// A new account's row, hidden while it signs in, by `fold_key`.
    hidden: Option<String>,
    confirm_remove: Option<(SessionAgent, Option<String>)>,
    confirm_reset: Option<String>,
    reset_note: Option<(String, String)>,
    copied: Option<String>,
    /// The rows that show every limit.
    open: HashSet<String>,
    /// The login whose removal runs, by `fold_key`.
    removing: Option<String>,
    /// Why a login could not be removed, by `fold_key`.
    removal_errors: HashMap<String, String>,
    /// The logins removed, by `fold_key`: their rows fold away.
    removed: HashSet<String>,
    /// What the last removal did, under its agent's card.
    removed_note: Option<(SessionAgent, String)>,
    reduce_motion: bool,
}

impl Seen {
    /// Whether a login of `agent` was removed and has not been read again.
    fn removed_any(&self, agent: SessionAgent) -> bool {
        let prefix = format!("{}:", agent.as_str());
        self.removed.iter().any(|key| key.starts_with(&prefix))
    }

    fn row_state(&self, plan: &PlanUsage) -> RowState {
        let key = fold_key(plan);
        if self.removing.as_deref() == Some(key.as_str()) || self.removed.contains(&key) {
            RowState::Pending
        } else if let Some(error) = self.removal_errors.get(&key) {
            RowState::Failed(error.clone())
        } else {
            RowState::Normal
        }
    }
}

/// How an account row draws: as usual, while its removal runs (or it folds away once removed), or after its removal
/// failed, with why.
#[derive(Clone, Debug, PartialEq)]
enum RowState {
    Normal,
    Pending,
    Failed(String),
}

fn seen(page: &AgentsPage, cx: &App) -> Seen {
    let plans = page.plans.read(cx);
    let open = match &plans.plans {
        Load::Ready(list) => list.iter().filter(|plan| plans.is_open(plan)).map(fold_key).collect(),
        _ => HashSet::new(),
    };
    Seen {
        plans: plans.plans.clone(),
        waiting: plans.waiting.clone(),
        checking: plans.checking,
        busy: plans.busy,
        sign_in: plans.sign_in.as_ref().map(Shown::of),
        hidden: plans.hidden(),
        confirm_remove: plans.confirm_remove.clone(),
        confirm_reset: plans.confirm_reset.clone(),
        reset_note: plans.reset_note.clone(),
        copied: plans.copied.clone(),
        open,
        removing: plans.removal.as_ref().map(|removal| removal.key()),
        removal_errors: plans.removal_errors.clone(),
        removed: plans.removed.keys().cloned().collect(),
        removed_note: plans.removed_note.clone(),
        reduce_motion: cx.reduce_motion(),
    }
}

/// The agent's accounts card, its heading and its notes. None when the agent is not on this computer, or when there is
/// nothing to show or suggest.
pub(super) fn section(
    page: &AgentsPage,
    agent: SessionAgent,
    entry: &AgentEntry,
    access: &Access,
    checklist_has_card: bool,
    cx: &mut Context<AgentsPage>,
) -> Option<Div> {
    let palette = palette(cx);
    let seen = seen(page, cx);
    let local = page.account.is_none();
    let plan_open = local || (plan_locked(entry, access).is_none() && !access.unsupported);
    let title = format!("{} accounts on this computer", vendor(agent));
    let list = match &seen.plans {
        Load::Loading => {
            return Some(
                v_flex()
                    .child(ui::group_head(title, palette))
                    .child(ui::card_frame(palette).p(px(16.)).child(ui::skeleton(2, palette))),
            );
        }
        Load::Failed(error) => {
            let plans = page.plans.clone();
            return Some(
                v_flex()
                    .child(ui::group_head(title, palette))
                    .child(ui::failed(error.clone(), palette))
                    .child(
                        h_flex().justify_center().child(
                            ui::button(
                                SharedString::from(format!("plans-retry-{}", agent.as_str())),
                                "Try again",
                                palette,
                            )
                            .on_click(move |_, _, cx| plans.update(cx, |plans, cx| plans.check(cx))),
                        ),
                    ),
            );
        }
        Load::Ready(list) => list,
    };
    let all = groups(list);
    let group = all.iter().find(|group| group.agent == agent);
    let failed = group.is_some_and(|group| keeps_failed(entry.route, group));
    // Windows: an agent whose plan is not read there (agy) gets a note, not an error, when installed. An error read
    // without the check (agy set to bill an API key) shows as on other systems.
    if let Some(note) = crate::agents::ui(agent)
        .windows_plan_note()
        .filter(|_| cfg!(windows) && !failed)
    {
        return (group.is_some() || seen.waiting.contains(&agent)).then(|| {
            v_flex().child(ui::group_head(title, palette)).child(
                ui::card_frame(palette)
                    .p(px(16.))
                    .child(ui::setting_detail(note, palette)),
            )
        });
    }
    // Its logins have not all answered, and none read as signed in yet: a slow agent holds back none of the others,
    // and a signed-out own login does not offer to connect while another account may still be signed in. A failed
    // reading stays on screen while the next check runs. A card that just lost a login stays: its rows fold, and the
    // suggestion unfolds, in place.
    if !failed
        && seen.waiting.contains(&agent)
        && group.is_none_or(|group| group.accounts == 0)
        && !seen.removed_any(agent)
    {
        return Some(
            v_flex()
                .child(ui::group_head(title, palette))
                .child(ui::card_frame(palette).p(px(16.)).child(ui::skeleton(2, palette))),
        );
    }
    // Not installed here: the setup steps say so, and there is no login to show.
    let group = group?;

    // The account rows, with those removed while they fold away. The rest of the card counts only the others. A login
    // whose check failed shows as a row, with the error and Check again (`keeps_failed`).
    let account_rows: Vec<&PlanUsage> = group
        .rows
        .iter()
        .copied()
        .filter(|plan| plan.signed_in || plan.account.is_some() || failed)
        .collect();
    // A new account signing in is drawn folded: its row unfolds once it signs in, as the card folds.
    let hidden = |plan: &PlanUsage| seen.hidden.as_deref() == Some(fold_key(plan).as_str());
    let staying: Vec<&PlanUsage> = account_rows
        .iter()
        .copied()
        .filter(|plan| !seen.removed.contains(&fold_key(plan)) && !hidden(plan))
        .collect();
    // The sign-in card of this agent: under its row for Sign in and Sign in again, else under the list.
    let card = seen
        .sign_in
        .clone()
        .filter(|shown| shown.agent == agent && !checklist_has_card);
    let under_row = card
        .as_ref()
        .filter(|shown| !shown.new && account_rows.iter().any(|plan| fold_key(plan) == shown.key))
        .map(|shown| shown.key.clone());
    let accounts = staying.len();
    let signed_in = staying.iter().filter(|plan| plan.signed_in).count();
    let choosing = signed_in > 1;
    let use_plan = plan_open && entry.route == Route::Gateway;
    // Never signed in: a suggestion, when the plan is open to the person and the agent pays with Nexus now.
    let own = list.iter().find(|plan| plan.agent == agent && plan.account.is_none());
    let suggest = own.filter(|_| local || use_plan);
    let note = seen
        .removed_note
        .clone()
        .filter(|(each, _)| *each == agent)
        .map(|(_, note)| note);
    if account_rows.is_empty() && suggest.is_none() && note.is_none() && card.is_none() {
        return None;
    }
    let settings = crate::desktop_settings::load();
    let writable = crate::desktop_settings::writable();

    // Each part of the card opens and closes in place, a rule above it when something shows above it.
    let mut parts: Vec<AnyElement> = Vec::new();
    let mut above = false;
    // `ruled`: a rule may go above it. A sign-in card under its row has none.
    let mut part = |id: String, open: bool, shows: bool, content: Option<AnyElement>, ruled: bool| {
        let rule = above && ruled;
        above |= shows;
        let content = content.map(|content| {
            v_flex()
                .when(rule, |part| part.child(ui::card_rule(palette)))
                .child(content)
                .into_any_element()
        });
        parts.push(motion::reveal(SharedString::from(id), open, content).into_any_element());
    };
    for plan in &account_rows {
        let key = fold_key(plan);
        let state = seen.row_state(plan);
        let leaving = seen.removed.contains(&key);
        let shows = !leaving && !hidden(plan);
        part(
            format!("plans-row-{key}"),
            shows,
            shows,
            Some(account_row(page, &seen, entry, plan, state, choosing, use_plan, cx).into_any_element()),
            true,
        );
        // Each row has a slot for the card, so it folds in place when the sign-in ends.
        let here = under_row.as_deref() == Some(key.as_str());
        let content = card
            .as_ref()
            .filter(|_| here)
            .map(|shown| sign_in_card::card(page, shown, seen.copied.as_deref(), false, seen.reduce_motion, cx));
        part(format!("plans-sign-in-slot-{key}"), here, false, content, false);
    }
    let switch_row = (accounts > 1).then(|| {
        let on = switches(&settings, agent);
        let locked = seen.busy || !writable;
        let plans = page.plans.clone();
        let detail = if moves_running_session(agent) {
            "At 90% of a limit, the session moves to the account with the most usage left."
        } else {
            "At 90% of a limit, a new session starts on the account with the most usage left. A running session keeps \
             its account."
        };
        ui::setting_row(
            "Switch accounts near a limit",
            detail,
            ui::switch(
                SharedString::from(format!("plans-rotate-{}", agent.as_str())),
                on,
                locked,
                palette,
            )
            .when(!locked, |switch| {
                let keys = plans.clone();
                switch
                    .tab_index(0)
                    .on_key_down(move |event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            cx.stop_propagation();
                            keys.update(cx, |plans, cx| plans.rotate(agent, !on, cx));
                        }
                    })
                    .on_click(move |_, _, cx| plans.update(cx, |plans, cx| plans.rotate(agent, !on, cx)))
            }),
            palette,
        )
        .into_any_element()
    });
    part(
        format!("plans-rotate-row-{}", agent.as_str()),
        accounts > 1,
        accounts > 1,
        switch_row,
        true,
    );
    if let Some(own) = suggest {
        let open = accounts == 0;
        let connect = open.then(|| {
            let plans = page.plans.clone();
            let target = own.clone();
            let id = SharedString::from(format!("plans-connect-{}", agent.as_str()));
            let label = format!("Connect {}", vendor(agent));
            ui::setting_row(
                format!("Use your {} plan", vendor(agent)),
                if local {
                    format!("Connect a {} account for local sessions.", vendor(agent))
                } else {
                    format!(
                        "Connect a {} account. Then {} can pay with your plan, not your organization's key.",
                        vendor(agent),
                        entry.name
                    )
                },
                if seen.busy {
                    ui::disabled_button(id, label, palette).into_any_element()
                } else {
                    ui::button(id, label, palette)
                        .on_click(move |_, _, cx| plans.update(cx, |plans, cx| plans.sign_in(&target, cx)))
                        .into_any_element()
                },
                palette,
            )
            .into_any_element()
        });
        part(format!("plans-suggest-{}", agent.as_str()), open, open, connect, true);
    }
    let showing = !account_rows.is_empty() || (suggest.is_some() && accounts == 0);

    let add = {
        let plans = page.plans.clone();
        let id = SharedString::from(format!("plans-add-{}", agent.as_str()));
        if seen.busy {
            ui::disabled_button(id, "Add account", palette).into_any_element()
        } else {
            ui::ghost_button(id, "Add account", palette)
                .on_click(move |_, _, cx| plans.update(cx, |plans, cx| plans.add(agent, cx)))
                .into_any_element()
        }
    };
    let mut notes: Vec<String> = Vec::new();
    if choosing {
        notes.push(if moves_running_session(agent) {
            ACTIVE_NOTE.into()
        } else {
            format!(
                "New {} sessions start on the active account. Open sessions keep the account they started on.",
                nexus_workbench::harness::harness(agent).label()
            )
        });
    }
    if use_plan && signed_in > 0 {
        notes.push(format!(
            "{} pays with Nexus now. Click Use this plan to pay with your {} plan.",
            entry.name,
            vendor(agent)
        ));
    }
    // Antigravity has no extra accounts in Nexus: agy keeps one Google sign-in. With no account, the suggestion
    // connects one instead.
    let head = ui::group_head(title, palette).when(keeps_accounts(agent) && accounts > 0, |head| head.child(add));
    let note_open = note.is_some();
    let note = note.map(|note| ui::setting_detail(note, palette).pt(px(8.)).into_any_element());
    let column = v_flex()
        .child(head)
        .when(showing, |column| column.child(ui::card_frame(palette).children(parts)))
        .when(!notes.is_empty(), |column| {
            column.child(ui::footnote(notes.join(" "), palette))
        })
        .child(motion::reveal(
            SharedString::from(format!("plans-removed-note-{}", agent.as_str())),
            note_open,
            note,
        ));
    // Add account and Connect: the card under the list.
    let at_end = card.as_ref().filter(|_| under_row.is_none());
    let end = at_end.map(|shown| sign_in_card::card(page, shown, seen.copied.as_deref(), true, seen.reduce_motion, cx));
    Some(column.child(motion::reveal(
        SharedString::from(format!("plans-sign-in-{}", agent.as_str())),
        at_end.is_some(),
        end,
    )))
}

/// Whether the card stays though no login reads as signed in: the agent pays with its plan and a login's check failed,
/// so a check that fails for a moment never hides the plan. Its rows show the error and Check again.
fn keeps_failed(route: Route, group: &Group<'_>) -> bool {
    route != Route::Gateway && group.rows.iter().any(|plan| plan.error.is_some())
}

/// One login: the radio that makes it active, who it is, what is left, its main action, its "…" menu and a chevron.
/// The row itself shows or hides every limit under it (a click, Enter or Space), as the chevron says: `>` folded, `⌄`
/// open. A login with nothing to show under it (signed out) keeps the chevron's slot empty, so the columns line up.
///
/// While its removal runs, the row dims, takes no clicks, folds its limits, and says "Removing…" over its controls,
/// which keep their place so nothing moves. After a failed removal, why shows under it, with Try again and Dismiss.
#[allow(clippy::too_many_arguments)]
fn account_row(
    page: &AgentsPage,
    seen: &Seen,
    entry: &AgentEntry,
    plan: &PlanUsage,
    state: RowState,
    choosing: bool,
    use_plan: bool,
    cx: &mut Context<AgentsPage>,
) -> Div {
    let palette = palette(cx);
    let agent = plan.agent;
    let key = fold_key(plan);
    let id = |what: &str| row_id(what, &key);
    let pending = state == RowState::Pending;
    let writable = crate::desktop_settings::writable();
    let detail = if plan.signed_in {
        if plan.active && choosing {
            format!("{} · Active", plan_name(plan))
        } else {
            plan_name(plan)
        }
    } else if plan.error.is_some() {
        // A failed check says nothing of the sign-in, so the row does not say signed out.
        "The check failed".into()
    } else if plan.account.is_some() {
        "Signed out".into()
    } else {
        "Not connected".into()
    };

    let radio = choosing.then(|| {
        let picked = plan.active && plan.signed_in;
        let locked = !plan.signed_in || seen.busy || !writable || pending;
        let plans = page.plans.clone();
        let account = plan.account.clone();
        div()
            .id(id("use"))
            .flex_none()
            .size(px(16.))
            .rounded(px(8.))
            .border_color(if picked { palette.strong } else { palette.icon })
            .map(|radio| {
                if picked {
                    radio.border(px(5.))
                } else {
                    radio.border(px(1.5))
                }
            })
            .when(locked && !picked && !pending, |radio| radio.opacity(0.4))
            .when(pending, |radio| radio.opacity(PENDING_OPACITY))
            .when(!locked && !picked, |radio| {
                radio.cursor_pointer().on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    plans.update(cx, |plans, cx| plans.use_account(agent, account.clone(), cx))
                })
            })
    });

    let main_action = if !plan.signed_in && plan.error.is_some() && seen.checking {
        Some(ui::disabled_button(id("check"), "Check again", palette).into_any_element())
    } else if !plan.signed_in && plan.error.is_some() {
        let plans = page.plans.clone();
        Some(
            ui::button(id("check"), "Check again", palette)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    plans.update(cx, |plans, cx| plans.check(cx))
                })
                .into_any_element(),
        )
    } else if !plan.signed_in {
        let plans = page.plans.clone();
        let target = plan.clone();
        Some(
            ui::button(id("sign-in"), "Sign in", palette)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    plans.update(cx, |plans, cx| plans.sign_in(&target, cx))
                })
                .into_any_element(),
        )
    } else if use_plan {
        let account = plan.account.clone();
        let agent_id = entry.agent.clone();
        Some(
            ui::button(id("use-plan"), "Use this plan", palette)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.use_plan(agent_id.clone(), account.clone(), cx)
                }))
                .into_any_element(),
        )
    } else {
        None
    };
    let foldable = folds(plan);
    // The summary follows the person's fold, not the removal's: a row that folds while it is removed keeps its
    // controls' widths.
    let open = foldable && seen.open.contains(&key);
    let summary = (plan.signed_in && !open)
        .then(|| tightest(plan))
        .flatten()
        .map(|limit| summary(limit, palette));

    let confirming = seen.confirm_remove.as_ref() == Some(&(agent, plan.account.clone()));
    let chevron = div().flex_none().size(px(16.)).when(foldable, |slot| {
        slot.child(ui::icon(
            if open {
                Lucide::ChevronDown
            } else {
                Lucide::ChevronRight
            },
            palette.icon,
        ))
    });
    // The controls on the right. While the removal runs they keep their place, hidden, under its status.
    let controls = h_flex()
        .flex_none()
        .gap(px(16.))
        .when(pending, |controls| controls.invisible())
        .children(summary)
        .children(main_action)
        // agy keeps one sign-in and has no "…" items (`menu`), so its one row has no empty slot before the chevron.
        .when(keeps_accounts(agent), |controls| {
            controls.child(menu(page, seen, plan, &key, confirming, cx))
        })
        .child(chevron);
    let head = ui::card_row()
        .id(id("fold"))
        .when(foldable && !pending, |head| {
            let plans = page.plans.clone();
            let target = plan.clone();
            head.tab_index(0)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .focus_visible(|style| style.bg(palette.hover))
                .on_click(move |_, _, cx| plans.update(cx, |plans, cx| plans.toggle(&target, cx)))
        })
        .children(radio)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .when(pending, |name| name.opacity(PENDING_OPACITY))
                .child(ui::two_lines(login_name(plan), detail, palette)),
        )
        .child(div().relative().flex_none().child(controls).when(pending, |slot| {
            slot.child(
                h_flex()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .pl(px(8.))
                    .bg(palette.raised)
                    .child(removing_status(&key, seen.reduce_motion, palette)),
            )
        }));
    let failed = match &state {
        RowState::Failed(error) => Some(removal_error(page, seen, plan, error.clone(), palette)),
        _ => None,
    };
    let now = nexus_workbench::now_ms();
    let body = open.then(|| {
        v_flex()
            .px(px(16.))
            .pb(px(14.))
            .gap(px(12.))
            .when_some(plan.error.clone(), |body, error| {
                body.child(ui::error_line(error, palette))
            })
            .when(plan.error.is_none() && plan.limits.is_empty(), |body| {
                body.child(ui::setting_detail(
                    if plan.google_cloud {
                        nexus_workbench::plans::CLOUD_USAGE_NOTE
                    } else {
                        "This plan reports no usage limits."
                    },
                    palette,
                ))
            })
            .children(plan.limits.iter().map(|limit| meter(limit, now, palette)))
            .children(banked_resets(page, seen, plan, now, palette))
            .into_any_element()
    });
    v_flex()
        .child(head)
        .child(motion::reveal(id("error-area"), failed.is_some(), failed))
        .when(confirming, |row| {
            row.child(
                ui::setting_detail(
                    format!(
                        "Press Remove again to sign out {} on this computer. This also affects the agent outside Nexus.",
                        plan.email
                            .clone()
                            .unwrap_or_else(|| format!("this {} account", vendor(agent)))
                    ),
                    palette,
                )
                .px(px(16.))
                .pb(px(12.)),
            )
        })
        .when(foldable, |row| {
            row.child(motion::reveal(id("limits"), open && !pending, body))
        })
}

/// How much a row being removed keeps of its radio and its name.
const PENDING_OPACITY: f32 = 0.55;

/// An element id for a part of a login's row, from the login's key: a row keeps its ids when the rows above it go.
fn row_id(what: &str, key: &str) -> SharedString {
    SharedString::from(format!("plans-{what}-{key}"))
}

/// "Removing…" after a turning loader, as the New Code form's payer check turns. With less motion, a still dot.
fn removing_status(key: &str, reduce_motion: bool, palette: Palette) -> Div {
    let mark = if reduce_motion {
        div()
            .flex_none()
            .size(px(6.))
            .rounded(px(3.))
            .bg(palette.muted)
            .debug_selector(|| "removing-dot".into())
            .into_any_element()
    } else {
        div()
            .flex_none()
            .debug_selector(|| "removing-spinner".into())
            .child(
                ui::icon(Lucide::LoaderCircle, palette.muted)
                    .size(px(14.))
                    .with_animation(
                        row_id("removing-spinner", key),
                        // A loop asks for frames at the display's rate unless capped (AGENTS.md, render rule 4).
                        Animation::new(std::time::Duration::from_millis(1000))
                            .repeat()
                            .with_max_fps(30.),
                        |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
                    ),
            )
            .into_any_element()
    };
    h_flex()
        .h_full()
        .flex_none()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(mark)
        .child("Removing…")
}

/// Why the login is still here, with Try again (no second press: it was confirmed once) and Dismiss.
fn removal_error(page: &AgentsPage, seen: &Seen, plan: &PlanUsage, error: String, palette: Palette) -> AnyElement {
    let key = fold_key(plan);
    let (agent, account) = (plan.agent, plan.account.clone());
    let retry = if seen.busy {
        ui::disabled_button(row_id("remove-retry", &key), "Try again", palette).into_any_element()
    } else {
        let plans = page.plans.clone();
        let account = account.clone();
        ui::button(row_id("remove-retry", &key), "Try again", palette)
            .on_click(move |_, _, cx| plans.update(cx, |plans, cx| plans.retry_remove(agent, account.clone(), cx)))
            .into_any_element()
    };
    let plans = page.plans.clone();
    let dismiss = ui::ghost_button(row_id("remove-dismiss", &key), "Dismiss", palette).on_click(move |_, _, cx| {
        plans.update(cx, |plans, cx| {
            plans.dismiss_removal_error(agent, account.as_deref(), cx)
        })
    });
    h_flex()
        .px(px(16.))
        .pb(px(12.))
        .gap(px(8.))
        .child(div().flex_1().min_w(px(0.)).child(ui::error_line(error, palette)))
        .child(retry)
        .child(dismiss)
        .into_any_element()
}

/// The resets banked on a login: how many, when the first expires, and the button that uses one on a second press.
/// Then what the reset did.
fn banked_resets(page: &AgentsPage, seen: &Seen, plan: &PlanUsage, now: i64, palette: Palette) -> Option<Div> {
    let key = fold_key(plan);
    let note = seen
        .reset_note
        .clone()
        .filter(|(login, _)| *login == key)
        .map(|(_, note)| note);
    let available = plan.resets.as_ref().map_or(0, |resets| resets.available);
    if available == 0 && note.is_none() {
        return None;
    }
    let confirming = seen.confirm_reset.as_deref() == Some(key.as_str());
    let id = |what: &str| row_id(what, &key);
    let detail = if confirming {
        "Use one now? It clears this account's Codex limits, and it cannot be undone.".to_string()
    } else {
        let expires = plan
            .resets
            .as_ref()
            .and_then(|resets| resets.expires_at)
            .filter(|at| *at > now)
            .map(|at| format!(" · the next expires in {}", resets_in(at, now)))
            .unwrap_or_default();
        match available {
            1 => format!("1 reset banked{expires}"),
            count => format!("{count} resets banked{expires}"),
        }
    };
    let label = if confirming { "Use? Press again" } else { "Use a reset" };
    let use_button = if seen.busy {
        ui::disabled_button(id("reset"), label, palette).into_any_element()
    } else {
        let plans = page.plans.clone();
        let target = plan.clone();
        ui::button(id("reset"), label, palette)
            .on_click(move |_, _, cx| plans.update(cx, |plans, cx| plans.use_reset(&target, cx)))
            .into_any_element()
    };
    let cancel = confirming.then(|| {
        let plans = page.plans.clone();
        ui::ghost_button(id("reset-cancel"), "Cancel", palette).on_click(move |_, _, cx| {
            plans.update(cx, |plans, cx| {
                plans.confirm_reset = None;
                cx.notify();
            })
        })
    });
    Some(
        v_flex()
            .gap(px(6.))
            .when(available > 0, |column| {
                column.child(
                    h_flex()
                        .gap(px(8.))
                        .child(div().flex_1().min_w(px(0.)).child(ui::setting_detail(detail, palette)))
                        .children(cancel)
                        .child(use_button),
                )
            })
            .when_some(note, |column, note| column.child(ui::setting_detail(note, palette))),
    )
}

/// The row's "…" menu: sign in again, and remove an account (a second press). With neither, its slot stays empty so
/// the chevrons line up.
fn menu(
    page: &AgentsPage,
    seen: &Seen,
    plan: &PlanUsage,
    key: &str,
    confirming: bool,
    cx: &mut Context<AgentsPage>,
) -> Div {
    let palette = palette(cx);
    let agent = plan.agent;
    let menu_open = page.menu.as_deref() == Some(key);
    let id = |what: &str| row_id(what, key);
    let mut items: Vec<AnyElement> = Vec::new();
    // agy signs in on its own first start; Nexus has no sign-in command to run for it.
    if plan.signed_in && !seen.busy && keeps_accounts(agent) {
        let plans = page.plans.clone();
        let target = plan.clone();
        items.push(
            row::menu_item(id("again"), "Sign in again", false, 0, palette)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    plans.update(cx, |plans, cx| plans.sign_in(&target, cx));
                }))
                .into_any_element(),
        );
    }
    if !seen.busy && (plan.account.is_some() || (plan.signed_in && keeps_accounts(agent))) {
        let account = plan.account.clone();
        let plans = page.plans.clone();
        items.push(
            row::menu_item(
                id("remove"),
                if confirming {
                    "Remove? Press again"
                } else {
                    "Remove from this computer"
                },
                false,
                0,
                palette,
            )
            .text_color(palette.destructive)
            .on_click(cx.listener(move |this, _, _, cx| {
                let account = account.clone();
                let done = plans.read(cx).confirm_remove.as_ref() == Some(&(agent, account.clone()));
                if done {
                    this.menu = None;
                }
                plans.update(cx, |plans, cx| plans.remove(agent, account, cx));
            }))
            .into_any_element(),
        );
    }
    if items.is_empty() {
        return div().flex_none().size(px(ui::CONTROL_HEIGHT));
    }
    let key = key.to_string();
    let toggle_key = key.clone();
    div()
        .relative()
        .child(
            row::icon_button(id("more"), Lucide::Ellipsis, "More", palette).menu_trigger(
                menu_open,
                cx,
                move |this: &AgentsPage| this.menu.as_deref() == Some(key.as_str()),
                move |this, _, cx| {
                    this.menu = (this.menu.as_deref() != Some(toggle_key.as_str())).then(|| toggle_key.clone());
                    cx.notify();
                },
            ),
        )
        .when(menu_open, |anchor| {
            let plans = page.plans.clone();
            anchor.child(row::popover(
                v_flex()
                    .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        plans.update(cx, |plans, cx| {
                            plans.confirm_remove = None;
                            cx.notify();
                        });
                        cx.notify();
                    }))
                    .children(items),
                palette,
            ))
        })
}

/// A row's tightest limit: "62% left" over a short bar. Nearly out, it turns to the warning color.
fn summary(limit: &Limit, palette: Palette) -> Div {
    let used = used_pct(limit);
    let left = 100 - used;
    let low = left <= LOW_LEFT;
    let color = if low { palette.warning } else { palette.muted };
    v_flex()
        .flex_none()
        .w(px(200.))
        .gap(px(5.))
        .child(
            h_flex()
                .justify_between()
                .gap(px(10.))
                .text_size(px(12.))
                // Only the window ("This week"): the open row names the model or group ("This week · Fable"). A
                // window name too long for the column ends in an ellipsis before it reaches the number.
                .child(
                    div()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(palette.faint)
                        .child(window_label(&limit.label)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(if low { palette.warning } else { palette.foreground })
                        .child(format!("{left}% left")),
                ),
        )
        .child(
            div()
                .h(px(4.))
                .w_full()
                .rounded(px(2.))
                .bg(palette.selected)
                .child(div().h_full().rounded(px(2.)).w(relative(left as f32 / 100.)).bg(color)),
        )
}

/// One limit, in full: its name, what is left, a bar, and when it resets.
fn meter(limit: &Limit, now: i64, palette: Palette) -> Div {
    let used = used_pct(limit);
    let left = 100 - used;
    let low = left <= LOW_LEFT;
    v_flex()
        .gap(px(4.))
        .child(
            h_flex()
                .justify_between()
                .text_size(px(12.))
                .child(div().text_color(palette.foreground).child(limit_label(&limit.label)))
                .child(
                    div()
                        .text_color(if low { palette.warning } else { palette.muted })
                        .child(format!("{left}% left")),
                ),
        )
        .child(
            div().h(px(6.)).w_full().rounded(px(3.)).bg(palette.hover).child(
                div()
                    .h_full()
                    .rounded(px(3.))
                    .w(relative(left as f32 / 100.))
                    .bg(if low { palette.warning } else { palette.muted }),
            ),
        )
        .when_some(limit.resets_at, |meter, at| {
            meter.child(ui::setting_detail(format!("Resets in {}", resets_in(at, now)), palette))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use nexus_workbench::plans;

    fn own_login(error: Option<&str>) -> PlanUsage {
        PlanUsage {
            agent: SessionAgent::Antigravity,
            error: error.map(String::from),
            ..plans::read_codex(None, &serde_json::json!({"account": null}), None)
        }
    }

    #[test]
    fn failed_check_keeps_a_plan_agents_card() {
        let failed = own_login(Some("The quota check stopped before it answered."));
        let signed_out = own_login(None);
        let group = |plan| Group {
            agent: SessionAgent::Antigravity,
            accounts: 0,
            rows: vec![plan],
        };
        assert!(!failed.signed_in && !signed_out.signed_in);
        // Paying with its plan, a failed check keeps the card, with the error.
        assert!(keeps_failed(Route::Subscription, &group(&failed)));
        // A signed-out login with no error, and an agent that pays with Nexus, look as before.
        assert!(!keeps_failed(Route::Subscription, &group(&signed_out)));
        assert!(!keeps_failed(Route::Gateway, &group(&failed)));
    }

    struct Status(Palette);

    impl Render for Status {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(300.))
                .child(removing_status("claude:a_1", cx.reduce_motion(), self.0))
        }
    }

    fn drawn(reduce: bool, cx: &mut TestAppContext) -> (bool, bool) {
        cx.update(|cx| cx.set_reduce_motion(reduce));
        let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
        let (_, cx) = cx.add_window_view(|_, _| Status(palette));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (
            cx.debug_bounds("removing-spinner").is_some(),
            cx.debug_bounds("removing-dot").is_some(),
        )
    }

    #[gpui_kit::test]
    fn a_row_being_removed_turns_a_loader_or_with_less_motion_shows_a_still_dot(cx: &mut TestAppContext) {
        assert_eq!(drawn(false, cx), (true, false));
        assert_eq!(drawn(true, cx), (false, true));
    }

    /// How many times the status asks its view to draw again over a second of the test clock, drawing each
    /// millisecond (`motion/mark.rs`).
    fn frames_in_a_second(reduce: bool, cx: &mut TestAppContext) -> usize {
        cx.update(|cx| cx.set_reduce_motion(reduce));
        let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
        let (status, cx) = cx.add_window_view(|_, _| Status(palette));
        let asks = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = asks.clone();
        cx.update(|_, cx| cx.observe(&status, move |_, _| count.set(count.get() + 1)).detach());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let before = asks.get();
        for _ in 0..1000 {
            cx.executor().advance_clock(std::time::Duration::from_millis(1));
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        asks.get() - before
    }

    #[gpui_kit::test]
    fn the_loader_turns_at_thirty_frames_a_second_and_the_still_dot_asks_for_none(cx: &mut TestAppContext) {
        let turning = frames_in_a_second(false, cx);
        assert!((27..=31).contains(&turning), "{turning} frames in a second");
        assert_eq!(frames_in_a_second(true, cx), 0);
    }

    #[test]
    fn row_ids_follow_the_login_not_its_place() {
        assert_eq!(
            row_id("fold", "claude:a_1"),
            SharedString::from("plans-fold-claude:a_1")
        );
        assert_eq!(row_id("fold", "codex:own"), SharedString::from("plans-fold-codex:own"));
    }
}
