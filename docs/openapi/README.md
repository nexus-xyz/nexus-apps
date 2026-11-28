# Nexus public API contract

This folder holds the OpenAPI 3.1 contract for the Gateway's public client endpoints.
Client and Gateway developers read it.
Admin, staff and internal endpoints are out of scope.

## Files

- `openapi.yaml`: the entry point. It holds the servers, tags and security, and points to the other files.
- `paths/*.yaml`: the path items, one file per tag.
- `components/schemas.yaml`: the request and response schemas.
- `components/common.yaml`: the shared parameters, responses and headers.
- `DEPRECATION.md`: how long an old version keeps working, and how clients learn about a change.
- `gateway.json`: the Gateway's whole client-callable document in one JSON file, cleaned the same way. Clients that build on every Gateway operation, such as nexus-apps' `nexus-client`, read it. The deprecation windows cover only the operations in `openapi.yaml`.

Every path starts with `/v1`.
Every operation has an `operationId`, unique in the contract.

## Beta operations

Operations marked `x-internal-stability: beta` are public and you can call them.
They are not stable yet: the deprecation windows in `DEPRECATION.md` do not cover them.
A beta operation can change or go away with a changelog entry and at least 30 days' notice.
These tags are beta, every operation in them:

- `Billing`: every path under `/v1/billing`.
- `Cloud machines`: every path under `/v1/cloud`.
- `Sessions`: every path under `/v1/sessions`. The Gateway answers them only where a server setting turns them on.

To check the contract, run `npx @redocly/cli lint openapi/openapi.yaml`.
To get one file, run `npx @redocly/cli bundle openapi/openapi.yaml -o nexus-api.yaml`.

## Sign-in (OAuth 2.0)

This section is the sign-in contract for the `/v1/auth/*` endpoints.
It says what the Gateway does today.
A client gets a Gateway token through one of two OAuth 2.0 grants.
Both grants end at `POST /v1/auth/token` with the same token answer.

### OAuth client tiers

When the Gateway enables OAuth client tiers, it binds each new sign-in to a client ID.
An omitted or blank ID selects `nxc_unregistered`.
The Gateway returns 400 `invalid_client` for a malformed, unknown, or revoked client ID.
When tiers are disabled, the Gateway ignores the client ID.

| Tier | Who uses it | Client limits |
| --- | --- | --- |
| `official` | Nexus apps, such as `nxc_desktop`, `nxc_cli`, and `nxc_mobile` | No additional client limits |
| `registered_fork` | A fork with its own registered ID | Default: 300 requests per minute per organization, 6,000 across organizations, and 8 in flight per person |
| `unregistered` | Clients that share `nxc_unregistered` | Default: 120 requests per minute per organization and 4 in flight per person. No global limit. |

The two third-party tiers limit what a credential can do.
The Gateway returns 403 `client_scope` if the client does not allow the requested action.
The usual permissions and rate limits still apply.

A client rate limit returns 429 `rate_limited`.
If a person has too many requests in flight through a third-party client, the Gateway returns 429 `client_concurrency_limit`.
Honor `Retry-After` in either response.

### Manage a fork's OAuth client

Use a signed-in person's credential with permission to manage their own keys.
You cannot use a third-party token to manage clients.

- `GET /v1/developer/clients` lists your clients, including revoked clients, and returns the `grantableScopes` for forks.
- `POST /v1/developer/clients` registers a `registered_fork` client and returns its `nxc_…` ID with status 201.
  Send `name` and, optionally, `allowedScopes` from `grantableScopes`.
  Omit `allowedScopes` to allow all grantable scopes.
- `POST /v1/developer/clients/{id}/revoke` revokes a client you registered, its tokens, and its agent links.
  Send an optional `reason`.
  A repeat returns the original revocation time and reason.

Send `Idempotency-Key` on both `POST` operations.
When tiers are disabled, these routes return 404 `oauth_client_tiers_off` for an authorized caller.

### Device grant (RFC 8628)

The CLI and the desktop app use this grant.

