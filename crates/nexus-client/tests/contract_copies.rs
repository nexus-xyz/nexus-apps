//! `contract/openapi.json` is a byte-for-byte copy of `docs/openapi/gateway.json`: the Gateway's document, cleaned of
//! server-only details before it is published here. The publish sync writes both files in the same pull request. To
//! refresh by hand: `cp docs/openapi/gateway.json crates/nexus-client/contract/openapi.json`. The crate keeps its own
//! copy because it embeds it and is published alone. `contract/mcp-tools.json` is this crate's own list.

use std::path::Path;

/// Upper-case names a client uses, which the cleaned contract keeps.
const ALLOWED_NAMES: [&str; 3] = ["ANTHROPIC_BASE_URL", "NPM_TOKEN", "REQUEST_CHANGES"];

/// The `UPPER_SNAKE` words of `text`: two or more parts of `A-Z 0-9`, the first starting with a letter.
fn upper_snake_words(text: &str) -> Vec<&str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| {
            let parts: Vec<&str> = word.split('_').collect();
            parts.len() > 1
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
                && word.as_bytes()[0].is_ascii_uppercase()
        })
        .collect()
}

#[test]
fn the_contract_copy_is_the_cleaned_one() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(manifest.join("contract/openapi.json")).expect("the copy is read");
    let names: Vec<&str> = upper_snake_words(&text)
        .into_iter()
        .filter(|word| !ALLOWED_NAMES.contains(word))
        .collect();
    assert!(
        names.is_empty(),
        "contract/openapi.json names server settings ({names:?}): copy docs/openapi/gateway.json"
    );
    for internal in [
        "nexus-org-logos",
        "storage.googleapis.com",
        "localhost",
        "uiSecret",
        "agentsSecret",
        "/v1/staff/",
    ] {
        assert!(
            !text.contains(internal),
            "contract/openapi.json has {internal}: copy docs/openapi/gateway.json"
        );
    }
}

#[test]
fn the_contract_copy_equals_the_published_document() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // In the repository only: the published crate has no `docs/` next to it.
    let Ok(source) = std::fs::read(manifest.join("../../docs/openapi/gateway.json")) else {
        return;
    };
    let copy = std::fs::read(manifest.join("contract/openapi.json")).expect("the copy is read");
    assert!(
        copy == source,
        "contract/openapi.json differs from docs/openapi/gateway.json: cp docs/openapi/gateway.json crates/nexus-client/contract/openapi.json"
    );
}

#[test]
fn upper_snake_words_finds_setting_names_only() {
    assert_eq!(
        upper_snake_words("needs `NEXUS_X_1` and STRIPE_KEY, not Foo_Bar or a_b"),
        vec!["NEXUS_X_1", "STRIPE_KEY"]
    );
}
