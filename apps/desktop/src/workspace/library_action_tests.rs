//! Old menu rows must be revalidated against the current grouped lists and the current account.
use crate::gateway::{Account, Gateway};
use crate::library::{Action, Lists};
use crate::workspace::{Auth, View, Workspace, machine_source::MachineSource};
use gpui_kit::{Task, TestAppContext};
use serde_json::json;
use std::sync::Arc;

fn lists(revoked: bool, owned: bool) -> Lists {
    let shared = json!({"artifacts":[
        {"id":"primary","kind":"page","pageId":"source","pageFormat":"md","mine":false,
            "title":"Spec","status":"active","createdAt":"2026-11-06T00:00:00Z"},
        {"id":"other-share","kind":"page","pageId":"source","pageFormat":"md","mine":false,
            "title":"Spec","status":if revoked { "revoked" } else { "active" },"createdAt":"2026-11-05T00:00:00Z"}
    ]});
    let pages = json!({"pages":if owned { json!([{"id":"source","format":"md","title":"Spec"}]) } else { json!([]) }});
    Lists::from_responses(&shared, &json!({}), Some(&pages), None)
}

#[gpui_kit::test]
fn stale_share_menus_recheck_revocation_ownership_and_the_current_role(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
    let (gateway, _) = Gateway::start();
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace.runner = None;
        workspace.auth = Auth::SignedIn;
        workspace.view = View::Library;
        workspace
    });
    workspace.update_in(cx, |this, window, cx| {
        this.account = Account::from_json(&json!({"tenant":{"id":"acme"},"role":"admin"}));
        let original = lists(false, true);
        let stale = original.find("other-share").unwrap().clone();
        assert!(stale.actions(true).contains(&Action::Revoke));
        assert!(stale.actions(true).contains(&Action::Share));
        assert!(
            !original.with.iter().any(|row| row.key == stale.key),
            "the menu belongs to a grouped-away share"
        );

        this.library.note = Some("unchanged".into());
        this.library.lists = Some(Ok(lists(true, true)));
        this.library_action(Action::Revoke, &stale, window, cx);
        assert!(
            this.library.dialog.is_none(),
            "a share revoked since the menu opened cannot be revoked again"
        );
        assert_eq!(this.library.note.as_deref(), Some("unchanged"));

        this.library.lists = Some(Ok(lists(false, false)));
        this.library_action(Action::Share, &stale, window, cx);
        assert!(
            this.library.dialog.is_none(),
            "removed page ownership must block an old Share action"
        );
        assert_eq!(this.library.note.as_deref(), Some("unchanged"));

        this.library.lists = Some(Ok(original));
        this.account.as_mut().unwrap().role = "member".into();
        this.library_action(Action::Revoke, &stale, window, cx);
        assert!(
            this.library.dialog.is_none(),
            "a former admin cannot revoke someone else's share"
        );
        assert_eq!(this.library.note.as_deref(), Some("unchanged"));
    });
}
