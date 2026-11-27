//! Settings search draws in the section's own frame: the title stays put when a query starts.
use super::layout::draw;
use super::{BackendProbe, SettingsSection, Workspace, fixture};
use crate::pages::ui;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px};

/// A typed query, as the search field's change handler leaves it.
fn type_query(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, query: &'static str) {
    workspace.update_in(cx, |workspace, window, cx| {
        let shell = &mut workspace.settings.shell;
        shell.query.update(cx, |input, cx| input.set_value(query, window, cx));
        shell.query_text = query.into();
        shell.searching = true;
        shell.selected = 0;
        cx.notify();
    });
    cx.run_until_parked();
    draw(cx);
}

#[gpui_kit::test]
fn search_results_keep_the_sections_title_and_column(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_settings(SettingsSection::Appearance, window, cx)
    });
    cx.run_until_parked();
    draw(cx);
    let section = cx.debug_bounds("settings-page-title").expect("the section's title");
    assert!(cx.debug_bounds("settings-result-0").is_none());

    type_query(&workspace, cx, "ssh");
    let row = cx.debug_bounds("settings-result-0").expect("the first hit");
    let search = cx.debug_bounds("settings-page-title").expect("the search's title");
    assert_eq!(
        search.origin, section.origin,
        "the title must not move when a search starts"
    );
    assert_eq!(
        row.origin.x + px(ui::ROW_BLEED),
        search.origin.x,
        "a hit's text lines up with the title"
    );

    type_query(&workspace, cx, "zzqqxxjj");
    let button = cx
        .debug_bounds("settings-empty-computer")
        .expect("the empty state's button");
    assert!(
        button.size.width < px(ui::SETTINGS_COLUMN / 2.),
        "the button hugs its label"
    );
}
