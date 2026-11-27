# The nexus terminal session

`nexus` alone on a terminal opens a session: a composer, and answers that stream.
It opens when the CLI is signed in, on an interactive terminal, outside CI. `NEXUS_NO_TUI=1` turns it off.
Read this page when the person asks how to do something in the session.

## Two modes

`shift+tab` switches the mode. The next prompt goes to the mode chosen.

| Mode | What answers |
| --- | --- |
| Chat | A Nexus chat session, the same as `nexus chat` and Nexus UI's home. It has no tools on this computer |
| Agent | You. You work in the folder with tools |

Chat sessions live in Nexus. The agent's conversations are files on this computer.

## Keys

| Key | What it does |
| --- | --- |
| `enter` | Send |
| `shift+enter`, `ctrl+j` | New line |
| `up`, `down` | Earlier prompts |
| `esc` | Stop the answer |
| `ctrl+c` | Clear, stop, or quit |
| `shift+tab` | Switch the mode |
| `left` on an empty composer | Switch to another chat |
| `down` on an empty composer | The agents panel: `main` and each subagent. `enter` opens a subagent's transcript |
| `@` | Mention a file |
| `ctrl+v` | Paste an image |
| `!` | Run a shell command in the folder. In agent mode, you see what it printed |
| `?` on an empty composer | Show the keys |

An edit or a command of yours waits for an answer: `y` allows it, `a` allows that kind for the session, `n` or `esc` declines.

## Slash commands

The person types these. You cannot run them. Tell the person which one to type.

| Command | What it does |
| --- | --- |
| `/model` | Choose the model for this session. `/model <id>` names one |
| `/new` | Start a new session. `/clear` is the same |
| `/resume` | Continue an earlier session: a chat session, or an agent conversation of this folder |
| `/status` | The organization, mode, approvals, model, session and budget |
| `/spend` | What is left of the budget |
| `/suggestions` | Turn suggested next prompts on or off |
| `/approvals` | What the agent may do without asking: `ask`, `auto-edit` or `auto` |
| `/diff` | What changed in this folder, from git |
| `/compact` | Summarize the agent's conversation, so each step costs less |
| `/cost` | What this session has spent, turn by turn |
| `/btw` | Ask a side question. It has no tools, and it does not join the conversation |
| `/help` | The commands and the keys |
| `/quit` | Leave. `/exit` is the same |

## Outside the session

- `nexus models pick` sets the model that a new session starts with.
- `nexus chat "prompt"` asks one question without the session. See `chat`.
- `nexus help` lists every command. See `commands`.
