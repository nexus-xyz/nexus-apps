//! Issues: the person's Linear issues, read through Gateway's Linear connector (`issues`, assignee `me`), as T3 Code's
//! `IssuesPage.tsx`. Start on a row triages the issue into a session: New Code opens with the issue in the composer,
//! and the person picks the project and sends.
//!
//! Gateway checks the connector's availability and policy. A refusal for missing setup (no Linear account, Linear off
//! for the person) is not a load failure: the view says what it needs and connects in place (`issues_connect.rs`).
//! Only the first page is read.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Place, View, Workspace};
use crate::connector_gap::{self, Gap};
use crate::gateway::Verb;
use crate::palette::Palette;
use crate::workspace::overlay::OverlaySlot;

/// The list's widest, as the chat column.
const PAGE: f32 = 760.;

/// One row of the list: the fields of the connector's `issues` answer the page shows.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct Issue {
    pub identifier: String,
    pub title: String,
    pub url: String,
    pub state: State,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct State {
    pub name: String,
}

#[derive(Deserialize)]
struct Answer {
    issues: Vec<Issue>,
}

/// Why the list is not there.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Unread {
    /// Setup is missing: Linear is not connected, or not on for the person.
    Missing(Gap),
    /// The read failed (network, 5xx, an answer of another shape): the red ✕ and Try again.
    Failed(SharedString),
}

/// What the Issues view shows: `None` until the first read comes back.
pub(super) struct IssuesState {
    pub list: Option<Result<Vec<Issue>, Unread>>,
    pub scroll: ScrollHandle,
    /// Connect Linear was pressed: the browser (or Settings) is open, and the view waits for the account.
    pub connecting: bool,
    /// Re-reads the connections until Linear's account lands (`issues_connect.rs`).
    pub watch: Task<()>,
}

impl IssuesState {
    pub fn new() -> IssuesState {
        IssuesState {
            list: None,
            scroll: ScrollHandle::new(),
            connecting: false,
            watch: Task::ready(()),
        }
    }
}

/// The connector's `issues` answer as rows; an answer of another shape is an error, never an empty list.
fn parse(answer: Value) -> Result<Vec<Issue>, String> {
    serde_json::from_value::<Answer>(answer)
        .map(|answer| answer.issues)
        .map_err(|error| format!("Linear's answer was not a list of issues: {error}"))
}

/// The read's answer as the view shows it: the rows, the setup it misses, or the failure.
pub(super) fn read_answer(answer: nexus_client::Result<Value>) -> Result<Vec<Issue>, Unread> {
    match answer {
        Ok(answer) => parse(answer).map_err(|error| Unread::Failed(error.into())),
        Err(error) => Err(match connector_gap::classify(&error) {
            Some(gap) => Unread::Missing(gap),
            None => Unread::Failed(error.to_string().into()),
        }),
    }
}

/// The first message of the session an issue is triaged into. Its first line names the session's branch
/// (`NewSession::worktree`), so it starts with the identifier. The list has no description: the agent reads the issue.
pub(super) fn triage_prompt(issue: &Issue) -> String {
    format!(
        "{}: {}\n\n{}\n\nRead this Linear issue. Find where the change belongs and propose a short plan before you edit.",
        issue.identifier, issue.title, issue.url
    )
}

impl Workspace {
    /// Open the Issues view and read the list again.
    pub(super) fn open_issues(&mut self, cx: &mut Context<Self>) {
        if self.account_gateway().is_none() {
            return;
        }
        self.visit(Place::Issues);
        if !matches!(self.view, View::Issues) {
            self.reset_chat();
            self.view = View::Issues;
        }
        self.overlay.close_person_menu();
        self.title = "Issues".into();
        self.load_issues(cx);
        cx.notify();
    }

    /// Read the list again, where the view stands: a fix that lands elsewhere (`setup::done`) does not move the person.
    pub(super) fn load_issues(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        // Gateway's read of a connector operation (`read_connector`), typed, so its refusal keeps its code. The path
        // stays beside its verb: a release check reads it there.
        let request = account_gateway.request_typed(
            Verb::Post,
            "/v1/connectors/linear/issues/read".into(),
            Some(json!({ "assignee": "me" })),
        );
        crate::pages::fetch(cx, request, |this: &mut Workspace, answer, _| {
            let list = read_answer(answer);
            // A connect in progress keeps waiting while Linear still misses.
            if !matches!(list, Err(Unread::Missing(Gap::NotConnected | Gap::Expired))) {
                this.issues.connecting = false;
            }
            this.issues.list = Some(list);
        });
    }

    /// Triage `issue` into a session: New Code, with the issue as the first message.
    fn triage(&mut self, issue: &Issue, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_code(window, cx);
        let text = triage_prompt(issue);
        self.composer.update(cx, |state, cx| state.set_value(text, window, cx));
        self.focus_composer(window, cx);
    }

