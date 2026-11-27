//! Focused fixtures exercise the native page's asynchronous requests, not a second state implementation.
#[path = "skills_test_backend.rs"]
mod backend;
use super::super::{Ctx, Load};
use super::{Selection, SkillsPage, model};
use backend::{BackendFixture, Pending};
use gpui_kit::{
    AnyWindowHandle, AppContext, Context, Entity, IntoElement, Render, ScrollHandle, TestAppContext, Window, div,
};
use serde_json::{Value, json};
use std::{
    rc::Rc,
    sync::{Arc, atomic::Ordering},
};

fn skill(id: &str) -> Value {
    json!({"id":id,"name":id,"description":"Use for fixtures","latestVersion":2,
        "createdAt":"2026-11-01","updatedAt":"2026-11-02"})
}
fn detail(id: &str) -> Value {
    let mut value = skill(id);
    value["versions"] = json!([
        {"version":2,"name":id,"createdAt":"2026-11-02","bytes":20,"contentSha256":"two"},
        {"version":1,"name":id,"createdAt":"2026-11-01","bytes":10,"contentSha256":"one"}]);
    value
}
fn file(id: &str, version: u64) -> Value {
    json!({"skillId":id,"version":version,"content":format!("{id} version {version}")})
}
fn answer(pending: Pending, value: nexus_client::Result<Value>, cx: &mut TestAppContext) {
    let consumed = pending.consumed.clone();
    pending.answer.send(value).unwrap();
    cx.run_until_parked();
    assert!(
        consumed.load(Ordering::SeqCst),
        "the exact stale request must have completed"
    );
}
struct Empty;
impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
fn fixture(cx: &mut TestAppContext) -> (Entity<SkillsPage>, Arc<BackendFixture>, AnyWindowHandle) {
    cx.update(|cx| gpui_kit::init(cx));
    let gateway = BackendFixture::new();
    let mut context = None;
    let window = cx.add_window(|window, _| {
        context = Some(Ctx {
            device: crate::pages::DeviceCtx {
                local_workspace: false,
                settings_fields: crate::settings_fields::SettingsFields::new(window, ScrollHandle::new()),
                gateway_url: "http://gateway.invalid".into(),
                navigate: Rc::new(|_, _, _| {}),
                navigate_settings: Rc::new(|_, _, _| {}),
                switch_gateway: Rc::new(|_, _, _| Ok(())),
                settings_saved: Rc::new(|_, _| {}),
                restart_for_update: Rc::new(|_| Ok(())),
                restart_for_service: Rc::new(|_| Ok(())),
                code: Rc::new(|_| super::super::CodeAccess::default()),
                new_code_session: Rc::new(|_, _| {}),
            },
            gateway: crate::workspace::gateway_access::AccountGateway::fixture(gateway.clone()),
            org: "example".into(),
            org_name: "Example".into(),
            email: "a@example.test".into(),
            new_cloud_session: Rc::new(|_, _| {}),
            team: Rc::new(|_| Default::default()),
            watch: Rc::new(|_, _, _| {}),
            fixer: crate::setup::Fixer::inert(),
        });
        Empty
    });
    let page = cx.new(|cx| SkillsPage::new(context.unwrap(), cx));
    cx.run_until_parked();
    (page, gateway, window.into())
}
fn settle_lists(page: &Entity<SkillsPage>, gateway: &BackendFixture, cx: &mut TestAppContext) {
    answer(gateway.take("/v1/skills"), Ok(json!({"skills":[skill("mine")]})), cx);
    answer(gateway.take("/v1/skill-library"), Ok(json!({"skills":[]})), cx);
    assert!(page.read_with(cx, |page, _| page.owns("mine")));
}

#[gpui_kit::test]
fn skills_selection_and_version_replies_cannot_replace_a_later_selection(cx: &mut TestAppContext) {
    let (page, gateway, _) = fixture(cx);
    settle_lists(&page, &gateway, cx);
    page.update(cx, |page, cx| page.select("mine".into(), cx));
    let old = gateway.take("/v1/skills/mine");
    page.update(cx, |page, cx| page.select("other".into(), cx));
    answer(gateway.take("/v1/skills/other"), Ok(detail("other")), cx);
    answer(gateway.take("/v1/skills/other/versions/2"), Ok(file("other", 2)), cx);
    answer(old, Ok(detail("mine")), cx);
    assert!(page.read_with(cx, |page, _| matches!(&page.selection,
        Some(Selection { id, file: Load::Ready(file), .. }) if id == "other" && file.version == 2)));
    page.update(cx, |page, cx| page.load_version(1, cx));
    let old_version = gateway.take("/v1/skills/other/versions/1");
    page.update(cx, |page, cx| page.load_version(2, cx));
    answer(gateway.take("/v1/skills/other/versions/2"), Ok(file("other", 2)), cx);
    answer(old_version, Ok(file("other", 1)), cx);
    assert!(page.read_with(cx, |page, _| matches!(&page.selection,
        Some(Selection { file: Load::Ready(file), .. }) if file.version == 2)));
}

