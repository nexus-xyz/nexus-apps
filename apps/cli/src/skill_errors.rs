//! What every `nexus skills` verb prints when Gateway refuses with a skill code (`skill-errors.ts`): Gateway's sentence,
//! its code in brackets, and one line that says the fix. The C1 codes belong to a person's own skills; the others
//! belong to promotion requests and the library. A refusal with any other code goes on unchanged.

use crate::error::{CliError, Error};
use crate::safe_text::safe_text;

/// The fix for each code Gateway gives for a skill, a promotion request or the library.
const FIXES: &[(&str, &str)] = &[
    (
        "invalid_frontmatter",
        "A SKILL.md starts with a line `---`, then `name:` and `description:`, then a line `---`.",
    ),
    (
        "missing_name",
        "`name` is 1 to 64 characters of a-z, 0-9 and single hyphens.",
    ),
    (
        "invalid_name",
        "`name` is 1 to 64 characters of a-z, 0-9 and single hyphens.",
    ),
    (
        "missing_description",
        "`description` says what the skill does and when to use it, in 1 to 1024 characters.",
    ),
    (
        "invalid_description",
        "`description` says what the skill does and when to use it, in 1 to 1024 characters.",
    ),
    ("skill_too_large", "A SKILL.md is at most 1 MiB."),
    ("skill_limit", "Delete one first: `nexus skills delete <name> --yes`."),
    (
        "skill_version_limit",
        "A skill keeps at most 500 versions. Import this one under a new name.",
    ),
    (
        "duplicate_name",
        "Write a new version of it: `nexus skills import <path> --update`.",
    ),
    (
        "skill_version_conflict",
        "Someone wrote a newer version. Export the latest, make your change on it, and import again.",
    ),
    (
        "skill_not_found",
        "`nexus skills` lists your skills. `nexus skills library` lists the library's.",
    ),
    (
        "skill_version_not_found",
        "`nexus skills` lists your skills. `nexus skills library` lists the library's.",
    ),
    (
        "invalid_promotion",
        "`nexus skills help` lists the options. Use `--all-versions` with an approved request: `nexus skills promotions --state approved` lists them.",
    ),
    (
        "invalid_cursor",
        "Send the cursor exactly as the last page printed it, or leave --cursor out.",
    ),
    (
        "person_session_required",
        "Run `nexus auth login` to sign in as yourself. A saved API key cannot decide or withdraw an approved version.",
    ),
    (
        "not_in_team",
        "Promote to a team you are in (`nexus team list` marks yours), or to the organization with `--to org`.",
    ),
    (
        "unknown_team",
        "`nexus team list` lists the teams. Name one as `team/<slug>`.",
    ),
    (
        "personal_org",
        "A personal organization has no one to approve. Switch to a company organization with `nexus org switch <org>`.",
    ),
    (
        "promotion_exists",
        "This version is already requested or approved. `nexus skills promotions --mine` shows its state.",
    ),
    (
        "promotion_limit",
        "Withdraw a held request with `nexus skills withdraw <id>`, or wait for an answer.",
    ),
    (
        "promotion_not_found",
        "`nexus skills promotions` lists the requests you can see.",
    ),
    (
        "promotion_decided",
        "`nexus skills promotions <id>` shows its state. A decided request does not change.",
    ),
    (
        "self_approval",
        "Ask another owner or admin to decide it. You can still withdraw your own request.",
    ),
    (
        "requester_left",
        "The person who asked left. Withdraw it with `nexus skills withdraw <id>`.",
    ),
    (
        "team_deleted",
        "Nobody can load it. Refuse it with `nexus skills refuse <id> --reason TEXT`, or withdraw it.",
    ),
    (
        "name_taken",
        "Refuse it and ask the author to rename the skill. Or withdraw the other with `nexus skills withdraw <id> --all-versions`.",
    ),
    (
        "library_limit",
        "Withdraw a library skill you no longer need with `nexus skills withdraw <id> --all-versions`.",
    ),
    (
        "role_required",
        "Only a signed-in person with the right role can do this. Members ask; owners, admins and team leads decide.",
    ),
];

/// The line the person reads for a refusal: Gateway's sentence, its code, and the fix. Any other error goes on unchanged.
pub(crate) fn refused(error: Error) -> Error {
    let Error::Core(core) = &error else { return error };
    let Some(code) = core.code.as_deref() else { return error };
    let Some((_, fix)) = FIXES.iter().find(|(name, _)| *name == code) else {
        return error;
    };
    Error::Cli(CliError::hint(
        format!("{} ({code})", capitalized(&safe_text(&core.message))),
        *fix,
    ))
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CoreError;

    fn core(status: u16, code: &str, message: &str) -> Error {
        Error::Core(Box::new(CoreError {
            status,
            message: message.into(),
            code: Some(code.into()),
            interval: None,
            details: None,
        }))
    }

    #[test]
    fn every_code_has_a_fix_of_one_line_and_the_sentence_and_code_come_first() {
        for (code, fix) in FIXES {
            let Error::Cli(shown) = refused(core(409, code, "gateway said so")) else {
                panic!("{code} was not turned into a CLI error")
            };
            assert_eq!(shown.message, format!("Gateway said so ({code})"));
            assert_eq!(shown.hint.as_deref(), Some(*fix));
            assert!(!fix.contains('\n') && !fix.is_empty(), "{code}");
            // One idea per sentence, 20 words or fewer (AGENTS.md rule 24).
            for sentence in fix.split(". ") {
                assert!(sentence.split_whitespace().count() <= 20, "{code}: {sentence}");
            }
        }
    }

    #[test]
    fn the_codes_of_promotion_requests_and_the_library_are_all_covered() {
        for code in [
            "self_approval",
            "person_session_required",
            "promotion_not_found",
            "promotion_decided",
            "requester_left",
            "name_taken",
            "library_limit",
            "promotion_limit",
            "promotion_exists",
            "not_in_team",
            "unknown_team",
            "personal_org",
            "team_deleted",
            "invalid_promotion",
            "invalid_cursor",
            "role_required",
            "skill_not_found",
            "skill_version_not_found",
        ] {
            assert!(FIXES.iter().any(|(name, _)| *name == code), "{code}");
        }
    }

    #[test]
    fn the_fixes_are_the_fixes_in_skills_cli_json() {
        // The TypeScript CLI's test reads the same file, so the two CLIs print the same line for a code.
        let shared: serde_json::Value =
            serde_json::from_str(include_str!("../skills-cli.json")).expect("skills-cli.json");
        let ours: Vec<[&str; 2]> = FIXES.iter().map(|(code, fix)| [*code, *fix]).collect();
        assert_eq!(serde_json::json!(ours), shared["fixes"]);
    }

    #[test]
    fn another_code_and_a_refusal_with_none_go_on_unchanged() {
        let other = core(500, "internal_error", "boom");
        assert_eq!(refused(other.clone()), other);
        let none = Error::Core(Box::new(CoreError {
            status: 404,
            message: "no such endpoint".into(),
            code: None,
            interval: None,
            details: None,
        }));
        assert_eq!(refused(none.clone()), none);
    }

    #[test]
    fn a_sentence_with_a_hidden_character_shows_it() {
        let Error::Cli(shown) = refused(core(404, "unknown_team", "no team a\u{1b}[2Jb")) else {
            panic!("not a CLI error")
        };
        assert_eq!(shown.message, "No team a[U+001B][2Jb (unknown_team)");
    }
}
