//! Promotion fixtures complete the exact deferred requests and assert their real production callbacks ran.
use super::super::{
    Scope, SkillsPage,
    promotions::model::{self, Action, Viewer},
};
use super::backend::BackendFixture;
use super::{answer, detail, file, fixture, skill};
use crate::pages::Load;
use gpui_kit::{
    AnyWindowHandle, AppContext, Context, Entity, InteractiveElement, IntoElement, Modifiers, ParentElement, Render,
    StatefulInteractiveElement, Styled, TestAppContext, VisualTestContext, Window, div, px, size,
};
use serde_json::{Value, json};
fn promotion(id: &str, state: &str, requester: &str) -> Value {
    json!({"id":id,"state":state,"scope":"org","name":"mine","description":"fixture instructions",
        "contentSha256":"abc","bytes":20,"sourceSkillId":"mine","sourceVersion":2,
        "requestedBy":requester,"requestedVia":"web","requestedAt":"2026-11-12",
        "sourceWrittenVia":"cli","sourceWrittenAt":"2026-11-11","selfApproved":false})
}
fn snapshot(id: &str, state: &str, requester: &str) -> Value {
    let mut p = promotion(id, state, requester);
    p["content"] = json!("---\nname: mine\ndescription: fixture\n---\n# Do not execute <script>\nsecret\u{200b}text\n");
    p["previous"] = json!({"name":"mine","libraryVersion":1,"content":"old instructions\r\n"});
    p
}
fn me(user: &str, permissions: &[&str]) -> Value {
    json!({"tenant":{"id":"org-a","kind":"company"},"user":{"id":user},"role":"owner",
        "permissions":permissions,"teams":[{"id":"team-child","slug":"child","name":"Child","role":"member"}]})
}
fn auth(g: &BackendFixture, cx: &mut TestAppContext, user: &str, permissions: &[&str]) {
    answer(g.take("/v1/tenants/me"), Ok(me(user, permissions)), cx);
    answer(
        g.take("/v1/teams"),
        Ok(
            json!({"teams":[{"id":"team-child","slug":"child","name":"Child","parentId":"team-root"},
        {"id":"team-root","slug":"root","name":"Root"}]}),
        ),
        cx,
    );
}
fn own(page: &Entity<SkillsPage>, g: &BackendFixture, cx: &mut TestAppContext) {
    answer(g.take("/v1/skills"), Ok(json!({"skills":[skill("mine")]})), cx);
    answer(g.take("/v1/skill-library"), Ok(json!({"skills":[]})), cx);
    page.update(cx, |p, cx| p.select("mine".into(), cx));
    answer(g.take("/v1/skills/mine"), Ok(detail("mine")), cx);
    answer(g.take("/v1/skills/mine/versions/2"), Ok(file("mine", 2)), cx);
}
fn review(page: &Entity<SkillsPage>, g: &BackendFixture, cx: &mut TestAppContext, user: &str, permissions: &[&str]) {
    own(page, g, cx);
    page.update(cx, |p, cx| {
        p.set_promotion_target(
            super::super::Target::Requests {
                of: Some("mine".into()),
                state: Some("held".into()),
            },
            cx,
        )
    });
    auth(g, cx, user, permissions);
    answer(
        g.take(&model::query(true, "held", None)),
        Ok(json!({"promotions":[promotion("spr-one","held","author")]})),
        cx,
    );
    page.update(cx, |p, cx| p.open_promotion("spr-one".into(), cx));
    answer(
        g.take("/v1/skill-promotions/spr-one"),
        Ok(snapshot("spr-one", "held", "author")),
        cx,
    );
}
// A successful decision refreshes these three scoped reads in addition to its selected snapshot.
// Complete the exact requests through answer(), including its consumed acknowledgement; never drop them at teardown.
fn answer_decision_lists(g: &BackendFixture, cx: &mut TestAppContext) {
    answer(g.take("/v1/skills"), Ok(json!({"skills":[skill("mine")]})), cx);
    answer(g.take("/v1/skill-library"), Ok(json!({"skills":[]})), cx);
    answer(
        g.take(&model::query(true, "held", None)),
        Ok(json!({"promotions":[]})),
        cx,
    );
}
fn answer_decision_refresh(g: &BackendFixture, cx: &mut TestAppContext, state: &str, requester: &str) {
    answer_decision_lists(g, cx);
    answer(
        g.take("/v1/skill-promotions/spr-one"),
        Ok(snapshot("spr-one", state, requester)),
        cx,
    );
    assert!(
        g.pending.lock().unwrap().is_empty(),
        "every mutation-triggered read was answered"
    );
}
fn fail(status: u16, code: &str) -> nexus_client::Error {
    nexus_client::error::core_error(
        status,
        &json!({"error":{"message":"fixture refusal","code":code,"nexus":{"promotionId":"spr-existing"}}}),
    )
    .into()
}
fn begin(page: &Entity<SkillsPage>, window: AnyWindowHandle, cx: &mut TestAppContext, action: Action) {
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| p.begin_decision(action, window, cx))
    })
    .unwrap();
}
#[test]
fn skills_promotions_permissions_self_review_and_known_team_lineage_match_web_policy() {
    let p: model::Promotion = crate::pages::skills::model::parse(promotion("spr-one", "held", "author")).unwrap();
    let role_only = Viewer::parse(me("reviewer", &[])).unwrap();
    assert!(!p.allows(Action::Approve, &role_only, None));
    let self_approver = Viewer::parse(me("author", &["skills.promote", "skills.approve_org"])).unwrap();
    assert!(!p.allows(Action::Approve, &self_approver, None));
    assert!(p.allows(Action::Withdraw, &self_approver, None));
    let mut lead = me("lead", &["skills.promote", "skills.approve_team"]);
    lead["teams"] = json!([{"id":"team-root","slug":"root","name":"Root","role":"lead"}]);
    let lead = Viewer::parse(lead).unwrap();
    let mut team = p.clone();
    team.scope = "team".into();
    team.team_id = Some("team-child".into());
    let directory=crate::pages::skills::model::parse::<Vec<model::Team>>(json!([
        {"id":"team-child","slug":"child","name":"Child","parentId":"team-root"},{"id":"team-root","slug":"root","name":"Root"},
        {"id":"team-other","slug":"other","name":"Other"}])).unwrap();
    assert!(team.allows(Action::Approve, &lead, Some(&directory)));
    team.team_id = Some("team-other".into());
    assert!(!team.allows(Action::Approve, &lead, Some(&directory)));
    assert!(
        team.allows(Action::Approve, &lead, None),
        "unknown lineage leaves final authority to Gateway, as web does"
    );
    let mut personal = me("reviewer", &["skills.promote", "skills.approve_org"]);
    personal["tenant"]["kind"] = json!("personal");
    assert!(!p.allows(Action::Approve, &Viewer::parse(personal).unwrap(), None));
}
#[gpui_kit::test]
fn skills_promotions_request_pins_older_version_and_member_team_without_touching_personal_data(
    cx: &mut TestAppContext,
) {
    let (page, g, window) = fixture(cx);
    own(&page, &g, cx);
    page.update(cx, |p, cx| p.load_version(1, cx));
    answer(g.take("/v1/skills/mine/versions/1"), Ok(file("mine", 1)), cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| p.begin_promotion(window, cx))
    })
    .unwrap();
    auth(&g, cx, "author", &["skills.promote"]);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            let draft = p.promotions.request.as_mut().unwrap();
            draft.team = Some("team-child".into());
            draft
                .note
                .update(cx, |text, cx| text.set_value("  Share this version  ", window, cx));
            p.request_promotion(cx);
            p.request_promotion(cx);
            p.remove(cx);
            p.select("other".into(), cx);
        })
    })
    .unwrap();
    let write = g.take("/v1/skills/mine/versions/1/promotions");
    assert_eq!(
        write.body,
        Some(json!({"scope":"team","team":"child","note":"Share this version"}))
    );
    assert!(matches!(write.verb, crate::gateway::Verb::Post));
    assert!(g.pending.lock().unwrap().is_empty());
    let mut response = promotion("spr-new", "held", "author");
    response["scope"] = json!("team");
    response["teamId"] = json!("team-child");
    response["sourceVersion"] = json!(1);
    answer(write, Ok(response), cx);
    assert!(page.read_with(cx, |p, _| p.scope == Scope::Requests
        && !p.busy
        && p.promotions.request.is_none()
        && p.owns("mine")));
    assert!(page.read_with(
        cx,
        |p, _| matches!(&p.library,Load::Ready(Some(rows)) if rows.is_empty())
    ));
    assert!(
        g.pending
            .lock()
            .unwrap()
            .iter()
            .all(|r| matches!(r.verb, crate::gateway::Verb::Get))
    );
    // The server accepted v1 for this team; complete both actual reads started by that success callback.
    let mut opened = snapshot("spr-new", "held", "author");
    opened["scope"] = json!("team");
    opened["teamId"] = json!("team-child");
    opened["sourceVersion"] = json!(1);
    answer(
        g.take(&model::query(true, "held", None)),
        Ok(json!({"promotions":[]})),
        cx,
    );
    answer(g.take("/v1/skill-promotions/spr-new"), Ok(opened), cx);
    assert!(page.read_with(cx, |p, _| matches!(&p.promotions.selected,Some((id,Load::Ready(detail)))
        if id=="spr-new" && detail.promotion.source_version==1 && detail.promotion.team_id.as_deref()==Some("team-child"))));
    assert!(g.pending.lock().unwrap().is_empty());
}
#[gpui_kit::test]
fn skills_promotions_duplicate_conflict_retains_form_and_links_the_server_named_request(cx: &mut TestAppContext) {
    let (page, g, window) = fixture(cx);
    own(&page, &g, cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| p.begin_promotion(window, cx))
    })
    .unwrap();
    auth(&g, cx, "author", &["skills.promote"]);
    page.update(cx, |p, cx| p.request_promotion(cx));
    answer(
        g.take("/v1/skills/mine/versions/2/promotions"),
        Err(fail(409, "promotion_exists")),
        cx,
    );
    assert!(page.read_with(cx, |p, _| !p.busy
        && p.promotions.request.is_some()
        && p.promotions.existing.as_deref() == Some("spr-existing")
        && p.problem.as_deref().unwrap().contains("promotion_exists")));
    page.update(cx, |p, cx| p.open_promotion("spr-existing".into(), cx));
    answer(
        g.take("/v1/skill-promotions/spr-existing"),
        Ok(snapshot("spr-existing", "held", "author")),
        cx,
    );
    assert!(page.read_with(cx, |p, _| !p.promotion_allowed(Action::Approve)
        && p.promotion_allowed(Action::Withdraw)));
}
#[gpui_kit::test]
fn skills_promotions_stale_version_and_account_callbacks_cannot_retarget_a_request(cx: &mut TestAppContext) {
    let (page, g, window) = fixture(cx);
    own(&page, &g, cx);
    page.update(cx, |p, cx| p.load_version(1, cx));
    let old = g.take("/v1/skills/mine/versions/1");
    page.update(cx, |p, cx| p.load_version(2, cx));
    answer(g.take("/v1/skills/mine/versions/2"), Ok(file("mine", 2)), cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| p.begin_promotion(window, cx))
    })
    .unwrap();
    auth(&g, cx, "author", &["skills.promote"]);
    answer(old, Ok(file("mine", 1)), cx);
    assert_eq!(
        page.read_with(cx, |p, _| p.promotions.request.as_ref().unwrap().version),
        2
    );
    page.update(cx, |p, cx| p.request_promotion(cx));
    let write = g.take("/v1/skills/mine/versions/2/promotions");
    *g.marker.lock().unwrap() = Some("account-b".into());
    answer(write, Ok(promotion("spr-new", "held", "author")), cx);
    assert!(page.read_with(cx, |p, _| p.busy
        && p.promotions.request.is_some()
        && p.scope == Scope::Own));
    assert!(g.pending.lock().unwrap().is_empty());
}
#[gpui_kit::test]
fn skills_promotions_approval_has_no_canceled_refusal_reason_and_refreshes_scoped_lists(cx: &mut TestAppContext) {
    let (page, g, window) = fixture(cx);
    review(&page, &g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    begin(&page, window, cx, Action::Refuse);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.promotions
                .step
                .as_ref()
                .unwrap()
                .reason
                .update(cx, |text, cx| text.set_value("old refusal", window, cx));
            p.promotions.step = None;
            p.begin_decision(Action::Approve, window, cx);
            p.decide_promotion(cx);
            p.decide_promotion(cx);
        })
    })
    .unwrap();
    let write = g.take("/v1/skill-promotions/spr-one/decision");
    assert_eq!(write.body, Some(json!({"decision":"approved"})));
    assert!(g.pending.lock().unwrap().is_empty());
    let mut approved = promotion("spr-one", "approved", "author");
    approved["libraryVersion"] = json!(1);
    answer(write, Ok(approved), cx);
    assert!(page.read_with(cx, |p, _| !p.busy
        && p.promotions.step.is_none()
        && p.selection.is_none()));
    answer(g.take("/v1/skills"), Ok(json!({"skills":[skill("mine")]})), cx);
    answer(g.take("/v1/skill-library"), Ok(json!({"skills":[]})), cx);
    assert!(page.read_with(cx, |p, _| p.owns("mine")));
    answer(
        g.take(&model::query(true, "held", None)),
        Ok(json!({"promotions":[]})),
        cx,
    );
    answer(
        g.take("/v1/skill-promotions/spr-one"),
        Ok(snapshot("spr-one", "approved", "author")),
        cx,
    );
    assert!(page.read_with(cx, |p, _| p.promotion_allowed(Action::WithdrawAll)
        && !p.promotion_allowed(Action::Approve)));
    assert!(g.pending.lock().unwrap().is_empty());
}
#[gpui_kit::test]
fn skills_promotions_refusal_validates_reason_and_keeps_server_rejection_distinct_from_success(
    cx: &mut TestAppContext,
) {
    let (page, g, window) = fixture(cx);
    review(&page, &g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    begin(&page, window, cx, Action::Refuse);
    page.update(cx, |p, cx| p.decide_promotion(cx));
    assert!(g.pending.lock().unwrap().is_empty());
    assert!(page.read_with(cx, |p, _| p.problem.as_deref().unwrap().contains("Say why")));
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.promotions
                .step
                .as_ref()
                .unwrap()
                .reason
                .update(cx, |text, cx| text.set_value("  Unsafe instructions  ", window, cx));
            p.decide_promotion(cx);
        })
    })
    .unwrap();
    let write = g.take("/v1/skill-promotions/spr-one/decision");
    assert_eq!(
        write.body,
        Some(json!({"decision":"refused","reason":"Unsafe instructions"}))
    );
    answer(write, Err(fail(403, "role_required")), cx);
    assert!(page.read_with(cx, |p, _| !p.busy
        && p.promotions.selected.is_none()
        && p.problem.as_deref().unwrap().contains("role_required")));
    page.update(cx, |p, cx| p.promotion_context(cx));
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    answer(
        g.take(&model::query(true, "held", None)),
        Ok(json!({"promotions":[]})),
        cx,
    );
    page.update(cx, |p, cx| p.open_promotion("spr-one".into(), cx));
    answer(
        g.take("/v1/skill-promotions/spr-one"),
        Ok(snapshot("spr-one", "held", "author")),
        cx,
    );
    begin(&page, window, cx, Action::Refuse);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.promotions
                .step
                .as_ref()
                .unwrap()
                .reason
                .update(cx, |text, cx| text.set_value("Fix the instructions", window, cx));
            p.decide_promotion(cx);
        })
    })
    .unwrap();
    answer(
        g.take("/v1/skill-promotions/spr-one/decision"),
        Ok(promotion("spr-one", "refused", "author")),
        cx,
    );
    assert!(page.read_with(cx, |p, _| p.notice.as_deref() == Some("Promotion refused.")));
    answer_decision_refresh(&g, cx, "refused", "author");
    assert!(page.read_with(
        cx,
        |p, _| matches!(&p.promotions.selected,Some((_,Load::Ready(detail))) if detail.promotion.state=="refused")
    ));
}
#[gpui_kit::test]
fn skills_promotions_decision_conflict_reloads_current_state_before_another_action(cx: &mut TestAppContext) {
    let (page, g, window) = fixture(cx);
    review(&page, &g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    begin(&page, window, cx, Action::Approve);
    page.update(cx, |p, cx| p.decide_promotion(cx));
    answer(
        g.take("/v1/skill-promotions/spr-one/decision"),
        Err(fail(409, "promotion_decided")),
        cx,
    );
    assert!(page.read_with(cx, |p, _| p.promotions.step.is_none()
        && !p.promotion_allowed(Action::Approve)));
    answer(
        g.take("/v1/skill-promotions/spr-one"),
        Ok(snapshot("spr-one", "approved", "author")),
        cx,
    );
    assert!(page.read_with(cx, |p, _| !p.promotion_allowed(Action::Approve)
        && p.promotion_allowed(Action::WithdrawAll)));
    // Finish the conflict's list refresh before another mutation invalidates that request generation.
    answer(
        g.take(&model::query(true, "held", None)),
        Ok(json!({"promotions":[]})),
        cx,
    );
    assert!(g.pending.lock().unwrap().is_empty());
    begin(&page, window, cx, Action::WithdrawAll);
    page.update(cx, |p, cx| p.decide_promotion(cx));
    let write = g.take("/v1/skill-promotions/spr-one/withdraw");
    assert_eq!(write.body, Some(json!({"allVersions":true})));
    answer(write, Ok(promotion("spr-one", "withdrawn", "author")), cx);
    answer_decision_refresh(&g, cx, "withdrawn", "author");
    assert!(page.read_with(cx, |p, _| !p.promotion_allowed(Action::WithdrawAll)
        && !p.promotion_allowed(Action::Approve)));
}
#[gpui_kit::test]
fn skills_promotions_old_list_filter_detail_and_scope_replies_are_rejected(cx: &mut TestAppContext) {
    let (page, g, _) = fixture(cx);
    own(&page, &g, cx);
    page.update(cx, |p, cx| {
        p.set_promotion_target(
            super::super::Target::Requests {
                of: Some("mine".into()),
                state: Some("held".into()),
            },
            cx,
        )
    });
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    let old = g.take(&model::query(true, "held", None));
    page.update(cx, |p, cx| {
        p.promotions.filter = "approved";
        p.promotion_list(None, cx);
    });
    answer(
        g.take(&model::query(true, "approved", None)),
        Ok(json!({"promotions":[promotion("spr-new","approved","author")],"nextCursor":"opaque+/= x"})),
        cx,
    );
    answer(
        old,
        Ok(json!({"promotions":[promotion("spr-old","held","author")]})),
        cx,
    );
    assert!(page.read_with(
        cx,
        |p, _| matches!(&p.promotions.list,Load::Ready(list) if list.promotions[0].id=="spr-new")
    ));
    page.update(cx, |p, cx| p.promotion_list(Some("opaque+/= x".into()), cx));
    let page_two = g.take(&model::query(true, "approved", Some("opaque+/= x")));
    page.update(cx, |p, cx| p.open_promotion("spr-old".into(), cx));
    let old = g.take("/v1/skill-promotions/spr-old");
    page.update(cx, |p, cx| p.open_promotion("spr-new".into(), cx));
    answer(
        g.take("/v1/skill-promotions/spr-new"),
        Ok(snapshot("spr-new", "approved", "author")),
        cx,
    );
    answer(old, Ok(snapshot("spr-old", "held", "author")), cx);
    assert!(page.read_with(
        cx,
        |p, _| matches!(&p.promotions.selected,Some((id,_)) if id=="spr-new")
    ));
    page.update(cx, |p, cx| p.change_scope(Scope::Library, cx));
    answer(page_two, Ok(json!({"promotions":[]})), cx);
    assert!(page.read_with(cx, |p, _| p.scope == Scope::Library
        && p.promotions.selected.is_none()
        && matches!(p.promotions.list, Load::Loading)));
}
#[test]
fn skills_promotions_diff_and_limits_preserve_hidden_text_and_do_not_claim_reviewed_scroll() {
    let diff = model::diff("same\nold\r\n", "same\nnew\u{200b}");
    assert!(diff.contains("-old\r\n+new\u{200b}"));
    assert!(diff.contains("No newline at end of file"));
    assert!(crate::pages::skills::model::visible(&diff).contains("[U+200B]"));
    assert!(model::text(&"é".repeat(500), false).is_ok());
    assert!(model::text(&"é".repeat(501), false).is_err());
    assert_eq!(
        model::decision(Action::Approve, "discarded refusal").unwrap(),
        json!({"decision":"approved"})
    );
    assert_eq!(
        model::query(false, "all", Some("+/= x")),
        "/v1/skill-promotions?limit=50&cursor=%2B%2F%3D%20x"
    );
}

#[gpui_kit::test]
fn skills_promotions_old_authority_and_decision_callbacks_cannot_restore_an_old_account(cx: &mut TestAppContext) {
    let (page, g, window) = fixture(cx);
    own(&page, &g, cx);
    page.update(cx, |p, cx| {
        p.set_promotion_target(
            super::super::Target::Requests {
                of: Some("mine".into()),
                state: Some("held".into()),
            },
            cx,
        )
    });
    let old_me = g.take("/v1/tenants/me");
    let old_teams = g.take("/v1/teams");
    page.update(cx, |p, cx| {
        p.change_scope(Scope::Own, cx);
        p.set_promotion_target(
            super::super::Target::Requests {
                of: Some("mine".into()),
                state: Some("held".into()),
            },
            cx,
        );
    });
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    answer(old_me, Ok(me("unrelated", &[])), cx);
    answer(old_teams, Ok(json!({"teams":[]})), cx);
    assert_eq!(
        page.read_with(cx, |p, _| p.promotion_viewer().unwrap().user.clone()),
        Some("reviewer".into())
    );
    answer(
        g.take(&model::query(true, "held", None)),
        Ok(json!({"promotions":[]})),
        cx,
    );
    page.update(cx, |p, cx| p.open_promotion("spr-one".into(), cx));
    answer(
        g.take("/v1/skill-promotions/spr-one"),
        Ok(snapshot("spr-one", "held", "author")),
        cx,
    );
    begin(&page, window, cx, Action::Approve);
    page.update(cx, |p, cx| p.decide_promotion(cx));
    let write = g.take("/v1/skill-promotions/spr-one/decision");
    *g.marker.lock().unwrap() = Some("account-b".into());
    answer(write, Ok(promotion("spr-one", "approved", "author")), cx);
    assert!(page.read_with(cx, |p, _| p.busy && p.promotions.step.is_some() && p.notice.is_none()));
    assert!(g.pending.lock().unwrap().is_empty());
}
#[gpui_kit::test]
fn skills_promotions_access_denial_clears_loaded_snapshot_and_rejects_a_late_detail(cx: &mut TestAppContext) {
    let (page, g, _) = fixture(cx);
    review(&page, &g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    page.update(cx, |p, cx| p.open_promotion("spr-next".into(), cx));
    let late = g.take("/v1/skill-promotions/spr-next");
    page.update(cx, |p, cx| p.promotion_list(None, cx));
    answer(
        g.take(&model::query(true, "held", None)),
        Err(fail(403, "role_required")),
        cx,
    );
    answer(late, Ok(snapshot("spr-next", "held", "author")), cx);
    assert!(page.read_with(cx, |p, _| p.promotions.selected.is_none()
        && matches!(p.promotions.authority, Load::Failed(_))));
}
#[gpui_kit::test]
fn skills_promotions_requester_can_withdraw_held_snapshot_but_cannot_approve_or_withdraw_approved_library(
    cx: &mut TestAppContext,
) {
    let (page, g, window) = fixture(cx);
    review(&page, &g, cx, "author", &["skills.promote"]);
    begin(&page, window, cx, Action::Approve);
    assert!(page.read_with(cx, |p, _| p.promotions.step.is_none()));
    begin(&page, window, cx, Action::Withdraw);
    page.update(cx, |p, cx| p.decide_promotion(cx));
    let write = g.take("/v1/skill-promotions/spr-one/withdraw");
    assert_eq!(write.body, Some(json!({})));
    answer(write, Ok(promotion("spr-one", "withdrawn", "author")), cx);
    answer(
        g.take("/v1/skill-promotions/spr-one"),
        Ok(snapshot("spr-one", "approved", "author")),
        cx,
    );
    assert!(page.read_with(cx, |p, _| !p.promotion_allowed(Action::Withdraw)
        && !p.promotion_allowed(Action::WithdrawAll)));
    answer_decision_lists(&g, cx);
    assert!(g.pending.lock().unwrap().is_empty());
}
struct PromotionRoot(Entity<SkillsPage>);
impl Render for PromotionRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("promotion-test-scroll")
            .size_full()
            .overflow_y_scroll()
            .debug_selector(|| "promotion-test-viewport".into())
            .child(self.0.clone())
    }
}
fn draw_promotions(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}
fn click_promotion(cx: &mut VisualTestContext, selector: &'static str) {
    draw_promotions(cx);
    let control = cx.debug_bounds(selector).expect("native control is drawn");
    let viewport = cx
        .debug_bounds("promotion-test-viewport")
        .expect("native scroll viewport");
    assert!(control.size.height >= px(24.) && control.size.width > px(20.));
    assert!(
        control.top() >= viewport.top() && control.bottom() <= viewport.bottom(),
        "control must be reachable in the viewport: {selector}"
    );
    cx.simulate_click(control.center(), Modifiers::none());
    draw_promotions(cx);
}
#[gpui_kit::test]
fn skills_promotions_native_renderer_draws_snapshot_diff_and_confirmation(cx: &mut TestAppContext) {
    let (page, g, _) = fixture(cx);
    review(&page, &g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    // Populate the real own-list read with a look-alike from another personal skill.
    page.update(cx, |page, cx| page.load(cx));
    let mut lookalike = skill("other");
    lookalike["name"] = json!("m1ne");
    answer(
        g.take("/v1/skills"),
        Ok(json!({"skills":[skill("mine"),lookalike]})),
        cx,
    );
    answer(g.take("/v1/skill-library"), Ok(json!({"skills":[]})), cx);
    cx.update(|cx| {
        cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::System,
            gpui_kit::WindowAppearance::Light,
        )))
    });
    let reader = cx.add_window(|_, _| PromotionRoot(page.clone()));
    let mut visual = VisualTestContext::from_window(reader.into(), cx);
    visual.simulate_resize(size(px(1200.), px(2000.)));
    draw_promotions(&mut visual);
    assert!(visual.debug_bounds("promotion-visible-snapshot").is_none());
    assert!(visual.debug_bounds("promotion-visible-diff").is_none());
    assert!(visual.debug_bounds("promotion-confirm-native").is_none());
    let warning = visual
        .debug_bounds("promotion-name-match-0")
        .expect("the confusable-name warning is drawn");
    assert!(warning.size.height > px(12.));
    click_promotion(&mut visual, "promotion-content-toggle");
    click_promotion(&mut visual, "promotion-diff-toggle");
    let card = visual.debug_bounds("promotion-review-card").unwrap();
    for selector in ["promotion-visible-snapshot", "promotion-visible-diff"] {
        let text = visual
            .debug_bounds(selector)
            .expect("requested plain review text is rendered");
        assert!(
            text.size.height > px(24.) && text.size.width > px(100.),
            "the text has a readable layout"
        );
        assert!(
            text.top() >= card.top() && text.bottom() <= card.bottom(),
            "the text remains inside its review card"
        );
    }
    click_promotion(&mut visual, "promotion-action-Approve");
    let heading = visual
        .debug_bounds("promotion-confirmation-heading")
        .expect("the native confirmation is visible");
    let confirm = visual
        .debug_bounds("promotion-confirm-native")
        .expect("the confirmation control is drawn");
    assert!(
        confirm.top() >= heading.bottom(),
        "the action follows its confirmation text"
    );
    assert!(
        g.pending.lock().unwrap().is_empty(),
        "opening the confirmation sends no decision"
    );
    click_promotion(&mut visual, "promotion-confirm-native");
    let write = g.take("/v1/skill-promotions/spr-one/decision");
    assert!(matches!(write.verb, crate::gateway::Verb::Post));
    assert_eq!(
        write.body,
        Some(json!({"decision":"approved"})),
        "the actual click reaches the production decision handler"
    );
    assert!(visual.debug_bounds("promotion-saving-native").is_some());
    assert!(visual.debug_bounds("promotion-confirm-native").is_none());
    answer(write, Ok(promotion("spr-one", "approved", "author")), cx);
    answer(
        g.take("/v1/skill-promotions/spr-one"),
        Ok(snapshot("spr-one", "approved", "author")),
        cx,
    );
    draw_promotions(&mut visual);
    assert!(
        visual.debug_bounds("promotion-action-Approve").is_none(),
        "Gateway's decided state removes approval"
    );
    assert!(visual.debug_bounds("promotion-action-WithdrawAll").is_some());
    answer_decision_lists(&g, cx);
    draw_promotions(&mut visual);
    assert!(visual.debug_bounds("promotion-action-WithdrawAll").is_some());
    assert!(g.pending.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn skills_promotions_mismatched_source_version_response_never_confirms_a_different_snapshot(cx: &mut TestAppContext) {
    let (page, g, window) = fixture(cx);
    review(&page, &g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    begin(&page, window, cx, Action::Approve);
    page.update(cx, |p, cx| p.decide_promotion(cx));
    let mut wrong = promotion("spr-one", "approved", "author");
    wrong["sourceVersion"] = json!(3);
    answer(g.take("/v1/skill-promotions/spr-one/decision"), Ok(wrong), cx);
    assert!(page.read_with(cx, |p, _| !p.busy
        && p.promotions.step.is_some()
        && p.notice.is_none()
        && p.problem.as_deref().unwrap().contains("did not confirm")));
    assert!(g.pending.lock().unwrap().is_empty());
}
#[gpui_kit::test]
fn skills_promotions_library_scope_and_owner_label_never_grant_request_authority(cx: &mut TestAppContext) {
    let (page, g, window) = fixture(cx);
    own(&page, &g, cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| p.begin_promotion(window, cx))
    })
    .unwrap();
    auth(&g, cx, "author", &[]);
    page.update(cx, |p, cx| p.request_promotion(cx));
    assert!(g.pending.lock().unwrap().is_empty());
    page.update(cx, |p, _| {
        p.promotions.request = None;
        p.library = Load::Ready(Some(
            crate::pages::skills::model::library(json!({"skills":[{"id":"mine","name":"mine","description":"library",
            "version":2,"scope":"org","approvedAt":"now"}]}))
            .unwrap(),
        ));
    });
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| p.begin_promotion(window, cx))
    })
    .unwrap();
    assert!(page.read_with(cx, |p, _| p.promotions.request.is_none()));
    assert!(g.pending.lock().unwrap().is_empty());
}

