//! Eligibility only. The shared viewer owns parsing, escaping, and KaTeX rendering.
pub(super) fn contains_math(source: &str) -> bool {
    let trimmed = source.trim_start();
    if trimmed.starts_with("$$") || trimmed.starts_with("\\[") {
        return true;
    }
    // Avoid moving ordinary code-only paragraphs into a web view.
    let mut code = false;
    for part in source.split('`') {
        if !code && inline_math(part) {
            return true;
        }
        code = !code;
    }
    false
}
fn inline_math(text: &str) -> bool {
    if let Some((_, rest)) = text.split_once("\\(")
        && rest.contains("\\)")
    {
        return true;
    }
    let chars: Vec<char> = text.chars().collect();
    for start in 0..chars.len() {
        if chars[start] != '$' || escaped(&chars, start) {
            continue;
        }
        let Some(first) = chars.get(start + 1) else { continue };
        if first.is_whitespace() || *first == '$' {
            continue;
        }
        for end in start + 2..chars.len() {
            if chars[end] != '$' || escaped(&chars, end) {
                continue;
            }
            if !chars[end - 1].is_whitespace() && !chars.get(end + 1).is_some_and(|c| c.is_ascii_digit()) {
                return true;
            }
            break;
        }
    }
    false
}
fn escaped(chars: &[char], index: usize) -> bool {
    chars[..index].iter().rev().take_while(|c| **c == '\\').count() % 2 == 1
}
/// The words to show while the math draws: the source without its math delimiters, so `$2^n$` reads `2^n`.
pub(super) fn placeholder(source: &str) -> String {
    let mut text = source.trim();
    if let Some(body) = text.strip_prefix("```math") {
        text = body.trim_end().trim_end_matches('`').trim();
    }
    let text = ["$$", "\\[", "\\]", "\\(", "\\)"]
        .iter()
        .fold(text.to_string(), |text, delimiter| text.replace(delimiter, ""));
    let chars: Vec<char> = text.chars().collect();
    chars
        .iter()
        .enumerate()
        .filter(|(index, c)| **c != '$' || escaped(&chars, *index))
        .map(|(_, c)| *c)
        .collect()
}
#[cfg(test)]
mod tests {
    use super::{contains_math, placeholder};
    #[test]
    fn the_placeholder_drops_math_delimiters_only() {
        assert_eq!(
            placeholder("Grows like $2^n$ and \\(n^3\\)."),
            "Grows like 2^n and n^3."
        );
        assert_eq!(placeholder("$$x^2+1$$"), "x^2+1");
        assert_eq!(placeholder("```math\nx+1\n```"), "x+1");
        assert_eq!(placeholder("Costs \\$5"), "Costs \\$5");
    }
    #[test]
    fn accepts_shared_math_forms() {
        for source in [
            "The result is $x^2$.",
            "Use \\(x+1\\) now",
            "$$x^2$$",
            "\\[x+1\\]",
            "**$x$**",
            "$α+β$",
        ] {
            assert!(contains_math(source), "{source}");
        }
    }
    #[test]
    fn leaves_currency_code_and_incomplete_inline_native() {
        for source in [
            "$5 and $10",
            "`$x$`",
            "The result is $x",
            "\\$x\\$",
            "$ x $",
            "plain text",
        ] {
            assert!(!contains_math(source), "{source}");
        }
    }
}
