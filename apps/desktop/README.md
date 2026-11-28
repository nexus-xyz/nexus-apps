# Nexus Desktop on GPUI

To install a release, download it from <https://nexus.inc/download>.

GPUI draws the native interface on the GPU. Embedded pages use CEF.
The app calls Nexus Gateway through `nexus-client`, which the `nexus` CLI also uses.
The shared crates are in `crates/`, members of this Cargo workspace: `nexus-workbench`, `nexus-desktop-core`, `nexus-desktop-mcp`, `nexus-host` and others.

## What it does

- Signs in with the device flow (`nexus auth login`'s): the browser opens to approve the computer.
- Keeps the token in the OS credential store under the entry `xyz.nexus.desktop`, one per Gateway URL.
  An older Nexus install (0.10.x and before) uses the same entry, so a sign-in there carries over, and signing out
  here signs out both.
- Lists your recent chat sessions, grouped by day (Today, Yesterday, Previous 7 days and so on), and opens one.
  Search filters them by title.
- Searches projects from the magnifier in the Projects row. The input replaces the label beside its button.
  It matches project names, branches, and session titles. Chats stay unchanged; Escape clears, then closes search.
- Sends a message and streams the answer as Markdown, with code highlighting.
- Lets you pick the model from the composer, or leave it on Auto.
- Stops an answer with Esc or the stop button. What streamed stays, marked as stopped.
- Offers a suggested next prompt after each answer, as the composer's placeholder. Tab takes it.
- Copies a message with the button under it.
- Follows a streaming answer only while you are at the bottom. Scroll up to read, and an arrow brings you back.
- Retries a failed answer with "Try again".
- Follows the system's light or dark appearance, with Nexus UI's Brand v5 colors.
- Runs once per person: a second launch brings the open window forward and exits.
  `nexus-desktop-gpui --new-window` opens another window in the running copy.
- Opens more windows with Cmd+Shift+N or the tray's New Window. Each has its own page and history. All share one
  sign-in and one Code sessions runner. The tray, a notification and a link go to the window focused last.
- Puts an item in the tray or menu bar. Closing the window keeps the app running there; Quit Nexus
  ends it. On Linux the item needs libayatana-appindicator (or libappindicator) and a StatusNotifierItem host. Without
  them there is no item, and closing the window quits.
- Opens Ask Nexus, the small chat window, from any app with the global shortcut (Alt+Space by default), a click on the
  tray item or its Ask Nexus row. Esc or a click elsewhere closes it, and Ctrl+Enter (Cmd+Enter on macOS) moves its
  chat to the app window. `NEXUS_DESKTOP_SHOW_PANEL=1` opens it at launch. It opens at login if
  you turn that on in Settings › App. An older Nexus install (0.10.x and before) shares the shortcut and the login
  item. On Linux the shortcut goes through X11 (XWayland on Wayland).
- Runs Code sessions on this computer (Claude Code or Codex): starts one in a project, sends to it, stops it, and
  answers its questions to use a tool. One app owns the sessions at a time. While another app holds them (an older
  Nexus install still running), this build only reads them and says so. Take over, then quit the other app: this
  build runs them once it closes.
- Switch between Claude Code and Codex from a Code session's model picker.
  Confirm which provider receives the original request and recent messages, and who pays for the new agent.
  The old agent and its sub-agents stop. The next message continues in the same thread and worktree.
  History stays in Nexus. The text handoff carries shell commands with the end of their output, and changed files.
  Private reasoning, what the agents read, other tool output, and earlier image attachments stay out.
  Switching back to an agent the session used continues that agent's own conversation, told only what it missed.
  It starts fresh instead after a payer change, a revert of turns it saw, or a move to a new worktree.
  Independently started ChatGPT app runs do not publish their live activity into this session.
- The session's three-dot menu opens Fork: "Fork from which point?" lists each recorded turn, the latest first.
  The latest turn can also go to another signed-in local subscription, which carries recent conversation text into a
  fresh agent conversation. Each fork gets a new worktree. The window's notice shows its progress and any failure.
- Shows a Code session's GitHub pull request on a page of its own: checks, reviews and review threads. It replies,
  resolves, reopens and merges through `gh`. The topbar's link and a pull request notification open it.
- Has a browser in each Code session's Workbench, as the Nexus app: up to eight tabs, each with its own history, and
  Reopen closed tab. A sign-in popup opens in a window of its own and keeps its opener. Pages go only to web addresses,
  and download nothing. Import sign-ins copies the chosen sites' cookies from your browser into this browser only.
  It checks the dev servers the agent printed and opens the first that answers. The camera button puts a screenshot of
  the page in your message.
- Lets a Code session's agent use that browser with the Nexus app's `nexus_browser` tools: open, read, click, type,
  screenshot and the console. Take over or Stop ends its control; its next call asks you again.
  A floating activity pill stays visible while a browser run has access, including over other apps.
  Hover to reveal details, click to pin, or press Escape to retract. Stop stays visible when retracted.
  Pause blocks browser operations until Resume. Take over revokes browser control while other work can continue.
- Routes every agent's browsing through those tools (`nexus-workbench/src/browser_routing.rs`). An agent that needs a
  capability they lack calls `browser_bypass` with the reason, and the conversation shows it. Claude Code runs refuse
  other browser tools until then, in every permission mode. Other agents get a notice when they skip the bypass.
- Records your screen for an agent on macOS: Alt+Shift+S (⌥⇧S), the tray's Record for an Agent or
  the panel's ⋮ menu starts it, and the same again or a click on the menu-bar item stops it. The folder an agent reads
  goes to Nexus's recordings folder, and its path and contact sheet go on the clipboard. Not run on a Mac yet.
- Keeps this computer and display awake while agents work, including background subagents and linked sessions.
  Settings › App › Keep awake while agents work is on by default and applies immediately.
  It releases after work finishes or you turn it off. It applies while Nexus is open, including in the tray.
  Manual locks and system policies still apply. Linux needs ScreenSaver and logind inhibition services on D-Bus.
- Keeps agents running after it quits when "Keep agents running when Nexus quits" is on (Settings › App).
  A shared background service keeps the sessions running; either app can reconnect.
  GPUI can start that service from its own binary.

Code sessions start with the Workbench closed. Each session keeps its visibility choice while Nexus stays open.
Follow agent selects the exact resource while that session's Workbench is open.
Choose Manual, switch tools, interact with content, or hide the pane to keep your place.
View latest opens one item in Manual. Follow agent resumes the latest ready item.
A hidden pane stays hidden and shows background activity. Resizing preserves the mode.
Following changes navigation only. Permission requests stay in the conversation, and Manual does not stop execution.

Workbench Terminal opens the computer's shell in the current folder.
Use New for another terminal, or Split for another shell beside the current one.
Normal shell keys, including Ctrl+C, reach the focused shell.
The terminal strip supports selection, double-click rename, and Close.
Hiding the Workbench keeps the shells running. Closing the primary window also keeps its private shells,
scrollback, renamed groups and splits while Nexus stays open in the tray or another window.
Reopening restores those terminals in fresh views under the same account and Gateway.
Closing a terminal explicitly, closing an additional window, signing out, switching identity or quitting ends its
private shells. With no tray and no other window, closing the last window quits and ends them.
CEF browser tabs also survive primary close/reopen while Nexus stays open, keeping the live page, selected tab
and history in fresh views. Browser tools are unavailable while the primary window is closed. Reopening under
the same identity resumes them without reloading the pages. Sign-in popups close; Servo pages are excluded.
Signing out, changing identity or quitting releases retained pages.

The primary window also keeps supported Back/Forward routes and each Settings entry's scroll, search and selection
while Nexus stays open. Reopening at the middle of the journal preserves its forward entries.
A new visit truncates that forward branch. Explicit links, tray actions and New Chat take priority over the parked route.
Settings positions wait for delayed content and yield to scrolling, search changes or a new destination.
Code entries verify their original local store or remote machine before loading. Identity changes and quit clear
the journal; additional windows remain independent. Full Workbench detachment remains unavailable.
Your agent conversation selection and page-specific tabs or filters remain outside this slice.
Native close/reopen acceptance remains pending.

Local Code sessions can detach their CEF Browser into a separate window.
Return to session or native close moves the same live tabs and history back, using a fresh app window if needed.
Browser tools, Stop, screenshots and text keep their original session after its app window closes or shows another
session. Output waits for a saved draft to finish loading; conflicting local copies keep output pending for Retry.
An explicit navigation choice cancels an automatic return route while the pages wait for their original session.
Signing out, changing identity or credentials, or quitting releases these owners. Full Workbench detachment,
Home/cloud browsers, Servo, sign-in popups and process restart remain outside this slice. Native CEF acceptance is pending.

Local Code Files can also move into a separate window and return.
Transfers preserve filters, expanded folders, Show all, file/line selection, preview mode, image zoom and PDF page.
Fresh views reread the original session folder. Open in editor keeps that folder/path/line and uses the current saved editor.
Agent completion refreshes detached file lists, including after the source window closes.
Files and Browser return independently, including when their shared destination waits for Account or local store readiness.
Identity or source changes retire pending reads and controls. Quit releases both tools without reopening source windows.
HTML/video previews rebuild; playback position and embedded navigation continuity remain separate gaps.
Home/cloud/remote/new-session Files, Commands, whole Workbench and process restart remain excluded. Native and media acceptance remain pending.

Local Code browser tools and approvals stay with the authenticated local store and session that issued the call.
Another store cannot take over a browser or answer its request, even when session IDs match.
Matching views can answer the same request; the original browser applies the answer. Account or credential changes,
retired views and ended runs reject outstanding calls. Creating a browser in a matching background window leaves
its selected pane unchanged. Browser detachment is tracked separately; native CEF acceptance is pending.

Main Gateway chats keep streaming and dispatching queued follow-ups after primary close while Nexus stays alive.
Reopening builds fresh conversation and composer views around the retained execution, including a
chat that has not received its session ID yet. Accepted sends settle once; later draft text and attachments survive.
Stop and failures hold the queue until Resume; Send now keeps its existing priority. Sign-out, account or Gateway
changes and quit revoke execution. Changed credentials also invalidate its original transport, including switching
away and back before UI events arrive. Additional windows keep independent owners.
Closing a panel or additional window stops its chats. Pending rewind operations,
transcript scroll and selection, Code/Your agent execution and restart are excluded. Native acceptance remains pending.
Refreshing the same nonempty credential preserves active Gateway requests.

Side chats also keep drafts, attachments, history and pending command outcomes across primary close/reopen.
Reopening attaches fresh views to the original local store or remote machine. An unavailable source keeps the draft read-only.
Completed sends clear only their submitted draft and attachments; later edits survive, and reopening never resends a message.
Restored chats refresh verified saved events, status and live partial text from the original owner.
Updated owners return coherent, bounded partial snapshots; older hosts fall back to completed responses and periodically retry support.
The shared bare-delta cache is never relabeled as belonging to a restored chat. Native local/service/cloud acceptance remains pending.
Secret entry fields clear on close. Identity changes or quitting discard retained side-chat state.

Agent commands appear under Commands above the conversation's composer.
Expand an execution to read its output. The viewer never forwards input or takes keyboard focus.
Command requests show Run command and Decline in the conversation.
Project commands and code-block Run actions show their output there without opening a human shell.

Motion follows Nexus UI's tokens (`nexus-ui.css`): a message you send lands from 12px below, an answer rises 6px,
menus fade in, and the sidebar slides in 280ms on the same ease-out curve.
Only what arrives now moves; a session you open does not animate its history.

Keys (Cmd on macOS, Ctrl on Windows and Linux):

| Key | Does |
| --- | --- |
| Enter, Shift+Enter | Send, new line |
| Cmd+N | New chat |
| Cmd+Shift+N | New window |
| Cmd+B | Show or hide the sidebar |
| Cmd+K | Search chats |
| Cmd+= / Cmd+- / Cmd+0 | Larger text, smaller text, normal size (Settings › Appearance) |
| Esc | Close the model menu, or stop the answer or the Code session |
| Tab | Take the suggested prompt, in an empty composer |
| Cmd+Q | Quit |

It also has the Inbox, the docs, the models catalog and the settings, drawn natively.
The organization's admin pages (members, teams, billing and the rest) open in the browser.
It includes a Workbench terminal and composer attachments (`src/side/terminal.rs` and `src/attachments.rs`).
It applies the organization's managed policy through `nexus-desktop-core` (`src/managed.rs`).

## Run it

Install the toolchain once:

| | What to install |
| --- | --- |
| Everyone | Rust 1.97 (`rustup toolchain install 1.97`), matching the build |
| macOS | The Xcode command line tools (`xcode-select --install`), cmake and Ninja (`brew install cmake ninja`). GPUI compiles its Metal shaders at run time, so full Xcode is not needed. |
| Linux (Arch) | `sudo pacman -S base-devel clang cmake fontconfig freetype2 wayland libxkbcommon libxkbcommon-x11 libxcb vulkan-icd-loader ninja alsa-lib gtk3 libayatana-appindicator` and your GPU's Vulkan driver |
| Linux (Debian, Ubuntu) | `sudo apt install build-essential clang cmake libfontconfig-dev libfreetype-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libvulkan1 ninja-build libasound2-dev libgtk-3-dev libayatana-appindicator3-dev` |
| Windows | The Microsoft C++ Build Tools ("Desktop development with C++"), and Ninja |

Then, from this folder:

```sh
cargo +1.97 run --locked --release
```

The first build takes several minutes. Use the committed `Cargo.lock`.
Use `--release` to judge the feel: a debug build of GPUI is much slower.

To build while you use the computer for other things (a call, say), give the build the lowest priority and fewer
cores:

```sh
nice -n 19 ionice -c 3 cargo +1.97 build --locked --release -j 4
```

Later builds recompile only this crate, about 1.5 minutes with `--release` for a one-line edit (73–103 s, measured).
For the edit loop, use the `fast` profile:

```sh
cargo +1.97 run --locked --profile fast
```

It optimizes every dependency as `release` does, so GPUI, web frames and images run at the same speed.
It builds this crate at opt-level 1, incrementally, so a one-line edit rebuilds in about 36 s (36–38 s, measured).
Measure performance and judge the shipped feel with `--release`.

Two variables skip work that every worktree repeats:

- `CEF_PATH=$HOME/.cache/nexus-cef` keeps one CEF download for all worktrees. Without it, each worktree downloads
  129 MB and unpacks 339 MB, and its next build compiles CEF and this crate again.
- `NEXUS_BUILD_SHA=local` stops a commit or a checkout from rebuilding this crate and `nexus-host` for the version's
  git sha. The build then reports `+local` as its sha.

Point it at another Gateway with `NEXUS_URL`, as the CLI does:

```sh
NEXUS_URL=http://localhost:8080 cargo +1.97 run --locked --release
```

### Connect a phone

Phone connection requires encrypted session sync.
Sync is on by default and needs no environment variable.
These gates still apply:

- The organization's `AllowLocalCache` policy allows local storage.
- You are signed in, and this computer has a device key on the same Gateway.
- A trusted device approved this computer, and you confirmed its approver under This device.
- The organization's `sessionContent` setting is `user` or `user_and_org_admins`.
- This build owns the Code sessions store.

Set up this computer in Settings › Devices.
Connect phone appears when encrypted session sync is ready.
Then turn on Phone control for this computer.

## Measure it

With `NEXUS_DESKTOP_MEASURE` set, the app times itself (`nexus_desktop_core::metrics`, `src/measure.rs`). It signs
in with the API key in `NEXUS_DESKTOP_MEASURE_KEY`, in memory and never in the keychain, and runs a measured send.
With `NEXUS_DESKTOP_MEASURE_OUT` naming a file, it writes its marks there as JSON when that send is done.
A harness launches the app with these variables, reads the file and compares runs. Nexus's own harness is not in
this repository.

This build marks:

- `process` first in `main`, and `window` when the main window opens;
- `first-frame` from GPUI's frame callbacks, once the first frame is on screen;
- `interactive` when the account and the recent sessions have arrived;
- `send`, `first-token`, `answer-done`, `send-2` and `first-token-2` as the UI sees the measured send;
- `send-core` and `first-token-core` as `gateway.rs` sees the request and its first token.

The interactive mark records account and session data arrival.
The token marks record UI event receipt.
Visible input latency and painted token latency need separate measurements.
With `NEXUS_DESKTOP_MEASURE_PAGES` set, the measured send is followed by a page journey, marked as `page-<n>-<event>`.
On macOS, measure the `dist` package (`packaging/package.sh`): the bare release binary has no CEF beside it, so a
page never opens.
Set `NEXUS_WEB_TRACE=1` when collecting embedded-page frame traces.

## Package it

Package a build with CEF, from this folder:

```sh
packaging/package.sh        # Linux (x86_64) and macOS (arm64)
pwsh packaging/package.ps1  # Windows (x86_64)
```

The scripts build with the `dist` profile in `Cargo.toml`: thin LTO and no symbols.
It builds slower than `release`, so build it only to ship.
Set `NO_BUILD=1` to package the `dist` binaries you already built.
The package lands in `target/package/`:

- Linux: a `.tar.xz` folder. CEF's files sit beside the binary, which finds them through its `$ORIGIN` rpath.
  Run `install.sh` in the folder to add the app to the launcher.
  For a separate Linux install, run `NEXUS_DESKTOP_APP_ID=nexus-desktop-gpui-custom ./install.sh`.
  Launch it through that entry so KDE uses its packaged icon.
  Custom launchers must set the same value in `Exec`, `StartupWMClass`, and the filename without `.desktop`.
- macOS: a `.dmg` holding `Nexus.app`, with CEF's framework and Chromium's helper apps inside. Each helper app
  holds the small helper binary (`src/helper.rs`), not a copy of the app. A plain `cargo run` on macOS has no bundle,
  so web views do not start there.
  The image opens a compact window with the app on the left and the Applications link on the right.
  `packaging/macos/dmg_layout.py` writes that window's `.DS_Store`, and `test_dmg_layout.py` checks it.
  Set `APPLE_SIGNING_IDENTITY` to sign with a Developer ID, from the inside out, with the hardened runtime and
  `packaging/macos/*.entitlements`. Without it, the app is signed ad hoc.
  Also set `APPLE_API_KEY_PATH`, `APPLE_API_KEY` and `APPLE_API_ISSUER` to notarize and staple the app and the `.dmg`.
- Windows: an installer `.exe` (`packaging/windows/installer.nsi`). It installs for the current user in
  `%LOCALAPPDATA%\Programs\Nexus`, with no administrator prompt, and adds a Start menu shortcut and an
  uninstaller. It removes an earlier `Programs\Nexus GPUI` install.

Every package carries only CEF's English locale (`locale` in `src/web/host.rs`).
It also carries `LICENSE`, `NOTICE` and `THIRD_PARTY_NOTICES.txt`, and CEF's license and Chromium's credits beside
CEF's files. The scripts fail when one is missing.

Signing needs your own certificates; without them the packages are unsigned or, on macOS, signed ad hoc.
On Windows, SmartScreen warns on an unsigned installer: choose "More info", then "Run anyway".

### Updates

The app updates through `nexus-desktop-updates` (`src/updates.rs`). It reads a channel's `latest.json`, picks its
package by its key, `<os>-<arch>-<format>` (`darwin-aarch64-dmg`, `windows-x86_64-exe`, `linux-x86_64-tarball`),
and installs it only when the package's minisign signature checks against the public key compiled in.
Settings › App shows the version, channel and state, with Check for updates and Restart to update.

The channels are stable (`desktop/stable/`), internal (`desktop/internal/`, Nexus employees) and beta
(`desktop-oss/beta/`, the `-beta.<N>` releases of this repository). A build stays on its own channel: the beta is
signed with its own key and a stable or internal build refuses a beta version. To try the beta from a stable or internal
install, install a beta by hand from this repository's
[releases](https://github.com/nexus-xyz/nexus-apps/releases); it then updates from the beta feed. A beta whose update
key is broken sends you to that page too, not to the stable download page.

- Linux: the new `.tar.xz` folder is unpacked beside this one and takes its place. The launcher entry still works.
- Windows: the installer runs silently with `/UPDATE` after the app quits, then starts the new version.
- macOS: Restart to update mounts the verified `.dmg` and checks the staged app's identity, version and code signature.
  It replaces the installed bundle and starts the new copy from the same path.
  macOS asks for administrator authorization when the installed app requires it.
  A failed replacement restores the previous app.

Automatic updates are on by default. The app checks at startup and every 30 minutes while open, plus up to five
minutes of jitter, then downloads and verifies what it finds.
The bottom-right prompt offers Restart and Not now. Turning automatic updates off keeps manual checks available.

Restart to update asks before stopping active local agents. New work started during confirmation requires another confirmation.
The app saves their session identifiers before installing. After reopening, Continue all resumes those conversations on the same sign-in and organization.
Review lets the person choose sessions and retry failures. Later keeps recovery for the next launch.
Agents in the background service, on servers, and in the cloud keep running across the UI restart.
A pending change that stops the background service still waits until its work finishes.
An organization that disables local caching must finish its agents before restarting, because their recovery cannot be saved.

A build run from `target/`, or one built without `NEXUS_UPDATER_PUBKEY` (the public half of a minisign key), has no
updater and says so. A build from this repository therefore never takes Nexus's packages: to ship updates of your
own, build with your key's public half and sign your packages with its secret half.
Nexus's releases also compile in the official OAuth client id as `NEXUS_DESKTOP_CLIENT_ID`.
A build without it, such as one from source, signs in as the shared third-party client.
A fork must also change `Channel::feed` in `crates/nexus-desktop-updates/src/lib.rs`, because the feed is hard-coded to
`downloads.nexus.inc`.

## How it is built

- `src/main.rs`: the app, the key bindings and the window.
- `src/helper.rs`: Chromium's helper processes on macOS, a separate small binary.
- `src/single_instance.rs`: one running copy. A second launch hands its arguments to the first and exits.
- `src/workspace/mod.rs`: the window's state, what changes it, the title bar and the sign-in.
  - `sidebar.rs`: New chat, search, the sessions by day and the account.
  - `transcript.rs`: the messages, the empty state and the jump to the latest.
  - `composer.rs`: the prompt box, the model menu and send or stop.
- `src/measure.rs` and `src/workspace/measure_send.rs`: the timing marks and the measured send ("Measure it").
- `src/motion.rs`: Nexus UI's durations and ease-out curve, and the arrive and fade-in animations.
- `src/backend.rs`: what the UI asks of Nexus, as one trait. The workspace holds a `Backend`, not the Gateway client,
  so a fake or a local engine can stand behind it.
- `src/gateway.rs`: every Gateway call, on a tokio runtime of its own. Answers come back to the UI as events on a
  channel, because GPUI runs its own executor and reqwest needs tokio.
- `src/web/`: pages (artifacts, Nexus Pages) in CEF, off screen, in a separate web host process.
  - `host.rs` and `handlers.rs`: the host process. It is this binary, started with `--nexus-web-host`.
  - `client.rs`: the app's side. The first page starts the host. The host exits 30 seconds after the last page closes.
  - `protocol.rs`: the commands and events, one JSON object per line. Frames go through a memory file.
  - `frames/`: GPU frames. `mod.rs` holds the switches every system shares; `linux.rs`, on Linux, frames as GPU
    buffers (dma-bufs) passed over a socket and drawn as textures.
  - `dmabuf.rs`: imports a dma-buf with Vulkan and copies it into a texture on GPUI's device.
  - `mod.rs`: `WebView`, the page as a GPUI element.
  - `snapshot.rs`: a picture of each page version, shown while the page opens again. It keeps 50 JPEGs in the cache.
- `vendor/`: patched copies of two GPUI crates, so GPUI can draw a texture made outside its renderer on Linux. Its
  README lists the changes.
- `src/code.rs`: the Code sessions, read from the shared store (`workbench.sqlite3`).
- `src/code_run.rs`: runs them, through `nexus-workbench`, when no other process holds the store's lock.
- `src/pull_page.rs` and `src/pages/pull_request.rs`: a Code session's pull request page. Its reads, writes and merge
  rules are `nexus_pull_requests::pull_page`.
- `src/service.rs` and `nexus_remote::engine`: the background service that keeps agents running, and the commands
  sent to it. The service is this binary, started with `service`.
- `src/model.rs`: the sessions and messages, read from Gateway's JSON.
- `src/palette.rs`: the Brand v5 colors.

## Decisions

These choices describe the current implementation and its remaining tradeoffs.

- **GPUI through GPUI Kit 0.9.0.** Zed does not publish a current `gpui` crate.
  Longbridge's `gpui-kit` pins a matching set of GPUI snapshots (`gpui-pre` 0.3.11) and adds the components: text
  input, Markdown view, buttons and the title bar. Bump the pin on purpose, because any snapshot can change GPUI's
  API.
- **Its own Cargo workspace.** The rs workspace never builds GPUI.
- **The shared keychain entry.** An older Nexus install (0.10.x and before) and this app use the same sign-in.
- **Colors copied by hand.** The palettes currently duplicate the brand tokens across CSS and Rust.
- **Esc and Tab through a keystroke interceptor.** GPUI runs keybindings before key listeners, and the composer binds
  both keys. The interceptor sees them first and lets them through when it has nothing to do.
- **A backdrop closes the model menu.** A click outside the menu lands on a transparent layer under it. A
  click-outside handler would close the menu and then reopen it when the click was on the menu's own button.
- **Stop aborts the request.** Gateway keeps whatever it stored. The session list refreshes after a stop.
- **No LTO in the release profile.** It makes the link much longer and heavier. Add it back for a shipped build.
- **CEF in a web host process, not in the app.** The app gets its memory back when no page is open. A page crash does
  not stop the app. CEF's message loop does not run on the UI thread.
- **Frames through a memory file, one at a time.** The host writes a frame, and writes the next only after the app
  reads it. So the app never reads a half-written frame. It costs one more copy than before, until GPU sharing.
- **GPU frames on Linux through patched GPUI crates.** Chromium paints into a dma-buf. The host passes it to the app
  over a socket, and the app copies it into a texture on GPUI's own device. The host holds the buffer until the
  copy is done (100ms at most). The CPU path stays as the fallback. We patch copies of the crates in `vendor/`
  instead of moving to a GPUI fork, so the change stays small and gpui-kit's pins stay as they are.
- **Chromium on X11 with ANGLE on Vulkan when GPU frames are on.** Only that pair shares filled buffers: the headless
  and Wayland backends paint no frame at all, and ANGLE on OpenGL paints empty ones. `VulkanFromANGLE` is needed
  too, or WebGL and worker canvases are missing. On Wayland, Chromium goes through XWayland.
- **Our own dma-buf import, not the `cef` crate's.** Chromium's buffers come without a modifier (the driver's own
  tiled layout). The `cef` crate imports every buffer with an explicit modifier, which scrambles these. Ours lets the
  driver read the layout from the buffer. It also copies only the visible part, into a texture kept per page.
- **A page with no GPU frame in 8 seconds turns GPU frames off.** It opens again with CPU frames, as after a failed
  import.
- **DevTools Protocol through CEF's API, not a debugging port.** An agent browser can drive a page. No other program
  on the computer can.

- **Our own workbench host for Code sessions, not `nexus-host`.** `nexus-host` needs Unix sockets and does not
  build on Windows. `code_run.rs` ports its launch: the agent starts through Gateway with the keychain's token.
  It gets the shared MCP servers and the built-in Nexus server (`src/mcp.rs`, this binary with `--mcp`).
  It starts on the plan account named in the shared desktop settings.
  The native MCP editor updates those servers and their saved credentials for the next agent start.
- **New-session choices are remembered here, not shared.** They go in this build's own settings file, per
  organization. An older Nexus install keeps its own in its webview's storage, which this build cannot read.
- **One copy per person, through a lock file and a local socket.** The first launch locks
  `nexus-desktop-gpui/run/instance.lock` in the cache folder and listens on a socket beside it (a named pipe on
  Windows). A later launch sends its arguments there and exits. The OS drops the lock when the app ends, so a crash
  leaves nothing to clean up. It runs in debug builds too: a `cargo run` while a packaged copy is open brings that copy
  forward.
- **One owner for the Code sessions.** This build, the background service and an older Nexus install share the
  store. Whichever starts first takes its lock and runs the agents. The others read the store every 2 seconds. With Keep agents running on, the
  background service holds the lock, and each app sends it the commands.

## Known gaps

- Hover and press colors change at once. GPUI has no CSS-style transitions, so the 80ms fade on hover is not there.
- Cmd+K may clash with a text field's own Ctrl+K (delete to the end of the line) on Linux and Windows.
- The session list does not page past the 50 most recent sessions.
- No accessibility pass. GPUI's screen-reader support is new (AccessKit, 2026-05).
- GPU frames are Linux only and off by default (`NEXUS_WEB_GPU=1` turns them on). Every system copies frames
  through the CPU until they are tested on more GPUs. macOS (IOSurface into Metal) is the next platform for them.
- GPU frames are tested on AMD with Mesa only. NVIDIA's buffers carry explicit modifiers and take the other import
  path, untested.
- Without GPU frames, Linux uses the headless backend, which has no system clipboard.
- Chromium's sandbox is off by default. On Linux, `NEXUS_CEF_SANDBOX=1` turns it on where the system lets the app
  make user namespaces (`sandbox_on` in `src/web/host.rs`). Elsewhere, and on macOS and Windows, pages run without it.
- The macOS and Windows packages are not tried on those systems yet.

- Export copies a Code session's full conversation as Markdown or saves it to a file you choose.

## Interactive command shells

Run starts the displayed command in the small shell below it.
Type answers, paste text, or use Ctrl+C directly in that shell.
The command runs once; you do not retype it.
For an agent-owned command, choose Use shell to take input control.

Commands separates Active from History.
Decline starts no process and moves the request to History.
Delete individual history rows or all declined rows, then use Undo to restore them.
Deletion preserves the execution receipt, so retries cannot approve or repeat a declined command.

### Command routing and recovered history

Agent commands use the shared workbench even in Full access.
Commands from an unavailable provider route show their bypass reason in the conversation and command history.
Nexus never replays a command to display it.

History survives restart; raw output and process handles do not.
An unconfirmed launch shows Unknown outcome.
Check its effects before running it again.
Expanded persistent-shell cards show shared agent input, without private or human input.
