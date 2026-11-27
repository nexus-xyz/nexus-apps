# License policy for Nexus Desktop's own code

This page says which license Nexus's own desktop code carries, and how a file shows it.
It is for anyone who adds a crate or a file to the app, and for the legal review of the open-source export.
Status: approved by legal.

## The license

- Nexus's own crates are MIT: this app, the ten shared crates in `crates/`, and
  `nexus-client`.
- Each crate says so in its `Cargo.toml`, with `license = "MIT"`.
- The repository carries the license text once, as `LICENSE` at its root, with a `NOTICE` beside it.
  MIT needs no `NOTICE` of its own. The file stays because the Apache-2.0 code
  that the app ports or vendors (GPUI, GPUI Base, zui) must carry its notices.
- Code that a project offers under "Apache-2.0 OR MIT" is taken under MIT, the same license as ours.
- Contributions come in under the same license, with a Developer Certificate of Origin sign-off on each commit.
  `CONTRIBUTING.md` at the repository root says how.

## File headers

- A file Nexus wrote has no license header. The crate's `license` field and the root `LICENSE` cover it.
  Zed's GPUI and most Rust crates work this way.
- A file copied or adapted from another project keeps that project's license.
  Its first comment names the source, the license and the copyright line, as `src/workspace/launch_motion.rs` does.
  The project also gets an entry in `NOTICE`.
- A vendored crate (`vendor/`) keeps its own license files. A changed file in it carries a comment that starts with
  "Nexus" where it changed, and `vendor/README.md` lists the changes.
- Do not add `SPDX-License-Identifier` lines to Nexus files one by one. If legal asks for them, add them to every
  file in one change, with a check, so that they never drift.

## The checks

- `cargo deny --locked check licenses sources` (`deny.toml`) checks every crate the app builds, Nexus's crates
  included. A crate with no license, or a license outside the allowed list, fails.
- `licenses/notices.py` fails unless every Nexus crate (a path dependency outside `vendor/`) declares the license in
  `licenses/inputs.toml` (`first_party_license`). CI runs it with `--check`.
