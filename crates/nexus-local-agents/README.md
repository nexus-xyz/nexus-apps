# nexus-local-agents

`nexus-local-agents` finds out what this machine has of each coding agent Nexus launches: whether it is installed,
its version, and how it is signed in. It runs the agent's own status command (for example
`claude auth status --json`) with a deadline, so a hung agent does not hold up the caller.

It reads only the sign-in method and the plan's name. It never reads the account's organization.

A desktop app started from the Finder or the Dock gets a minimal `PATH`. `search_path` adds the folders agents
are usually installed in, and the login shell's `PATH` when an agent is not found otherwise.

## Usage

```rust,no_run
use nexus_local_agents::detect_all;

let env = |name: &str| std::env::var(name).ok();
for agent in detect_all(&env) {
    println!("{}: installed {}, version {:?}, login {:?}", agent.agent, agent.installed, agent.version, agent.login);
}
```

## License

MIT.
