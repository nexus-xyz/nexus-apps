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
| "new session", "separate session", "independent" | Start an independent session. Do nothing more unless the person asks |
| "hand off", "hand over" | Start an independent session, and leave this session as it is |
| Work in another project | Start an independent session in that project |

- Start an independent session with `nexus sessions start "<prompt>" --project <owner/repo or the project's folder>`.
  From a worktree, always pass --project: a bare `nexus sessions start` finds no project there.
  `--project` takes a project's id, a folder inside it, or its GitHub owner/repo. It does not take the project's name.
- To hand work over without replacing this session, first commit your work on its branch.
  Then name the branch in the new session's prompt. It does not get your worktree or uncommitted changes.
- When the words were unclear, say which tool you chose and why.
- Use `handoff_session` only when the person asks to replace or archive this session.
  It archives this session and moves its worktree to the new session.
- `start_session` works only in this project.

## Get an independent session's result

Do this only when the person needs the result.

- Put your own id (`$NEXUS_SESSION_ID`) in its prompt, and ask it to report with `nexus sessions send <id> "<report>"`.
  Keep this to one or two sentences of the prompt.
- Or watch it: run `nexus sessions wait <id>` with `terminal_run`, with watch on.

## Stop or archive a subsession

- Stop is a pause. Stop a subsession only when its work may resume.
- Archive a subsession you started when you replaced it, or the person cancelled it.
  Archive it too when you read its result, need nothing more from it, and the person won't need it in the sidebar.
- Its branch stays. Its worktree stays until the person's worktree cleanup removes it, which never drops uncommitted or unpushed work.
  The person sets the cleanup in Settings › Projects and worktrees.
  Tell the person.

## Your own session

- Never archive your own session (`archive_session` with no `session_id`) unless the person says so.
  Never hand it off (`handoff_session`) unless the person says so.
  A saved memory that says to always do so counts. Do not guess.
- The person may state a standing preference, such as "always archive" or "never archive".
  Save it with shared memory (`remember`), so every new session follows it.
  If shared memory is unavailable, say so.
