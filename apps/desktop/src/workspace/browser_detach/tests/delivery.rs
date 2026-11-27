//! Actual Browser events deliver to frozen source drafts, including preparation and storage races.
use super::*;
use crate::attachments::{Attachment, Incoming, Prepared};
use crate::workspace::drafts::ParkedDraft;

fn screenshot() -> Incoming {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("small real PNG");
    let bytes = Arc::new(bytes.into_inner());
    Incoming {
        name: "original-browser.png".into(),
        media_type: Some("image/png".into()),
        size: bytes.len() as u64,
        source: crate::attachments::Source::Bytes(bytes),
    }
}
fn csv() -> Attachment {
    Attachment {
        artifact: None,
        id: 92_001,
        name: "existing.csv".into(),
        bytes: Arc::new(b"original,attachment\n1,2".to_vec()),
        media_type: None,
        thumbnail: None,
    }
}
fn seed(id: u64, text: &str, cx: &mut VisualTestContext) {
    let target = origin(id, cx).draft;
    cx.cx.update(|cx| {
        assert!(target.text(text, cx));
        assert!(target.attachments(
            Prepared {
                files: vec![csv()],
                ..Default::default()
            },
            cx
        ));
    });
}
fn revision(id: u64, cx: &mut VisualTestContext) -> u64 {
    let target = origin(id, cx).draft;
    cx.cx.update(|cx| target.revision(cx))
}
fn assert_image(draft: &ParkedDraft) {
    assert_eq!(draft.images.len(), 1);
    assert_eq!(draft.images[0].name, "original-browser.png");
    assert_eq!(draft.images[0].media_type, Some("image/png"));
    let decoded = image::load_from_memory(draft.images[0].bytes.as_slice()).expect("actual prepared screenshot");
    assert_eq!((decoded.width(), decoded.height()), (2, 2));
    assert_eq!(draft.files.len(), 1);
    assert_eq!(draft.files[0].bytes.as_slice(), csv().bytes.as_slice());
}
fn retry(id: u64, cx: &mut VisualTestContext) {
    cx.cx.update(|cx| super::super::outputs::retry(id, cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn actual_text_screenshot_and_stop_follow_original_session_after_navigation(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, _page_probe) = crate::web::fixture_page(521);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, _, browser) = detach(&workspace, cx);
    seed(id, "Original draft", cx);
    workspace.update_in(cx, |workspace, window, cx| {
        select(workspace, OTHER, window, cx);
        workspace.sync_draft(window, cx);
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("Keep other session input", window, cx));
        workspace.draft.files = vec![csv()];
        workspace.sync_draft(window, cx);
    });
    let other = workspace.read_with(cx, |workspace, cx| {
        workspace.browser_draft(OTHER, cx).expect("other scoped draft")
    });
    let other_revision = cx.cx.update(|cx| other.revision(cx));
    emit(&browser, BrowserEvent::Text("Picked from original page".into()), cx);
    emit(&browser, BrowserEvent::Screenshot(screenshot()), cx);
    emit(&browser, BrowserEvent::StopAgent, cx);
    let original = snapshot(id, cx);
    assert_eq!(original.text, "Original draft\n\nPicked from original page");
    assert_image(&original);
    cx.cx.update(|cx| {
        assert_eq!(other.snapshot(cx).text, "Keep other session input");
        assert_eq!(other.snapshot(cx).files[0].bytes.as_slice(), csv().bytes.as_slice());
        assert_eq!(
            other.revision(cx),
            other_revision,
            "output must not touch another draft revision"
        );
    });
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.composer.read(cx).value().as_ref(), "Keep other session input");
        assert!(matches!(&workspace.view, View::Code(view) if view.id == OTHER));
    });
    let stops = rpc
        .calls()
        .into_iter()
        .filter(|(name, _)| *name == "wb_interrupt")
        .collect::<Vec<_>>();
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].1["threadId"], THREAD);
}

