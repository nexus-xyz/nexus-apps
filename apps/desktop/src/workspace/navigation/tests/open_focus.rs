//! A place the person opens puts the keys in its main field, ready for typing.
use super::{BackendProbe, Workspace, chat, fixture};
use gpui_kit::{Focusable as _, TestAppContext};

fn composer_focused(workspace: &Workspace, window: &gpui_kit::Window, cx: &gpui_kit::App) -> bool {
    workspace.composer.read(cx).focus_handle(cx).is_focused(window)
}

#[gpui_kit::test]
fn a_code_session_opens_in_its_message_box(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_library(window, cx);
        assert!(!composer_focused(workspace, window, cx));
        workspace.open_code("thread-focus", window, cx);
        assert!(
            composer_focused(workspace, window, cx),
            "a Code session opens ready for typing"
        );
    });
}

#[gpui_kit::test]
fn artifacts_opens_in_its_search_and_the_inbox_takes_the_keys(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open(&chat("a"), window, cx);
        workspace.open_library(window, cx);
        assert!(
            workspace.library_search.read(cx).focus_handle(cx).is_focused(window),
            "Artifacts opens in its search"
        );
        workspace.open_inbox(window, cx);
        assert!(
            workspace.focus.is_focused(window),
            "the Inbox takes the keys for its list"
        );
    });
}

#[gpui_kit::test]
fn a_new_worktree_opens_in_its_branch_search(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.choose_worktree(window, cx);
        assert!(workspace.code_form.base.search_focused(window, cx));
    });
}
