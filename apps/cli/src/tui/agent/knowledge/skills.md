# Skills

A skill is a `SKILL.md` file: YAML front matter with a `name` and a `description`, then instructions for an agent.
A person keeps their skills in Nexus, in versions that never change.
Read this page when the person asks about skills, or asks you to use one.

## A person's own skills

```sh
nexus skills                                  # the person's skills
nexus skills create release-notes             # write a new one in their editor
nexus skills import ./pdf-processing          # add a SKILL.md, or a folder that holds one
nexus skills import ./pdf-processing --update # write a new version of one they have
nexus skills export pdf-processing --out skills
nexus skills delete pdf-processing --yes
```

- A skill belongs to the person, not to an organization. Nobody else can read it.
- A name is 1 to 64 characters: `a-z`, `0-9` and single hyphens. A description is at most 1,024 characters.
- A file holds at most 1 MiB. A person keeps at most 200 skills, and a skill at most 500 versions.
- `import` sends only the `SKILL.md`. It names the other files of the folder and leaves them.

## In Nexus GPUI

Settings › Skills opens your personal skills and the approved Library inside the app.
New skill and Import a file open a SKILL.md editor for review before saving.
Import accepts UTF-8 text up to 1 MiB.
A version opens its text and history. Export saves that version as SKILL.md through the system dialog.
Edit writes a new version and checks the latest version observed when editing began.
A conflicting save keeps the draft. Copy it before cancelling; cancellation loads the latest version.
Delete asks again before removing every version.
Library skills stay read-only and show only the versions Gateway makes available.
Request promotion pins the saved version currently selected, with an optional note and organization or member-team target.
Promotion requests lists your requests or an approver's reach, with state filters and cursor pages.
Inbox and organization notification links open the exact request or filtered list inside Settings.
The initial list shows an approver’s held requests within reach, or all of a requester’s own requests.
A reviewer opens the immutable file and changes from the previous approved version as plain text.
Hidden characters remain visible as codes. The diff conservatively removes and adds the changed middle.
Name warnings use the web’s NFKC and font-pair comparison against visible library and personal skills.
Permissions and team lineage control approval, refusal and withdrawal hints; Gateway rechecks every action.
Nobody approves their own request. Refusal requires a reason. Withdrawal can remove one approved version or all versions.
Decisions ask for confirmation. A concurrent decision reloads the current request before another action.
Promotion and personal-file writes share one lock. Account changes reject old replies.
The native reader does not report optional reviewed-scroll flags because it does not measure end-of-text visibility.
System file dialogs, promotion keyboard/layout flows, and signed-in account switches still need manual platform acceptance.

## The skill library

- A person asks to promote a skill to their organization or to a team they are in.
- The request is held until an approver who is not the author approves it. An approver is an owner, an admin or a team lead.
- An approved version joins the library of that organization or team.

```sh
nexus skills promote pdf-processing --to team/web --note "Used by the web team"
nexus skills promotions          # the requests the person can see
nexus skills approve <id>        # approvers; it needs their sign-in, not an API key
nexus skills refuse <id> --reason "Needs a test"
nexus skills library             # the skills approved for the person to load
```

## Where skills load

| Place | How |
| --- | --- |
| {{skills_launchers}} | The launcher writes the person's skills and the library's into the agent's skills folder. `--no-skills` skips it |
| Nexus chat | The chat lists them and loads the one it needs |

## You and skills

- You do not load skills by yourself. You have no skill tool.
- When the person asks you to use one, export it and read it:

```sh
nexus skills export pdf-processing --out .nexus/skills
```

Then read `.nexus/skills/pdf-processing/SKILL.md` and follow it. Say that you wrote that folder into the workspace.