1. Send `POST /v1/auth/device` (`startDeviceLogin`) without a credential.
   Send the OAuth client ID in the optional `client_id` JSON field.
   The answer holds `device_code`, `user_code`, `verification_uri`, `verification_uri_complete`, `expires_in` and `interval`.
   The code lives 600 seconds. The first `interval` is 5 seconds.
2. Show the person `verification_uri_complete`, or `verification_uri` and `user_code`.
   The person signs in to Nexus UI there and approves the code in one organization.
3. Poll `POST /v1/auth/token` (`pollDeviceLogin`) with `grant_type` `urn:ietf:params:oauth:grant-type:device_code` and `device_code`.
   A body without `grant_type` is a device-code poll too.
4. Before approval, the answer is 400, and `error.code` holds the state:
   - `authorization_pending`: poll again after `interval` seconds.
   - `slow_down`: you polled too fast. The body's `interval` is the new interval, 5 seconds longer.
   - `expired_token`: the code expired or was already used. Start again.
   - `access_denied`: the person denied the code. Start again.
5. The first poll after approval gets the token answer. A later poll with the same code gets `expired_token`.

With `agent` and `scope` in step 1, the approval issues an agent link (`nxo_…`) that holds only that scope.
Its token answer then has `scope`.

### Authorization code with PKCE (RFC 6749 4.1, RFC 7636, RFC 8252)

The mobile app uses this grant.

1. Make a code verifier and its `S256` challenge. The Gateway accepts no other method.
2. Open `{gateway}/authorize` in the system's sign-in sheet.
   Send `redirect_uri`, `code_challenge`, and `code_challenge_method=S256`.
   Optionally send `client_id`, `client`, and `state`.
   The Gateway sends the sheet to Nexus UI, which signs the person in.
   Nexus UI forwards `client_id` as `clientId` when it requests the code.
3. Nexus UI sends `code` and your `state` to `redirect_uri`.
   The Gateway accepts only the Nexus app's registered redirect URIs.
4. Send `POST /v1/auth/token` with `grant_type` `authorization_code`, `code`, `code_verifier` and `redirect_uri`.
   The code lives 2 minutes and works once.
   An unknown, used, expired or mismatched code gets 400 `invalid_grant`. A failed exchange also uses the code.

Do not send a client ID at token exchange. The Gateway uses the ID from the original sign-in.
If the client is no longer valid at token issuance, the Gateway returns 400 `invalid_client`.

### The token answer

- `access_token` is an opaque bearer token: `nx_…` for a person, `nxo_…` for an agent link. It is not a JWT.
  The Gateway keeps only its hash. Send it as `Authorization: Bearer <token>`.
- `token_type` is `Bearer`.
- `client_id` is the OAuth client bound to the token, if the sign-in used OAuth client tiers.
- `expires_at` is the time the token expires. A person's token lives 90 days.
- `user.id` is the Nexus user id (`usr_…`). See "User id" below.
- `tenant`, `role`, `scope` and `ui_url` give the organization, the person's role, an agent link's scope and Nexus UI's address.

### Refresh

The Gateway issues no refresh token and takes no `refresh_token` grant.
When a token expires or you revoke it, calls with it return 401 `invalid_credential`. Sign in again.
The Gateway can return 401 `client_revoked` when it rejects a credential because its client was revoked or is unknown.
Sign in with another client in that case.

### Revocation

- `POST /v1/auth/logout` revokes the token that calls it.
- `DELETE /v1/auth/tokens/{id}` revokes one of your tokens by id. Owners and admins can revoke anyone's.

### User id

Every user id a client sees is the Nexus user id (`usr_…`).
This holds whichever provider signed the person in to Nexus UI: the hosted sign-in provider, or Better Auth with Google, Apple or an email code.
The Gateway finds the person by the provider and the provider's own id for them.
It never sends the provider's id in place of the Nexus user id.
The same id is in these fields:

- `user.id` in the token answer;
- `userId` in `GET /v1/auth/whoami`;
- `user.id` in `GET /v1/tenants/me`;
- `userId` on members, tokens and agent links.

### Where the Gateway differs from the RFCs