#[gpui_kit::test]
fn actual_output_and_stop_survive_source_window_close_without_old_workspace_callbacks(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, _page_probe) = crate::web::fixture_page(522);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, _, browser) = detach(&workspace, cx);
    seed(id, "Before closing source", cx);
    workspace.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert!(
        cx.cx.update(|cx| crate::windows::windows(cx).is_empty()),
        "detached presentation is not a source Workspace"
    );
    emit(&browser, BrowserEvent::Text("After source close".into()), cx);
    emit(&browser, BrowserEvent::Screenshot(screenshot()), cx);
    emit(&browser, BrowserEvent::StopAgent, cx);
    let draft = snapshot(id, cx);
    assert_eq!(draft.text, "Before closing source\n\nAfter source close");
    assert_image(&draft);
    let stops = rpc
        .calls()
        .into_iter()
        .filter(|(name, _)| *name == "wb_interrupt")
        .collect::<Vec<_>>();
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].1["threadId"], THREAD);
}

#[gpui_kit::test]
fn same_id_other_store_keeps_output_pending_then_retries_once_after_secondary_close(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let a = SourceStore::new(&probe);
    let b = SourceStore::new(&probe);
    let rpc_a = Rpc::default();
    let rpc_b = Rpc::default();
    let (page, _page_a) = crate::web::fixture_page(523);
    let (first, _, cx) = setup(cx, &probe, &a, &rpc_a, page);
    let (id, _, browser) = detach(&first, cx);
    seed(id, "Existing original draft", cx);
    let (page, _page_b) = crate::web::fixture_page(524);
    let (second, _, cx) = setup(cx, &probe, &b, &rpc_b, page);
    second.update_in(cx, |workspace, window, cx| {
        workspace.sync_draft(window, cx);
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("Existing original draft", window, cx));
        workspace.sync_draft(window, cx);
    });
    let before = revision(id, cx);
    emit(&browser, BrowserEvent::Text("Pending original output".into()), cx);
    emit(&browser, BrowserEvent::Screenshot(screenshot()), cx);
    assert_eq!(pending(id, cx), 2);
    assert_eq!(snapshot(id, cx).text, "Existing original draft");
    assert_eq!(
        revision(id, cx),
        before,
        "collision does not mutate the shared persisted key"
    );
    second.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.composer.read(cx).value().as_ref(), "Existing original draft");
        assert!(workspace.draft.images.is_empty());
    });
    retry(id, cx);
    assert_eq!(pending(id, cx), 2, "retry must not bypass a live source collision");
    second.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    retry(id, cx);
    assert_eq!(pending(id, cx), 0);
    let delivered = snapshot(id, cx);
    assert_eq!(delivered.text, "Existing original draft\n\nPending original output");
    assert_image(&delivered);
    let after = revision(id, cx);
    retry(id, cx);
    assert_eq!(
        revision(id, cx),
        after,
        "an empty retry cannot append the same receipt twice"
    );
    assert_eq!(snapshot(id, cx).images.len(), 1);
}

#[gpui_kit::test]
fn parked_primary_source_still_blocks_same_id_detached_output(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let b = SourceStore::new(&probe);
    let a = SourceStore::new(&probe);
    let rpc_b = Rpc::default();
    let rpc_a = Rpc::default();
    let (page, _page_b) = crate::web::fixture_page(525);
    let (primary, _, cx) = setup(cx, &probe, &b, &rpc_b, page);
    let (page, _page_a) = crate::web::fixture_page(526);
    let (second, _, cx) = setup(cx, &probe, &a, &rpc_a, page);
    let (id, _, browser) = detach(&second, cx);
    seed(id, "Original draft", cx);
    let original = origin(id, cx);
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert!(
        cx.cx
            .update(|cx| crate::windows::browser_output_conflict(&original.key, cx)),
        "retained primary source must remain in collision detection"
    );
    let before = revision(id, cx);
    emit(
        &browser,
        BrowserEvent::Text("Kept until source is unambiguous".into()),
        cx,
    );
    assert_eq!(pending(id, cx), 1);
    retry(id, cx);
    assert_eq!(pending(id, cx), 1);
    assert_eq!(snapshot(id, cx).text, "Original draft");
    assert_eq!(revision(id, cx), before);
}

