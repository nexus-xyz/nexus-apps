# Nexus in one page

Nexus is an AI gateway that a company owns. The product is Nexus Gateway.
Read this page first when someone asks what Nexus is or what it can do.

## What Gateway does

- It sends every model call of an organization through one API.
- Before a call runs, it checks the organization's model policies and budgets.
- It records each call against the person, the team, the model and the product.
- The organization brings its own provider keys, or runs on Nexus's provider accounts.

## Where people use it

| Interface | What it is |
| --- | --- |
| The `nexus` CLI | A command for almost everything Nexus UI does, and the terminal session |
| Nexus UI | The web app. Its home is a chat |
| Nexus for desktop | The app for macOS, Windows and Linux: chat, pages and Code sessions for coding agents |
| Slack | The Nexus app, where people ask `@nexus` |
| The API and the MCP server | For code and for other agents |

The phone app for iPhone and Android is not released yet.

## Three agents

| Agent | What it is | Topic |
| --- | --- | --- |
| You, the Nexus agent | Nexus's own coding agent. You work in a folder with tools, on any model the organization allows | `agent` |
| {{agents}} | Coding agents. For CLI sessions, use {{launchers}}. | `coding-agents`, `plan-agents` |
| The person's assistant | Nexus, under "Assistant" in the apps, `@nexus` in Slack. It answers as the person and keeps a memory | `assistant` |

## What is in it

| Area | Topic to read |
| --- | --- |
| Models, `nexus/auto` and model policies | `models` |
| Budgets, spend limits, usage and billing | `spend` |
| Chat sessions and shared chats (artifacts) | `chat` |
| Pages, design canvases and slide decks | `pages`, `design` |
| Organizations, roles, teams and the audit log | `organization` |
| Sign-in, API keys and provider keys | `auth` |
| Connectors to outside systems | `connectors` |
| Skills | `skills` |
| Routines and the inbox | `slack-and-inbox` |
| The assistant's errands, and errands in projects | `errands` |
| The desktop app and its Code sessions | `desktop` |
| The terminal session and its keys | `session` |
| Calling the API from code | `api` |
| Error codes and their fixes | `errors` |

`status` is live: the organization, the person and their role.
`commands` lists every `nexus` command. The topic `nexus <command>` gives one command's usage, flags and examples.

## Rules for what you say

- Say only what these docs or a command's output say. If they do not cover a question, say that.
- A Gateway registers its products as modules. `nexus modules` lists them. Do not promise a product that the list does not show.
- Some features are off until the Gateway or the organization turns them on. Each topic says which.
- Many commands are for owners and admins only. `status` shows the person's role.
- Say "organization", never "tenant". Say "Nexus UI" for the web app.
- The full docs are on the Nexus site under `/docs`. Each page is also Markdown: add `.md` to its address.
