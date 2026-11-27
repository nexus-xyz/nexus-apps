# Slack, routines and the inbox

These belong to the person's assistant (`@nexus`). The `assistant` topic covers the assistant itself.
Read this page when the person asks about Slack, routines or notifications.

## Slack

- Owners and admins connect a Slack workspace to the organization: `nexus slack connect`, then `nexus slack connect --confirm <install-id>`.
- `nexus slack status` lists the organization's workspaces.
- A channel can bind to a team: `nexus slack channel bind <workspace id> <channel id> --team <slug>`.
- In a bound channel, `@nexus` attributes each call to that team, under its policy and caps.
- The assistant uses the person's connectors by default, in Slack and in the apps. Each write still asks as above.
- `tools off` in a direct message with `@nexus` stops that, and `tools on` turns it back on.

## Routines

A routine is a morning brief from the assistant, on a schedule. It is the only kind of routine today.

```sh
nexus routines create --at 08:00 --days mon-fri --timezone America/Costa_Rica
nexus routines list
nexus routines pause <id>
nexus routines resume <id>
nexus routines delete <id>
```

- Ask the person for their time zone. Without `--timezone`, the time is UTC.
- A routine runs as the person only. It reads and notifies only.
- It reports to the person's Slack direct messages.
- A routine is not a scheduled Code session. For a coding task on a schedule, see `desktop`.

## The inbox

Notifications are the person's inbox in an organization: what needs their attention.

```sh
nexus notifications                 # the items not yet done, newest first
nexus notifications list --unread
nexus notifications read <id>
nexus notifications done <id>       # it leaves the inbox
nexus notifications kinds           # every kind of notification and its default channels
```

In Nexus UI and the desktop Inbox, routine account changes on the same UTC day fold into one group.
Open the group to review each original event and its destination.
Mark read and Done apply to the individual event.
Changes on different days stay separate, and items that need you stay outside routine groups.