- Request bodies are JSON, not `application/x-www-form-urlencoded`.
- The token answer has `expires_at`, a time, not `expires_in`.
- An error body has the `Error` shape. `error` is an object, and `error.code` holds the OAuth error code.
- An unknown `grant_type` gets 400 `unsupported_grant_type`.
- The client ID is a public identifier. The Gateway does not require a client secret or client authentication.
- `client` is a display label, not a client ID.
- The Gateway issues no JWT and is not a token issuer for other services (ADR 0010, decision 4). ADR 0010 is still Proposed.

## Idempotency-Key

Send an `Idempotency-Key` header on every state-changing `POST`.
Without it, the Gateway answers 428 `idempotency_key_required`.
Each operation says its rule in `x-idempotency`: `required`, `optional` or `ignored`.

- Use a new random value, such as a UUID, for each new write.
- Use the same value only to retry the same request: same method, path and body.
- The key is 1 to 255 printable ASCII characters. Otherwise the answer is 400 `invalid_idempotency_key`.
- The Gateway keeps the first answer for 24 hours. A retry gets that answer again, with `Idempotent-Replayed: true`.
- A retry while the first request still runs gets 409 `idempotency_in_progress`. Wait, then retry with the same key.
- The same key with a different request gets 422 `idempotency_key_reused`.
- If the first request was cut off, a retry gets 409 `idempotency_interrupted`. The write may have run in part. Read the state, then decide. Use a new key for a new attempt.
- The Gateway does not keep answers of 429 or 500 and above. A retry with the same key runs the request again.
- A streamed model call is not replayed. A retry gets 409 `idempotency_stream_replay`, and the call is not charged again.

`PUT`, `PATCH` and `DELETE` accept the header but do not need it.

These `POST` operations do not need the header:

- OAuth 2.0 endpoints: `/v1/auth/device`, `/v1/auth/token` and `/v1/auth/logout`. Their OAuth rules make a repeat safe. See "Sign-in (OAuth 2.0)".
- Read-only checks: `/v1/agents/check`, `/v1/policies/test`, `/v1/pages/{id}/check`, `/v1/connectors/{name}/{operation}/read` and `/v1/anthropic/v1/messages/count_tokens`.
- Provider-compatible model routes: `/v1/anthropic/v1/messages`, `/v1/openai/v1/responses` and `/v1/chat/completions`. Provider SDKs do not send the header. The Gateway honors it when it is sent.
- Heartbeats: `/v1/presence`, `/v1/threads/{threadId}/presence` and `/v1/cloud/machines/{id}/activity`.
- An OpenTelemetry export: `/v1/agents/sessions/{id}/usage`. The exporter sends only `Authorization`. A repeated record counts once.
- A page's shared draft: `/v1/pages/{id}/doc`. A repeated Yjs update changes nothing. A key would store an answer per keystroke.

## If-Match

An editable resource is one you read with `GET` and change with `PUT` or `PATCH` on the same path.
When it serves an `ETag`, its `GET` answer carries the header. The `ETag` is the resource's version.

- Send that value in `If-Match` on every `PUT`, `PATCH` or `DELETE` of the resource.
- The Gateway's `ETag` has the form `"42"`. `If-Match` takes `"42"`, `42` or `*` (any version).
- Without `If-Match`, the Gateway answers 428 `if_match_required`.
- If someone changed the resource since your read, the Gateway answers 412 `version_mismatch`. The `ETag` header and the body's `nexus.etag` hold the current version. Read the resource again, merge your change, and retry.
- A successful write answers with the new `ETag`.
- Do not retry a 412 with the same `If-Match`. It fails again.

Every editable resource serves an `ETag`, and its edits carry `x-if-match: required`:

- `/v1/agents/access`
- `/v1/orgs/settings`
- `/v1/policies/organization`, `/v1/policies/teams/{slug}`, `/v1/policies/agents/{agent}`, `/v1/policies/review-events` and `/v1/policies/chat-writes`
- `/v1/teams/{slug}` and `/v1/chat/sessions/{id}` (`PATCH` and `DELETE`)
- `/v1/people/{userId}/budget`
- `/v1/presence/settings`
- `/v1/notifications/settings`
- `/v1/policies/grants/{product}/{capability}` (`PUT`; its `ETag` is the one `GET /v1/policies/grants` serves)

