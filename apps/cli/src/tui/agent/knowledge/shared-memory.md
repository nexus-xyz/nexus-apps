# Shared memory

Shared memory is off by default. When enabled, local Claude Code and Codex sessions start with personal and repository entries.
A repository entry follows the repository across worktrees. A personal entry is available in every repository.
With shared memory on, Nexus tells agents to use its `remember`, `recall` and `forget` tools by default for memory requests.
The `remember` tool saves repository notes. Add a personal note in Settings or with `nexus memory add`.

Claude Code imports memory from its worktree profiles. Codex imports profile instructions and the saved memory handbook.
Choose a repository to import its local Codex notes from `.codex/memory/project`.
Native export writes on request. Codex loads every exported repository block in every session on that profile.

In Desktop, open Settings › This computer › Shared memory. Choose a repository to see its entries alongside personal entries.
Use Forget all to clear personal and repository notes from Nexus Shared memory on this computer.
The CLI and Desktop use the same switch. CLI commands use the current directory unless you pass `--repo <path>`.

```sh
nexus memory enable
nexus memory import claude
nexus memory import codex
nexus memory list
nexus memory add "A personal note"
nexus memory edit <id> "Updated note"
nexus memory forget <id>
nexus memory export markdown --output shared-memory.md
nexus memory export claude
nexus memory disable --export shared-memory.md
```

To turn off shared memory and keep the entries in Nexus, run `nexus memory disable --skip-export`.
