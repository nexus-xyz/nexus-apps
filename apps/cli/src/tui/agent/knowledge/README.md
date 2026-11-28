# The Nexus agent's knowledge base

This folder holds what the Nexus agent knows about Nexus: what Nexus does today, and how.
The agent is the `nexus` CLI's own coding agent: agent mode in the terminal session, and Nexus's agent in the desktop app's Code sessions.
Read this file before you add or change a topic. The agent reads the topics, never this file.

## How the agent uses it

- The agent's system prompt (`../prompt.rs`) holds a short map of Nexus and the names of these topics. Every step sends it.
- The agent reads one topic at a time with its `nexus_docs` tool (`../knowledge.rs`), when a question or a task needs it.
- `nexus_docs` also answers three things that no file holds. `commands` and `nexus <command>` come from the CLI's own catalog. `status` comes from Gateway.

## Change a topic when the product changes

A topic is true only while someone keeps it true. Change it in the same pull request as the feature.

1. Find the topic by its name. `overview.md` lists them.
2. Write only what works today on `main`. Name what is off by default, and who may use it.
3. Show the real command. A test checks each `nexus` command and each `--flag` against the CLI's catalog.
4. Say what the agent itself must do or must not do, when that differs from what a person does.
5. From the repository root, run `cargo test --locked -p nexus-cli tui::agent`.

## Add a topic

1. Add `<name>.md` here. Start it with `# Title`, then say what it covers and when to read it.
2. Add it to `topics()` in `../knowledge.rs`, with a line on what it covers.
3. Name it in the map in `../prompt.rs` (`NEXUS`). A test fails until the map names it.
4. Add it to the table in `overview.md`.

## Rules for the text

- Keep a topic under 6,000 characters. Once read, it stays in the conversation, and each later step pays for it.
- One fact for each sentence. Short sentences, active voice, no filler.
- Do not copy a list that the CLI can print, such as the models or the connectors. Name the command that lists it.
- Do not write a list of agents. `knowledge.rs` (`places`) fills these from the catalog: `{{agents}}` (every agent's
  name), `{{gateway_agents}}` and `{{plan_agents}}` (the names of those with a Gateway route and those without one),
  `{{launchers}}` (each `nexus <agent>` command), `{{skills_launchers}}` (the launchers that write skills) and
  `{{policy_agents}}` (the agents a policy applies to).
- Do not write a price, a date or a number that changes. Name the command that shows it.
- Never write a secret, and never tell the agent to run a command that takes one. `auth.md` has the rule.
- Say only what has shipped.

## What the tests hold

| Test | What it holds |
| --- | --- |
| `every_command_the_topics_show_is_in_the_cli_with_its_flags` | A renamed or removed command or flag fails here |
| `every_topic_is_a_short_document_that_starts_with_its_title` | The size limit and the shape of a topic |
| `the_map_names_every_topic_of_the_knowledge_base_and_only_those` | The prompt and this folder name the same topics |
| `never_names_a_tool_set_that_is_off_as_one_the_agent_has` | The prompt claims no tool that the session lacks |
| `stays_short` | The prompt stays a map, not a manual |
