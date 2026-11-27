//! Emit the default command catalog for docs and release consumers, without configuration or a Gateway.
//! Run through `npm run commands -w @nexus/cli`, which disables optional features but keeps `staff` (the shipped CLI's).

use nexus_cli::{SEMVER, catalog::command_catalog, json::pretty};

fn main() {
    println!("{}", pretty(&command_catalog(SEMVER)));
}