#[gpui_kit::test]
fn other_detached_source_remains_a_collision_after_its_source_window_closes(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let a = SourceStore::new(&probe);
    let b = SourceStore::new(&probe);
    let rpc_a = Rpc::default();
    let rpc_b = Rpc::default();
    let (page, _page_a) = crate::web::fixture_page(527);
    let (first, _, cx) = setup(cx, &probe, &a, &rpc_a, page);
    let (id, _, browser) = detach(&first, cx);
    seed(id, "Original draft", cx);
    let (page, _page_b) = crate::web::fixture_page(528);
    let (second, _, cx) = setup(cx, &probe, &b, &rpc_b, page);
    let (other_id, _, _other_browser) = detach(&second, cx);
    second.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    cx.cx.update(|cx| {
        assert!(matches!(
            cx.global::<Registry>()
                .id(other_id)
                .expect("other owner stays live")
                .presentation,
            Presentation::Detached { .. }
        ))
    });
    let before = revision(id, cx);
    emit(&browser, BrowserEvent::Text("Pending original output".into()), cx);
    retry(id, cx);
    assert_eq!(pending(id, cx), 1);
    assert_eq!(snapshot(id, cx).text, "Original draft");
    assert_eq!(revision(id, cx), before);
}

#[gpui_kit::test]
fn screenshot_accepted_before_account_aba_cannot_publish_after_preparation(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, _page_probe) = crate::web::fixture_page(529);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, _, browser) = detach(&workspace, cx);
    seed(id, "Original draft", cx);
    let before = revision(id, cx);
    let scopes = cx.cx.update(|cx| crate::workspace::drafts::fixture_scopes(cx));
    browser.update(cx, |_, cx| cx.emit(BrowserEvent::Screenshot(screenshot())));
    cx.cx.update(|cx| {
        assert_eq!(
            cx.global::<Registry>().id(id).expect("accepted preparation").preparing,
            1
        )
    });
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    cx.run_until_parked();
    assert_eq!(snapshot(id, cx).text, "Original draft");
    assert!(snapshot(id, cx).images.is_empty());
    assert_eq!(revision(id, cx), before);
    assert_eq!(
        cx.cx.update(|cx| crate::workspace::drafts::fixture_scopes(cx)),
        scopes,
        "late capture cannot create a replacement identity's scope"
    );
    assert!(rpc.calls().into_iter().all(|(name, _)| name != "wb_interrupt"));
}

#[gpui_kit::test]
fn output_waits_for_real_saved_draft_read_then_merges_text_and_files_once(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, _page_probe) = crate::web::fixture_page(530);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, _, browser) = detach(&workspace, cx);
    let target = origin(id, cx).draft;
    let held = cx.cx.update(|cx| {
        target.fixture_hold_read(
            source.data.join("private-draft-read"),
            ParkedDraft {
                text: "Saved text still on disk".into(),
                files: vec![csv()],
                ..Default::default()
            },
            cx,
        )
    });
    let before = revision(id, cx);
    emit(&browser, BrowserEvent::Text("Output after load".into()), cx);
    emit(&browser, BrowserEvent::Screenshot(screenshot()), cx);
    assert_eq!(pending(id, cx), 2);
    assert_eq!(
        revision(id, cx),
        before,
        "pending read cannot be marked touched by output"
    );
    assert!(snapshot(id, cx).text.is_empty());
    cx.cx.update(|cx| held.finish(cx));
    cx.run_until_parked();
    assert_eq!(pending(id, cx), 0);
    let draft = snapshot(id, cx);
    assert_eq!(draft.text, "Saved text still on disk\n\nOutput after load");
    assert_image(&draft);
    let after = revision(id, cx);
    retry(id, cx);
    assert_eq!(revision(id, cx), after);
    assert_eq!(snapshot(id, cx).images.len(), 1);
}

