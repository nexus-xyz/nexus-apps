//! `contract/` holds copies of `openapi.json` and `mcp-tools.json` from `eng/factory/services/frontend/core/interfaces`,
//! so the `mcp` feature builds from the crate alone. While the monorepo holds both, they are the same bytes. Outside it
//! there is no source to compare.

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
            "rs/crates/nexus-client/contract/{name} differs from services/frontend/core/interfaces/{name}. \
             From eng/factory, copy it: cp services/frontend/core/interfaces/{name} rs/crates/nexus-client/contract/"
        );
    }
}
