# nexus-agents

`nexus-agents` names the coding agents Nexus launches: Claude Code, Codex, Antigravity, Cursor and OpenCode.
Each agent is a type of its own. The closed `Agent` enum names them in ids, serde and dispatch.
Each agent's `AgentEntry` holds its plain facts: its id, its command, how it installs and how it signs in.

The crate does no I/O. Nexus Gateway, the `nexus` CLI and Nexus Desktop all use it.

## Usage

```rust
use nexus_agents::Agent;

// A person may type an agent's id or its command.
let agent = Agent::parse_named("agy").expect("a known agent");
assert_eq!(agent, Agent::Antigravity);
assert_eq!(agent.as_str(), "antigravity");
assert_eq!(agent.command(), "agy");

for agent in Agent::ALL {
    println!("{agent}: nexus {}", agent.command());
}
```

A domain crate defines its own trait, implements it for each agent's type, and reaches it with the `of!` macro.
A new agent then fails to build until every domain implements its trait. [ADDING-AGENTS.md](ADDING-AGENTS.md) says
how to add an agent.

## License

MIT.
