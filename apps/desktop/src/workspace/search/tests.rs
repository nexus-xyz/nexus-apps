use super::{PER_GROUP, PageFocus, Target, focus_for, score, shortcut_rows};
use crate::pages::PageId;
use crate::workspace::settings_registry::Visibility;
use crate::workspace::{SettingsSection, View};

fn settings_rows(query: &str) -> Vec<super::Row> {
    super::settings_rows(query, &Visibility::all())
}

#[test]
fn a_setting_is_a_row_that_opens_its_page() {
    // `sources.test.ts`: "lists every setting a query names, opening its page at the control".
    let theme = &settings_rows("dark mode")[0];
    assert_eq!(theme.group.as_ref(), "Settings");
    assert_eq!(theme.title.as_ref(), "Theme");
    assert_eq!(theme.sub.as_deref(), Some("Preferences"));
    assert_eq!(theme.target, Target::SettingField(SettingsSection::Appearance, "theme"));
    let keys = &settings_rows("api key")[0];
    // API keys is a nav row of its own, so its breadcrumb is the row alone: no sub.
    assert_eq!(keys.sub.as_deref(), None);
    assert_eq!(keys.target, Target::Setting(SettingsSection::ApiKeys));
    let address = &settings_rows("gateway address")[0];
    assert_eq!(
        address.target,
        Target::SettingField(SettingsSection::Page(PageId::Gateways), "gateway-address")
    );
    // Data controls open the native General page at their specific control.
    let retention = &settings_rows("chat retention")[0];
    assert_eq!(
        retention.target,
        Target::SettingField(SettingsSection::Page(PageId::General), "retention-days")
    );
}

#[test]
fn the_settings_group_keeps_a_short_list_of_what_the_account_may_open() {
    assert!(settings_rows("gateway").len() <= PER_GROUP);
    assert!(settings_rows("").is_empty());
    assert!(super::settings_rows("BYOK", &Visibility::default()).is_empty());
}

#[test]
fn closing_the_palette_focuses_the_page_that_shows() {
    assert_eq!(focus_for(&View::Chat), PageFocus::Composer);
    assert_eq!(focus_for(&View::NewCode), PageFocus::Composer);
    assert_eq!(focus_for(&View::Inbox), PageFocus::Workspace);
    assert_eq!(
        focus_for(&View::Settings(SettingsSection::Appearance)),
        PageFocus::Workspace
    );
    assert_eq!(focus_for(&View::Models), PageFocus::Own);
}

#[test]
fn a_snooze_preset_reads_as_the_nexus_app_words_it() {
    assert_eq!(super::snooze_title("For 1 hour"), "Snooze for 1 hour");
    assert_eq!(super::snooze_title("Until next week"), "Snooze until next week");
}

#[test]
fn scores_rank_exact_then_prefix_then_inside() {
    assert_eq!(score("Wiki", "wiki"), Some(1.));
    assert!(score("Wikipedia", "wiki") > score("My wiki", "wiki"));
    assert!(score("My wiki", "wiki") > score("mywiki", "wiki"));
    assert_eq!(score("Usage", "zzz"), None);
}

#[test]
fn quit_is_the_last_row_of_the_shortcuts_sheet() {
    let rows = shortcut_rows();
    let last = rows
        .iter()
        .rev()
        .find(|row| row.group.as_ref() == "Keyboard shortcuts")
        .expect("the sheet lists the app's shortcuts");
    assert_eq!(last.title.as_ref(), "Quit");
}

#[test]
fn the_sheet_lists_the_session_keys_with_the_jumps_on_one_row() {
    let rows = shortcut_rows();
    let keys_of = |title: &str| {
        rows.iter()
            .filter(|row| row.title.as_ref() == title)
            .map(|row| row.keys.clone().unwrap_or_default().to_string())
            .collect::<Vec<_>>()
    };
    let jump = keys_of("Jump to the 1st to 9th session in the sidebar");
    assert_eq!(jump.len(), 1);
    assert!(jump[0].contains("1–9"));
    for title in [
        "Stop the agent's turn",
        "Next model",
        "Archive or restore the session",
        "Delete the session",
    ] {
        assert_eq!(keys_of(title).len(), 1, "{title}");
    }
    if !cfg!(target_os = "macos") {
        assert_eq!(keys_of("Delete the session"), ["Ctrl Backspace"]);
        assert_eq!(keys_of("Workbench: Browser"), ["Ctrl Alt 3"]);
        assert_eq!(keys_of("Workbench: Terminal"), ["Ctrl Alt 4"]);
    }
}
