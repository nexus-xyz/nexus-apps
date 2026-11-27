# {{gateway_agents}} through Nexus

`nexus claude` and `nexus codex` start the real Claude Code and Codex. The agents stay as they are.
Read this page when the person asks how to run those agents through Nexus, or about `nexus mcp`.
You are none of them: `overview` lists the agents.

## Two routes

| Route | Where model calls go | Metered and checked by policy |
| --- | --- | --- |
| The person's own plan (the default) | Straight to Anthropic or OpenAI, on their Claude or ChatGPT plan | No. Nexus records the session only |
| Gateway | Through Nexus Gateway | Yes |

```sh
nexus agents                       # what is installed and signed in, each route, and who pays
nexus claude --api                 # this run through Gateway
nexus agents use claude gateway    # always through Gateway
nexus agents setup claude          # how to set the Gateway route up, step by step
```

- Nexus never installs a coding agent. The person installs it.
- On the Gateway route, a call uses the organization's own provider key, or Nexus's provider accounts.
- `nexus agents` shows who pays on each route, and what is left to set up, with the command for each step.
- Owners and admins can turn the plan route off for the organization. Then every run goes through Gateway.
- In the desktop app, Settings › Agents and plans shows each agent: who pays, what is left to set up, and its plan accounts.
- The desktop app can hold several Claude and ChatGPT accounts. New sessions start on the active account.
- With "Switch accounts near a limit" on, a session at 90% of a limit takes the account with the most usage left. A running Codex session keeps its account. Only a new one switches.

## Run

```sh
nexus claude -p "hello"         # arguments go to Claude Code unchanged
nexus codex exec "hello"
nexus claude --team platform    # this session's spend goes to that team
nexus claude --model auto       # Auto: picks on your plan first, or via Gateway with --via gateway
```

- On the plan route, `--model auto` never starts a paid model. When the plan has no room, the launch stops and tells you to run `nexus claude --via gateway --model auto` (or `nexus codex --via gateway --model auto`) or pass `--model <id>`.
- Put an argument after `--` when Nexus would read it: `nexus claude -- --help`.
- Each run opens a session that the person sees in Nexus UI.
- On the Gateway route, a refused model or a spent budget reaches the agent as the provider's own error.
- Both launchers explain pages and connector discovery. Connector tools use Nexus MCP.
- They also write the person's skills into the agent's skills folder. `--no-skills` skips that.
- `nexus agents share latest --org` shares this folder's newest Claude Code session as an artifact. See `chat`.
- Do not start another coding agent from here unless the person asks.

## Agents on the person's own plan only

{{plan_agents}} use their own provider setup. Read `plan-agents`.

## Nexus as tools for another agent

`nexus mcp` is a local MCP server. It gives an agent Nexus as tools, under the person's own sign-in.

```sh
claude mcp add nexus -- nexus mcp
codex mcp add nexus -- nexus mcp
```

- Its tools read spend, budgets, usage, models, teams and agent sessions.
- For owners and admins, its tools also change members, teams, budgets, policies and keys. The agent asks before each change.
- It can publish a page, read or change a connector, and ask the person's assistant.
- Connector writes need a session agent link. Organization and personal settings start on; saved switches and lists take precedence.
- MCP discovery works outside Nexus Desktop, without repository instructions.
- It has no tool that adds a provider key or creates an API key. Those secrets would stay in the transcript.

## Agents in Nexus for desktop

With multiplayer sessions enabled, GPUI's Team and Following open a native read-only session viewer.
It shows recent sessions, text, thinking, tools, notices, errors, and active watchers.
Following changes here and reconnects catch up automatically. Earlier pages load up to 1,000 events in memory.
Teammate content requires trusted devices, session sync, and shared team-key support. Missing keys show a locked state.
Rich attachments and full source-app transcript rendering remain separate parity work.

The desktop app runs Claude Code, Codex and you in Code sessions, side by side. See `desktop`.

For a dirty session worktree, choose **Force delete**, then **Confirm delete**. This removes the worktree and its submodules and keeps the session branch.

Private GPUI Workbench shells survive closing the primary window while Nexus runs.
Closing a terminal or extra window, signing out, switching account or Gateway, or quitting ends them.
CEF browser tabs, with live pages and history, survive primary close/reopen.
Browser tools wait for the primary window to reopen. Identity changes or quit close those pages.
Side chats retain selection, ordinary drafts, attachments and pending command outcomes across primary close/reopen.
Fresh controls reconnect to the original store or machine; unavailable sources keep drafts read-only.
Completed responses and status refresh there; Stop requires validation.
Updated owners and machines resume live partial text. Older hosts show completed responses; reconnecting retries live support.
Secret-entry text and secret approval answers are cleared.
A large side-chat file shows upload progress on its chip; ✕ cancels the send and keeps text and files.
Side-chat text-only pastes of at least 32 KiB become text files.
Cmd+Shift+V on macOS, or Ctrl+Shift+V elsewhere, inserts literal text.
Identity changes or quit discard side chats. Servo pages and sign-in popups still end with the primary window.
