# Code sessions from a script or another agent

Read this before you start, stop, archive or hand off a Code session of Nexus for desktop.
`nexus sessions` reaches the app's Code sessions from a shell, a script or another agent.

## Reach a session

- To reach another session, run `nexus sessions list`, then `nexus sessions send <name> "<message>"`.
  `nexus sessions` also starts, shows, names, waits for and stops them.
  A session is its id (`t_…`), name (any case) or title. A name wins over a title,
  which must be unique in the newest 200.
  `nexus sessions name` or the app's `/name` sets a unique name for Claude Code's SendMessage.
- It needs Outside access (macOS, Linux): Settings › MCP servers. Each client asks once.

## Which tool the person's words ask for

| The person says | What you do |
| --- | --- |
| "subsession", "spawn", "delegate", or words that are unclear | Use `start_session`. It starts a linked subsession, and its result comes back to you |
| "new session", "separate session", "independent" | Run `nexus sessions start`. Do nothing more unless the person asks |
| "hand off", "hand over" | Run `nexus sessions start`, and leave this session as it is |
| Work in another project | Run `nexus sessions start --project <folder or owner/repo>` |

- When the words were unclear, say which tool you chose and why.
- Use `handoff_session` only when the person asks to replace or archive this session.
  It archives this session and moves its worktree to the new session.
- `start_session` works only in this project.

## Get an independent session's result

Do this only when the person needs the result.

- Put your own id (`$NEXUS_SESSION_ID`) in its prompt.
  Ask it to report with `nexus sessions send <id> "<report>"`.
- Or watch it: run `nexus sessions wait <id>` with `terminal_run`, with watch on.

Keep this to one or two sentences of the prompt.

## Stop or archive a subsession

- Stop is a pause. Stop a subsession only when its work may resume.
- Archive a subsession you started when you replaced it, or the person cancelled it.
  Archive it too when you read its result and need nothing more from it.
- Archiving leaves the worktree and branch on disk.
  The person's worktree cleanup can remove them: Settings › Projects and worktrees.
  Tell the person they are there.

## Your own session

- Never archive your own session (`archive_session` with no `session_id`) unless the person says so.
  Never hand it off (`handoff_session`) unless the person says so.
  A saved memory that says to always do so counts. Do not guess.
- The person may state a standing preference, such as "always archive" or "never archive".
  Save it with shared memory (`remember`), so every new session follows it.
  If shared memory is unavailable, say so.
