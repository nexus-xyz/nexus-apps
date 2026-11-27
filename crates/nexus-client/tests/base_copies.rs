//! nexus-client's client-safe copies of nexus-base (`nexus_client::base`) answer as the originals: the
//! answers in `fixtures/client_copies.json` are nexus-base's, and nexus-base's `tests/client_copies.rs` checks them
//! and that this copy of the file matches its own. When nexus-base's answers change, copy its file here.

use nexus_client::base;
use serde_json::{Value, json};

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/client_copies.json");

fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("the golden file's shape")
        .iter()
        .map(|v| v.as_str().expect("the golden file's shape"))
        .collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

/// What the copies answer to `questions`.
fn answers(questions: &Value) -> Value {
    let versions = strings(&questions["versions"]);
    let parse: Vec<Value> = versions
        .iter()
        .map(|text| match base::version::Version::parse(text) {
            Some(v) => json!([v.major, v.minor, v.patch, v.prerelease, v.build]),
            None => Value::Null,
        })
        .collect();
    let compare: Vec<String> = versions
        .iter()
        .map(|a| {
            versions
                .iter()
                .map(|b| match base::version::compare_versions(a, b) {
                    Some(std::cmp::Ordering::Less) => '<',
                    Some(std::cmp::Ordering::Equal) => '=',
                    Some(std::cmp::Ordering::Greater) => '>',
                    None => '?',
                })
                .collect()
        })
        .collect();
    let binary_version: Vec<Option<String>> = questions["binaries"]
        .as_array()
        .expect("the golden file's shape")
        .iter()
        .map(|binary| {
            let mut code = vec![0u8; binary["zeros"].as_u64().expect("the golden file's shape") as usize];
            code.extend_from_slice(binary["text"].as_str().expect("the golden file's shape").as_bytes());
            base::version::binary_version(&code)
        })
        .collect();
    let sha256_hex: Vec<String> = questions["sha256_parts"]
        .as_array()
        .expect("the golden file's shape")
        .iter()
        .map(|parts| {
            let parts: Vec<Vec<u8>> = strings(parts).into_iter().map(unhex).collect();
            base::sha256_hex(&parts.iter().map(Vec::as_slice).collect::<Vec<_>>())
        })
        .collect();
    let decode_uri_component: Vec<Option<String>> = strings(&questions["uri_components"])
        .into_iter()
        .map(base::decode_uri_component)
        .collect();
    json!({
        "cli_version_header": nexus_client::CLI_VERSION_HEADER,
        "parse": parse,
        "compare": compare,
        "binary_version": binary_version,
        "images": {
            "max_images": base::images::MAX_IMAGES,
            "max_image_bytes": base::images::MAX_IMAGE_BYTES,
            "max_total_bytes": base::images::MAX_TOTAL_BYTES,
        },
        "sha256_hex": sha256_hex,
        "decode_uri_component": decode_uri_component,
    })
}

#[test]
fn the_copies_give_nexus_bases_answers() {
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(GOLDEN).expect("the golden file")).expect("JSON");
    assert_eq!(golden["answers"], answers(&golden["questions"]));
}
