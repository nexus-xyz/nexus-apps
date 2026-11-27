# Releasing

This repository ships three kinds of artifact: the desktop app, the `nexus` CLI and the shared crates.
Each has its own version and its own tags. No release has been cut from this repository yet.

## Versions

| Artifact | Version now | Where it is set |
| --- | --- | --- |
| Nexus Desktop (`apps/desktop`) | 0.11.2 | `version` in `apps/desktop/Cargo.toml` |
| `nexus` CLI (`apps/cli`) | 0.45.0 | the root `VERSION` file |
| `nexus-client` | 0.1.0 | `version` in `crates/nexus-client/Cargo.toml` |
| `nexus-agents`, `nexus-providers`, `nexus-local-agents`, `nexus-agent-memory` | 0.1.0 | `[workspace.package] version` in the root `Cargo.toml` |

The root `VERSION` file is the CLI's version. `apps/cli/build.rs` reads it through `build/version.rs` and adds the
git short sha, so `nexus version` says `0.45.0+<sha>`. The `version` in the CLI's own `Cargo.toml` (0.1.0, from the
workspace) is not used by the CLI.

`VERSION` does not set the desktop app's version. The app's version is `version` in `apps/desktop/Cargo.toml`. A
release build overrides it with `NEXUS_DESKTOP_BUILD_VERSION`, which the release workflow takes from the tag.
The app and `nexus-host` read `VERSION` only to get the git sha they add to the host's version.

The crates' versions are separate again. `nexus-client` has its own. The other four take the workspace version.
The internal crates (`publish = false`) are never released on their own.

## Tags

| Artifact | Tag | Example |
| --- | --- | --- |
| Desktop, stable | `v<x.y.z>` | `v0.12.0` |
| Desktop, beta | `v<x.y.z>-beta.<N>` | `v0.12.0-beta.1` |
| Desktop, rehearsal (publishes nothing) | `v0.0.0-test.<N>` | `v0.0.0-test.1` |
| CLI | defined by its release workflow | |
| Crates | none yet | |

The desktop release workflow (`.github/workflows/release.yml`, open in pull request #20) runs on every `v*` tag.
It builds a beta from `v<x.y.z>-beta.<N>` and rehearses from `v0.0.0-test.<N>`. It refuses any other `v*` tag,
including a stable one, until stable releases are added. So a tag for another artifact must not start with `v`.

The desktop's release steps, and what the release needs, are in `apps/desktop/packaging/RELEASE.md` (also in #20).

A published version never changes. To fix a release, tag the next number.

## Before you tag

1. Make sure the commit is on `main` and its checks are green.
2. For the CLI or a crate, set the new version in a pull request first. A desktop release build takes its version
   from the tag.
3. Move the entries under "Unreleased" in [CHANGELOG.md](CHANGELOG.md) to a section for the new version.
