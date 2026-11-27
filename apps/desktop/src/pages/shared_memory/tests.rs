use super::{Repository, SharedMemory, Source, Store, TestAppContext, build_view};
use crate::pages::Theme;
use crate::palette::{Palette, ThemeChoice};
use gpui_kit::{Context, Entity, IntoElement, Modifiers, Render, Window, WindowAppearance, point, px};
use nexus_agent_memory::Scope;
use std::process::Command;

#[gpui_kit::test]
fn shared_memory_restores_repository_across_app_restarts(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().expect("fixture directory");
    let checkout = temp.path().join("checkout");
    std::fs::create_dir(&checkout).expect("checkout directory");
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&checkout)
            .status()
            .expect("start git")
            .success()
    );
    let checkout = Repository::checkout(&checkout).expect("checkout root");
    let repo = Repository::discover(&checkout).expect("fixture repository");
    let store = Store::new(temp.path().join("memory"));
    store.set_enabled(true).expect("enable shared memory");
    let note = store
        .remember(repo.scope(), Source::Person, "A repository note.")
        .expect("save repository note");

    let selection_file = temp.path().join("config/shared-memory-repository.json");
    let page = cx.update(|cx| build_view(Ok(store.clone()), Some(selection_file.clone()), cx));
    cx.run_until_parked();
    page.update(cx, |page, cx| page.select_repository(Some(checkout.clone()), cx));
    cx.run_until_parked();
    cx.read(|cx| assert_eq!(page.read(cx).snapshot.entries[0].entry.id, note.id));
    drop(page);

    let mut cx = TestAppContext::single();
    let reopened = cx.update(|cx| build_view(Ok(store.clone()), Some(selection_file.clone()), cx));
    cx.run_until_parked();
    cx.read(|cx| {
        let snapshot = &reopened.read(cx).snapshot;
        assert_eq!(snapshot.checkout.as_ref(), Some(&checkout));
        assert_eq!(snapshot.entries[0].entry.id, note.id);
    });

    reopened.update(&mut cx, |page, cx| page.select_repository(None, cx));
    cx.run_until_parked();
    drop(reopened);

    let cx = TestAppContext::single();
    let personal = cx.update(|cx| build_view(Ok(store), Some(selection_file), cx));
    cx.run_until_parked();
    cx.read(|cx| {
        let snapshot = &personal.read(cx).snapshot;
        assert!(snapshot.checkout.is_none());
        assert!(snapshot.entries.is_empty());
    });
}

struct SettingsFixture(Entity<SharedMemory>);

impl Render for SettingsFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

#[gpui_kit::test]
fn entries_scroll_and_forget_all_requires_confirmation(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().expect("fixture directory");
    let store = Store::new(temp.path().join("memory"));
    store.set_enabled(true).expect("enable shared memory");
    for i in 0..24 {
        store
            .remember(Scope::Personal, Source::Person, &format!("Note {i}"))
            .expect("save note");
    }
    let (fixture, cx) = cx.add_window_view(|_, cx| {
        gpui_kit::init(cx);
        cx.set_global(Theme(Palette::for_choice(ThemeChoice::Dark, WindowAppearance::Dark)));
        SettingsFixture(build_view(Ok(store.clone()), None, cx))
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let track = cx.debug_bounds("memory-entries-scrollbar").expect("visible scrollbar");
    cx.simulate_click(
        point(
            track.origin.x + track.size.width - px(3.),
            track.origin.y + track.size.height * 0.75,
        ),
        Modifiers::default(),
    );
    cx.update(|_, cx| {
        let page = fixture.read(cx).0.read(cx);
        assert!(page.entries_scroll.0.borrow().base_handle.offset().y < px(0.));
    });

    let button = cx.debug_bounds("memory-forget-all").expect("bulk forget button");
    cx.simulate_click(button.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(store.entries(None).expect("notes before confirmation").len(), 24);

    let confirm = cx
        .debug_bounds("memory-confirm-forget-all")
        .expect("confirmation button");
    cx.simulate_click(confirm.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(store.entries(None).expect("notes after confirmation").is_empty());

    cx.update(|window, cx| window.draw(cx).clear(cx));
    let switch = cx.debug_bounds("shared-memory-switch").expect("shared memory switch");
    cx.simulate_click(switch.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(!store.enabled().expect("read switch after a single click"));
    cx.update(|_, cx| assert!(!fixture.read(cx).0.read(cx).confirming_off));
}

#[gpui_kit::test]
fn stored_notes_in_another_repository_still_prompt_for_export(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().expect("fixture directory");
    let store = Store::new(temp.path().join("memory"));
    store.set_enabled(true).expect("enable shared memory");
    store
        .remember(
            Scope::Repository("another-repo".into()),
            Source::Person,
            "Note in another repository",
        )
        .expect("store note");

    let page = cx.update(|cx| build_view(Ok(store.clone()), None, cx));
    cx.run_until_parked();
    cx.read(|cx| assert!(page.read(cx).snapshot.entries.is_empty()));
    page.update(cx, |page, cx| page.toggle(cx));
    cx.run_until_parked();
    assert!(store.enabled().expect("read switch"));
    cx.read(|cx| assert!(page.read(cx).confirming_off));
}
