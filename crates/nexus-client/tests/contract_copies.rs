//! `contract/openapi.json` is a copy of `openapi/gateway.json` from nexus-xyz/nexus-api-service: the Gateway's
//! document, cleaned of server-only details by that repo's generator (`tools/openapi-gen/redact.cjs`). Copy it from
//! there, never from the Gateway: `cp <nexus-api-service>/openapi/gateway.json crates/nexus-client/contract/openapi.json`.
//! `contract/mcp-tools.json` is this crate's own list. The copy cannot be compared with its source here, so this test
//! checks that it is the cleaned one.

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
        "contract/openapi.json names server settings ({names:?}): copy nexus-api-service's openapi/gateway.json"
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
            "contract/openapi.json has {internal}: copy nexus-api-service's openapi/gateway.json"
        );
    }
}

#[test]
fn upper_snake_words_finds_setting_names_only() {
    assert_eq!(
        upper_snake_words("needs `NEXUS_X_1` and STRIPE_KEY, not Foo_Bar or a_b"),
        vec!["NEXUS_X_1", "STRIPE_KEY"]
    );
}
