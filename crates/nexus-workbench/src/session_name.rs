//! A session's name: a short word a person, the CLI or `start_session` gives a session, unique among the
//! store's sessions. Claude Code gets it as `--name` ([`crate::claude::args`]), so its cross-session tools (ListAgents
//! and SendMessage, which address a session by its `--name`) reach the session, and the CLIs and the outside tools
//! accept it wherever an id goes. A session has none until someone sets one: nothing makes one up.
//!
//! The rule lives here alone: 1 to [`MAX`] characters of `A-Z`, `a-z`, `0-9`, `.`, `_` and `-`, starting with a letter
//! or a digit (so a name never looks like a flag) and not with `t_` (so it never looks like a session id). Names are
//! unique regardless of case: `Lead` and `lead` cannot both exist. The store's unique index on `lower(name)` backs the
//! clash check ([`taken`]).

/// The longest name, in characters.
pub const MAX: usize = 40;

/// Why `name` is not a valid name; Ok when it is.
pub fn check(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX {
        return Err(format!("A session name is 1 to {MAX} characters."));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(format!(
            "{name} is not a session name: use letters, digits, dots, dashes and underscores."
        ));
    }
    if !name.as_bytes()[0].is_ascii_alphanumeric() {
        return Err(format!(
            "{name} is not a session name: a name starts with a letter or a digit."
        ));
    }
    if name.starts_with("t_") {
        return Err(format!(
            "{name} looks like a session id: a session name never starts with t_."
        ));
    }
    Ok(())
}

/// The refusal when `holder` already has `name`.
pub fn taken(name: &str, holder: &str) -> String {
    format!("{name} is taken by {holder}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_a_short_word_that_never_looks_like_an_id() {
        for good in ["orchestrator", "a", "build-2", "v1.2_x", &"n".repeat(MAX)] {
            assert_eq!(check(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "two words",
            "naïve",
            "a/b",
            "t_abc",
            "-x",
            "--x",
            "-",
            ".",
            "_x",
            "--dangerously-skip-permissions",
            &"n".repeat(MAX + 1),
        ] {
            assert!(check(bad).is_err(), "{bad}");
        }
        // Only the id prefix is refused: a name may start with t otherwise.
        assert_eq!(check("tests"), Ok(()));
        assert_eq!(check("T_x"), Ok(()));
    }
}
