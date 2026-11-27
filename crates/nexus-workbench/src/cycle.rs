//! The steps behind Code's keys for the composer's chips: Mod+Shift+M steps the
//! model, Mod+Shift+E the effort, Mod+Alt+Shift+A the permission mode. Each step wraps round. The model's list is the
//! organization's default, then the newest model of each family (`splitModels` in `models.tsx`). Nexus GPUI calls
//! these; the Nexus app keeps its TypeScript copy, and the tests here are its tests.

use crate::event::{Effort, PermissionMode};

/// The value after `current` in `list`, wrapping round; one not in the list steps to the first. None for an empty list.
pub fn next_of<T: PartialEq + Clone>(list: &[T], current: &T) -> Option<T> {
    let next = match list.iter().position(|entry| entry == current) {
        Some(at) => (at + 1) % list.len(),
        None => 0,
    };
    list.get(next).cloned()
}

/// The organization's default (None), then the newest model of each family, as the model menu leads with them.
pub fn next_model(latest: &[String], current: Option<&str>) -> Option<String> {
    let list: Vec<Option<&str>> = std::iter::once(None)
        .chain(latest.iter().map(|model| Some(model.as_str())))
        .collect();
    next_of(&list, &current).flatten().map(str::to_string)
}

/// The effort after `current` among `efforts`, the levels the model takes with the default (None) among them.
pub fn next_effort(efforts: &[Option<Effort>], current: Option<Effort>) -> Option<Effort> {
    next_of(efforts, &current).flatten()
}

/// The modes the key steps through. Full access is left out: it is the riskiest mode
/// ([`PermissionMode::rank`]). The person picks it from its menu, on purpose.
pub const CYCLED_MODES: [PermissionMode; 4] = [
    PermissionMode::Default,
    PermissionMode::Auto,
    PermissionMode::AcceptEdits,
    PermissionMode::Plan,
];

pub fn next_mode(current: PermissionMode, offered: &[PermissionMode]) -> Option<PermissionMode> {
    let next = CYCLED_MODES
        .iter()
        .position(|mode| *mode == current)
        .map_or(0, |at| at + 1);
    CYCLED_MODES[next..]
        .iter()
        .chain(&CYCLED_MODES[..next])
        .copied()
        .find(|mode| offered.contains(mode))
}