#[gpui_kit::test]
fn skills_account_change_discards_pending_lists_and_detail_and_refuses_writes(cx: &mut TestAppContext) {
    let (page, gateway, _) = fixture(cx);
    let own = gateway.take("/v1/skills");
    let library = gateway.take("/v1/skill-library");
    page.update(cx, |page, cx| page.select("mine".into(), cx));
    let detail_request = gateway.take("/v1/skills/mine");
    *gateway.marker.lock().unwrap() = Some("account-b".into());
    answer(own, Ok(json!({"skills":[skill("mine")]})), cx);
    answer(library, Ok(json!({"skills":[]})), cx);
    answer(detail_request, Ok(detail("mine")), cx);
    page.update(cx, |page, cx| {
        page.confirming = true;
        page.remove(cx);
        page.save(cx);
        page.load(cx);
    });
    assert!(page.read_with(cx, |page, _| !page.current()
        && !page.owns("mine")
        && matches!(page.own, Load::Loading)));
    assert!(gateway.pending.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn skills_save_serializes_mutations_keeps_conflicted_draft_and_pins_base_version(cx: &mut TestAppContext) {
    let (page, gateway, window) = fixture(cx);
    settle_lists(&page, &gateway, cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |page, cx| {
            page.edit(model::SKELETON.into(), Some(("mine".into(), 2)), window, cx);
            page.editor.as_ref().unwrap().text.update(cx, |text, cx| {
                text.set_value(format!("{}Changed", model::SKELETON), window, cx)
            });
            page.save(cx);
            page.save(cx);
            page.confirming = true;
            page.remove(cx);
        })
    })
    .unwrap();
    let request = gateway.take("/v1/skills/mine/versions");
    assert!(matches!(request.verb, crate::gateway::Verb::Post));
    assert_eq!(request.body.as_ref().unwrap()["baseVersion"], 2);
    assert!(
        gateway.pending.lock().unwrap().is_empty(),
        "the second write must never start"
    );
    let error = nexus_client::error::core_error(
        409,
        &json!({"error":{"code":"skill_version_conflict","message":"A newer version exists"}}),
    );
    answer(request, Err(error.into()), cx);
    assert!(page.read_with(cx, |page, _| !page.busy
        && page.editor.is_some()
        && page.problem.as_deref().unwrap().contains("reload the latest version")));
    page.update(cx, |page, cx| page.cancel_edit(cx));
    let mut latest = detail("mine");
    latest["latestVersion"] = json!(3);
    latest["versions"].as_array_mut().unwrap().push(json!({
        "version":3,"name":"mine","createdAt":"2026-11-03","bytes":30,"contentSha256":"three"
    }));
    answer(gateway.take("/v1/skills/mine"), Ok(latest), cx);
    answer(gateway.take("/v1/skills/mine/versions/3"), Ok(file("mine", 3)), cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |page, cx| page.edit_selected(window, cx))
    })
    .unwrap();
    assert!(
        page.read_with(cx, |page, _| page.editor.as_ref().is_some_and(|editor| editor.target
            == Some(("mine".into(), 3))
            && editor.initial == "mine version 3"))
    );
}

