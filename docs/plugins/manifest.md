# The plugin manifest, `nexus-plugin.json`

This page is for plugin authors. It says what each field of a Nexus Desktop plugin's manifest means and which values the app accepts. The app's own parser is the authority: when this page and the app differ, the app wins.

A plugin is a folder with `nexus-plugin.json` at its root and the files of one MCP server. The plugin adds the server's tools to every Code session's agent, as an MCP server in Settings does.

## Example

```json
{
  "apiVersion": 0,
  "id": "acme-notes",
  "name": "Acme Notes",
  "version": "1.2.0",
  "description": "Search and add notes in Acme.",
  "author": "Acme",
  "homepage": "https://example.com/acme-notes",
  "mcp": { "command": "bin/acme-notes", "args": ["--stdio"] },
  "secrets": ["ACME_TOKEN"],
  "permissions": {
    "network": ["api.example.com"],
    "filesystem": [{ "path": "~/Documents/Acme", "access": "read" }],
    "commands": ["git"]
  }
}
```

## Fields

| Field | Rule |
| --- | --- |
| `apiVersion` | `0`. The app refuses another value and says which version it reads. |
| `id` | 3 to 40 characters: lower-case letters, digits and `-`. It is the MCP server's name, so the tools are `mcp__<id>__<tool>`. No installed plugin or Settings MCP server may have the same name. |
| `name` | 1 to 60 characters. The approval card shows it. |
| `version` | SemVer, for example `1.2.0`. |
| `description` | Up to 280 characters. Optional. |
| `author` | Optional. The approval card shows it. |
| `homepage` | Optional. `https` only. |
| `mcp` | Exactly one of `{ "command", "args" }` (stdio) and `{ "url" }` (streamable HTTP). A `command` is a path inside the plugin folder, or a program that `permissions.commands` names. A `url` is `https`, and its host is in `permissions.network`. |
| `secrets` | Variable names (stdio) or header names (HTTP). The person enters the values, and the app keeps them in the keychain. A name that looks like a Nexus or model provider credential is refused. |
| `permissions.network` | Host names, or `*` for any host. Empty means the plugin needs no network. |
| `permissions.filesystem` | Paths with `read` or `write` access. `~` is the home folder. The plugin's own folder is always readable. |
| `permissions.commands` | Programs the plugin runs, by name. |

The app refuses an unknown field, so a misspelt permission never passes in silence.

## What the person approves

A new plugin starts disabled. The approval card lists its name, author, version and every permission and secret. The plugin runs only after the person approves it.

A new version that asks for the same things stays approved. A version that changes `mcp`, `secrets` or a permission stops until the person approves it again. The update card lists only what changed.

## How the plugin runs

- The app starts a stdio plugin in its own folder, through Nexus's launcher, for Claude Code, Codex and Antigravity.
- The plugin gets the values of the secrets it declares, as environment variables. It gets no Nexus token and no model provider key.
- Only the app on the person's own computer starts plugins. A remote server reached over SSH does not.
- In API version 0 the app does not yet stop a plugin's process from using a host or a path it did not declare. The approval card says so. Declare everything the plugin uses.
