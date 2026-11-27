# Desktop update coordinator

This crate owns update scheduling and state for Nexus Desktop.
Nexus for desktop uses it (`apps/desktop/src/updates.rs`), on the `desktop/<channel>/latest.json` feed.
The app reads its update file under its package key, `manifest::package_target()`, such as `darwin-aarch64-dmg`.
Older Nexus installs read `<os>-<arch>-<installer>`, then `<os>-<arch>`. The feed carries neither.
The app checks every 30 minutes, plus up to five minutes of jitter.

The core has no window, network, or system-clock dependency.
The desktop host supplies time and a feed, then publishes snapshots to every window.

`verify` checks an update file's minisign signature and signed version. `manifest` reads a channel's `latest.json`.
The `net` feature adds the network side. `access` decides the channel from the session.
`http` reads a feed and downloads a file. The host's reqwest features supply TLS.

Run `cargo test --locked -p nexus-desktop-updates --features net` from the repository root.
The tests use fake clocks and feeds, and a local fake HTTP feed.
The native adapter's signed-fixture test runs with `cargo test --locked --features updater --lib updates::tests`.

See the desktop README's "Updates" section for intervals, restart rules and channels.
