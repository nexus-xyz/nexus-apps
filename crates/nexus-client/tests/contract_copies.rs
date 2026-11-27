//! `contract/` holds copies of Nexus's `openapi.json` and `mcp-tools.json` interface files, so the `mcp` feature builds
//! from the crate alone. Nexus maintains the copies. Where the interface files are present (`interfaces` below), the
//! copies must be the same bytes; otherwise there is no source to compare and the test returns.

use std::path::Path;

#[test]
fn the_contract_copies_are_the_interfaces_files() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let interfaces = manifest.join("../../../services/frontend/core/interfaces");
    if !interfaces.is_dir() {
        return;
    }
    for name in ["openapi.json", "mcp-tools.json"] {
        let source = std::fs::read(interfaces.join(name)).unwrap_or_else(|_| panic!("interfaces/{name} is read"));
        let copy = std::fs::read(manifest.join("contract").join(name)).expect("the copy is read");
        assert!(
            source == copy,
            "contract/{name} differs from interfaces/{name}; copy interfaces/{name} over it"
        );
    }
}