#[test]
fn skills_promotions_confusable_skeleton_matches_web_unicode_and_font_pairs() {
    for (a, b) in [
        ("code-review", "codereview"),
        ("Code Review", "code--review"),
        ("he11o", "hello"),
        ("hei1o", "hello"),
        ("f00", "foo"),
        ("rnail", "mail"),
        ("vvork", "work"),
        ("ｃｏｄｅ", "code"),
        ("𝐜𝐨𝐝𝐞", "code"),
        ("ﬀ0", "ffo"),
        ("he¹¹o", "hello"),
        ("É", "e\u{301}"),
        ("co\u{feff}de", "code"),
        ("co\u{a0}de", "code"),
        ("co_ .de", "code"),
    ] {
        assert_eq!(model::name_skeleton(a), model::name_skeleton(b), "{a} and {b}");
    }
    assert_eq!(model::name_skeleton("---"), "");
    assert_ne!(model::name_skeleton("code-reveiw"), model::name_skeleton("code-review"));
    assert_ne!(
        model::name_skeleton("co\u{85}de"),
        model::name_skeleton("code"),
        "JS whitespace excludes NEL"
    );
}
#[test]
fn skills_promotions_name_warning_order_exclusions_and_places_match_web() {
    let mut p: model::Promotion = crate::pages::skills::model::parse(promotion("spr", "held", "author")).unwrap();
    p.name = "code-review".into();
    p.library_skill_id = Some("library-update".into());
    let library=crate::pages::skills::model::library(json!({"skills":[
        {"id":"same-scope","name":"code-review","description":"d","version":1,"scope":"org","approvedAt":"now"},
        {"id":"look","name":"codereview","description":"d","version":1,"scope":"org","approvedAt":"now"},
        {"id":"team-copy","name":"code-review","description":"d","version":1,"scope":"team","teamId":"team-child","approvedAt":"now"},
        {"id":"library-update","name":"codereview","description":"d","version":1,"scope":"team","teamId":"team-child","approvedAt":"now"},
        {"id":"duplicate","name":"codereview","description":"d","version":1,"scope":"org","approvedAt":"now"}
    ]})).unwrap();
    let mut source = skill("mine");
    source["name"] = json!("code-review");
    let mut same = skill("same");
    same["name"] = json!("code-review");
    let mut look = skill("look");
    look["name"] = json!("c0de-review");
    let own = crate::pages::skills::model::own(json!({"skills":[source,same,look]})).unwrap();
    let teams = crate::pages::skills::model::parse::<Vec<model::Team>>(
        json!([{"id":"team-child","slug":"child","name":"Child"}]),
    )
    .unwrap();
    let found = model::name_matches(&p, &library, &own, Some(&teams));
    let entries: Vec<_> = found
        .iter()
        .map(|m| (m.name.as_str(), m.place.as_str(), m.same))
        .collect();
    assert_eq!(
        entries,
        vec![
            ("code-review", "in the library of team Child", true),
            ("code-review", "in your skills", true),
            ("codereview", "in the organization's library", false),
            ("c0de-review", "in your skills", false)
        ]
    );
    let missing = model::name_matches(&p, &library, &[], None);
    assert_eq!(missing[0].place, "in a team's library");
    let mut unnamed = teams.clone();
    unnamed[0].name.clear();
    assert_eq!(
        model::name_matches(&p, &library, &[], Some(&unnamed))[0].place,
        "in a team's library"
    );
    p.scope = "team".into();
    p.team_id = Some("team-child".into());
    let found = model::name_matches(&p, &library, &[], Some(&teams));
    assert!(
        found
            .iter()
            .any(|m| m.same && m.place == "in the organization's library")
    );
    assert!(
        !found
            .iter()
            .any(|m| m.same && m.place == "in the library of team Child")
    );
    p.name = "---".into();
    assert!(model::name_matches(&p, &library, &own, Some(&teams)).is_empty());
}

