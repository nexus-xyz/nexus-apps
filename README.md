# Nexus Desktop

Nexus Desktop is a native desktop app for working with AI agents. It runs on macOS, Linux and Windows.
It draws its interface on the GPU with [GPUI](https://www.gpui.rs), the UI framework behind the Zed editor.

<!-- Screenshot placeholder: add a window screenshot at docs/images/screenshot.png, then replace this comment with ![The app window](docs/images/screenshot.png) -->

## What it does

- Signs you in with a browser-approved device flow and keeps the token in the operating system's credential store.
- Chats with models, streams answers as Markdown with code highlighting, and lets you pick a model or leave it on Auto.
- Runs coding agent sessions on your computer (Claude Code or Codex) and shows their work.
- Gives each coding session a workbench with a browser, a terminal, files and changes.
- Shows a session's GitHub pull request: checks, reviews and review threads.
- Lives in the tray or menu bar, and opens a small ask window from a global shortcut.

You need a Nexus account to sign in. The app talks to the Nexus Gateway through the `nexus-client` crate.
Point it at another Gateway with the `NEXUS_URL` environment variable.

## Install

Download a package for your platform from the [download page](https://nexus.inc/download).

## Build from source

You need Rust 1.97. `rust-toolchain.toml` pins it, so rustup installs it, with rustfmt and clippy, on the first
`cargo` command in the repository.

Then install the system packages for your platform.

| Platform | What to install |
| --- | --- |
| macOS | Xcode command line tools (`xcode-select --install`), then `brew install cmake ninja` |
| Linux (Debian, Ubuntu) | `sudo apt install build-essential clang cmake libfontconfig-dev libfreetype-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libvulkan1 ninja-build libasound2-dev libgtk-3-dev libayatana-appindicator3-dev` |
| Linux (Arch) | `sudo pacman -S base-devel clang cmake fontconfig freetype2 wayland libxkbcommon libxkbcommon-x11 libxcb vulkan-icd-loader ninja alsa-lib gtk3 libayatana-appindicator`, plus your GPU's Vulkan driver |
| Windows | Microsoft C++ Build Tools ("Desktop development with C++") and Ninja |

Build and run from the repository root:

```sh
cargo run -p nexus-desktop-gpui --locked --release
```

The first build takes several minutes. Use the committed `Cargo.lock`.
A debug build of GPUI is much slower, so use `--release` to judge how the app feels.

For a faster edit loop, use the `fast` profile:

```sh
cargo run -p nexus-desktop-gpui --locked --profile fast
```

Two optional settings save time when you build more than once:

- `CEF_PATH=$HOME/.cache/nexus-cef` shares one download of the embedded browser (CEF) between checkouts.
- `NEXUS_BUILD_SHA=local` stops each commit from rebuilding the app for its version string.

## Architecture

The app is one Rust crate that draws its windows with GPUI. A root `Workspace` view holds the sidebar, the main area,
the side pane and the overlays. Calls to the Gateway go through `nexus-client`. Coding sessions, the workbench and
the shared host service live in the `nexus-workbench`, `nexus-host` and `nexus-remote` crates. Embedded web pages run
in CEF, in a separate web host process, so a page crash does not stop the app. Longer
architecture and contributor guides will come later.

## Contributing

We welcome bug reports, fixes and features. Read [CONTRIBUTING.md](CONTRIBUTING.md) first.
All participants follow the [Code of Conduct](CODE_OF_CONDUCT.md).
To report a security problem, follow [SECURITY.md](SECURITY.md).
What the app sends over the network, and how to turn usage data off, is in [PRIVACY.md](PRIVACY.md).
Project decisions are described in [GOVERNANCE.md](GOVERNANCE.md).

## License

Nexus Desktop is licensed under the [MIT license](LICENSE).
Third-party code keeps its own license. Its notices are in [NOTICE](NOTICE).
The Nexus name and logo are not covered by that license. See [TRADEMARKS.md](TRADEMARKS.md).
