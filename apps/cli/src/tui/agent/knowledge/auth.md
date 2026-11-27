# Sign-in, API keys and provider keys

Everything in Nexus is done as a person, in one organization, with their role there.
Read this page for signing in, for keys, and before any command that takes or prints a secret.

## Credentials

| Credential | How the person gets it | Lifetime |
| --- | --- | --- |
| CLI token | `nexus auth login` | 90 days, and it can be revoked |
| API key (`nx_…`) | `nexus auth tokens create --label <label>`, or Nexus UI under Settings, Keys | Until revoked |
| Service account key (`nxsa_…`) | `nexus service-accounts keys create` (owners and admins) | 90 days by default |
| Nexus UI session | Signing in to Nexus UI | 12 hours |

An API key belongs to the person and acts with their role.

## Sign in

```sh
nexus auth login                 # shows a one-time code and opens Nexus UI to approve it
nexus auth login --no-browser    # only prints the link, for example over SSH
nexus auth status                # the organization, the person, their role, the Gateway, and whether the token works
nexus auth logout                # revokes the token and forgets it
```

- The token is saved in `~/.nexus/factory/config.json`, which only the person can read. `NEXUS_CONFIG` moves it.
- `NEXUS_GATEWAY=<url>` runs one command against another Gateway, with its own sign-in.
- In CI, a key signs in from stdin: `echo "$NEXUS_API_KEY" | nexus auth login --with-token`.
- An older CLI than the Gateway supports gets `426` `cli_too_old`. `nexus update` fixes it.

## OAuth clients for forks

This is off by default. A Gateway turns it on with `NEXUS_OAUTH_CLIENT_TIERS=on`. Off, the routes answer 404.
In Nexus UI, open Settings › Developer apps. Enter the fork's name, keep the scopes it needs and choose Register app.
The page shows the new client ID (`nxc_…`) with a copy button. It lists each app with its scopes, limits and state.
A live app has Revoke. While the switch is off, the page says so and Settings hides its row.
The fork sends the client ID as `client_id` in `POST /v1/auth/device`, or in the query of `/app/device/authorize`.
A person registers an OAuth client for a fork of a Nexus app with `POST /v1/developer/clients`.
`GET /v1/developer/clients` lists their own clients and the scopes a fork may ask for.
`POST /v1/developer/clients/{id}/revoke` revokes one of their own clients.
Only a person may use these routes. Agent links, service accounts and module keys get 403.
A fork may hold only third-party scopes, such as `models.use` or `chat.ask`. It never holds admin, billing, key or invitation scopes.
No `nexus` command does this yet.

## Connect a computer from the phone

On Nexus Desktop, open Settings › Devices and choose Show connection code.
On the phone, open Settings › Connections › Computers › Connect a computer.
You can also open Connect a computer from Run on.
Scan the QR code or paste the connection link from the computer.
Choose Approve phone on the computer. The preview shows Approve device.
If the computer is new and your account has trusted devices, use a phone you already trust.
Choose Open computer when the phone shows that it is ready.

A trusted phone lists each computer that waits for approval under Computers › Waiting for approval.
Tap it, check that its code matches the computer's screen, and tap Approve computer.
The phone controls the session. The computer runs the agent and keeps its files.

Gateway's `NEXUS_DEVICE_TRUST` setting picks the mode. The default, `approvals`, needs the steps above.
With `gateway`, signing in on a device trusts it at once, with no code or approval.
Remote control is then on for the person's own devices. A computer turns it off with its Remote control switch.
Revoking a device in Settings › Devices ends its access at once.

## Provider keys

Gateway calls each provider with the organization's own key for it. This is "bring your own key".

```sh
nexus keys list                           # which providers have a key, masked
nexus keys add anthropic <key>            # owners and admins
nexus keys replace anthropic <key> --dry-run
nexus keys disable anthropic --dry-run
```

A key is encrypted at rest and used for that organization only.
A call that needs a provider key the organization does not have is refused with `403` `provider_key_required`. An owner or admin adds the key.

## Local providers in Nexus Desktop

With `NEXUS_BYO_PROVIDERS=1`, open Settings › Local providers.
If you are signed out, choose Continue without an account.
On first entry, choose a CLI subscription, an API key, an OpenAI-compatible server or Nexus Cloud.
An API key or server saved during setup is selected for the first local session.
Nexus Cloud opens the Nexus sign-in.
Open Settings from local Code to manage providers later.
Add an Anthropic or OpenAI key, or add an OpenAI-compatible model server.
Check contacts the saved provider and offers any model IDs the server lists.
Clear a server's saved key before changing its address.

## Secrets and you

- Never read `~/.nexus/factory/config.json`, and never print a token or a key.
- Do not run a command that takes a secret as an argument or prints one. Such commands are `nexus keys add`, `nexus keys replace`, `nexus auth tokens create`, `nexus service-accounts keys create`, and `nexus connect` with a token.
- Give the person the command instead, with a placeholder for the secret: `nexus keys add anthropic <key>`.
- Never repeat a secret that the person pasted, not even in the command that you give them.
- They run the command in another terminal. In the terminal session, `!` is not safe for it: you see what a `!` command printed.
- A secret that reached the conversation is no longer secret. Tell the person to replace it: `nexus auth tokens revoke <id>` for an API key, and the provider's own site for a provider key.
