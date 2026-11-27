//! Settings search, as the Nexus app's (`ui/src/components/nexus-ui/settings-navigation.ts`, `searchSettings`). It
//! finds a setting by its destination's name, its own label, its keywords or its description. Settings' own search
//! field and the search palette (Ctrl+K) both read it.
//!
//! The index is the registry (`settings_registry.rs`): the Nexus app's destinations and fields, with their stable
//! anchors. Only what this account sees enters it. A hit names the section here that draws the setting, or, for a
//! destination this build does not draw (Slack), the browser page that does.

use super::SettingsSection;
use super::settings_registry::{Dest, Group, Visibility, nav};

/// A setting a query found (`SettingsSearchResult`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Hit {
    pub dest: Dest,
    /// The section here that draws it; None when the browser does.
    pub section: Option<SettingsSection>,
    /// The field's stable anchor; None for a destination's own row.
    pub anchor: Option<&'static str>,
    account_field: Option<crate::settings_fields::AccountField>,
    pub label: &'static str,
    pub description: &'static str,
    /// The nav row, its tab when the destination is one, then the setting when the hit is one: "This computer ›
    /// Gateway address", "Model access › Providers".
    pub breadcrumb: Vec<&'static str>,
}

impl Hit {
    pub(super) fn requires_sign_in(&self) -> bool {
        self.account_field.is_some() || self.section.is_none_or(SettingsSection::needs_account)
    }
}

