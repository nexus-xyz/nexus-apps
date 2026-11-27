# Releasing the desktop app

This page is for the people who release Nexus for desktop. It says how to cut a beta, how to rehearse one, and what
the release needs. The workflow is `.github/workflows/release.yml`.

## Cut a beta

1. Make sure the commit is on `main` and its checks are green.
2. Push a tag `v<x.y.z>-beta.<N>` on that commit. A tag ruleset limits who can push it.
3. The workflow builds the app and the Nexus host. It reads no secret until the next step.
4. A second person approves the `release` environment. The person who pushed the tag cannot approve.
5. The workflow signs every file, uploads the version's folder, creates the GitHub prerelease, and then moves the
   beta feed. Installed betas see the new version within a minute.

A published version never changes. To fix a release, tag the next number.

## Rehearse

Push a tag `v0.0.0-test.<N>`. The workflow runs every step but the three that publish. It keeps the signed files as
the `rehearsal` artifact for three days. Rehearse after a change to the workflow, the keys or the environment.

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
| `DOWNLOADS_BUCKET` | variable | The bucket behind `downloads.nexus.inc`. |
| `GCP_WORKLOAD_IDENTITY_PROVIDER` | variable | The identity provider that trusts this repository's `release` job. |
| `GCP_SERVICE_ACCOUNT` | variable | The account that writes `desktop-oss/` in the bucket. |

The repository variable `NEXUS_DESKTOP_CLIENT_ID` is Nexus's OAuth client for the app. The `build` job reads it, so
it is a repository variable, not an environment one. Without it, a build signs in as the shared client of builds
from source.

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
Make two keys: the active key and the next key. Put both public halves in the file, the active key first.
Keep the next key's secret half offline, and keep an offline copy of the active one.

To rotate: release once more with the active key, so every beta trusts the next key. Then put the next key's secret
half in `UPDATER_PRIVATE_KEY`, make a new next key, and replace the old key's line with it in a pull request.

A lost key cannot be replaced from here. Betas that trust only lost keys never update again, and their users must
install a new build by hand. That is why the next key exists.

## When a release stops halfway

The workflow refuses to write a version's folder twice.

- It stopped before the upload: run the failed job again.
- It stopped after the upload: the feed did not move, so no installed beta saw the version. Tag the next number.