The agent memory and agent state routes keep their own codes: 409 `agent_memory_version_conflict` and 428 `agent_memory_precondition_required`.

## Errors

Every error answer has one shape, the `Error` schema:

```json
{ "error": { "type": "invalid_request_error", "code": "version_mismatch", "message": "..." } }
```

Branch on `code`, not on `message`.
If a client gets a code it does not know, it uses the HTTP status.
The provider-compatible routes (`/v1/anthropic/`, `/v1/openai/`) answer in the provider's error shape.

"Retry: yes" means: wait, then send the same request again with the same `Idempotency-Key`.
Honor `Retry-After` when the answer has it.
"Retry: no" means: change the request or the state first.
The same table is in `components/schemas.yaml`, as `x-error-codes` on the `Error` schema's `code` field.

| Code | HTTP | Retry | Meaning |
| --- | --- | --- | --- |
| `invalid_request` | 400 | no | The request is malformed. The message says what to fix. |
| `unauthenticated` | 401 | no | No valid credential. Sign in again. |
| `payment_required` | 402 | no | The organization must pay before this call runs. |
| `forbidden` | 403 | no | The caller's role does not allow this action. |
| `not_found` | 404 | no | The resource does not exist or the caller cannot see it. |
| `method_not_allowed` | 405 | no | The path does not take this HTTP method. |
| `request_timeout` | 408 | yes | The request body arrived too slowly. |
| `conflict` | 409 | no | The request conflicts with the current state. |
| `unprocessable` | 422 | no | The request is well formed but cannot be applied. |
| `rate_limited` | 429 | yes | The caller or its OAuth client exceeded a rate limit. Wait for `Retry-After`, then retry. |
| `idempotency_key_required` | 428 | no | A state-changing POST has no `Idempotency-Key` header. Add one and send again. |
| `if_match_required` | 428 | no | A write to a resource that serves an `ETag` has no `If-Match` header. Read the resource, then send its `ETag`. |
| `version_mismatch` | 412 | no | The `If-Match` value is not the current version. The `ETag` header (and `nexus.etag` on the Gateway) holds the current one. Read the resource again, merge, and retry. |
| `invalid_idempotency_key` | 400 | no | The `Idempotency-Key` is not 1 to 255 printable ASCII characters. |
| `idempotency_key_reused` | 422 | no | The key was used before for a different request. Use a new key for a new request. |
| `idempotency_in_progress` | 409 | yes | The first request with this key still runs. Retry with the same key in a moment. |
| `idempotency_interrupted` | 409 | no | The first request with this key was cut off and may have run in part. Check the state, then use a new key. |
| `idempotency_response_not_stored` | 409 | no | The first answer was too large to keep. The request is not run again. |
| `idempotency_stream_replay` | 409 | no | The key belongs to a finished streamed model call. A stream is not replayed or charged again. |
| `idempotency_already_charged` | 409 | no | A model call with this key was already charged. It is not run again. |
| `idempotency_already_sent` | 409 | no | A message with this key was already sent to this session. It is not sent again. |
| `invalid_credential` | 401 | no | The token is unknown, expired or revoked. Sign in again. |
| `invalid_client` | 400 | no | The sign-in names a malformed, unknown or revoked OAuth client. Start again with a valid client id. |
| `client_revoked` | 401 | no | The OAuth client for this credential is revoked or unknown. Sign in with another client. |
| `client_scope` | 403 | no | The OAuth client does not allow the requested action. |
| `client_concurrency_limit` | 429 | yes | You have too many requests in flight through this OAuth client. Wait for `Retry-After`, then retry. |
| `oauth_client_tiers_off` | 404 | no | This Gateway has not enabled OAuth client tiers. |
| `invalid_grant` | 400 | no | The authorization code is unknown, used, expired or mismatched, or an agent link's grant is empty. Start the sign-in again. |
| `invalid_code` | 400 | no | The code is unknown or expired. Start the sign-in again. |
| `code_used` | 409 | no | The one-time code was already used. |
| `reserved_client` | 400 | no | The client id is reserved for another agent's sign-in. |
| `invalid_scope` | 400 | no | A requested scope cannot be granted to an agent. |
| `grant_not_allowed` | 400 | no | A requested permission cannot be granted to an agent. |
| `person_required` | 403 | no | Only a signed-in person can do this, not a service account. |
| `role_required` | 403 | no | Only a signed-in member can do this. |
| `not_a_person` | 403 | no | Your agent's routes take a person's own token or their agent link, not a service account's, a module's or an operator's. |
| `missing_credential` | 401 | no | Send `Authorization: Bearer <key>`: your agent's routes take your own token. |
| `owner_only` | 403 | no | Only an owner of the organization can do this. |
| `confirmation_required` | 400 | no | Type the confirmation text to confirm a destructive action. |
| `api_key_limit` | 409 | no | The person holds the most API keys allowed. Revoke one first. |
| `agent_link_limit` | 409 | no | The person holds the most agent links allowed. Unlink one first. |
| `unknown_agent_link` | 404 | no | No such agent link. |
| `key_not_live` | 409 | no | The key is already revoked or expired. |
| `key_signup_off` | 403 | no | Key-only sign-up is off. Sign in with the device flow. |
| `organization_required` | 409 | no | The action needs an organization of your own. |
| `unknown_organization` | 404 | no | No such organization. |
| `personal_org` | 409 | no | This is a personal organization. Name a company organization. |
| `not_a_member` | 403 | no | The named person is not a member of this organization. |
| `owner_not_a_member` | 400 | no | The new owner is not a member of this organization. |
| `owner_role` | 409 | no | The owner's role changes only by a transfer of ownership. |
| `not_an_admin` | 409 | no | Only an admin can be allowed to set spend limits. |
| `limit_grant_required` | 403 | no | Only owners, and admins an owner allows, can set person spend limits. |
| `use_nexus_ui` | 409 | no | Do this in the web app, which keeps the identity provider in step. |
| `clerk_failed` | 502 | yes | The identity provider failed. Retry later. |
| `clerk_refused` | 409 | no | The identity provider refused the change. |
| `not_a_clerk_org` | 409 | no | The organization does not use single sign-on, so it has no email domains. |
| `domain_not_yours` | 403 | no | Only a person who signs in with single sign-on can add an email domain. |
| `invalid_domain` | 400 | no | The email domain is not valid. |
| `public_mail_domain` | 400 | no | The domain is a public mail provider and cannot admit people. |
| `count_changed` | 409 | no | The invitation list changed since you reviewed it. Review it again. |
| `plan_changed` | 409 | no | The organization changed since the dry run. Run it again. |
| `slug_taken` | 409 | no | Another resource already uses this slug. |
| `verified_email_required` | 400 | no | The action needs a verified email on the sign-in. |
| `unknown_team` | 404 | no | No such team in this organization. |
| `not_in_team` | 403 | no | You are not in this team. |
| `has_children` | 409 | no | The team has teams under it. Move or delete them first. |
| `team_cycle` | 409 | no | A team cannot be its own ancestor. |
| `team_limit` | 409 | no | The organization holds the most teams allowed. |
| `too_deep` | 409 | no | The team tree would be too deep. |
| `budget_exceeded` | 402 | no | A budget is used up for this period. |
| `credit_exhausted` | 402 | no | The credits are used up. Buy credits or turn on pay-after-the-fact. |
| `subsidy_exhausted` | 402 | no | The call may cost more than the credit left. |
| `payment_past_due` | 402 | no | The organization has an unpaid invoice. An owner can pay it in billing. |
| `invalid_amount` | 400 | no | The amount is out of range or not in whole cents. |
| `too_many_decimals` | 400 | no | A decimal amount has more digits than allowed. |
| `invalid_period` | 400 | no | The period is not valid. |
| `invalid_mode` | 400 | no | The budget mode is not valid. |
| `mode_not_available` | 400 | no | The budget mode is not available yet. |
| `billing_setup_in_progress` | 503 | yes | A card setup for this organization is starting. Retry in a moment. |
| `stripe_error` | 502 | yes | The payment processor failed or did not answer. |
| `settlement_unconfirmed` | 504 | no | The payment outcome is not confirmed yet. Read the billing state before you retry. |
| `subscriptions_disabled` | 403 | no | Agent subscriptions are off for this organization. |
| `provider_key_required` | 403 | no | The organization has no key for this provider. |
| `provider_key_not_found` | 404 | no | No key is stored for this provider. |
| `provider_key_rejected` | 400 | no | The provider rejected the key. |
| `provider_key_changed` | 409 | no | The key changed while it was checked. Enable it again. |
| `provider_key_invalid_character` | 400 | no | The key has a character a provider key cannot have. |
| `provider_retired` | 400 | no | The provider is retired. Add a key for a supported provider. |
| `provider_disabled` | 403 | no | The organization turned off every provider for this model. |
| `unknown_provider` | 400 | no | The policy names an unknown provider. |
| `providers_allow_empty` | 400 | no | The policy's provider allow list is empty. |
| `model_not_allowed` | 403 | no | A model policy refuses this model. |
| `model_not_found` | 404 | no | No such model in the catalog. |
| `model_not_supported` | 400 | no | The model does not take this kind of call. |
| `model_no_image_input` | 400 | no | The model does not read images. |
| `model_no_tools` | 400 | no | The model does not take tool calls. |
| `output_over_limit` | 400 | no | `max_tokens` is above the limit for this call. |
| `reasoning_effort_not_offered` | 400 | no | The model does not offer this reasoning effort. |
| `response_format_not_offered` | 400 | no | The model does not offer this response format. |
| `tool_choice_not_offered` | 400 | no | The model does not offer a forced tool choice. |
| `hosted_tool_not_priced` | 400 | no | A hosted tool option has no price. |
| `route_unsupported` | 400 | no | The call runs only on another plan. |
| `image_too_large` | 413 | no | An image is too large. |
| `invalid_image` | 400 | no | An image is not valid. |
| `invalid_policy` | 400 | no | The model policy is not valid. |
| `invalid_agent` | 400 | no | The agent name is not valid. |
| `decisions_not_configured` | 503 | no | Typed decisions are not available on this server. |
| `decisions_upstream_failed` | 502 | yes | The decision model answered none of the requests. |
| `chat_session_not_found` | 404 | no | No such chat session. |
| `chat_session_empty` | 409 | no | The chat session has no messages to share. |
| `chat_attachment_not_found` | 404 | no | No such attachment in this chat session. |
| `continue_not_allowed` | 403 | no | The author did not allow readers to continue this artifact. |
| `stays_in_organization` | 403 | no | Work never moves between organizations on its own. |
| `artifact_not_found` | 404 | no | No artifact at this link. It was revoked, expired or never existed. |
| `artifact_revoked` | 404 | no | The artifact was revoked. |
| `artifact_access_required` | 403 | no | The artifact is shared with named people only. Ask its author for access. |
| `artifact_attachment_not_found` | 404 | no | No such image in this artifact. |
| `not_artifact_author` | 403 | no | Only the author, an owner or an admin can do this. |
| `not_external` | 400 | no | Only an artifact shared with named outside people has people to grant. |
| `audience_empty` | 400 | no | An artifact shared with named people keeps at least one. |
| `invalid_email` | 400 | no | A value is not an email address. |
| `invalid_expiry` | 400 | no | The expiry is too far in the future. |
| `expiry_required` | 400 | no | The organization requires public links to expire. |
| `expiry_too_long` | 400 | no | The expiry is longer than the organization allows. |
| `access_request_limited` | 429 | yes | Too many access requests today. Try again tomorrow. |
| `public_share_rate_limited` | 429 | yes | Too many public links in the last hour. |
| `public_links_off` | 403 | no | The organization allows sharing with named people only. |
| `domain_not_allowed` | 403 | no | The organization does not allow sharing with this domain. |
| `external_sharing_off` | 403 | no | The organization turned off sharing outside it. |
| `external_sharing_admins_only` | 403 | no | Only owners and admins can share outside the organization. |
| `external_continue_off` | 403 | no | The sharing organization does not let outside people continue its chats. |
| `external_code_not_allowed` | 403 | no | The organization does not allow code sources outside it. |
| `invalid_runner_id` | 400 | no | The `runnerId` is not 1 to 200 characters. |
| `invalid_token` | 400 | no | The `seenToken` is negative. Send the session's `fencingToken` as you read it. |
| `takeover_conflict` | 409 | no | Another runner holds the session, or took it after the token you saw. `nexus.currentFencingToken` holds the current token. Read the session, then take it over with that token. |
| `lost_runner` | 409 | no | Another runner took the session, so your fencing token is not current (`nexus.currentFencingToken` holds the current one). Stop running the session and stop writing for it. |
| `session_ended` | 409 | no | The session stopped or failed. Start a new one. |
| `session_not_found` | 404 | no | The named session does not exist or is not the caller's. |
| `auth_required` | 403 | no | The connector is not connected for this organization. |
| `invalid_query` | 400 | no | The connector query is not valid. |
| `invalid_session_fence` | 400 | no | A write inside a session must name both `x-nexus-cloud-session` and `x-nexus-fencing-token`, a positive integer. |
| `fence_required` | 409 | no | The credential's runner holds a live lease on a native session, so a write must name that session's fence (`x-nexus-cloud-session` and `x-nexus-fencing-token`). `nexus.session` names the session. Nothing was sent. |
| `write_consent_required` | 403 | no | The connection was consented for reading only. `nexus.scopes` names the write scopes the change needs. Ask for them with `nexus.consent` (POST /v1/connections/{connector}/oauth with `access: write`), then send the change again. Nothing was sent. |
| `invalid_cursor` | 400 | no | The cursor is invalid or the list changed. Start again without a cursor. |
| `invalid_search` | 400 | no | The search query is not valid. |
| `invalid_reason` | 400 | no | The reason is empty or too long. |
| `invalid_utf8` | 400 | no | The request body is not valid UTF-8. |
| `nul_in_string` | 400 | no | A string contains a NUL character. |
| `repeated_field` | 400 | no | A field appears more than once in the body. |
| `request_too_large` | 413 | no | The request body is too large. |
| `invalid_signature` | 401 | no | A signature is not valid. |
| `timeout` | 504 | no | The server did not answer in time. For a write, check the state before you retry with a new key. |
| `overloaded` | 503 | yes | The server is at its limit. Retry shortly. |
| `database_busy` | 503 | yes | The database is busy. Retry shortly. |
| `draining` | 503 | yes | The server is stopping and takes no new request. Wait for `Retry-After`, then retry. |
| `cutover_paused` | 503 | yes | Sign-in, account deletion and organization changes are paused for the auth cutover. Wait for `Retry-After`, then retry. |
| `gateway_stopping` | 503 | yes | The server stopped before the model call answered. Send the call again. |
| `vault_unavailable` | 503 | yes | The secret store is unavailable. Retry later. |
| `not_configured` | 503 | no | The feature is not configured on this server. |
| `not_ported` | 501 | no | The server does not implement this yet. |
| `upstream_error` | 502 | yes | An upstream service failed. |
| `agents_api_unreachable` | 502 | yes | The agent service did not answer. It may be off. Retry later. |
| `upstream_timeout` | 504 | yes | An upstream service did not answer in time. |
| `upstream_overloaded` | 503 | yes | The model provider is overloaded. |
| `upstream_auth` | 502 | no | The provider rejected the organization's key. |
| `upstream_billing` | 502 | no | The provider account has no credit. |
| `upstream_forbidden` | 502 | no | The provider refused this model for the key. |
| `gone` | 410 | no | The operation passed its sunset date. See DEPRECATION.md. |
| `agent_memory_version_conflict` | 409 | no | Agent memory or state: the `If-Match` value is not the row's current version. Read the row again. (legacy) |
| `agent_memory_precondition_required` | 428 | no | Agent memory or state: a write has neither `If-Match` nor `If-None-Match: *`. (legacy) |