/// Accents off a Latin letter (the Nexus app's NFKD and mark strip, for the letters people type).
fn fold(c: char) -> char {
    match c {
        'à'..='å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'è'..='ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ì'..='ï' | 'ī' | 'į' => 'i',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò'..='ö' | 'ø' | 'ō' | 'ő' => 'o',
        'ù'..='ü' | 'ū' | 'ů' | 'ű' => 'u',
        'ý' | 'ÿ' => 'y',
        'ś' | 'š' => 's',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

/// Lower case, accents off, and every run of other characters one space (`normalize`).
fn normalize(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars().flat_map(char::to_lowercase).map(fold) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// Within a bounded edit distance (`near`): one edit under 7 letters, two from 7. Words under 4 letters must match.
fn near(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() < 4 || a.len() > 32 {
        return false;
    }
    let limit = if a.len() < 7 { 1 } else { 2 };
    if a.len().abs_diff(b.len()) > limit {
        return false;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut diagonal = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let previous = row[j];
            row[j] = (row[j] + 1)
                .min(row[j - 1] + 1)
                .min(diagonal + usize::from(a[i - 1] != b[j - 1]));
            diagonal = previous;
        }
        if row.iter().min().is_some_and(|&least| least > limit) {
            return false;
        }
    }
    row[b.len()] <= limit
}

/// Every destination and setting the query names, best first (`searchSettings`). Spelling slips count only when
/// nothing matches as typed.
pub(super) fn search(query: &str, visibility: &Visibility) -> Vec<Hit> {
    let normalized = normalize(&query.chars().take(200).collect::<String>());
    if normalized.is_empty() {
        return Vec::new();
    }
    let terms: Vec<&str> = normalized.split(' ').collect();
    let mut matches: Vec<(u32, bool, Hit)> = Vec::new();
    for (group, rows) in nav(visibility) {
        let group_label = if group == Group::Global {
            "Global"
        } else {
            group.label()
        };
        for dest in rows {
            // This build's names: the row, then the tab when the destination is one ("Model access › Providers").
            let page = dest.title();
            let row = dest.row();
            let context = normalize(&format!("{group_label} {} {page} {}", dest.label(), row.title()));
            let fields = dest.fields(visibility);
            for field in std::iter::once(None).chain(fields.iter().map(Some)) {
                let shown = field.map_or(page, |field| field.label);
                let label = normalize(shown);
                let keywords = normalize(&field.map_or(dest.keywords(), |field| field.keywords).join(" "));
                let description = normalize(field.map_or(dest.description(visibility), |field| field.description));
                let joined = format!("{label} {keywords} {description} {context}");
                let tokens: Vec<&str> = joined.split(' ').collect();
                let mut score = if label == normalized { 100 } else { 0 };
                let mut fuzzy = false;
                let found = terms.iter().all(|term| {
                    if label.contains(term) {
                        score += 40;
                    } else if keywords.contains(term) {
                        score += 25;
                    } else if description.contains(term) || context.contains(term) {
                        score += 10;
                    } else if tokens.iter().any(|token| near(term, token)) {
                        score += 1;
                        fuzzy = true;
                    } else {
                        return false;
                    }
                    true
                });
                if !found {
                    continue;
                }
                let mut breadcrumb = vec![row.title()];
                breadcrumb.extend((row != dest).then_some(page));
                breadcrumb.extend(field.map(|field| field.label));
                // A destination drawn in the browser has no section here, even for its fields.
                let section = dest
                    .first_for(visibility)
                    .and_then(|first| field.map_or(Some(first), |field| field.section));
                matches.push((
                    score,
                    fuzzy,
                    Hit {
                        dest,
                        section,
                        anchor: field.map(|field| field.anchor).or_else(|| dest.target()),
                        account_field: field.and_then(|field| field.account),
                        label: shown,
                        description: field.map_or(dest.description(visibility), |field| field.description),
                        breadcrumb,
                    },
                ));
            }
        }
    }
    if matches.iter().any(|(_, fuzzy, _)| !fuzzy) {
        matches.retain(|(_, fuzzy, _)| !fuzzy);
    }
    // Stable: equal scores keep the nav's order.
    matches.sort_by(|a, b| b.0.cmp(&a.0));
    matches.into_iter().map(|(_, _, hit)| hit).collect()
}

#[cfg(test)]
mod tests {
    //! The Nexus app's cases (`settings-frame.test.ts`, "settings navigation"), on the shared registry.
    use super::*;
    use crate::pages::PageId;

    fn all(query: &str) -> Vec<Hit> {
        search(query, &Visibility::all())
    }

    fn lands(query: &str) -> (Option<SettingsSection>, &'static str) {
        let hit = all(query)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("nothing for {query:?}"));
        (hit.section, hit.label)
    }

    #[test]
    fn search_distinguishes_account_controls_on_mixed_settings_pages() {
        for label in ["Show when I'm online", "Codex routing", "Claude Code routing"] {
            let hit = all(label).into_iter().find(|hit| hit.label == label).expect(label);
            assert!(hit.requires_sign_in());
            assert!(hit.section.is_some_and(|section| !section.needs_account()));
        }
        for label in ["Text size", "Plan usage"] {
            let hit = all(label).into_iter().find(|hit| hit.label == label).expect(label);
            assert!(!hit.requires_sign_in());
        }
    }

    #[test]
    fn budgets_search_always_carries_the_native_limits_target() {
        for query in ["Budgets", "default person", "card usage limit", "team caps"] {
            let hits = search(query, &Visibility::all());
            let hit = hits.iter().find(|hit| hit.dest == Dest::Budgets).expect(query);
            assert_eq!(hit.section, Some(SettingsSection::Page(crate::pages::PageId::Usage)));
            assert_eq!(hit.anchor, Some("usage-limits"));
        }
        let hits = search("Usage", &Visibility::all());
        let usage = hits.iter().find(|hit| hit.dest == Dest::Usage).unwrap();
        assert_eq!(usage.anchor, None);
    }

    #[test]
    fn finds_a_setting_through_its_words_and_opens_its_section() {
        use SettingsSection as S;
        for (query, section, label) in [
            ("font", S::Appearance, "Text size"),
            ("dark mode", S::Appearance, "Theme"),
            ("alerts", S::Notifications, "Notifications"),
            ("hotkeys", S::Shortcuts, "Keyboard shortcuts"),
            ("SSH", S::Page(PageId::Servers), "Servers"),
            ("API key", S::ApiKeys, "API keys"),
            ("startup", S::Page(PageId::DesktopApp), "Open at login"),
            ("version", S::Page(PageId::DesktopApp), "Desktop updates"),
            ("automation", S::Page(PageId::Hooks), "Hooks"),
            ("setup", S::Page(PageId::Worktrees), "Project setup scripts"),
            ("branch prefix", S::Page(PageId::Worktrees), "Branch names"),
            ("codex routing", S::Page(PageId::Agents), "Codex routing"),
            ("MCP", S::Page(PageId::Mcp), "MCP servers"),
            ("connectors", S::Page(PageId::Connectors), "Connectors"),
            ("connected accounts", S::Page(PageId::Connectors), "Connectors"),
            ("skills", S::Page(PageId::Skills), "Skills"),
            ("default model", S::Page(PageId::Policy), "Chat default model"),
            ("web search", S::Page(PageId::Policy), "Web search"),
            ("BYOK", S::Page(PageId::Providers), "Providers"),
            ("pairing", S::Page(PageId::Devices), "Devices"),
            ("resume", S::Page(PageId::LocalSessions), "History"),
        ] {
            assert_eq!(lands(query), (Some(section), label), "{query}");
        }
    }

    #[test]
    fn service_names_find_connectors_without_requiring_repository_controls() {
        for visibility in [Visibility::all(), Visibility::default()] {
            for service in ["Linear", "GitHub", "Slack", "Jira", "Notion"] {
                assert!(
                    search(service, &visibility).iter().any(|hit| {
                        hit.section == Some(SettingsSection::Page(PageId::Connectors)) && hit.label == "Connectors"
                    }),
                    "{service} must lead to the connector manager"
                );
            }
        }
    }

    #[test]
    fn data_fields_open_general_and_unported_destinations_still_open_the_browser() {
        let hit = all("chat retention").into_iter().next().expect("retention");
        assert_eq!(
            (hit.dest, hit.section, hit.anchor),
            (
                Dest::Data,
                Some(SettingsSection::Page(PageId::General)),
                Some("retention-days")
            )
        );
        let sharing = all("sharing and pages").into_iter().next().expect("sharing");
        assert_eq!(
            (sharing.section, sharing.anchor),
            (Some(SettingsSection::Page(PageId::General)), Some("sharing-settings"))
        );
    }

    #[test]
    fn names_where_a_setting_lives_by_this_builds_rows_and_tabs() {
        assert_eq!(
            all("Gateway address").first().map(|hit| hit.breadcrumb.clone()),
            Some(vec!["This computer", "Gateway address"])
        );
        assert_eq!(
            all("BYOK").first().map(|hit| hit.breadcrumb.clone()),
            Some(vec!["Model access", "Providers"])
        );
    }

    #[test]
    fn privacy_search_opens_the_native_controls_including_for_personal_accounts() {
        let personal = Visibility {
            personal: true,
            ..Visibility::default()
        };
        for visibility in [&Visibility::all(), &personal] {
            for query in ["claude isolation", "outside access"] {
                assert!(
                    search(query, visibility)
                        .iter()
                        .any(|hit| hit.dest == Dest::Privacy
                            && hit.section == Some(SettingsSection::Page(PageId::Privacy))),
                    "{query}"
                );
            }
        }
    }

    #[test]
    fn leaves_out_what_this_account_cannot_open() {
        let member = Visibility::default();
        assert!(search("BYOK", &member).is_empty());
        // "invoice" is a spelling slip from Members' "invite"; Billing itself stays out.
        assert!(search("invoice", &member).iter().all(|hit| hit.dest != Dest::Billing));
        let personal = Visibility {
            personal: true,
            ..Visibility::default()
        };
        assert!(search("departments", &personal).is_empty());
        assert!(search("presence", &personal).is_empty());
        let managed = Visibility {
            managed_gateway: true,
            ..Visibility::all()
        };
        assert!(
            search("add a gateway", &managed)
                .iter()
                .all(|hit| hit.anchor != Some("add-gateway"))
        );
    }

    #[test]
    fn matches_words_in_any_order_and_normalizes_punctuation_case_and_accents() {
        for query in [" MODE  DARK ", "dark-mode", "DÁRK mode"] {
            assert_eq!(lands(query), (Some(SettingsSection::Appearance), "Theme"), "{query}");
        }
    }

    #[test]
    fn accepts_a_missing_letter_or_transposed_letters_when_nothing_matches_as_typed() {
        for query in ["gatewy", "gatweay"] {
            assert!(
                all(query)
                    .iter()
                    .any(|hit| hit.section == Some(SettingsSection::Page(PageId::Gateways))),
                "{query}"
            );
        }
        assert!(
            all("notificatons")
                .iter()
                .any(|hit| hit.section == Some(SettingsSection::Notifications))
        );
    }

    #[test]
    fn keeps_exact_results_ahead_of_spelling_suggestions() {
        let labels: Vec<&str> = all("font").iter().map(|hit| hit.label).collect();
        assert_eq!(labels, ["Text size"]);
    }

    #[test]
    fn returns_nothing_for_blank_queries_and_unrelated_words() {
        for query in ["", "   ", "unrelatedxyz", "zz", "💭"] {
            assert_eq!(all(query), Vec::new(), "{query:?}");
        }
    }

    #[test]
    fn near_bounds_the_edit_distance() {
        assert!(near("gatewy", "gateway"));
        assert!(near("gatweay", "gateway"));
        assert!(!near("gat", "gate"));
        assert!(!near("gtwy", "gateway"));
    }
}
