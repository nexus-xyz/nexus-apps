# Chat sessions and artifacts

Nexus UI's home is a chat. The same chat sessions are in the CLI (`nexus chat`) and in the terminal session's chat mode.
Read this page for chat, for sharing a chat as an artifact, and for what chat can do that you cannot.

## Chat sessions

- A session keeps its messages, the model that ran, the team its spend goes to, and its cost.
- A session is private to the person who starts it. Owners and admins cannot read it.
- Messages are kept for 90 days by default. Owners and admins choose from 1 to 3650 days.
- Chat answers with any model the person may use. Nexus Wiki and Ask your code were removed. Their routes answer `404` `wiki_off`. For code, read the workspace's files, or paste what the model needs into Chat.
- A click on the sidebar's Sessions head folds the list to the four newest sessions. Another click opens it again. The browser or the app keeps the choice. To find an older session while it is folded, the person opens the list or runs `nexus chat sessions`.
- In the desktop app, Sessions lists chats only, as the web does: 12, then Show more, then All sessions, with Today, This week and Older headings past 12. Folded, its head shows how many sessions it hides. With four sessions or fewer, the head does not fold.
- In the desktop app, an agent run is listed under its repository's project, wherever it runs. A cloud run shows a cloud. A cloud run on a repository with no project on this device gets a cloud project named for the repository.

```sh
nexus chat "How should we structure our onboarding docs?"    # continues the last session
nexus chat "Shorter, please" --model anthropic/claude-haiku-5.5
nexus chat "Why is this cut off?" --image shot.png --new      # a new session, with an image
nexus chat sessions                                           # the person's sessions
nexus chat sessions show <id>
nexus chat export <session> --md --out chat.md
```

## What chat can do

- It writes pages when the person asks for a page, a site, an app or a dashboard.
- When the person may want a page but did not ask clearly, chat ends its answer with "Want me to make it as a page?". A yes on the next turn writes the page.
- It reads the connectors that the person may use, as the person. See `connectors`.
- It loads the person's skills and their organization's skill library. See `skills`.
- In Nexus UI, typing `@` in the message box lists the person's artifacts. A pick attaches the artifact's Markdown to the message as a file, and the model reads it with the message. Chat cannot read an artifact or a session by its link alone.
- It searches the web by default. An owner or admin can turn web search off for the organization, and `nexus chat --no-web` turns it off for one session.
- On iPhone, Share › Nexus keeps text, web links and photos on the phone, with no upload. When the app opens, the person picks a new chat or the Assistant, and the share waits there as an unsent draft. A shared link gives chat its address only.

Your own conversation is not a chat session. It has none of these tools.
`nexus chat "<question>" --new` asks chat from here. It starts a session in the person's account, and it costs a model call.

## Artifacts: share a chat

A shared chat is an artifact: a read-only snapshot with its own link. Share one only when the person asks.

```sh
nexus chat share <session> --org                        # or --team web, or --people a@x,b@y
nexus chat share <session> --to ada@partner.example     # named people outside, by email
nexus chat share <session> --public --expires 30d       # anyone with the link
nexus chat share <session> --private                    # stop sharing: every version revoked
nexus artifacts list                                    # shared with the person and by them
nexus artifacts show <id>
nexus artifacts revoke <id>
```

- Sharing again makes a new version. `--replace` revokes the earlier ones.
- `--allow-continue` lets a reader carry the chat on in a private copy of their own.
- Nexus does not email the people you share with. The person sends the link.
- `nexus agents share latest --org` shares this folder's newest Claude Code session the same way.
- You cannot share your own conversation with these commands. It is not a chat session.

## The organization's limits on sharing

Owners and admins set them with `nexus org settings set <key> <value>`. Everyone can read them with `nexus org settings`.

| Setting | What it does |
| --- | --- |
| `artifacts.external` | `public`, `named` (named people only) or `off` (nothing leaves the organization) |
| `artifacts.external.who` | `everyone`, or `admins` for owners and admins only |
| `artifacts.external.expiry` | A maximum life for public links, or a required one |
| `artifacts.pages` | Off: nobody writes or shares pages |
| `artifacts.pages.public` | Off: no public links to pages |
| `artifacts.pages.queries` | Off: pages cannot run approved live data queries. Off by default |

A choice that a setting forbids is refused, and the message says why.

In Nexus GPUI, General holds sharing, live page data and chat retention.
Settings search opens Sharing and pages or Chat retention at its native control.
Data addresses also open General. Only people with `org.settings` may change these values.
Native focus and account-switch flows still need manual platform acceptance.