    pub(super) fn render_issues(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &self.issues.list {
            None => div()
                .text_size(px(13.))
                .text_color(palette.faint)
                .child("Reading your Linear issues…")
                .into_any_element(),
            Some(Err(Unread::Missing(gap))) => self.render_issues_missing(*gap, palette, cx),
            Some(Err(Unread::Failed(error))) => self.render_issues_failed(error.clone(), palette, cx),
            Some(Ok(issues)) if issues.is_empty() => div()
                .text_size(px(13.))
                .text_color(palette.faint)
                .child("No Linear issues are assigned to you.")
                .into_any_element(),
            Some(Ok(issues)) => v_flex()
                .gap(px(2.))
                .children(issues.iter().enumerate().map(|(ix, issue)| {
                    let url = issue.url.clone();
                    let picked = issue.clone();
                    h_flex()
                        .id(SharedString::from(format!("issue-{ix}")))
                        .gap(px(12.))
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(8.))
                        .hover(|style| style.bg(palette.hover))
                        .child(
                            div()
                                .id(SharedString::from(format!("issue-link-{ix}")))
                                .flex_none()
                                .w(px(88.))
                                .text_size(px(12.))
                                .text_color(palette.faint)
                                .cursor_pointer()
                                .child(issue.identifier.clone())
                                .on_click(move |_, _, cx| crate::external_link::open(&url, cx)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(13.))
                                .text_color(palette.strong)
                                .child(issue.title.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(palette.muted)
                                .child(issue.state.name.clone()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("issue-start-{ix}")))
                                .flex_none()
                                .px(px(10.))
                                .py(px(3.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(palette.border)
                                .text_size(px(12.))
                                .cursor_pointer()
                                .hover(|style| style.bg(palette.selected))
                                .child("Start")
                                .on_click(cx.listener(move |this, _, window, cx| this.triage(&picked, window, cx))),
                        )
                }))
                .into_any_element(),
        };
        div()
            .id("issues-page")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.issues.scroll)
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(PAGE))
                    .mx_auto()
                    .px(px(32.))
                    .pt(px(28.))
                    .pb(px(48.))
                    .gap(px(16.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.strong)
                            .child("Issues"),
                    )
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Issue, State, Unread, parse, read_answer, triage_prompt};
    use crate::connector_gap::Gap;
    use nexus_client::error::{CoreError, Transport};
    use serde_json::json;

    fn refused(status: u16, code: &str) -> nexus_client::Error {
        CoreError {
            status,
            message: "linear is not connected".into(),
            code: Some(code.into()),
            interval: None,
            details: None,
        }
        .into()
    }

    #[test]
    fn a_missing_connection_is_setup_not_a_load_failure() {
        assert_eq!(
            read_answer(Err(refused(403, "connection_required"))),
            Err(Unread::Missing(Gap::NotConnected))
        );
        assert_eq!(
            read_answer(Err(refused(403, "connector_disabled"))),
            Err(Unread::Missing(Gap::Off))
        );
    }

    #[test]
    fn a_real_load_error_stays_a_failure() {
        assert!(matches!(
            read_answer(Err(refused(502, "upstream_error"))),
            Err(Unread::Failed(_))
        ));
        assert!(matches!(
            read_answer(Err(Transport::Timeout.into())),
            Err(Unread::Failed(_))
        ));
        assert!(matches!(read_answer(Ok(json!([]))), Err(Unread::Failed(_))));
    }

    fn issue() -> Issue {
        Issue {
            identifier: "ACME-12".into(),
            title: "Fix the login loop".into(),
            url: "https://linear.app/x/issue/ACME-12".into(),
            state: State { name: "Todo".into() },
        }
    }

    #[test]
    fn reads_the_connectors_issues_answer() {
        // The `issues` answer's shape (`connectors/linear/issues.rs`, `Issues`), with fields the page ignores.
        let answer = json!({
            "issues": [{
                "identifier": "ACME-12", "title": "Fix the login loop", "url": "https://linear.app/x/issue/ACME-12",
                "priority": 2, "state": { "name": "Todo", "type": "unstarted" }, "assignee": null,
                "created_at": "2026-11-20T00:00:00Z", "updated_at": "2026-11-21T00:00:00Z",
                "started_at": null, "completed_at": null, "canceled_at": null
            }],
            "next": null
        });
        assert_eq!(parse(answer), Ok(vec![issue()]));
        assert_eq!(parse(json!({ "issues": [], "next": null })), Ok(vec![]));
    }

    #[test]
    fn an_answer_of_another_shape_is_an_error_not_an_empty_list() {
        assert!(parse(json!({ "error": "linear is not connected" })).is_err());
        assert!(parse(json!([])).is_err());
    }

    #[test]
    fn the_prompt_names_the_issue() {
        let prompt = triage_prompt(&issue());
        assert!(prompt.contains("ACME-12"), "{prompt}");
        assert!(prompt.contains("Fix the login loop"), "{prompt}");
        assert!(prompt.contains("https://linear.app/x/issue/ACME-12"), "{prompt}");
    }
}
