# Build a fork or a client on the Nexus API

This page is for people who build a fork of a Nexus app, or their own client, on the public API.
It covers the OAuth client ID, the scopes your client gets, its limits, the write rules, the session protocol, the terms, and how to name your client.
The contract is `../openapi/`, and `../openapi/README.md` says the rules in plain words.
When this page and the contract differ, the contract wins.

## Your OAuth client ID

Every sign-in names an OAuth client ID (`nxc_…`). The ID puts your client in a tier:

- `official`: Nexus's own apps, in the builds that Nexus signs.
- `registered_fork`: a client that a person registered. It has its own ID.
- `unregistered`: every other client. They share the ID `nxc_unregistered`.

Do not send an official ID from a build you made. Register your own ID, or send none.
A sign-in without an ID gets `nxc_unregistered`.
The client ID identifies a client. It is not a secret and does not authenticate the client.

### Register a client

1. Sign in as yourself, with a credential that may manage your own keys.
2. Send `GET /v1/developer/clients`. The answer lists your clients and the `grantableScopes`.
3. Send `POST /v1/developer/clients` with `name` and an `Idempotency-Key`.
   Optionally send `allowedScopes`, a subset of `grantableScopes`. Omit it to allow all of them.
4. Keep the `nxc_…` ID from the 201 answer. Send it as `client_id` when you start a sign-in.

To stop a client, send `POST /v1/developer/clients/{id}/revoke`.
The revocation also revokes the client's tokens and ends its agent links.
You cannot use a token of a third-party client to manage clients.
The sign-in flows are in `../openapi/README.md`, "Sign-in (OAuth 2.0)".

The Gateway applies client IDs only where a server setting turns OAuth client tiers on.
Where the setting is off, the Gateway ignores `client_id`, and the `/v1/developer/clients` routes answer 404 `oauth_client_tiers_off`.

## The scopes your client gets

A token of a `registered_fork` or `unregistered` client holds at most these scopes.
A registered client holds only its `allowedScopes`.
The person's role and the organization's policies still apply on top.

| Scope | What it allows |
| --- | --- |
| `models.use` | Model calls, and the list of models you may use. |
| `spend.read_own` | Your own spend, analytics and spend limits, and your teams'. |
| `connectors.use` | Connector reads, and module writes with a standing policy grant. |
| `chat.ask` | `POST /v1/chat/ask` alone, in an ask-mode session of your own. |
| `chat.web` | `POST /v1/chat/web` alone: a question answered with web search. |
| `agents.memory` | Your agent's own memory (`/v1/agents/memory`). |
| `agents.profiles` | Read your agents' profiles. |
| `routines.use` | Your own routines (`/v1/routines`). |
| `cloud.runs.use` | Start and read your own fix runs (`/v1/cloud/runs`). |
| `sessions.read` | Read your own cloud sessions. |
| `sessions.start` | Start cloud sessions as you, billed to you. |
| `sessions.steer` | Send messages to your own cloud sessions, and stop them. |

`grantableScopes` in `GET /v1/developer/clients` is the current list. Read it rather than copy this table.

A third-party token never holds administration, billing, provider keys, invitations or token management.
It also cannot use cloud machines, so it cannot call the `/v1/sessions` routes today.
A request outside your client's scopes gets 403 `client_scope`.

When a person approves a third-party device sign-in, the approval page names your client as an "Unverified third-party app".
It lists each scope the token will hold.

## Limits and concurrency

A third-party client has its own limits, on top of the person's and the organization's.
`../openapi/README.md`, "OAuth client tiers", gives the default numbers for each tier.

- Over the client's rate limit, the Gateway answers 429 `rate_limited`.
- Over the requests in flight for one person through your client, the Gateway answers 429 `client_concurrency_limit` with `Retry-After: 1`.
- A request holds its place until its answer ends. A stream holds it for as long as it streams.
- Honor `Retry-After`. Retry a write with the same `Idempotency-Key`.

## Write rules: Idempotency-Key and If-Match

Two rules keep retries and concurrent edits safe:

- Send an `Idempotency-Key` on every state-changing `POST`. Without it, the Gateway answers 428 `idempotency_key_required`.
- Send `If-Match` with the last `ETag` on every edit of an editable resource. A stale value gets 412 `version_mismatch`.

`../openapi/README.md` has the full rules in "Idempotency-Key" and "If-Match".
Each operation states its rule in `x-idempotency` and `x-if-match`.

## The session protocol

The `/v1/sessions` routes run a headless cloud session. They are beta, and the Gateway answers them only where a server setting turns them on.

1. Start a session with `POST /v1/sessions` and an `Idempotency-Key`. A resend with the same key answers the same session.
2. Read it with `GET /v1/sessions/{id}`. It moves from `starting` to `running`.
3. Send the next message with `POST /v1/sessions/{id}/messages` and an `Idempotency-Key`.
4. Stop it with `POST /v1/sessions/{id}/stop` and an `Idempotency-Key`. A stop of an ended session changes nothing.

The runner is the program that runs the session's agent: the desktop app, `nexus-host` or a cloud machine.
One runner holds a session at a time. A fencing token and a lease enforce this:

1. Take the session with `POST /v1/sessions/{id}/runner`. You get the session's next fencing token and a lease of 60 seconds.
   Without `seenToken`, the take succeeds only when no runner's lease is live.
   With `seenToken`, the take is a handoff from the runner that holds that token.
2. Renew the lease every 20 seconds with `POST /v1/sessions/{id}/runner/renew` and `{ fencingToken }`.
3. Name the session and the token on each connector write: `x-nexus-cloud-session` and `x-nexus-fencing-token`. Send both or neither.

Each take raises the token by one. The runner it replaced then gets 409 `lost_runner`.
On `lost_runner`, stop running the session and stop writing for it.
Of two takes at once, one wins and the other gets 409 `takeover_conflict`.
`error.nexus.currentFencingToken` holds the current token in both answers.
A session that stopped or failed answers 409 `session_ended`.

## Terms

A fork or client that calls the Nexus API is bound by the [Customer Terms](https://nexus.inc/legal/customer-terms) and the [Acceptable Use Policy](https://nexus.inc/legal/customer-terms#schedule-b-acceptable-use-policy) (Schedule B of the Customer Terms).
It uses its own client ID, and it may not present itself as an official Nexus app.

## Names

Your client's `name` shows on the approval page. Do not choose a name that suggests Nexus made or endorses your client.
