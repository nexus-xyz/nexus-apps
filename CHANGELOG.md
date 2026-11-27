# Changelog

Notable changes to what this repository ships. Each artifact has its own version. [RELEASING.md](RELEASING.md) says
where each version is set and how each is tagged.

Current versions:

- Nexus Desktop: 0.11.2
- `nexus` CLI: 0.45.0 (the root `VERSION` file)
- `nexus-client`, `nexus-agents`, `nexus-providers`, `nexus-local-agents`, `nexus-agent-memory`: 0.1.0

No version has been released from this repository yet.

## Unreleased

### Documentation

- README: sections for the `nexus` CLI and the shared crates, a note that a build from source uses stand-in logos,
  and a note that the rich-text renderer (`renderer.js`, `renderer.css`) is prebuilt and its source is not published yet.
- CONTRIBUTING: the per-package checks that CI runs, including `cargo deny check licenses sources`.
- RELEASING.md and this changelog.
- A README for `nexus-agents`, `nexus-providers`, `nexus-local-agents` and `nexus-agent-memory`.

### Legal

- NOTICE: the written offer for the LGPL source of CEF and FFmpeg now goes to <legal@nexus.inc>.
- NOTICE: points to `apps/desktop/THIRD_PARTY_NOTICES.txt` and to the Apache-2.0 text for the code adapted from
  gpui-kit, and lists every file with code ported from zui.
- Code adapted from gpui-kit and zui now carries a header naming its source, its license and that Nexus modified it.
- TRADEMARKS.md no longer mentions a mobile app. This repository has none.
