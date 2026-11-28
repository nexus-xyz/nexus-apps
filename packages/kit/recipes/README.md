# Recipes

One JSON file per tool: how a person runs it through Nexus Gateway, the limits to read first, and what is claimed. The
schema is `nexus-recipe/1`, checked by `recipeProblems` in `recipes.ts`, which lives in Nexus's private kit package
with the Nexus UI pages that read it. Each recipe's docs page is generated into [`pages/`](pages/), and
`recipes.test.ts` there fails when a committed page and its recipe disagree. Do not edit a page: change the recipe, and
Nexus generates the page again.

| Field | What it holds |
| --- | --- |
| `schema` | `nexus-recipe/1` |
| `id` | A lower-case slug, the file's name and the page's |
| `agent` | The launcher that runs it: an agent in `AGENT_CATALOG` (`claude`, `codex`) |
| `title`, `description` | The page's front matter |
| `route` | `gateway`, the only value. A recipe never sets the person's own plan route or runs a plan sign-in |
| `launch` | What the launcher sets for one run: the Gateway path, the variable holding the Nexus key, the variables it removes |
| `gitRepo` | Set when the tool refuses to start outside a git repository, with the flag that lets it |
| `limits` | Printed before any step: what the route costs and what it does not cover |
| `steps` | In order, each with `id`, `who` (`you` or `admin`), `text` and an optional `command` |
| `verify` | How the person checks the call was recorded |
| `test` | The features a run exercises. The runs are files of their own, in [`runs/`](runs/) |
| `claim` | `tested` once the latest run backs it (rules below), and the claims register's row once there is one. The page says unclaimed until it has both |
| `owner` | The pod that owns the recipe, as `ops/pods.yaml` names it |
| `retireWhen` | When the recipe stops being true |

Rules the check enforces:

- Every `nexus claude` or `nexus codex` command names `--api` before the agent's arguments, since without it the run
  takes the person's own plan.
- The `install` step is the command in `AGENT_CATALOG`, and the steps shared with `setupSteps` keep its ids and commands.
- No text says "works with", and nothing looks like a key: commands carry placeholders such as `<key>`.
- `claim.state` is `tested` only when the recipe's latest run in `runs/` passed with no step skipped but a conditional
  one (`provider_key`): a later failing run, or a run from a HOME that already had the CLI or a sign-in, backs nothing.

A `nexus-recipe/1` recipe writes nothing: the launcher passes its settings for one run and changes no file. A recipe
that writes a tool's own settings needs a later schema version, with a diff before writing, a backup and an exact undo.

## Environment recipes

For a tool with no launcher that takes a base URL of its own, such as Aider. The person creates a Nexus API key and sets
the tool's two variables in one terminal, and the tool calls Gateway's OpenAI compatible `/v1/chat/completions`. It is a
`nexus-recipe/1` file with `"kind": "environment"`, listed in `ENVIRONMENT_RECIPES` rather than `RECIPES`, so
`nexus agents setup` and the connect page never show it. Its fields differ from a launcher recipe's in three places:

| Field | What it holds |
| --- | --- |
| `kind` | `environment` |
| `tool` | The tool's name, as the page writes it. It takes the place of `agent` |
| `environment` | `baseUrlPath`, the Gateway path the tool calls, and `baseUrlVariable` and `keyVariable`, where it reads the URL and the key. It takes the place of `launch` and `gitRepo` |

Its steps are `install_cli`, `nexus_login`, `api_key`, `install`, `provider_key`, `environment` and `run`, of which
`api_key`, `environment` and `run` are required. The check also enforces that the `environment` step exports the URL as
`%CORE_URL%` plus the path and reads the key with `read -rs`, so it is neither echoed nor kept in history, and that no
command runs a launcher. A run of an environment recipe records its call under `core`, with no capability or agent
session: Gateway cannot tell which tool made it.

## Run records

One JSON file per run of a recipe's test, in `runs/`, named `<recipe>-<date>-<n>.json`. The schema is
`nexus-recipe-run/1`, checked by `runProblems` in `recipe-runs.ts` (the private kit package) against the recipe it
names. A run follows the generated page from a clean HOME and records what it did with each step. A new record is added
to `RUNS` in `recipes.ts`, and the recipe's page is generated again.

The records stay private: each names a production ledger call, its agent session and the tester's machine, and the
open-source export leaves `runs/` out. The CLI carries [`runs.json`](runs.json) instead: the launchers'
runs with the fields its status line reads, written by `carriedRuns` in `recipes.ts`. Nexus writes it again after
adding a record.

| Field | What it holds |
| --- | --- |
| `schema` | `nexus-recipe-run/1` |
| `id`, `recipe`, `date` | `<recipe>-<date>-<n>`, the recipe's id, and the UTC day the run started |
| `toolRelease`, `cliRelease` | The tool's version and the nexus CLI's, as each reports it |
| `pageCommit` | The commit whose generated page the run followed |
| `environment` | The machine, and that the HOME was clean |
| `steps` | Every step of the page in its order, each with `id`, `seconds` (to a tenth, waits included), `result` (`done`, `skipped` or `failed`) and a `note`, required when skipped |
| `secondsToFirstCall` | The steps' sum, or `null` when no call went through |
| `firstCall` | The ledger's row for the call, as `nexus spend --json` lists it, without who made it or the team: a `settled` call under `agents` and the recipe's agent, or under `core` for an environment recipe |
| `result` | `pass` or `fail`. A pass has the call, ran the `run` step and has no failed step |
| `frictions` | What got in the way, one line each. A failed run lists at least one |

No text in a record looks like a key or an email address: records are committed.
