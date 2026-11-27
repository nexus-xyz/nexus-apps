# Shared command lifecycle

This module runs shared terminal commands for desktop clients and their agents.
The command journal records metadata before the process starts.
Both clients use the same execution ID and PTY.

## States

```text
awaitingUser -> starting -> running -> completed | failed | interrupted
     |                         |
     +-> declined | cancelled  +-> unknown after an unrecovered restart
     +-> denied in Plan mode
```

Auto and Full access start a recorded command without another prompt.
Other modes require the person's Run action.
Plan records a denial and starts no process.
Approval never changes the executor's sandbox capabilities.

A stop request revokes input immediately.
The process stays running until its exit is confirmed.
A failed spawn keeps a receipt with no exit code.

## Recovery and privacy

SQLite keeps command metadata, transitions, and request fingerprints in the existing workbench store.
Memory-only workbenches keep this journal in memory too.
Raw PTY output and private input are never journaled.
Private commands store a placeholder instead of launch details.
Their persistent fingerprint excludes command text, so it cannot confirm guesses of that text.
A private retry returns the original receipt when its directory and handoff requirements match.
The journal follows the conversation store's access and retention rules.
Do not put secrets in command arguments; use private input.

A restart marks unconfirmed launches Unknown and cancels pending approvals.
It never restores process handles or raw output buffers.
Repeating a request ID returns its receipt and never restarts the command.
A changed directory or handoff requirement conflicts with that receipt.
Changed public command text conflicts too.
Submit a new request only when a rerun is intentional.
Deleting a session removes its command history and transition events.

The journal survives process crashes.
It does not claim exactly-once side effects across power loss or a missing host.

## Routing and bypasses

Every desktop agent receives instructions to use shared terminal tools for all commands.
Full access changes approval, not routing.
When shared tools attach, Claude excludes Bash and Codex disables its native command tools.
The Nexus agent removes native command tools when it discovers the shared terminal catalog.
Its dispatcher also refuses native calls from older conversation history.

If tool injection fails before launch, the provider retains its existing permissions and executor.
Observed provider commands enter history without being replayed.
A host-generated conversation notice gives the bypass reason, or states that the cause is unknown.
Provider results keep a nullable exit code when the protocol reports no process exit.
Their original activity retains output and process controls.
An unavailable shared executor never grants additional native permissions.
A failed or uncertain launch is never replayed automatically.

Shared agent input to persistent shells also enters the journal before delivery.
These records are input requests, not confirmed command boundaries or individual process exits.
GPUI shows the latest 64 requests in the expanded command card.
Human input, private input, and raw terminal output stay out of this journal.
Recovered and provider-owned records cannot accept inline terminal input.

These controls cover known native command tools.
They do not intercept arbitrary subprocesses started inside extensions or other execution-capable tools.
Auto continues to ask until a trusted command reviewer supplies decisions.
A shared-tool failure after startup can require repairing or restarting the session.
