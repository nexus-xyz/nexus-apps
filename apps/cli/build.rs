//! The CLI's version, as `scripts/build.mjs` makes it for the TypeScript bundle: `<semver>+<git short sha>`,
//! e.g. `0.3.0+abc1234`. The semver is the Factory's, `eng/VERSION` (one version for the whole Factory:
//! eng/RELEASES.md); the sha is `NEXUS_BUILD_SHA` when set, else the checkout's HEAD (`build/version.rs`). Exposed as
//! `NEXUS_CLI_SEMVER` and `NEXUS_CLI_VERSION`.

include!("../../build/version.rs");

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let (semver, version) = factory_version(&manifest);
    println!("cargo:rustc-env=NEXUS_CLI_SEMVER={semver}");
    println!("cargo:rustc-env=NEXUS_CLI_VERSION={version}");
}
