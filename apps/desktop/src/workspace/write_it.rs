//! "Write it" in the git panel's commit form, the pull request form and the Changes tab's commit box, as the Nexus app
//! has it: the commit message, or the pull request's title and description, drafted
//! from the change by a model through Nexus Gateway. The change is read where the session is (`wb_draft_context`,
//! here or on its server); the call is this computer's, with the person's sign-in, and its body and answer are
//! `nexus_workbench::draft_write`'s, shared with the Nexus app.

use std::future::Future;

use futures::FutureExt;
use futures::future::LocalBoxFuture;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::draft::{self, Draft};
use nexus_workbench::draft_write;
use serde_json::Value;

use super::Workspace;
use crate::gateway::Verb;
use crate::palette::Palette;

/// What "Write it" says on its button, while it writes, and in its tooltip: the Nexus app's `WRITE_IT`.
pub(crate) const LABEL: &str = "Write it";
pub(crate) const BUSY: &str = "Writing…";
pub(crate) const TITLE: &str =
    "Draft it from the changes with a model, through Nexus Gateway. You can edit it before sending.";

/// The draft from a read `context`, with `post` making the Gateway call: its body, then its answer read back.
pub(crate) async fn draft_from<F, P>(
    context: Result<draft::Context, String>,
    asked: Option<String>,
    post: P,
) -> Result<Draft, String>
where
    P: FnOnce(Value) -> F,
    F: Future<Output = Result<Value, String>>,
{
    let context = context?;
    let answer = post(draft_write::body(&context, asked.as_deref())).await?;
    draft_write::answered(context.kind, &answer)
}

impl Workspace {
    /// Draft `kind` for session `id` (its folder, or `repo` inside it): None when its commands are out of reach (the
    /// Nexus app holds this computer's sessions).
    pub(super) fn write_draft(
        &self,
        id: &str,
        repo: Option<String>,
        kind: draft::Kind,
    ) -> Option<LocalBoxFuture<'static, Result<Draft, String>>> {
        let gateway = self.account_gateway()?;
        let runs = self.runs(id)?;
        let session = self.session_by_id(id)?;
        let asked = draft_write::asked(session.via, session.model.as_deref()).map(str::to_string);
        let context = runs.draft_context(id.to_string(), repo, kind);
        Some(
            async move {
                let context = context.await.unwrap_or_else(|_| Err("It was cancelled.".into()));
                draft_from(context, asked, |body| {
                    gateway.request(Verb::Post, "/v1/chat/completions".into(), Some(body))
                })
                .await
            }
            .boxed_local(),
        )
    }
}

/// The quiet button beside a form's submit, as the Nexus app's `wb-draft-write`.
pub(crate) fn write_button(
    id: &'static str,
    drafting: bool,
    enabled: bool,
    reason: Option<&'static str>,
    palette: Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(palette.muted)
        .when(!enabled, |button| button.opacity(0.5))
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
        })
        .tooltip(move |window, cx| Tooltip::new(reason.unwrap_or(TITLE)).build(window, cx))
        .child(if drafting { BUSY } else { LABEL })
}

#[cfg(test)]
mod tests {
    use super::{draft, draft_from};
    use serde_json::{Value, json};

    fn context() -> draft::Context {
        draft::Context {
            kind: draft::Kind::Commit,
            branch: Some("ada/add-two".into()),
            base: None,
            subjects: vec!["feat: one".into()],
            commits: vec![],
            template: None,
            diff: "diff --git a/a.txt b/a.txt".into(),
            cut: false,
            untracked: vec![],
        }
    }

    #[test]
    fn a_draft_posts_the_shared_body_and_reads_its_answer() {
        let mut sent = None;
        let draft =
            futures::executor::block_on(draft_from(Ok(context()), Some("openai/gpt-6".into()), |body: Value| {
                sent = Some(body);
                async { Ok(json!({ "choices": [{ "message": { "content": "Add two" } }] })) }
            }));
        assert_eq!(draft.map(|draft| draft.title), Ok("Add two".to_string()));
        assert_eq!(sent, Some(nexus_workbench::draft::request("openai/gpt-6", &context())));
    }

    #[test]
    fn a_failed_read_makes_no_call_and_gateways_error_shows_as_it_said_it() {
        let called = std::cell::Cell::new(false);
        let draft = futures::executor::block_on(draft_from(Err("Nothing to commit.".into()), None, |_| {
            called.set(true);
            async { Ok(Value::Null) }
        }));
        assert_eq!(draft, Err("Nothing to commit.".to_string()));
        assert!(!called.get());
        let draft = futures::executor::block_on(draft_from(Ok(context()), None, |_| async {
            Err("Your team may not use this model.".to_string())
        }));
        assert_eq!(draft, Err("Your team may not use this model.".to_string()));
    }
}
