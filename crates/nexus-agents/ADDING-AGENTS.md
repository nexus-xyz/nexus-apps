# Add a coding agent to Nexus

This guide is for engineers and coding agents who add a coding agent. Cursor is the worked example.
It says where each part of an agent lives, and in which order to add the parts.
A path is relative to the crate named next to it: `src/lib.rs` in `nexus-agents` is this crate's `src/lib.rs`.

## The words

- **Coding agent.** A program Nexus launches, such as `cursor-agent`.
  In code it is a type in this crate (`pub struct Cursor;`) and a variant of the closed `Agent` enum.
- **Id.** The agent's name in the API, in policies, in sessions and in stored rows (`cursor`).
  An id never changes once it ships. The command a person types can differ (`cursor-agent`).
- **Fact.** A plain value that more than one domain reads: the agent's names, plan, vendor, install and sign-in
  commands, profile folder and the like.
  Facts are the agent's `AgentEntry`, in its own file in this crate's `src/agents/`.
  The entry holds one struct per concern: `Identity`, `Install`, `SignIn`, `Billing`, `Profile` and `Session`.
  A value that one domain alone reads stays in that domain.
- **`AgentT`.** The trait every agent's type implements in this crate. It gives the agent's entry.
- **Domain.** A crate that does one kind of work for every agent, such as what this machine has of each agent
  (`nexus-local-agents`) or the session engine (`nexus-workbench`).
  A domain defines its own trait and implements it for each agent's type.
  It reaches the implementation with `of!` or an exhaustive dispatch over `SessionAgent`.
- **Not offered.** A domain cannot skip an agent.
  When an agent lacks a capability, its implementation returns a named value for that, such as `None`.
  A trait has no default method bodies.
- **Nexus's own agent.** The desktop's Nexus agent is a kind of session, not a coding agent.
  It is not in this crate.

## Where an agent lives

- `nexus-agents` (this crate): the type, the `Agent` variant and the facts.
- `nexus-local-agents`: what this machine has of the agent: its sign-in check, its launch and its models.
- `nexus-client` re-exports `Agent`, `AgentEntry`, its parts and `GatewayBinding` in `nexus_client::agent_entries`.
  The CLI and the desktop read them there.
- `nexus-workbench`: the session engine, which runs the agent in a session.
- `nexus-host` and the desktop app: where the agent runs, and what the desktop shows of it.
- The server side, maintained by Nexus: the agent's route through Nexus Gateway, the agent ids in the API contract,
  and the stored rows that name the agent. It is not in this repository. See step 6.

## Add an agent

Work from top to bottom. After step 1, the compiler names most of the places that still need the agent.

### 1. This crate

1. Copy the nearest sibling in `src/agents/` to `<agent>.rs`.
2. Declare its type, its entry and its `AgentT`. State every field of each part of the entry.
3. Where a value comes from the vendor's documentation, link that page in a comment.
4. Add the module to `src/agents.rs`, and the name to the `agents!` list in `src/lib.rs`.
5. Update the tests in `src/lib.rs` that pin the catalog's order, ids and agent files.
6. Set `Session::cli_launcher` for published Rust CLI launchers. Keep `Session::legacy_access` false for new agents.

Set `session.platforms` to an empty list when the agent cannot start locally.
The Desktop picker, local launchers and self-hosted servers use this list.
Cloud hosts apply their own launch rules.

A domain trait that `of!` reaches needs `Sync` as a supertrait, as `AgentT` has.
`of!` makes a `&'static dyn Trait`, and a caller may hold it across an `.await`.

### 2. This machine

`nexus-local-agents` does not build until it has the agent's checks and launch on this machine:

1. Copy the nearest sibling in its `src/agents/` to `<agent>.rs`.
2. Implement `LocalAgentT` there: the sign-in check's reading, the launch's arguments and environment, and the model
   flags. Implement async model discovery with the supplied environment.
   Implement the optional Gateway model-catalog override, returning `None` when unnecessary.
   Add the module to `src/agents/mod.rs`.
   Say how it takes Nexus's instructions and a screen recording, and where its transcripts are for the session log.
3. State every method. A capability the agent lacks returns the value the trait's comment names, such as `None`.
4. Do not add a method for a fact the entry holds. The binary's lookup, `NEXUS_<ID>_BIN`, `NEXUS_<ID>_AUTH` and
   the profile folder come from the entry.
