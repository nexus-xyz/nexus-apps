# Contributing to Nexus Desktop

Thanks for helping. This guide says how to set up, what we expect in a pull request, and how to sign your work.
By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Before you start

- Search the [issues](https://github.com/nexus-xyz/nexus-apps/issues) first. Someone may have reported it already.
- For a bug, open an issue with steps to reproduce, your platform and the app version.
- For a large change or a new feature, open an issue and agree on the approach before you write code.
- For a security problem, do not open an issue. Follow [SECURITY.md](SECURITY.md).
- Issues labeled `good first issue` and `help wanted` are good places to start.

## Set up

Follow "Build from source" in the [README](README.md). Then run the app in the `fast` profile:

```sh
cargo +1.97 run -p nexus-desktop-gpui --locked --profile fast
```

These settings help while you work:

- `CARGO_PROFILE_DEV_DEBUG=0` turns off debug info. It fills a lot of disk for this crate.
- `CEF_PATH=$HOME/.cache/nexus-cef` shares one CEF download between checkouts.
- `NEXUS_BUILD_SHA=local` avoids rebuilds when the commit changes.

A guide for where code goes, how state moves, and the rules that keep frames cheap will come later.
Until then, follow the patterns in the code next to your change.

## Tests and checks

CI checks each package on its own, never with `--workspace`. The desktop app turns on features of shared crates
that the CLI leaves off, so a workspace build tests a different CLI. Run the checks for the parts you changed
before you push. They are the commands the workflows in `.github/workflows` run.

Formatting, for every change:

```sh
cargo fmt --all -- --check
```

The desktop app (`desktop.yml`, on Linux, macOS and Windows):

```sh
cargo clippy --locked -p nexus-desktop-gpui --all-targets
cargo nextest run --locked -p nexus-desktop-gpui
```

The CLI (`cli.yml`):

```sh
cargo clippy --locked -p nexus-cli --no-default-features --all-targets -- -D warnings
cargo nextest run --locked -p nexus-cli --no-default-features
```

The shared crates (`crates.yml`), and the dependency licenses and sources:

```sh
cargo clippy --locked --workspace --exclude nexus-desktop-gpui --exclude nexus-cli --all-targets --all-features -- -D warnings
cargo nextest run --locked --workspace --exclude nexus-desktop-gpui --exclude nexus-cli --all-features
cargo deny --locked check licenses sources
```

The tests use [cargo-nextest](https://nexte.st). `cargo test --locked -p <package>` also works.
[cargo-deny](https://github.com/EmbarkStudios/cargo-deny) checks dependencies against `deny.toml`.
Pull requests run the same checks, so keep them green.
The CLI and the shared crates fail on any clippy warning. The desktop app is not clippy-clean yet, so add no new
warnings there.
GPUI tests use `TestAppContext` and run without a display. `apps/desktop/src/pages/ui_tests.rs` and `apps/desktop/src/motion/mark.rs` show the pattern.

Every fix lands with its test. For a bug fix, write a test that fails before your change and passes after it.
For a feature, add tests for the behavior you add. If a change cannot be tested, say why in the pull request.

## Style

- Run `cargo fmt`. The repository's `rustfmt.toml` sets the rules.
- Put logic that has no UI (sorting, labels, parsing, defaults) in a plain function with unit tests, not in `render`.
- Keep `render` cheap. It builds elements and does no heavy work.
- Log errors. Do not discard a `Result` with `let _ =`.
- In comments, docs and commit messages, use plain, short sentences.

## Sign your work (DCO)

We use the [Developer Certificate of Origin](https://developercertificate.org/), version 1.1. It says you wrote the change,
or you have the right to submit it under this project's license. You do not sign a separate agreement.

Add a sign-off line to every commit:

```sh
git commit -s -m "fix: describe the change"
```

This adds a line like the one below, using your `user.name` and `user.email`:

```text
Signed-off-by: Your Name <you@example.com>
```

Use your real name. A pull request with unsigned commits cannot be merged.
To fix commits you already made, run `git rebase --signoff main` and force-push your branch.

By contributing, you agree that your contribution is licensed under the [MIT license](LICENSE).

## Pull requests

1. Fork the repository and create a branch from `main`.
2. Make one focused change. Small pull requests get reviewed faster.
3. Add or update tests, and run the checks above.
4. Update the docs if behavior changes.
5. Open a pull request and fill in the template. Say what changed, why, and how you tested it.
6. A maintainer reviews it. Answer comments by pushing more commits to the same branch.
7. When it is approved and the checks pass, a maintainer squash-merges it.

We aim to give a first response within a few working days.

`.greptile/`, `.review-bot/`, `AGENTS.md` and `CLAUDE.md` are maintainer tooling, for review bots and coding agents.

## Help

Ask questions in [GitHub Discussions](https://github.com/nexus-xyz/nexus-apps/discussions).