#[gpui_kit::test]
fn old_browser_event_clones_cannot_deliver_or_stop_after_detach_and_return(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, _page_probe) = crate::web::fixture_page(531);
    let (workspace, old_attached, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, detached_window, old_detached) = detach(&workspace, cx);
    seed(id, "Original draft", cx);
    workspace.update_in(cx, |workspace, window, cx| {
        select(workspace, OTHER, window, cx);
        workspace.sync_draft(window, cx);
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("Other visible draft", window, cx));
        workspace.sync_draft(window, cx);
    });
    let other = workspace.read_with(cx, |workspace, cx| {
        workspace.browser_draft(OTHER, cx).expect("visible other draft")
    });
    let other_revision = cx.cx.update(|cx| other.revision(cx));
    let before = revision(id, cx);
    old_attached.read_with(cx, |browser, _| assert!(browser.is_retired()));
    emit(&old_attached, BrowserEvent::Text("Old attached text".into()), cx);
    emit(&old_attached, BrowserEvent::Screenshot(screenshot()), cx);
    emit(&old_attached, BrowserEvent::StopAgent, cx);
    assert_eq!(revision(id, cx), before);
    assert_eq!(snapshot(id, cx).text, "Original draft");
    assert!(snapshot(id, cx).images.is_empty());
    assert_eq!(pending(id, cx), 0);
    cx.cx.update(|cx| {
        assert_eq!(other.snapshot(cx).text, "Other visible draft");
        assert_eq!(other.revision(cx), other_revision);
    });
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.composer.read(cx).value().as_ref(), "Other visible draft")
    });
    assert!(rpc.calls().into_iter().all(|(name, _)| name != "wb_interrupt"));

    detached_window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("real detached close returns its pages");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| workspace.sync_browser_return(window, cx));
    cx.run_until_parked();
    let fresh = workspace.read_with(cx, |workspace, _| {
        workspace
            .fixture_session_browser(THREAD)
            .expect("fresh returned browser")
    });
    assert_ne!(fresh.entity_id(), old_detached.entity_id());
    old_detached.read_with(cx, |browser, _| assert!(browser.is_retired()));
    let current_text = workspace.read_with(cx, |workspace, cx| workspace.composer.read(cx).value().to_string());
    let before = revision(id, cx);
    emit(&old_detached, BrowserEvent::Text("Old detached text".into()), cx);
    emit(&old_detached, BrowserEvent::Screenshot(screenshot()), cx);
    emit(&old_detached, BrowserEvent::StopAgent, cx);
    assert_eq!(revision(id, cx), before);
    assert_eq!(snapshot(id, cx).text, "Original draft");
    assert!(snapshot(id, cx).images.is_empty());
    assert_eq!(snapshot(id, cx).files.len(), 1);
    assert_eq!(pending(id, cx), 0);
    assert!(rpc.calls().into_iter().all(|(name, _)| name != "wb_interrupt"));
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.composer.read(cx).value().as_ref(), current_text.as_str())
    });
    cx.cx.update(|cx| {
        assert_eq!(other.snapshot(cx).text, "Other visible draft");
        assert_eq!(other.revision(cx), other_revision);
    });
    emit(&fresh, BrowserEvent::Text("Fresh returned text".into()), cx);
    assert_eq!(snapshot(id, cx).text, "Original draft\n\nFresh returned text");
}

