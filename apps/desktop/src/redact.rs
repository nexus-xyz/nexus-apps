//! A crash report's free text, made safe before it leaves the computer: the rules of `@nexus/kit`'s `diagnostics.ts`
//! (`redact`, `redactShapes`), which the Nexus app's crash prompt runs in its webview. The Gateway redacts again on
//! intake (with the secret shapes listed here); this copy is for what leaves the device.
//! Each word that is a key, a token, an email, a URL's path or query, a quoted span or a file's path is replaced, and
//! the text is cut at 200 characters.

use std::sync::LazyLock;

use regex::Regex;

/// Nexus tokens, provider and Stripe keys, GitHub tokens, Notion and Intercom tokens, PEM blocks and JWTs: the
/// Gateway's list (its `SECRET_SHAPES`).
static SECRET_SHAPES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\bnx(?:sa|[sdro])?_[A-Za-z0-9_.-]{16,}",
        r"\bsk-[A-Za-z0-9_-]{12,}",
        r"\bgh[pousr]_[A-Za-z0-9]{16,}",
        r"github_pat_[A-Za-z0-9_]{22,}",
        r"\bntn_[A-Za-z0-9]{20,}",
        r"\bdG9rO[A-Za-z0-9+/=]{20,}",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
        r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]*\.[A-Za-z0-9_-]{8,}(?:\.[A-Za-z0-9_-]+)*",
        r"\b[rs]k_(?:live|test)_[A-Za-z0-9]{8,}",
        r"\bwhsec_[A-Za-z0-9+/=_-]{8,}",
        r"\b(?:pi|seti|cs)_(?:(?:live|test)_)?[A-Za-z0-9]+_secret_[A-Za-z0-9]+",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("valid"))
    .collect()
});

const REDACTED: &str = "[redacted]";
const QUOTED: &str = "[quoted]";
/// A file's path names the person's folders and projects (`/Users/ana/acme-merger/…`): stricter than the Gateway.
const PATH: &str = "[path]";
/// The most characters a redacted text keeps.
const MAX_CHARS: usize = 200;
/// The most of a message read before redaction.
const MAX_SOURCE_CHARS: usize = 2_000;
/// A word this long that mixes letters and digits is read as a token.
const TOKEN_CHARS: usize = 20;
const QUOTES: [char; 3] = ['\'', '"', '`'];
const EDGES: &str = "()[]{}<>,.;:!?'\"`";
/// Key starts the shapes do not cover (shorter keys, other providers), lowercased.
const KEY_STARTS: [&str; 8] = ["sk-", "sk_", "pk_", "rk_", "nxs_", "nxsa_", "nxr_", "nxo_"];
/// The characters after which a quote opens inside a word: `content='…'`, `content:"…"`.
const QUOTE_AFTER: [char; 3] = ['=', ':', '('];
/// Words that carry a secret in themselves, lowercased.
const SECRET_PARTS: [&str; 5] = ["key=", "token=", "secret=", "password=", "signature="];

/// `text` with every match of the secret shapes replaced: for a code location, which the word rules would read as a
/// token.
pub fn redact_shapes(text: &str) -> String {
    let mut shaped = text.to_string();
    for shape in SECRET_SHAPES.iter() {
        shaped = shape.replace_all(&shaped, REDACTED).into_owned();
    }
    shaped
}

/// `text` with every key, token, URL path and query, email, quoted span and file path replaced, cut at 200 characters.
pub fn redact(text: &str) -> String {
    let shaped = redact_shapes(text);
    let mut out: Vec<String> = Vec::new();
    let mut quote: Option<char> = None;
    let mut after_bearer = false;
    for word in shaped.split_whitespace() {
        let safe = if let Some(open) = quote {
            if closes(word, open) {
                quote = None;
            }
            continue;
        } else if let Some((at, open)) = opening_quote(word) {
            if !closes(&word[at + open.len_utf8()..], open) {
                quote = Some(open);
            }
            format!("{}{QUOTED}", &word[..at])
        } else if after_bearer {
            REDACTED.to_string()
        } else if let Some(at) = word.find("://") {
            origin(word, at)
        } else if secret(word) {
            REDACTED.to_string()
        } else if file_path(word) {
            PATH.to_string()
        } else {
            word.to_string()
        };
        after_bearer = word.eq_ignore_ascii_case("bearer");
        out.push(safe);
    }
    cut(&out.join(" "), MAX_CHARS, "…")
}

