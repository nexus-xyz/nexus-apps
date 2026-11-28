//! `nexus keys` reads the providers from Gateway (`GET /v1/keys/providers`), so no string literal in the
//! CLI's `keys` help or commands is a provider's id or names two of them. The CLI does not link the catalog
//! (`dependencies.rs`), so it reads the ids from `fixtures/provider-ids.txt`, a copy of the catalog's provider ids,
//! updated when the catalog's list changes. The scan is `nexus_catalog::by_hand`'s.

const PLACES: [(&str, &str); 2] = [
    ("src/catalog.rs", include_str!("../src/catalog.rs")),
    ("src/cli.rs", include_str!("../src/cli.rs")),
];

#[test]
fn the_keys_commands_name_no_provider_by_hand() {
    let ids: Vec<&str> = include_str!("fixtures/provider-ids.txt").lines().collect();
    assert!(ids.len() > 1, "the ids are read");
    let named = |text: &str| {
        let words: Vec<String> = text
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .map(str::to_ascii_lowercase)
            .collect();
        ids.iter().filter(|id| words.iter().any(|word| word == *id)).count()
    };
    let mut found = vec![];
    for (place, source) in PLACES {
        for (number, code) in production_lines(source) {
            for text in literals(code) {
                if ids.contains(&text.as_str()) || named(&text) >= 2 {
                    found.push(format!("nexus-cli/{place}:{number}: {text:?}"));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "read the providers from Gateway (GET /v1/keys/providers): {found:#?}"
    );
}

/// A file's lines, numbered and without comments, leaving out each `#[cfg(test)]` item. The item's braces are
/// matched with string literals ignored, so the lines after it are read again.
fn production_lines(source: &str) -> Vec<(usize, &str)> {
    let braces = |code: &str| {
        // A brace as a character literal (`'{'`) opens or closes nothing.
        let code = code.replace("'{'", "").replace("'}'", "");
        let (mut depth, mut quoted, mut escaped) = (0, false, false);
        for c in code.chars() {
            match c {
                _ if escaped => escaped = false,
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                '{' if !quoted => depth += 1,
                '}' if !quoted => depth -= 1,
                _ => {}
            }
        }
        depth
    };
    let (mut lines, mut test_item, mut depth) = (vec![], false, 0);
    for (number, line) in source.lines().enumerate() {
        let code = line.split("//").next().unwrap_or_default();
        if !test_item && code.trim_start().starts_with("#[cfg(test)]") {
            test_item = true;
            continue;
        }
        if test_item {
            depth += braces(code);
            if depth <= 0 && (code.contains('}') || code.trim_end().ends_with(';')) {
                test_item = false;
                depth = 0;
            }
            continue;
        }
        lines.push((number + 1, code));
    }
    lines
}

/// The `"…"` string literals on a line of Rust.
fn literals(code: &str) -> Vec<String> {
    let (mut found, mut open, mut escaped, mut text) = (vec![], false, false, String::new());
    for c in code.chars() {
        match c {
            _ if open && escaped => {
                escaped = false;
                text.push(c);
            }
            '\\' if open => escaped = true,
            '"' if open => {
                found.push(std::mem::take(&mut text));
                open = false;
            }
            _ if open => text.push(c),
            '"' => open = true,
            _ => {}
        }
    }
    found
}
