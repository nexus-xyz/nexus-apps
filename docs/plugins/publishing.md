# Publish a plugin: names and the Nexus API

This page is for plugin authors who share a plugin with other people.
It covers how to name the plugin and how a plugin uses the Nexus API.
The manifest rules are in `manifest.md`.

## Names

The approval card shows your plugin's `name` and `author`.
Do not choose a name or author that suggests Nexus made or endorses your plugin.

## Plugins and the Nexus API

A plugin gets no Nexus token and no model provider key.
The app refuses a secret whose name looks like a Nexus or model provider credential.
So a plugin cannot act as the person on Nexus, and cannot spend their credits.

If your plugin calls the Nexus API, it signs in as its own OAuth client, like any other third-party client.
It then holds only the third-party scopes, under that client's limits.
The rules are in [`docs/guides/forks.md`](../guides/forks.md): client IDs, scopes, limits, `Idempotency-Key`, `If-Match` and the session protocol.