#[gpui_kit::test]
fn screenshot_before_submit_cannot_repopulate_a_newer_identical_draft_but_fresh_capture_can(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, _page_probe) = crate::web::fixture_page(532);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let (id, _, browser) = detach(&workspace, cx);
    seed(id, "Same text", cx);
    workspace.update_in(cx, |workspace, window, cx| workspace.sync_draft(window, cx));
    let target = origin(id, cx).draft;
    let old_receipt = cx.cx.update(|cx| target.receipt(cx));
    browser.update(cx, |_, cx| cx.emit(BrowserEvent::Screenshot(screenshot())));
    cx.cx
        .update(|cx| assert_eq!(cx.global::<Registry>().id(id).expect("capture accepted").preparing, 1));
    workspace.update_in(cx, |workspace, window, cx| {
        let submitted = workspace
            .draft_submission(cx)
            .expect("actual synchronized submission receipt");
        workspace.draft_submitted(Some(submitted), window, cx);
        assert!(
            !old_receipt.generation_current(cx),
            "production submit acknowledgement invalidates prior preparation"
        );
        workspace
            .composer
            .update(cx, |input, cx| input.set_value("Same text", window, cx));
        let mut newer = csv();
        newer.id = 92_002;
        newer.name = "newer.csv".into();
        newer.bytes = Arc::new(b"newer,file\n3,4".to_vec());
        workspace.draft.files = vec![newer];
        workspace.sync_draft(window, cx);
    });
    let newer_revision = revision(id, cx);
    cx.run_until_parked();
    let newer = snapshot(id, cx);
    assert_eq!(newer.text, "Same text");
    assert!(
        newer.images.is_empty(),
        "old screenshot cannot revive the sent generation"
    );
    assert_eq!(newer.files.len(), 1);
    assert_eq!(newer.files[0].name, "newer.csv");
    assert_eq!(newer.files[0].bytes.as_slice(), b"newer,file\n3,4");
    assert_eq!(
        revision(id, cx),
        newer_revision,
        "stale completion cannot rewrite the newer identical text"
    );
    assert_eq!(pending(id, cx), 0);
    cx.cx.update(|cx| {
        assert!(
            cx.global::<Registry>().id(id).expect("current owner").problem.is_some(),
            "refused receipt has an explicit recoverable notice"
        )
    });
    emit(&browser, BrowserEvent::Screenshot(screenshot()), cx);
    let fresh = snapshot(id, cx);
    assert_eq!(fresh.text, "Same text");
    assert_eq!(fresh.images.len(), 1, "a fresh capture uses the new generation");
    assert_eq!(fresh.files.len(), 1);
    assert_eq!(fresh.files[0].name, "newer.csv");
    assert_eq!(fresh.files[0].bytes.as_slice(), b"newer,file\n3,4");
    assert!(revision(id, cx) > newer_revision);
}

/// The side pane's "Retry output" runs inside the source workspace's update. The retry reads every app window's
/// workspace (`windows::browser_output_conflict`), this one included, so it waits for the update to end instead of
/// panicking with "cannot read Workspace while it is already being updated".
#[gpui_kit::test]
fn retry_output_from_the_side_pane_runs_after_the_workspace_update(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let rpc = Rpc::default();
    let (page, _page_probe) = crate::web::fixture_page(533);
    let (workspace, _, cx) = setup(cx, &probe, &source, &rpc, page);
    let key = key(&workspace, cx);
    let (id, _, _) = detach(&workspace, cx);
    let problem = |cx: &mut VisualTestContext| {
        cx.cx
            .update(|cx| cx.global::<Registry>().id(id).expect("retained owner").problem.clone())
    };
    cx.cx.update(|cx| {
        cx.global_mut::<Registry>().id_mut(id).expect("retained owner").problem = Some("before retry".into());
    });
    workspace.update(cx, |workspace, cx| workspace.retry_browser_output(&key, id, cx));
    cx.run_until_parked();
    assert_ne!(
        problem(cx).as_deref(),
        Some("before retry"),
        "the retry ran once the update ended"
    );
}
