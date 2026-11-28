# nexus-agent-memory

`nexus-agent-memory` is the local memory that coding agents share in Nexus Desktop and the `nexus` CLI.
It stores short notes as Markdown files on this machine. A note is personal, or it belongs to one repository.

Memory is off until the person turns it on. While it is off, the store does not read or write notes.
Only the person can save a personal note. An agent's session can save notes for a repository.

The crate also has the shared tool contracts (`tools`) for the local MCP servers of the app and the CLI.

## Usage

```rust,no_run
use nexus_agent_memory::{Scope, Source, Store};
use std::path::Path;

fn main() -> std::io::Result<()> {
    // The default folder: `nexus/memory` in the platform's local data folder.
    let store = Store::local()?;
    store.set_enabled(true)?;
    store.remember(Scope::Personal, Source::Person, "Prefer small pull requests.")?;

    // The notes as an agent's instructions, for the repository at this folder.
    let instructions = store.instructions(Path::new("."), None)?;
    println!("{instructions}");
    Ok(())
}
```

## License

MIT.
