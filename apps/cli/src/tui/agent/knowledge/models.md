# Models and model policies

Every model call goes through Nexus Gateway, which checks it against the organization's policies first.
Read this page for model names, `nexus/auto`, who may use which model, and refusals.

## The catalog

- Each model has one name, `provider/model`, such as `anthropic/claude-opus-5.5`. Nexus reads other spellings as that name.
- A model is supported (callable) or coming soon (listed, not callable).
- Do not name a model from memory. Read the list.

```sh
nexus models          # the models the person may use
nexus models --all    # the whole catalog: prices, status, and whether each is allowed
```

## nexus/auto

- `nexus/auto` lets Nexus choose the model, inside the person's policy.
- `nexus/default` is the default that the policy names, else Nexus's own.
- Neither runs a refused model. Every answer names the model that ran.
- In the desktop app and CLI (`--model auto`), a new Claude Code or Codex session can pick **Auto**. Nexus picks the model as the session starts, on the person's plan first. An open session keeps its model.
- Auto can also pick the agent at launch: Gateway compares the person's agents (Claude Code, Codex) and their plans, and names the agent, the model and who pays. It picks once, at the start. The session keeps that agent and model, so its prompt cache stays.
- On the plan route, Auto never starts a paid model. When the plan has no room, the message names the model. In the desktop app, pick the organization under Who pays, or pick a model. In the CLI, run `nexus claude --via gateway --model auto` or `nexus codex --via gateway --model auto`, or pass `--model <id>`.

## Policies

- A rule has an allow list and a deny list of patterns.
- A pattern is an exact model, a provider (`anthropic/*`), a family (`openai/gpt-6-*`) or `*`.
- Rules exist for the organization, for each team, and for each coding agent ({{policy_agents}}). There are no rules for one person.
- Every allow list on the way must match. A deny always wins. A lower level can only narrow.
- Owners and admins set policies. A team lead can narrow the policy of a team they lead. Everyone can read them.

```sh
nexus policy show team/platform
nexus policy set org --deny "anthropic/claude-fable-*"
nexus policy set team/platform --allow "anthropic/*" --default anthropic/claude-haiku-5.5
nexus policy test --model anthropic/claude-haiku-5.5    # would it be allowed, and by which rule
```

## Operator controls

- A pause refuses every model call of its target until it expires.
- A limit narrows its target to the models it lists.
- An override allows the models it lists past a deny. It never passes a pause or a limit.
- A target is the organization, a team, a person or a key.
- Owners and admins set every control. A team lead pauses or limits a team they lead.

```sh
nexus policy controls
nexus policy pause person/usr_123 --reason "runaway script" --expires 12h
nexus policy clear <control-id>
```

## When a model is refused

A refused call never reaches the provider and costs nothing.

| Status and code | When | What to do |
| --- | --- | --- |
| `403` `model_not_allowed` | A policy refuses the model | Pick an allowed one from `nexus models`. `nexus policy test` names the rule |
| `403` `operator_paused` | A control pauses the caller | `nexus policy controls` shows it |
| `404` `model_not_found` | The catalog has no such model | Check the name with `nexus models --all` |
| `400` `model_not_supported` | The model is coming soon, or not served on that API | Pick another model |

## In this session

- The person picks your model: `/model` in the terminal, the model picker in the desktop app.
- `nexus models pick` sets the model that a new terminal session starts with.
- When your own call is refused, the turn ends with the error. Tell the person the code and what fixes it.