#[test]
fn skills_promotions_ingress_routes_decode_once_and_match_web_query_fallbacks() {
    use super::super::{Target, routes};
    for (path, id) in [
        ("/skills/promotions/spr%2Fa%3Fb", "spr/a?b"),
        ("skills/promotions/spr%252Fa", "spr%2Fa"),
        ("/skills/promotions/é+%25", "é+%"),
        ("/skills/promotions/spr-one?ignored=yes#read", "spr-one"),
    ] {
        assert_eq!(Target::parse(path), Some(Target::Detail(id.into())), "{path}");
        assert_eq!(
            Target::parse(&Target::Detail(id.into()).web_path()),
            Some(Target::Detail(id.into()))
        );
        assert_eq!(
            model::path(id),
            format!("/v1/skill-promotions/{}", nexus_client::url::enc(id))
        );
    }
    for path in [
        "/skills",
        "/skills?view=library",
        "/skills/promotions/",
        "/skills/promotions/a/b",
        "/skills/promotions/%",
        "/skills/promotions/%GG",
        "/skills/promotions/%FF",
        "/skills?view=skills&view=requests",
        "/other/skills/promotions/id",
    ] {
        assert!(Target::parse(path).is_none(), "{path}");
    }
    let target = Target::parse("/skills?view=requests&of=mine&of=reach&state=refused&other=x").unwrap();
    assert_eq!(Target::parse(&target.web_path()), Some(target.clone()));
    assert_eq!(
        target,
        Target::Requests {
            of: Some("mine".into()),
            state: Some("refused".into())
        }
    );
    for (approver, of, state, expected) in [
        (true, None, None, (false, "held")),
        (false, None, None, (true, "all")),
        (true, Some("mine"), None, (true, "all")),
        (false, Some("reach"), Some("approved"), (true, "approved")),
        (true, Some("unknown"), Some("invalid"), (false, "held")),
        (false, None, Some("invalid"), (true, "all")),
        (true, Some("reach"), Some("all"), (false, "all")),
    ] {
        assert_eq!(routes::filters(approver, of, state), expected);
    }
}
#[gpui_kit::test]
fn skills_promotions_ingress_initial_filters_wait_for_authority_and_preserve_user_edits(cx: &mut TestAppContext) {
    for (permissions, mine, state) in [
        (vec!["skills.promote"], true, "all"),
        (vec!["skills.promote", "skills.approve_org"], false, "held"),
    ] {
        let (page, g, _) = fixture(cx);
        own(&page, &g, cx);
        page.update(cx, |p, cx| p.change_scope(Scope::Requests, cx));
        assert_eq!(
            g.pending.lock().unwrap().len(),
            2,
            "only authority and directory are read first"
        );
        auth(&g, cx, "reviewer", &permissions);
        answer(
            g.take(&model::query(mine, state, None)),
            Ok(json!({"promotions":[]})),
            cx,
        );
        page.update(cx, |p, cx| p.choose_promotion_filters(None, Some("refused"), cx));
        answer(
            g.take(&model::query(mine, "refused", None)),
            Ok(json!({"promotions":[]})),
            cx,
        );
        page.update(cx, |p, cx| p.promotion_context(cx));
        auth(&g, cx, "reviewer", &permissions);
        answer(
            g.take(&model::query(mine, "refused", None)),
            Ok(json!({"promotions":[]})),
            cx,
        );
        assert!(page.read_with(cx, |p, _| p.promotions.filter == "refused" && p.promotions.mine == mine));
    }
}
#[gpui_kit::test]
fn skills_promotions_ingress_detail_opens_after_authority_without_waiting_for_lists(cx: &mut TestAppContext) {
    use super::super::Target;
    let (page, g, _) = fixture(cx);
    assert!(page.update(cx, |p, cx| p.set_promotion_target(Target::Detail("spr/a?b".into()), cx)));
    assert!(page.read_with(cx, |p, _| p.promotions.selected.is_none()));
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    let request = g.take("/v1/skill-promotions/spr%2Fa%3Fb");
    assert_eq!(
        g.pending.lock().unwrap().len(),
        2,
        "only unrelated own/library reads remain; no default list is required"
    );
    answer(request, Ok(snapshot("spr/a?b", "approved", "author")), cx);
    assert!(page.read_with(cx,|p,_|p.scope==Scope::Requests&&p.promotions.detail_target.is_some()
        &&matches!(&p.promotions.selected,Some((id,Load::Ready(detail))) if id=="spr/a?b"&&detail.promotion.state=="approved")));
    answer(g.take("/v1/skills"), Ok(json!({"skills":[]})), cx);
    answer(g.take("/v1/skill-library"), Ok(json!({"skills":[]})), cx);
    assert!(page.read_with(
        cx,
        |p, _| matches!(&p.promotions.selected,Some((id,_)) if id=="spr/a?b")
    ));
}
#[gpui_kit::test]
fn skills_promotions_ingress_new_target_and_tab_navigation_invalidate_pending_authority(cx: &mut TestAppContext) {
    use super::super::Target;
    let (page, g, _) = fixture(cx);
    own(&page, &g, cx);
    page.update(cx, |p, cx| p.set_promotion_target(Target::Detail("spr-old".into()), cx));
    let old_me = g.take("/v1/tenants/me");
    let old_teams = g.take("/v1/teams");
    page.update(cx, |p, cx| p.set_promotion_target(Target::Detail("spr-new".into()), cx));
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    let detail = g.take("/v1/skill-promotions/spr-new");
    answer(old_me, Ok(me("reviewer", &["skills.promote"])), cx);
    answer(old_teams, Ok(json!({"teams":[]})), cx);
    assert!(g.pending.lock().unwrap().is_empty());
    page.update(cx, |p, cx| p.change_scope(Scope::Library, cx));
    answer(detail, Ok(snapshot("spr-new", "approved", "author")), cx);
    assert!(page.read_with(cx, |p, _| p.scope == Scope::Library
        && p.promotions.pending.is_none()
        && p.promotions.selected.is_none()));
    page.update(cx, |p, cx| {
        p.set_promotion_target(Target::Detail("spr-abandoned".into()), cx)
    });
    page.update(cx, |p, cx| p.change_scope(Scope::Own, cx));
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    assert!(g.pending.lock().unwrap().is_empty());
    assert!(page.read_with(cx, |p, _| p.scope == Scope::Own && p.promotions.selected.is_none()));
}
#[gpui_kit::test]
fn skills_promotions_ingress_account_switch_and_denial_never_materialize_pending_detail(cx: &mut TestAppContext) {
    use super::super::Target;
    let (page, g, _) = fixture(cx);
    own(&page, &g, cx);
    page.update(cx, |p, cx| {
        p.set_promotion_target(Target::Detail("spr-denied".into()), cx)
    });
    answer(g.take("/v1/tenants/me"), Err(fail(403, "role_required")), cx);
    assert!(page.read_with(
        cx,
        |p, _| matches!(&p.promotions.pending,Some(Target::Detail(id)) if id=="spr-denied")
            && p.promotions.selected.is_none()
    ));
    let old_teams = g.take("/v1/teams");
    assert!(
        g.pending.lock().unwrap().is_empty(),
        "denial never waits for directory data"
    );
    page.update(cx, |p, cx| p.promotion_context(cx));
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    answer(
        g.take("/v1/skill-promotions/spr-denied"),
        Ok(snapshot("spr-denied", "approved", "author")),
        cx,
    );
    answer(old_teams, Ok(json!({"teams":[]})), cx);
    assert!(page.read_with(
        cx,
        |p, _| matches!(&p.promotions.selected,Some((id,Load::Ready(_))) if id=="spr-denied")
    ));

    page.update(cx, |p, cx| {
        p.set_promotion_target(Target::Detail("spr-old-account".into()), cx)
    });
    *g.marker.lock().unwrap() = Some("account-b".into());
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    assert!(g.pending.lock().unwrap().is_empty());
    assert!(page.read_with(cx, |p, _| !p.current() && p.promotions.selected.is_none()));
}
#[gpui_kit::test]
fn skills_promotions_ingress_cannot_replace_an_editor_or_a_real_decision(cx: &mut TestAppContext) {
    use super::super::Target;
    let (page, g, _) = fixture(cx);
    own(&page, &g, cx);
    cx.update(|cx| {
        cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::System,
            gpui_kit::WindowAppearance::Light,
        )))
    });
    let reader = cx.add_window(|_, _| PromotionRoot(page.clone()));
    let mut visual = VisualTestContext::from_window(reader.into(), cx);
    visual.simulate_resize(size(px(1200.), px(2000.)));
    click_promotion(&mut visual, "skill-delete-native");
    assert!(visual.debug_bounds("skill-delete-confirmation-native").is_some());
    assert!(!page.update(cx, |p, cx| {
        p.set_promotion_target(Target::Detail("spr-link".into()), cx)
    }));
    draw_promotions(&mut visual);
    assert!(
        visual.debug_bounds("skill-delete-confirmation-native").is_some(),
        "a link preserves the native Delete confirmation"
    );
    assert!(page.read_with(cx, |p, _| p.confirming
        && p.scope == Scope::Own
        && matches!(&p.selection,Some(selection) if selection.id=="mine")));
    assert!(g.pending.lock().unwrap().is_empty());
    let (page, g, window) = fixture(cx);
    own(&page, &g, cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| p.edit("draft".into(), None, window, cx))
    })
    .unwrap();
    assert!(!page.update(cx, |p, cx| p.set_promotion_target(Target::Detail("spr".into()), cx)));
    assert!(page.read_with(cx, |p, _| p.editor.is_some() && p.scope == Scope::Own));
    assert!(g.pending.lock().unwrap().is_empty());
    let (page, g, window) = fixture(cx);
    review(&page, &g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    begin(&page, window, cx, Action::Approve);
    page.update(cx, |p, cx| p.decide_promotion(cx));
    let write = g.take("/v1/skill-promotions/spr-one/decision");
    assert!(!page.update(cx, |p, cx| {
        p.set_promotion_target(Target::Detail("spr-other".into()), cx)
    }));
    assert!(g.pending.lock().unwrap().is_empty());
    answer(write, Ok(promotion("spr-one", "approved", "author")), cx);
    answer(
        g.take("/v1/skill-promotions/spr-one"),
        Ok(snapshot("spr-one", "approved", "author")),
        cx,
    );
    assert!(page.read_with(
        cx,
        |p, _| matches!(&p.promotions.selected,Some((id,_)) if id=="spr-one")
    ));
    answer_decision_lists(&g, cx);
    assert!(g.pending.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn skills_promotions_ingress_late_target_text_cannot_replace_a_new_route_or_account(cx: &mut TestAppContext) {
    use super::super::Target;
    let (page, g, _) = fixture(cx);
    own(&page, &g, cx);
    page.update(cx, |p, cx| p.set_promotion_target(Target::Detail("spr-old".into()), cx));
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    let old = g.take("/v1/skill-promotions/spr-old");
    page.update(cx, |p, cx| p.set_promotion_target(Target::Detail("spr-new".into()), cx));
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    answer(
        g.take("/v1/skill-promotions/spr-new"),
        Ok(snapshot("spr-new", "approved", "author")),
        cx,
    );
    answer(old, Ok(snapshot("spr-old", "held", "author")), cx);
    assert!(page.read_with(cx,|p,_|matches!(&p.promotions.selected,Some((id,Load::Ready(d))) if id=="spr-new"&&d.promotion.state=="approved")));
    page.update(cx, |p, cx| {
        p.set_promotion_target(Target::Detail("spr-account".into()), cx)
    });
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    let old = g.take("/v1/skill-promotions/spr-account");
    *g.marker.lock().unwrap() = Some("account-b".into());
    answer(old, Ok(snapshot("spr-account", "approved", "author")), cx);
    assert!(page.read_with(cx, |p, _| !p.current()
        && matches!(&p.promotions.selected, Some((_, Load::Loading)))));
    assert!(g.pending.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn skills_promotions_ingress_fresh_builder_accepts_a_changed_marker_with_the_same_identity(cx: &mut TestAppContext) {
    use super::super::Target;
    let (old, g, window) = fixture(cx);
    own(&old, &g, cx);
    let ctx = old.read_with(cx, |p, _| p.ctx.clone());
    *g.marker.lock().unwrap() = Some("account-b".into());
    assert!(!old.read_with(cx, |p, _| p.current()));
    let view = cx
        .update_window(window, |_, window, cx| super::super::build(ctx, window, cx).unwrap())
        .unwrap();
    let page = view.downcast::<SkillsPage>().ok().unwrap();
    assert!(page.read_with(cx, |p, _| p.current()));
    assert!(page.update(cx, |p, cx| {
        p.set_promotion_target(Target::Detail("spr-current".into()), cx)
    }));
    auth(&g, cx, "reviewer", &["skills.promote"]);
    answer(
        g.take("/v1/skill-promotions/spr-current"),
        Ok(snapshot("spr-current", "approved", "author")),
        cx,
    );
    answer(g.take("/v1/skills"), Ok(json!({"skills":[]})), cx);
    answer(g.take("/v1/skill-library"), Ok(json!({"skills":[]})), cx);
    assert!(page.read_with(
        cx,
        |p, _| matches!(&p.promotions.selected,Some((id,Load::Ready(_))) if id=="spr-current")
    ));
    assert!(g.pending.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn skills_promotions_authority_downgrade_narrows_reach_after_delayed_refresh_and_preserves_state_and_route(
    cx: &mut TestAppContext,
) {
    use super::super::Target;
    let (page, g, _) = fixture(cx);
    own(&page, &g, cx);
    page.update(cx, |p, cx| p.change_scope(Scope::Requests, cx));
    auth(&g, cx, "reviewer", &["skills.promote", "skills.approve_org"]);
    answer(
        g.take(&model::query(false, "held", None)),
        Ok(json!({"promotions":[]})),
        cx,
    );
    page.update(cx, |p, cx| p.choose_promotion_filters(None, Some("approved"), cx));
    answer(
        g.take(&model::query(false, "approved", None)),
        Ok(json!({"promotions":[]})),
        cx,
    );
    page.update(cx, |p, cx| p.promotion_context(cx));
    let me_call = g.take("/v1/tenants/me");
    let teams = g.take("/v1/teams");
    assert!(
        g.pending.lock().unwrap().is_empty(),
        "no list is issued before refreshed authority resolves"
    );
    assert!(
        page.read_with(cx, |p, _| matches!(p.promotions.authority, Load::Loading)
            && !p.promotions.mine)
    );
    answer(me_call, Ok(me("reviewer", &["skills.promote"])), cx);
    let list = g.take(&model::query(true, "approved", None));
    assert!(page.read_with(cx, |p, _| p.promotions.mine
        && p.promotions.filter == "approved"
        && !p.promotion_allowed(Action::Approve)));
    answer(
        list,
        Ok(json!({"promotions":[promotion("spr-own","approved","reviewer")]})),
        cx,
    );
    assert!(page.read_with(
        cx,
        |p, _| matches!(&p.promotions.list,Load::Ready(list) if list.promotions.len()==1)
    ));
    answer(teams, Ok(json!({"teams":[]})), cx);
    // An explicit detail link still wins over retained filters after the same authority shrink.
    page.update(cx, |p, cx| {
        p.set_promotion_target(Target::Detail("spr-direct".into()), cx)
    });
    answer(g.take("/v1/tenants/me"), Ok(me("reviewer", &["skills.promote"])), cx);
    let detail = g.take("/v1/skill-promotions/spr-direct");
    answer(detail, Ok(snapshot("spr-direct", "approved", "reviewer")), cx);
    answer(g.take("/v1/teams"), Ok(json!({"teams":[]})), cx);
    assert!(page.read_with(cx, |p, _| p.promotions.mine
        && p.promotions.filter == "approved"
        && matches!(&p.promotions.selected,Some((id,Load::Ready(_))) if id=="spr-direct")));
    assert!(g.pending.lock().unwrap().is_empty());
}
