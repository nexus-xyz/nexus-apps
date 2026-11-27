# Releasing the desktop app

This page is for the people who release Nexus for desktop. It says how to cut a beta or a stable release, how to
rehearse one, and what the release needs. The workflow is `.github/workflows/release.yml`.

| Tag | Channel | Folder on downloads.nexus.inc | Signed with |
| --- | --- | --- | --- |
| `v<x.y.z>-beta.<N>` | beta | `desktop-oss/beta/` | the beta key (`updater-keys.txt`, `UPDATER_PRIVATE_KEY`) |
| `v<x.y.z>` | stable | `desktop/stable/` | the stable key (`updater-keys-stable.txt`, `STABLE_UPDATER_PRIVATE_KEY`) |
| `v0.0.0-test.<N>` | rehearsal | none: it publishes nothing | what the environment holds, or a throwaway key |

The channels do not mix. A stable or internal install trusts another key than the beta and refuses a beta version, so
it never moves to the beta by itself: to try the beta, install a beta by hand from this repository's releases. The
beta's GitHub release says so too.

## Cut a beta

1. Make sure the commit is on `main` and its checks are green.
2. Push a tag `v<x.y.z>-beta.<N>` on that commit. A tag ruleset limits who can push it.
3. The workflow builds the app and the Nexus host. It reads no secret until the next step.
4. A second person approves the `release` environment. The person who pushed the tag cannot approve.
5. The workflow signs every file, uploads the version's folder, creates the GitHub prerelease, and then moves the
   beta feed. Installed betas see the new version within a minute.

A published version never changes. To fix a release, tag the next number.

Every publishing run's `release` job queues on one concurrency group, `release-feed`, so two runs never read and move a
feed at once. GitHub keeps one waiting run per group: if a third tag is pushed while one waits, the waiting one is
cancelled before it starts. Run it again once the queue is free; nothing of it was published.

## Cut a stable release

Stable is off until the `release` environment holds `STABLE_UPDATER_PRIVATE_KEY` and the variable
`GCP_STABLE_SERVICE_ACCOUNT`: until then a `v<x.y.z>` tag builds, then stops at "Check the environment" with nothing
signed or published.

1. On `main`, set `version` in `apps/desktop/Cargo.toml` to the new version, newer than `desktop/stable/latest.json`.
2. Push a tag `v<x.y.z>` with that version on that commit. The `plan` job refuses a tag that does not match.
3. As for a beta: a second person approves `release`. It publishes `desktop/stable/<version>/`, the GitHub release, and
   then moves `desktop/stable/latest.json`, which installed stable builds, the one-line installers and the AUR package
   read.

The stable folder keeps the layout and headers that Nexus's earlier release used: the same file names, `latest.json`
laid out the same way (`testdata/stable-0.11.2.json` is its last one, and `test_feed.py` checks it), and
`Cache-Control: private, no-store, max-age=0` on every object. Do not tag stable here while the earlier release
(nexus-xyz/nexus `factory-release.yml`) still publishes stable: both would write the same folder. The feed only moves
forward and only over the generation it read, so neither can take it back, but a version would come from two places.

## Rehearse

Push a tag `v0.0.0-test.<N>` on `main`. The workflow runs every step but the three that publish. It keeps the files as
the `rehearsal` artifact for three days. Rehearse after a change to the workflow, the keys or the environment.

A rehearsal needs no secret. For each one that is missing it says so in the run's summary and goes on: without the
Developer ID the app is signed ad hoc, without the notarization key it is not notarized, and without the updater key
(or with no key in `updater-keys.txt`) the files are signed with a key made in the run and thrown away. With the bucket
variables set, it signs in as the beta publisher and reads `desktop-oss/beta/latest.json`, and writes nothing. With
`STABLE_UPDATER_PRIVATE_KEY` set, it also signs a probe file with it and checks the signature against
`updater-keys-stable.txt`, so a wrong key or password shows before the first stable tag.

