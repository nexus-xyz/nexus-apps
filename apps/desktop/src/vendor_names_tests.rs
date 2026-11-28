//! The app names no sign-in vendor: its code, types and words say "sign-in provider". A field Gateway still sends only
//! under the vendor's name is read in one place, behind a neutral Rust name, and listed in `WIRE_NAMES` until Gateway
//! sends a neutral one.

use std::path::{Path, PathBuf};

/// The vendor's name, lowercase. Built from parts so that this file does not name it.
fn vendor() -> String {
    ["cl", "erk"].concat()
}

/// Team sharing's end-to-end test (`gateway_e2e/team.rs`).
const TEAM_E2E: &str = "crates/nexus-workbench/src/session_sync/gateway_e2e/team.rs";

/// Wire names Gateway sends only under the vendor's name: the file that reads it and the quoted literal, with `{}` for
/// the vendor. Each is a follow-up: Gateway sends a neutral name, the app reads it, the row goes.
const WIRE_NAMES: [(&str, &str); 8] = [
    // `GET /v1/orgs`: the provider's organization id, Nexus UI's URL segment when the organization has no slug.
    ("src/workspace/org_menu.rs", "\"{}OrgId\""),
    // An audit event's actor key when the provider's webhook made the change.
    ("src/pages/audit.rs", "\"{}\""),
    // Team sharing's end-to-end test signs people in the way Nexus UI's server does: its trusted identity headers and
    // `POST /v1/ui/session` body, and the provider-style ids it makes up for them.
    (TEAM_E2E, "\"x-nexus-{}-user-id\""),
    (TEAM_E2E, "\"x-nexus-{}-org-id\""),
    (TEAM_E2E, "\"{}OrgId\""),
    (TEAM_E2E, "\"{}UserId\""),
    (TEAM_E2E, "\"{}Role\""),
    (TEAM_E2E, "\"{}_{person}\""),
];

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if path.is_dir() {
            if !matches!(name, "target" | "node_modules" | ".git") {
                files(&path, out);
            }
        } else {
            out.push(path);
        }
    }
}

/// Every `file:line` under `root` that names the vendor, in any case, once the allowed wire names are taken out.
fn vendor_names(root: &Path, allowed: &[(&str, &str)]) -> Vec<String> {
    let vendor = vendor();
    let mut all = Vec::new();
    files(root, &mut all);
    all.sort();
    let mut found = Vec::new();
    for path in all {
        let relative = path
            .strip_prefix(root)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        for (number, line) in text.lines().enumerate() {
            let mut line = line.to_string();
            for (file, literal) in allowed {
                if *file == relative {
                    line = line.replace(&literal.replace("{}", &vendor), "");
                }
            }
            if line.to_ascii_lowercase().contains(&vendor) {
                found.push(format!("{relative}:{}", number + 1));
            }
        }
    }
    found
}

#[test]
fn the_app_names_no_sign_in_vendor() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let found = vendor_names(root, &WIRE_NAMES);
    assert!(
        found.is_empty(),
        "say \"sign-in provider\" instead, at:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_check_finds_the_name_in_any_case_and_allows_only_listed_wire_names() {
    let dir = std::env::temp_dir().join(format!("vendor-names-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).expect("dir");
    let vendor = vendor();
    let upper = vendor.to_ascii_uppercase();
    std::fs::write(
        dir.join("src/a.rs"),
        format!("let id = text(\"{vendor}OrgId\");\n// {upper} said no\n"),
    )
    .expect("a");
    std::fs::write(dir.join("src/b.rs"), format!("let id = text(\"{vendor}OrgId\");\n")).expect("b");
    let found = vendor_names(&dir, &[("src/a.rs", "\"{}OrgId\"")]);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(found, ["src/a.rs:2", "src/b.rs:1"]);
}
