# {{plan_agents}} without Gateway

These coding agents use the provider set up in the agent.
Read this page when the person asks about one of them. `coding-agents` has `nexus agents` and `nexus mcp`.

## Antigravity

`nexus agy` starts Google's Antigravity (`agy`) on the person's own Google AI plan.

```sh
nexus agy -p "hello"    # arguments go to agy unchanged
```

- To resume a conversation, put `agy`'s own `--conversation <id>` after `--`.
- Its calls go straight to Google. Nexus records the session and, when it can read them, its token counts, and charges nothing.
- `--api` and `--model auto` are refused with `route_unsupported` or a usage error. Name a model by `agy`'s own id.
- `agy` signs in on its first start. Nexus does not check that sign-in.
- It does not start while `~/.gemini/antigravity-cli/settings.json` sets `modelProvider`, which selects an API key. Removing that key fixes it.
- It gets no page instructions and no skills from Nexus.

## Cursor

`nexus cursor-agent` starts Cursor's CLI (`cursor-agent`) on the person's own Cursor plan.

```sh
nexus cursor-agent -p "hello"    # arguments go to cursor-agent unchanged
```

- It runs on macOS and Linux. On Windows, Nexus refuses to start it.
- To resume a conversation, put `cursor-agent`'s own `--resume <id>` after `--`.
- Its calls go straight to Cursor. Nexus records the session and charges nothing. It reads no token counts for it.
- `--api` and `--model auto` are refused with `route_unsupported` or a usage error.
- The person signs it in once with `cursor-agent login`. Nexus checks that sign-in with `cursor-agent status`.
- A run on the plan drops the variables that would sign it in another way or send its calls elsewhere, such as `CURSOR_API_KEY`.
- It gets no page instructions, no skills and no screen recording from Nexus.

## OpenCode

On a cloud machine, OpenCode runs on Nexus credits (Team API).
Pick OpenCode, or an open-weight model, for a Cloud session in Nexus Desktop or on the website.
Nexus configures OpenCode's provider. You configure nothing.
OpenCode never runs on a personal plan.
On Linux, Nexus Desktop can also start OpenCode locally with the provider configured in OpenCode.

## Grok

Gateway records sessions on the person's Grok plan when the organization allows subscriptions.
The person signs in with `grok login` on their computer.
Nexus CLI and Desktop do not launch Grok yet.