/// A model as people say it (`modelLabel`): "claude-opus-5-5" and "anthropic/claude-opus-5.5" read "Opus 5.5";
/// anything else as it is, without a provider's prefix.
pub fn model_label(model: &str) -> String {
    let name = model
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(model);
    let name = name.strip_prefix("claude-").unwrap_or(name);
    let name = match name.rsplit_once('-') {
        Some((head, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => name,
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let Some((family, version)) = name.split_once('-') else {
        return name.to_string();
    };
    if family.is_empty() || !family.bytes().all(|b| b.is_ascii_lowercase()) {
        return name.to_string();
    }
    let (major, minor) = match version.split_once(['-', '.']) {
        Some((major, minor)) => (major, Some(minor)),
        None => (version, None),
    };
    if !digits(major) || minor.is_some_and(|minor| !digits(minor)) {
        return name.to_string();
    }
    let mut out = family[..1].to_ascii_uppercase();
    out.push_str(&family[1..]);
    out.push(' ');
    out.push_str(major);
    if let Some(minor) = minor {
        out.push('.');
        out.push_str(minor);
    }
    out
}

/// The newest model of each family ("Opus 5.5", "Sonnet 5.5", "Haiku 5.5"), most capable first (`splitModels`'s
/// `latest`). A family is the label's first word, its version the numbers after it.
pub fn latest_models(models: &[String]) -> Vec<String> {
    let parse = |model: &String| {
        let label = model_label(model);
        let mut words = label.split(' ');
        let family = words.next().unwrap_or_default().to_string();
        let version: Vec<f64> = words
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|part| part.parse().unwrap_or(f64::NAN))
            .collect();
        (model.clone(), family, version)
    };
    // `newer` in the TypeScript: a missing part counts as 0, and a part that is not a number never wins.
    let newer = |a: &[f64], b: &[f64]| {
        for index in 0..a.len().max(b.len()) {
            let diff = a.get(index).copied().unwrap_or(0.) - b.get(index).copied().unwrap_or(0.);
            if diff != 0. && !diff.is_nan() {
                return diff > 0.;
            }
        }
        false
    };
    let rank = |family: &str| {
        ["Opus", "Sonnet", "Haiku"]
            .iter()
            .position(|f| *f == family)
            .unwrap_or(3)
    };
    let mut best: Vec<(String, String, Vec<f64>)> = Vec::new();
    for entry in models.iter().map(parse) {
        match best.iter_mut().find(|(_, family, _)| *family == entry.1) {
            Some(current) if newer(&entry.2, &current.2) => *current = entry,
            Some(_) => {}
            None => best.push(entry),
        }
    }
    // A stable sort, as JavaScript's: families of one rank keep the order they first came in.
    best.sort_by_key(|(_, family, _)| rank(family));
    best.into_iter().map(|(model, _, _)| model).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// `effortOptions` with no model's own list: the usual levels, Ultra only for Codex, the default first.
    fn usual_efforts(codex: bool) -> Vec<Option<Effort>> {
        let mut list = vec![None, Some(Effort::Low), Some(Effort::Medium), Some(Effort::High)];
        list.extend([Some(Effort::Xhigh), Some(Effort::Max)]);
        if codex {
            list.push(Some(Effort::Ultra));
        }
        list
    }

    #[test]
    fn steps_through_a_list_and_wraps_round() {
        let list = ["a", "b", "c"];
        assert_eq!(next_of(&list, &"a"), Some("b"));
        assert_eq!(next_of(&list, &"c"), Some("a"));
        assert_eq!(next_of(&list, &"z"), Some("a"));
        assert_eq!(next_of::<&str>(&[], &"a"), None);
    }

    #[test]
    fn goes_from_the_organizations_default_through_the_newest_models_and_back() {
        let latest = strings(&["claude-opus-5-5", "claude-sonnet-5-5"]);
        assert_eq!(next_model(&latest, None).as_deref(), Some("claude-opus-5-5"));
        assert_eq!(next_model(&latest, Some("claude-sonnet-5-5")), None);
        // An older model picked from the submenu steps to the default.
        assert_eq!(next_model(&latest, Some("claude-opus-4")), None);
        assert_eq!(next_model(&[], None), None);
    }

    #[test]
    fn steps_codex_through_xhigh_max_and_ultra() {
        let codex = usual_efforts(true);
        assert_eq!(next_effort(&codex, None), Some(Effort::Low));
        assert_eq!(next_effort(&codex, Some(Effort::High)), Some(Effort::Xhigh));
        assert_eq!(next_effort(&codex, Some(Effort::Xhigh)), Some(Effort::Max));
        assert_eq!(next_effort(&codex, Some(Effort::Max)), Some(Effort::Ultra));
        assert_eq!(next_effort(&codex, Some(Effort::Ultra)), None);
        let claude = usual_efforts(false);
        assert_eq!(next_effort(&claude, Some(Effort::High)), Some(Effort::Xhigh));
        assert_eq!(next_effort(&claude, Some(Effort::Xhigh)), Some(Effort::Max));
        assert_eq!(next_effort(&claude, Some(Effort::Max)), None);
    }

    #[test]
    fn never_steps_into_full_access() {
        assert!(!CYCLED_MODES.contains(&PermissionMode::BypassPermissions));
        for (current, next) in [
            (PermissionMode::Default, PermissionMode::Auto),
            (PermissionMode::Auto, PermissionMode::AcceptEdits),
            (PermissionMode::Plan, PermissionMode::Default),
            (PermissionMode::BypassPermissions, PermissionMode::Default),
        ] {
            assert_eq!(next_mode(current, &CYCLED_MODES), Some(next));
        }
    }

    #[test]
    #[cfg(feature = "cursor")]
    fn cursor_cycles_only_between_accept_edits_and_plan() {
        use crate::event::SessionAgent;
        let offered = crate::harness::harness(SessionAgent::Cursor).offered_modes();
        assert_eq!(
            next_mode(PermissionMode::AcceptEdits, offered),
            Some(PermissionMode::Plan)
        );
        assert_eq!(
            next_mode(PermissionMode::Plan, offered),
            Some(PermissionMode::AcceptEdits)
        );
    }

    #[test]
    fn a_model_reads_as_people_say_it() {
        assert_eq!(model_label("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_label("anthropic/claude-opus-5.5"), "Opus 5.5");
        assert_eq!(model_label("claude-haiku-5-5-20261101"), "Haiku 5.5");
        assert_eq!(model_label("claude-opus-4"), "Opus 4");
        assert_eq!(model_label("gpt-5"), "Gpt 5");
        assert_eq!(model_label("gpt-5.1-codex"), "gpt-5.1-codex");
        assert_eq!(model_label("sonnet"), "sonnet");
    }

    #[test]
    fn the_newest_model_of_each_family_leads_most_capable_first() {
        let models = strings(&[
            "claude-haiku-4-5",
            "claude-sonnet-4-5",
            "claude-opus-4-1",
            "claude-sonnet-5-5",
            "claude-opus-5-5",
            "claude-haiku-5-5",
            "gpt-5",
        ]);
        assert_eq!(
            latest_models(&models),
            strings(&["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "gpt-5"])
        );
        assert!(latest_models(&[]).is_empty());
    }
}