#[gpui_kit::test]
fn skills_library_veto_refuses_mutations_and_missing_library_does_not_fake_an_empty_list(cx: &mut TestAppContext) {
    let (page, gateway, _) = fixture(cx);
    answer(gateway.take("/v1/skills"), Ok(json!({"skills":[skill("mine")]})), cx);
    assert!(
        !page.read_with(cx, |page, _| page.owns("mine")),
        "wait for library discovery"
    );
    answer(
        gateway.take("/v1/skill-library"),
        Ok(json!({"skills":[{
        "id":"mine","name":"mine","description":"Approved","version":1,"scope":"team",
        "teamId":"team-a","approvedAt":"2026-11-02"}]})),
        cx,
    );
    page.update(cx, |page, cx| {
        page.selection = Some(Selection {
            id: "mine".into(),
            detail: Load::Loading,
            file: Load::Loading,
            version: None,
        });
        page.confirming = true;
        page.remove(cx);
    });
    assert!(!page.read_with(cx, |page, _| page.owns("mine")));
    assert!(gateway.pending.lock().unwrap().is_empty());
    page.update(cx, |page, cx| page.load(cx));
    answer(gateway.take("/v1/skills"), Ok(json!({"skills":[]})), cx);
    let error = nexus_client::error::core_error(404, &json!({"error":{"message":"Not found"}}));
    answer(gateway.take("/v1/skill-library"), Err(error.into()), cx);
    assert!(page.read_with(cx, |page, _| matches!(page.library, Load::Ready(None))));
}

#[test]
fn skills_file_roundtrip_preserves_bom_crlf_and_rejects_oversized_or_non_utf8_input() {
    let source = "\u{feff}---\r\nname: mine\r\n---\r\nBody\r\n";
    assert_eq!(model::decode(source.as_bytes().to_vec()).unwrap(), source);
    assert_eq!(
        model::edited("---\nname: mine\n---\nNew\n", source).unwrap(),
        "\u{feff}---\r\nname: mine\r\n---\r\nNew\r\n"
    );
    assert!(model::edited("New\n", "One\r\nTwo\n").is_err());
    assert!(model::decode(vec![255]).is_err());
    assert!(model::decode(vec![b'a'; model::MAX_BYTES + 1]).is_err());
    assert!(model::validate(&"é".repeat(model::MAX_BYTES / 2 + 1), "").is_err());
    assert!(model::validate("same", "same").is_err());
    assert!(model::validate("   ", "").is_err());
    assert_eq!(model::path("id/with?query"), "/v1/skills/id%2Fwith%3Fquery");
}

#[gpui_kit::test]
fn skills_import_create_and_confirmed_delete_use_existing_routes_and_exact_file_bytes(cx: &mut TestAppContext) {
    let (page, gateway, window) = fixture(cx);
    settle_lists(&page, &gateway, cx);
    let source = "\u{feff}---\r\nname: mine\r\ndescription: Fixture\r\n---\r\nBody\r\n";
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |page, cx| {
            page.edit(source.into(), None, window, cx);
            page.save(cx);
            page.save(cx);
        })
    })
    .unwrap();
    let create = gateway.take("/v1/skills");
    assert!(matches!(create.verb, crate::gateway::Verb::Post));
    assert_eq!(create.body.as_ref().unwrap()["content"], source);
    assert!(create.body.as_ref().unwrap().get("baseVersion").is_none());
    assert!(gateway.pending.lock().unwrap().is_empty());
    answer(create, Ok(json!({"skill":skill("mine")})), cx);
    settle_lists(&page, &gateway, cx);
    answer(gateway.take("/v1/skills/mine"), Ok(detail("mine")), cx);
    answer(gateway.take("/v1/skills/mine/versions/2"), Ok(file("mine", 2)), cx);
    page.update(cx, |page, cx| page.remove(cx));
    assert!(
        gateway.pending.lock().unwrap().is_empty(),
        "a first press cannot delete"
    );
    page.update(cx, |page, cx| {
        page.confirming = true;
        page.remove(cx);
        page.remove(cx);
    });
    let remove = gateway.take("/v1/skills/mine");
    assert!(matches!(remove.verb, crate::gateway::Verb::Delete));
    assert!(gateway.pending.lock().unwrap().is_empty());
    answer(remove, Ok(json!({"deleted":true,"id":"mine","versions":2})), cx);
    answer(gateway.take("/v1/skills"), Ok(json!({"skills":[]})), cx);
    answer(gateway.take("/v1/skill-library"), Ok(json!({"skills":[]})), cx);
    assert!(page.read_with(cx, |page, _| page.selection.is_none() && !page.busy && !page.confirming));
}

#[test]
fn skills_malformed_lists_do_not_grant_authority_and_hidden_text_is_visible() {
    assert!(model::own(json!({"owner":"current-person"})).is_err());
    assert!(!model::writable("unknown", None, None));
    assert_eq!(
        model::visible("a\u{200b}b\u{202e}\u{e0061}\r\n"),
        "a[U+200B]b[U+202E][U+E0061]\r\n"
    );
}

#[path = "skills_promotions_tests.rs"]
mod promotion_tests;