/// The first line of `text`, on one line with no control character: what follows a message's first line is content
/// (a parser's excerpt), and the Gateway refuses control characters.
pub fn first_line(text: &str) -> String {
    plain(text.split('\n').next().unwrap_or_default(), MAX_SOURCE_CHARS)
}

/// One line, no control characters, at most `max` characters.
pub fn plain(text: &str, max: usize) -> String {
    let spaced: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    cut(&spaced.split_whitespace().collect::<Vec<_>>().join(" "), max, "")
}

/// At most `max` characters of `text`, with `more` after a cut.
fn cut(text: &str, max: usize, more: &str) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}{more}", &text[..at]),
        None => text.to_string(),
    }
}

/// Where a quote opens in `word`, and which: at its start, or right after `=`, `:` or `(`.
fn opening_quote(word: &str) -> Option<(usize, char)> {
    let mut previous: Option<char> = None;
    for (at, c) in word.char_indices() {
        if QUOTES.contains(&c) && previous.is_none_or(|p| QUOTE_AFTER.contains(&p)) {
            return Some((at, c));
        }
        previous = Some(c);
    }
    None
}

/// Whether `word` ends the quote `open` began.
fn closes(word: &str, open: char) -> bool {
    word.trim_end_matches(|c: char| ",.;:!?)]}".contains(c)).ends_with(open)
}

fn bare(word: &str) -> &str {
    word.trim_matches(|c: char| EDGES.contains(c))
}

/// Whether `word` looks like a key, a token or an email.
fn secret(word: &str) -> bool {
    let bare = bare(word);
    let lower = bare.to_ascii_lowercase();
    KEY_STARTS.iter().any(|start| lower.starts_with(start))
        || SECRET_PARTS.iter().any(|part| lower.contains(part))
        || (bare.contains('@') && bare.contains('.'))
        || (bare.chars().count() >= TOKEN_CHARS
            && bare.chars().any(|c| c.is_ascii_digit())
            && bare.chars().any(|c| c.is_ascii_alphabetic()))
}

/// Whether `word` looks like a file's path: from the root, the home folder or a drive (`/Users/…`, `~/…`, `C:\…`),
/// with a backslash, or two folders deep (`a/b/c`). "and/or" is not one.
fn file_path(word: &str) -> bool {
    let bare = bare(word);
    let rooted = {
        let rest = bare
            .strip_prefix('~')
            .or_else(|| bare.strip_prefix(".."))
            .or_else(|| bare.strip_prefix('.'))
            .unwrap_or(bare);
        rest.strip_prefix('/')
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c != '/' && !c.is_whitespace())
    };
    let drive = {
        let mut chars = bare.chars();
        matches!(
            (chars.next(), chars.next(), chars.next()),
            (Some(letter), Some(':'), Some('\\' | '/')) if letter.is_ascii_alphabetic()
        )
    };
    rooted || drive || bare.contains('\\') || bare.matches('/').count() >= 2
}

/// A URL cut to its scheme and host: no user, path, query or fragment.
fn origin(word: &str, at: usize) -> String {
    let authority = word[at + 3..]
        .split(|c: char| "/?#)]>,;\"'".contains(c))
        .next()
        .unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    format!("{}://{host}", &word[..at])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_message_keeps_its_words_and_loses_paths_keys_and_quotes() {
        assert_eq!(
            redact("index out of bounds: the len is 3"),
            "index out of bounds: the len is 3"
        );
        let text = redact(
            "called `Result::unwrap()` on an `Err` value: Os { code: 2, message: \"No such file\" } at \
             /home/ana/acme-merger/notes.md with sk-proj-abcdefghijklmnop from https://x.example/a?b=c ana@acme.com",
        );
        for leaked in [
            "No such file",
            "/home/ana",
            "acme-merger",
            "sk-proj",
            "/a?b=c",
            "ana@acme.com",
        ] {
            assert!(!text.contains(leaked), "{leaked} in {text}");
        }
        assert!(text.contains("[path]") && text.contains("https://x.example"), "{text}");
    }

    #[test]
    fn long_text_is_cut_and_only_the_first_line_is_read() {
        assert_eq!(redact(&"word ".repeat(100)).chars().count(), 201);
        assert_eq!(first_line("boom\nsecret excerpt"), "boom");
        assert_eq!(plain("a\tb\u{7}  c", 10), "a b c");
    }

    #[test]
    fn a_code_location_loses_only_secret_shapes() {
        assert_eq!(redact_shapes("src/crash.rs:12:5"), "src/crash.rs:12:5");
        assert!(!redact_shapes("ghp_abcdefghijklmnopqrstu").contains("ghp_"));
    }
}
