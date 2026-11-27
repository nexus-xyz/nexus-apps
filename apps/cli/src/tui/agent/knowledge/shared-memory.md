# Shared memory

Shared memory is off by default. When enabled, local Claude Code and Codex sessions start with personal and repository entries.
A repository entry follows the repository across worktrees. A personal entry is available in every repository.
With shared memory on, Nexus tells agents to use its `remember`, `recall` and `forget` tools by default for memory requests.
The startup index lists visible notes, newest first.
It can show fewer than 100 notes when it reaches the byte limit.
The default limit is 16 KiB. Codex CLI batch launchers have a smaller budget.
It includes personal notes.
Each entry gives an ID, a name, and a one-line description.
Call `recall` with an ID to read one note in full.
A text query searches all visible notes, including notes outside the index.
It returns up to 10 full notes, up to 50 more summaries, and the total number of matches.
Call `recall` before reading native memory or before writing code, docs, or a pull request when a listed note matches.
The `remember` tool saves repository notes with an optional name and description.
Add a personal note in Settings or with `nexus memory add`.

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
