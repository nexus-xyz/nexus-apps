# Privacy: what Nexus Desktop sends

This page lists everything the app sends over the network, and why.
Read it if you want to know what leaves your computer before you install or build the app.

## Summary

- Usage data is off until you turn it on. The app asks once, at first start. "No" is the default.
- Usage data never contains your prompts, your code, model output, file names or paths, or repository names.
- Every usage field is listed below. If a field is not on this page, the app does not send it as usage data.
- Crash reports have their own setting. See [Crash reports](#crash-reports).
- The app also talks to servers to do its work: to sign you in, run models and check for updates.
  Those calls are listed under [Network traffic the app needs](#network-traffic-the-app-needs).
- Builds from source send no usage data. Only official release builds can send it.

## Who is responsible

Nexus Laboratories, Inc. decides how usage data is used. It is the controller of that data.
The [Nexus privacy policy](https://nexus.inc/legal/privacy) explains your other rights, how to complain to a
data protection authority, and how data is transferred to the United States.

## Usage data

### Turn it on or off

- At first start, the app asks: "Help improve Nexus by sending usage data?" The buttons are "Send usage data" and "Don't send".
- Change your choice any time in Settings › Privacy › "Send usage data".
- Set `NEXUS_TELEMETRY=off` or `DO_NOT_TRACK=1` in the environment to turn it off.
  Either variable wins over the setting, and the app does not ask.
- When usage data is off, the app sends nothing from this section. It does not send an "opted out" event.

### The install ID

- When you turn usage data on, the app makes a random ID for this install, for example `3f1c9a2e-5b7d-4e1a-9c0f-2d8b6a4e7f13`.
- The ID is stored in the app's data folder. It is not made from your computer, your account or your hardware.
- The app never sends the install ID together with your account token. We do not link it to your account.
- The install ID is a pseudonym, not anonymous data. It lets us count one install across days.
- Settings › Privacy shows the ID, so you can quote it in a deletion request.
- When you turn usage data off, the app shows the ID one last time, with a button to copy it. Then it deletes the ID.
  If you turn usage data on again, you get a new ID.

### What the app sends: install data

One event, at most once a day, while usage data is on.

**`daily_active`**: this install was used today. It counts active installs and how many sign in to Nexus Cloud.

| Field | Type | Example | Why we need it |
| --- | --- | --- | --- |
| `install_id` | UUID | `3f1c9a2e-5b7d-4e1a-9c0f-2d8b6a4e7f13` | Counts one install once per day, and over time |
| `day` | date (UTC) | `2026-11-24` | The day the app was used. An event sent late still counts for the right day |
| `app_version` | string | `0.42.0` | Shows which versions are in use, and when to stop supporting old ones |
| `channel` | the release channel name | `stable` | Splits release channels |
| `os` | `macos`, `linux` or `windows` | `macos` | Shows which platforms to test and build first |
| `os_version` | major and minor only | `15.1` | The same, per OS version |
| `arch` | `arm64` or `x86_64` | `arm64` | The same, per CPU type |
| `modes` | list from a fixed set: `own_subscription`, `own_key`, `own_model`, `cloud` | `["own_subscription", "cloud"]` | How the app reaches models. `cloud` means you are signed in to Nexus Cloud today |

### What the app sends: diagnostics while signed in

When you are signed in to Nexus Cloud and usage data is on, the app also sends these events.
They go with your account token, so they are linked to your account. They are covered by the
[Nexus privacy policy](https://nexus.inc/legal/privacy).

**Activity**: at most once a minute, while you use the app.

| Field | Type | Example | Why we need it |
| --- | --- | --- | --- |
| `surface` | `code`, `chat`, `review`, `pages` or `other` | `code` | Which part of the app is in use |
| `app` | the app's name | `desktop-gpui` | Which app sent it |
| `appVersion`, `os`, `osVersion` | as above | `0.42.0`, `macos`, `15.1` | Splits activity by version and platform |

**Error count**: when something fails. Up to 20 a minute, sent once a minute.

| Field | Type | Example | Why we need it |
| --- | --- | --- | --- |
| `kind` | `crash`, `render`, `network`, `sign_in`, `update`, `agent`, `storage` or `other` | `network` | What kind of failure |
| `component` | a name from the app's code | `gateway` | Where in the app it failed |
| `code` | a name from the app's code | `timeout` | Which failure |
| `fatal` | true or false | `false` | Whether the app stopped |
| `occurredAt` | time (UTC) | `2026-11-24T14:03:11Z` | When it happened |
| `app` | the app's name | `desktop-gpui` | Which app sent it |
| `appVersion`, `os`, `osVersion` | as above | `0.42.0`, `macos`, `15.1` | Splits errors by version and platform |

An error count never contains an error message, a stack trace, a path, a URL or an email address.

**Page load failure**: when a shared page fails to load in the app.

| Field | Type | Example | Why we need it |
| --- | --- | --- | --- |
| `kind` | a name from the app's code | `timeout` | What failed |
| `status` | HTTP status, or empty | `502` | The server's answer |
| `host` | the page's host name | `abc123.nexususercontent.dev` | Which page server failed |
| `artifactId` | the shared page's ID | `art_7Hq2Lm9Xr4Tc` | Which page failed to load |
| `surface` | always `subdomain` | `subdomain` | How the page was loaded |
| `browser` | always `chrome`, the embedded browser | `chrome` | Which browser engine failed |
| `app`, `appVersion` | as above | `desktop-gpui`, `0.42.0` | Splits failures by app and version |

**Connected plan tier**: added to the start of a coding session.

| Field | Type | Example | Why we need it |
| --- | --- | --- | --- |
| `plan` | `free`, `pro`, `max`, `team`, `enterprise` or `unknown` | `max` | Which Claude or ChatGPT plan the agent runs on. Never the plan's account, email or raw name |

### What usage data never contains

- Prompts, messages, or anything you type.
- Model output.
- Code, file contents, file names or paths.
- Repository, branch, folder or project names.
- Web addresses you open in the app's browser.
- Terminal input or output.
- Your name, email address, account ID, computer name or operating system user name.
- API keys, tokens or passwords.

### Where it goes and how long we keep it

- The app sends usage data to `https://api.nexus.inc` over HTTPS.
- If you point the app at another Gateway with `NEXUS_URL`, the app does not send `daily_active` at all.
  The signed-in diagnostics go to that Gateway, like every other Gateway call. Its operator's terms apply to them.
- The server reads your IP address to answer the request. It does not store it with usage data.
- We keep each event for 90 days. After that we keep only totals, such as active installs per day, with no install ID.
- We do not sell usage data, and we do not share it with advertisers.
- We process usage data only with your consent. You can take it back at any time by turning usage data off.

### Your choices

- Turn usage data off at any time. Nothing more is sent from then on.
- To ask us to delete the events of an install, email <privacy@nexus.inc> with the install ID.

## Crash reports

When the app crashes, it can send a crash report the next time it starts. A crash report is not usage data.
It has its own setting: Settings › Privacy › "Send crash and error reports".

- Setting off (the default): after a crash, the app asks you. You can add a note, send the report, or choose "Do not send".
- Setting on: the app sends the report without asking.
- A report is sent only while you are signed in to Nexus Cloud. It goes with your account token.

| Field | Type | Example |
| --- | --- | --- |
| `category` | always `crash` | `crash` |
| `message` | your note, if you write one, at most 4,000 characters | `It crashed when I closed a tab` |
| `appVersion`, `platform`, `osVersion` | the app's version and your operating system | `0.42.0`, `macos`, `15.1` |
| `crash.kind`, `crash.fatal` | always `crash` and `true` | `crash`, `true` |
| `crash.name` | the error's name, at most 120 characters | `panic` |
| `crash.message` | the first line of the error, at most 200 characters, with anything that looks like a secret removed | `index out of bounds` |
| `crash.stack` | a list with at most one place in the app's code, at most 300 characters. Empty when the place is outside the app's code | `["at panic (src/workspace/mod.rs:12:5)"]` |
| `crash.occurredAt` | time (UTC) | `2026-11-24T14:03:11Z` |
| `crash.appVersion`, `crash.platform`, `crash.osVersion` | as above | `0.42.0`, `macos`, `15.1` |
| `screen`, `appBuild`, `crash.appBuild` | always empty | `null` |

The report goes to our logging service. It also goes to our team's messaging tool with your name, email address and
organization, so we can contact you about it. We keep crash reports for 30 days.
A crash report never holds prompts, model output, code or tokens. The note holds only what you type in it.

## Network traffic the app needs

These calls are how the app works. They are not usage data, and the usage data setting does not turn them off.

| What | Where it goes | When | What it sends | Can you turn it off? |
| --- | --- | --- | --- | --- |
| Update check | `downloads.nexus.inc` | At start, then about every 30 minutes | A request for the latest version. Your IP address is visible to the server | Turn off automatic updates in Settings. The app still checks once when the Gateway says your version was withdrawn. Builds from source have no updater |
| Update channel check | `api.nexus.inc` | With an update check, when you are signed in to Nexus Cloud | Your account token, to ask which update channel your account may use | Sign out, or turn off automatic updates |
| Sign-in | The Nexus Gateway | When you sign in | Your computer's name, so you can tell your devices apart | Only used if you sign in |
| Nexus Cloud | The Nexus Gateway | When you use Cloud | What the feature needs: your chat messages, attachments, and coding-session prompts and code when a run is billed through Cloud | Use your own subscription, key or model instead |
| Coding session start and end | The Nexus Gateway | When a coding session starts and ends, while you are signed in | The agent, how the run is billed, the model, and when it started and ended | Sign out |
| Session sync and phone commands | The Nexus Gateway | While you are signed in on a trusted device. On by default | Your coding sessions, encrypted on your computer so the Gateway cannot read them. In the clear: session IDs, sizes, times, your computer's name and what it can do | Your organization's policy can turn it off |
| Presence | The Nexus Gateway | About every 30 seconds while a window is open and visible in a company organization | Whether you are chatting or idle | Turn off "Show when I'm online" in the Nexus web app's settings. You then appear offline, and we keep no time for you |
| Coding agents | The agent's own provider, for example Anthropic or OpenAI | When you run Claude Code or Codex | What the agent sends. That is between you and the provider, under their terms | Do not run the agent |
| Agent usage counts | The Nexus Gateway | When Claude Code or Codex runs on your own subscription while you are signed in | The agent's own usage export: token counts and timings. The app turns off its prompt and tool content | For Claude Code, set `NEXUS_CLAUDE_NO_USAGE=1`. Codex has no switch yet |
| Assistant | The assistant service you set up | Only if you set an assistant service in Settings or with `NEXUS_AGENTS_API_URL`. Off by default | Your assistant chats and memory, and your password manager's index: login names, sites and a hint of at most four characters. Never a password or a card number | Leave the assistant service unset |
| Dictation and voice | AssemblyAI, a speech-to-text provider | Only while you dictate or are on a voice call | Your microphone audio | Do not use dictation or voice |
| Images in messages | The image's own server | When a message with an image is shown | A request for the image. Your IP address is visible to that server | No |
| Pull request status | GitHub or GitLab, through your `gh` or `glab` command | While a coding session watches a pull request | Your own credentials and the pull request's ID | Stop watching the pull request |
| Another Gateway | A Gateway you choose | When `NEXUS_URL` is set | Every Gateway call above goes there instead | Unset `NEXUS_URL` |

## Questions

Email <privacy@nexus.inc>. For security problems, follow [SECURITY.md](SECURITY.md).