5. For a CLI launcher, add its `NEXUS_<ID>_AUTH` row to `ENVIRONMENT` in `nexus-cli`'s `src/catalog.rs`.
   The CLI checks the catalog against those rows.

### 3. The session engine

In `nexus-workbench`:

1. Add the agent to `SessionAgent` in `src/event.rs`. Update its list, id, parser and catalog conversions.
   Match their feature gates across builds. Cursor shows how to keep an agent out of a build that does not offer it.
2. Implement `Harness` in `src/harness/<agent>.rs`, and wire it into `src/harness/mod.rs`.
   This controls launch, models, permissions, plans, sync and tools. The dispatch must build with features off.
   Set `Harness::answers_asks` to false for third-party agents. Nexus forwards their permission requests.
   For an ACP agent, create its own `acp::Profile` and use the shared `acp::Driver`.
3. Give unsupported capabilities explicit values. Test the agent's offered modes and launch behavior.
4. Leave `Harness::syncs` off until the server side accepts the agent's synced sessions (step 6).

### 4. Cloud and desktop

1. Implement `CloudAgent` in `nexus-workbench`'s `src/cloud_agent/<agent>.rs`, and add its dispatch.
   Return a reason from `CloudAgent::refused` for each unsupported route. Set `SessionFeatures::local_only` when
   neither cloud machines nor remote servers may run the agent. The host enforces routes in
   `nexus-host`'s `src/agent_files.rs` independently of desktop preflight.
2. Implement `AgentUi` in the desktop app's `src/agents/<agent>.rs`, and add the dispatch in `src/agents/mod.rs`.
   This gives the desktop the agent's words, account controls, sign-in fields and server setup.
3. Choose supported platforms in the entry. Run the desktop and check that the agent appears on the machines
   where it can run. Check both the in-app launch and the desktop's local background service.

### 5. Fixtures and the other readers

1. Regenerate the CLI's capability fixture, from the root of the CLI's workspace, with
   `AGENT_CAPABILITIES_BLESS=1 cargo test --locked -p nexus-cli --lib agent_capabilities`.
2. Check the client readers. Older readers must still accept existing ids and stored rows.
3. Check the desktop icon maker in the desktop app's `src/workspace/new_code.rs` and phone capability filtering in
   `nexus-workbench`'s `src/computer/settings.rs`. Keep the older readers' fallback for unknown agents.
   Check `nexus-host`'s `src/agent_files.rs` for the agent's config files.
4. Update `ALIASES` in `nexus-cli`'s `src/tui/agent/knowledge.rs` and its pinned topic lists.
   Update the relevant `coding-agents.md` or `plan-agents.md` topic. Give CLI examples only for published launchers.

### 6. The server side, maintained by Nexus

An agent also needs a half on the server, which Nexus maintains outside this repository:

- its route through Nexus Gateway, or none when the agent runs only on the person's own plan;
- its id in the API contract, its policies and its setup steps;
- the stored rows and synced sessions that name it.

Say in your pull request that the agent needs its server half, or open an issue that names the agent.
Nexus adds that half and then refreshes `nexus-client`'s `contract/` folder, a byte-for-byte copy of the server's
API contract. Do not edit `contract/` by hand. An agent does not ship until its server half does.

## Checks

From the root of each Cargo workspace you touched:

- `cargo fmt --all --check` and `cargo check --locked --workspace --all-targets`.
- Tests for each touched crate. Check the workbench and the host without the desktop feature set.

Golden tests pin launch arguments, sign-in messages and plan-tier reads. Add a golden for the new agent.
Run `cargo test --locked -p nexus-client --test it golden`.
Run the CLI goldens with `cargo test --locked -p nexus-cli --test it golden` and its plan-tier tests
with `cargo test --locked -p nexus-cli --test it plan_tier`.

## Rules the tests hold

- `Agent::ALL` holds every agent once, in the catalog's order (`all_lists_every_variant`).
- An agent's serde value, id, text and parse are one string (`serde_value_is_the_id_parse_reads`).
- Each entry states every field of every part, with no `..`, `Default` or copied part
  (`every_entry_states_every_field`).
- `of!` names every agent: `Agent::entry` reaches each entry through it.
- An id never changes once it ships: stored rows, policies and the phone's readers hold it.
