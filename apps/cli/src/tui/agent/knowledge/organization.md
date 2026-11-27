# Organizations, roles, teams and the audit log

Browsers and phones require the approver's code from its own screen before trusting other devices.
Use browser Settings › Devices or the phone's Connect screen.
Never confirm codes for the person.

An organization holds members, provider keys, budgets, teams, repositories and audit events.

On the phone, Agents shows each run's reported person, usually an email, in the list and detail.
Owners and admins see the organization's runs. Members see their own runs.
Direct links open readable runs outside the recent list. Lookups have a deadline and cancel when you leave.
Temporary failures keep matching cached detail with an error.
Access denials and missing runs remove that run from the saved cache, including when the page opens again.
Returning to Agents refreshes its mounted list from the saved cache. Responses from an earlier focus cannot restore removed runs.

## Organizations

- Nothing is shared between organizations. A person can belong to several, and signs in to one at a time.
- Every person also has a personal organization. They are its owner and its only member.
- `nexus org` lists the person's organizations. `nexus org switch <org>` signs in to another one.
- `nexus org settings` shows the organization's settings. Owners and admins change one with `nexus org settings set <key> <value>`.
- Synced coding sessions open in Nexus UI in a browser until an owner or admin turns that off: `nexus org settings set webSessionViewer false`. The desktop app and the phone still open them.
- With `sessionContent: user_and_org`, computers encrypt sessions for the organization's recovery key after local confirmation.
  An administrator computes its fingerprint offline from the vault: `nexus devices org-key --vault-public-key <public-key-file>`.
  Compare it in `nexus devices org-key`, desktop Settings › Devices, or `nexus-host pair --org-key-fingerprint <fingerprint>`.
  Reconfirm legacy pins. Never confirm for the person.

## Phone control of terminal sessions

With session sync on (`NEXUS_SESSION_SYNC=1`) and the computer paired, the phone reads `nexus claude` and `nexus codex` sessions.
`nexus devices control on` turns Phone control on for this computer's terminal sessions. It is off until the person turns it on. `off` turns it off, and `nexus devices control` says where it stands.

- With it on, the phone can send a message to a `nexus codex` session, stop its turn and answer its approvals.
- The person keeps using Codex in the terminal. An approval shows in both places, and the first answer wins.
- For this, the CLI runs Codex's experimental app-server. If it cannot start, Codex runs as before, and one line says that the phone only reads the session.
- `nexus claude` sessions stay read-only from the phone. A closed terminal reads as offline.

## Roles

| Role | Can |
| --- | --- |
| Owner | Everything an admin can. Also billing, transferring ownership, and the default spend limit for each person. There is exactly one owner |
| Admin | Manage members, teams, repositories, provider keys, service accounts, the budget and model policies. See everyone's spend |
| Member | Use the products and coding agents. See their own spend and sessions, and their teams' |
| Team lead | A member who leads a team. In that team and the teams under it: narrow the model policy, pause or limit model traffic, read the audit events |

Unauthorized roles get `403 role_required`, which names the required role.
`status` shows your role.

## Members

```sh
nexus members                                # everyone, by role
nexus members invite new-hire@example.com    # owners and admins
nexus members invite --pending --dry-run     # who would be invited
nexus members remove ada@example.com --dry-run
nexus members removed                        # who can still be recovered, for 30 days
```

- An invitation is always explicit. Adding someone to a team by email never sends an email.
- Removing someone revokes their tokens and keys at once.
- Someone added to teams by email who has not joined is pending. Owners and admins remove them in Nexus UI under
  Members › Not joined yet or Teams › Pending, or with `nexus team pending --cancel <email>`. They leave every team.

## Teams

- Teams nest, up to 10 levels. A team has a slug, a name and an optional level label such as `department`.
- A person can be in several teams and has one default team. Their spend goes there when a call names no team.
- A team can have its own model policy and its own spend cap. See `models` and `spend`.

```sh
nexus team tree
nexus team show platform
nexus team create platform --parent engineering --level team    # owners and admins
nexus team add platform ada@example.com --lead
nexus team use platform                                         # the person's own default team
nexus team import teams.yaml --dry-run                          # load a whole tree from a file
```

- `--team <slug>` or `NEXUS_TEAM` names the team that a run or a session is attributed to.
- An import's dry run prints a plan. `nexus team import teams.yaml --plan <plan>` applies exactly that plan.

## Service accounts

Service accounts run CI and scripts without membership or seats.
Owners and admins use `nexus service-accounts`. Each key appears once.

## The audit log

The audit log records each change's actor, credential, origin, and before and after states.
Model calls appear in `nexus spend`. Message content never enters the log.

```sh
nexus audit --since 24h
nexus audit --action 'team.*' --actor ada@example.com
nexus audit --csv > audit.csv
```

Owners and admins see every event. A team lead sees their teams' events. Everyone else sees their own actions.

Encrypted team session sharing also needs a machine-local `sessionTeamSync` managed policy. Gateway cannot enable it alone. An administrator verifies and admits exact member roots locally. A missing thread remains private. See `nexus-workbench/src/session_sync/TEAM-SHARING.md`.
