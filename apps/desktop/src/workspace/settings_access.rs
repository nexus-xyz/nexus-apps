//! Which Settings sections require an account, and how to display them in the local workspace.

use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::settings_registry::{Dest, Visibility};
use super::{SettingsSection, Workspace, settings_registry};
use crate::pages::{PageId, ui};
use crate::palette::Palette;

impl Workspace {
    pub(super) fn resolve_settings_section(&self, path: &str) -> Option<SettingsSection> {
        settings_registry::resolve(path).or_else(|| {
            if !self.is_local() {
                return None;
            }
            let page = path.split(['?', '#']).next()?.trim_matches('/');
            match page {
                "usage" | "settings/usage" => Some(SettingsSection::Page(PageId::Usage)),
                _ => settings_registry::resolve_local_gateway_link(path),
            }
        })
    }
}

pub(super) fn render_locked_field(
    section: SettingsSection,
    anchor: &'static str,
    visibility: &Visibility,
    palette: Palette,
) -> Option<Div> {
    locked_fields(section, visibility)
        .find(|field| field.anchor == anchor)
        .map(|field| ui::requires_sign_in_row(field.label, field.description, palette))
}

fn locked_fields(section: SettingsSection, visibility: &Visibility) -> impl Iterator<Item = settings_registry::Field> {
    let destinations = [
        Some(section.dest()),
        (section.dest() == Dest::General).then_some(Dest::Data),
    ];
    destinations
        .into_iter()
        .flatten()
        .flat_map(|dest| dest.fields(visibility))
        .filter(move |field| field.section.is_none_or(|id| id == section))
}

pub(super) fn render_locked_section(
    section: SettingsSection,
    visibility: &Visibility,
    palette: Palette,
    fields: &crate::settings_fields::SettingsFields,
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    let dest = section.dest();
    let rows = locked_fields(section, visibility)
        .map(|field| {
            let row = ui::requires_sign_in_row(field.label, field.description, palette);
            Some(fields.field(field.anchor, row, None, cx).into_any_element())
        })
        .collect::<Vec<_>>();
    v_flex()
        .id("settings-requires-sign-in")
        .gap(px(12.))
        .child(ui::lead(dest.description(visibility), palette))
        .child(ui::group(rows, palette))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn settings_availability_follows_the_context_used_by_the_view() {
        assert!(!SettingsSection::LocalProviders.needs_account());
        assert!(!SettingsSection::Page(PageId::SharedMemory).needs_account());
        assert!(!SettingsSection::Page(PageId::Servers).needs_account());
        assert!(!SettingsSection::Page(PageId::Privacy).needs_account());
        assert!(!SettingsSection::Notifications.needs_account());
        assert!(SettingsSection::ApiKeys.needs_account());
        assert!(SettingsSection::Page(PageId::Providers).needs_account());
    }
}