The repository is public: any signed-in GitHub user can download a run's artifacts. A rehearsal's files are signed with
the live keys once they are set. Their version, `0.0.0-test.<N>`, passes as an update on no channel.

## What the release needs

The `release` environment holds every secret. Only the `release` job reads them.

| Name | Kind | What it is |
| --- | --- | --- |
| `APPLE_CERTIFICATE` | secret | The Developer ID Application certificate: a `.p12` file, in base64. |
| `APPLE_CERTIFICATE_PASSWORD` | secret | The password of that `.p12` file. |
| `APPLE_SIGNING_IDENTITY` | secret | The identity's name: `Developer ID Application: … (TEAMID)`. |
| `APPLE_API_KEY` | secret | The id of the App Store Connect API key that notarizes. |
| `APPLE_API_ISSUER` | secret | The issuer id of that key. |
| `APPLE_API_KEY_P8` | secret | The key itself: the text of its `.p8` file. |
| `UPDATER_PRIVATE_KEY` | secret | The active updater key's secret half: the minisign key file, in base64. |
| `UPDATER_KEY_PASSWORD` | secret | The password of that key. Leave it out for a key without one. |
| `STABLE_UPDATER_PRIVATE_KEY` | secret | Stable only. The stable key's secret half: the earlier release's `TAURI_SIGNING_PRIVATE_KEY`, as it is (already the base64 of a minisign key file). |
| `STABLE_UPDATER_KEY_PASSWORD` | secret | Stable only. The earlier release's `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. |
| `DOWNLOADS_BUCKET` | variable | The bucket behind `downloads.nexus.inc`. |
| `GCP_WORKLOAD_IDENTITY_PROVIDER` | variable | The identity provider that trusts this repository's `release` job. |
| `GCP_SERVICE_ACCOUNT` | variable | The account that writes `desktop-oss/` in the bucket. |
| `GCP_STABLE_SERVICE_ACCOUNT` | variable | Stable only. The account that writes `desktop/stable/` in the bucket. |

A publishing run stops before it signs anything when one of its channel's values is missing.

The repository variable `NEXUS_DESKTOP_CLIENT_ID` is Nexus's OAuth client for the app. The `build` job reads it, so
it is a repository variable, not an environment one. Without it, a build signs in as the shared client of builds
from source, so the `plan` job refuses a beta or stable tag while it is not set. A rehearsal goes on without it.

The `release` job installs minisign 0.12 from its GitHub release and checks the archive's SHA-256 before it runs it.

The Windows installer is not signed yet. `package.ps1` signs it when the `AZURE_SIGNING_*` values exist.

## The updater keys

An installed app takes an update only when one of its keys signed the file. The keys' public halves are in
`updater-keys.txt`, and every release build carries them. The file's header gives the format.

To make a key:

```bash
minisign -G -p updater.pub -s updater.key
```

```bash
base64 < updater.pub | tr -d '\n'
```

```bash
base64 < updater.key | tr -d '\n'
```

The second command prints the line for `updater-keys.txt`. The third prints the value of `UPDATER_PRIVATE_KEY`.
These are the beta's keys. The stable key is the earlier release's, which every installed stable build trusts; its
public half is in `updater-keys-stable.txt`.
Make two keys: the active key and the next key. Put both public halves in the file, the active key first.
Keep the next key's secret half offline, and keep an offline copy of the active one.

To rotate: release once more with the active key, so every beta trusts the next key. Then put the next key's secret
half in `UPDATER_PRIVATE_KEY`, make a new next key, and replace the old key's line with it in a pull request.

A lost key cannot be replaced from here. Betas that trust only lost keys never update again, and their users must
install a new build by hand. That is why the next key exists.

## When a release stops halfway

The workflow refuses to write a version's folder twice: `SHA256SUMS`, written last, marks it complete, and each
object is written only if it does not exist yet.

- It stopped before the upload: run the failed job again.
- It stopped during or after the upload: the feed did not move, so no installed build saw the version. Tag the next
  number.
