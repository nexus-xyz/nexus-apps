# Runner session sync

Use this guide to connect the desktop or a headless host to encrypted session storage.
The runner engine lives in `../session_sync.rs`.
The default `Host::session_sync` returns `None`, so a host that does not opt in syncs nothing.
The desktop opts in by default (`apps/desktop/src/code_run.rs`).
The managed `AllowLocalCache` policy, the sign-in, the device key and device trust still gate it.

## Adapter boundary

`gateway.rs` implements `Backend` and `ContentKey` from `contract.rs` with `nexus-client` and its crypto core (`e2e`).
`trust.rs` verifies the device directory's approval chains, and `keystore.rs` keeps keys and revocations in SQLite.
The contract's Rust types do not specify Gateway's HTTP schema.
Gateway's session store is the HTTP schema for `head`, `append` and `delete`.
`head` reads `GET /v1/threads/{threadId}`: `lastSeq` is the head, and 404 `thread_not_found` means an empty log.
`append` posts the batch to `POST /v1/threads/{threadId}/events` and returns its `lastSeq`.
`delete` calls `DELETE /v1/threads/{threadId}`, and treats 404 as already deleted.
Gateway names a key epoch `keyVersion` and starts it at 1, so send `epoch + 1`.
Gateway takes `sealed` as base64url without padding.
After a rotation, Gateway refuses a new event sealed with an older key version (409 `key_version_behind`).
The adapter returns `KEY_ROTATED` for it.
The engine confirms the head, drops the refused ciphertext, and re-seals those events with the new key.
Gateway refuses the whole upload, so no refused event was stored.
Events Gateway already stored come back unchanged.
One person's synced threads in an organization hold at most 1 GiB (413 `session_store_full`, `STORE_FULL`).
Use immutable credentials for one verified person, Gateway, and organization.
Read managed cache permission locally before any network call.
Fail closed when policy, device trust, or a required organization key cannot be verified.
The policy is `sessionContent` from `GET /v1/orgs/settings`.
An absent or unknown value is an error.

Keep private keys in the shared crypto core's secure storage.
Verify holder approval chains and pinned organization keys before publishing locked keys.
A device is trusted through approvals back to this device or a device this computer verified before.
The first refresh trusts only this device and its approver (ADR I4).
The approver counts only when the person confirmed its code on this computer (`approver.rs`).
Until then the runner locks nothing (`APPROVER_UNCONFIRMED`). A root device has no approver to confirm.
Devices above the approver come from its handover: its trusted holders and revoked ids, signed and sealed to this device.
The handover counts only when the confirmed approver signed it (`trust::verify_with`).
Without one, only approvals down from this device and its approver count.
An approver sends its own handover with each approval (`approver::handover`, `gateway/computer.rs`).
When `GET /v1/devices` says `trust: "gateway"`, Gateway vouches for the person's devices (`trust::verify_gateway`).
Then every listed device in state `trusted` is a holder, except a recovery key and an id this computer saw revoked.
No approval chain, approver code or handover is needed, and nothing is remembered as verified.
Remote control is then on until the person turns it off on this computer (`code_control_choices`).
A switch stored before that table stays as it was, off included.
An absent or other `trust` keeps the approvals rules above.
This computer pins its trust per person (`sync_trust_pins`, `KeyStore::settle_trust`).
A computer that read approvals, or remembers a verified device or a confirmed approver, keeps approvals when Gateway says `gateway`.
It follows Gateway's trust only after the person opts in on the Devices page (`KeyStore::trust_gateway`).
A return to approvals needs no opt-in, and a later Gateway trust needs a new one.
A new computer with no pin follows Gateway's trust.
Remember revocations durably and refresh the directory before preparing each batch's key.
Persist keys and locked copies before returning a `ContentKey`.
The key store holds each key only as this device's own HPKE copy.
Every trusted device and recovery key gets a copy of every epoch, so a device paired later reads the whole thread.
A recovery key (a passkey or a recovery code) has no signing key: it receives copies and approves nothing.
Bind encryption to the UUIDv7 thread identifier, key epoch, and zero-based event number, as ADR I1 specifies.

## The organization's recovery key

Under `user_and_org`, the runner also locks each epoch for the organization's recovery key (ADR I7).
`gateway/org.rs` stores those copies with `PUT /v1/threads/{threadId}/org-key`.
The runner locks only for the key the person confirmed on this computer (`org_key.rs`).
The person confirms the fingerprint the organization publishes from its vault.
The runner computes the fingerprint from the key Gateway serves, and never reads Gateway's own fingerprint.
Until the person confirms, the runner syncs for the person's own holders and locks nothing for the organization.
When Gateway serves another key, the runner locks nothing for it, and a key locked for the old one rotates.
A policy change back to `user` rotates a key the organization's key holds.
The runner prints nothing. Each shell that confirms the key shows a changed key as a warning.
On a server, `nexus-host pair --org-key-fingerprint <fingerprint>` confirms it, and `serve` logs where it stands.
The shells share their words (`org_key::copy`).

## The team key

`gateway/team.rs` makes, hands over and uses the team key (ADR 0011, M6 to M8).
While `multiplayerSessions` is on, a team-visible thread's new key is locked to the team key's current epoch too.
A key locked to another epoch rotates, and so does a team-locked key once the thread is private.
The first device that needs an epoch after a removal makes it, for every member's verified holders.
At most once a minute, a device hands every epoch it holds to members' holders that lack the current one.
A teammate's holders are verified from their root, pinned on first use; a changed root gets nothing.
The synced reader opens a teammate's thread through the team key when it has no copy of its own.

The desktop starts the worker and stops it during workbench shutdown.
Headless hosts can call `run_session_sync_until_shutdown` with the same adapter.
`nexus-host` does so when `NEXUS_HOST_SESSION_SYNC=1`, after `nexus-host pair` (`nexus-host/src/session_sync.rs`).

## The CLI's sessions

`nexus claude` and `nexus codex` sync with this engine and adapter too.
The `nexus` CLI links this crate and opens its own store in its config folder.
`cli_log.rs` makes each conversation's thread, bound at creation, and records how far the transcript was read.
The CLI reads the agent's transcript into it (`nexus-cli/src/session_log.rs`).
`NEXUS_SESSION_SYNC=1` turns it on, and `nexus devices pair` makes the computer a trusted device.

Phone control of a CLI run is off until the person runs `nexus devices control on`.
The CLI keeps that switch in its own store, as the desktop keeps its own.
Each run takes only its own thread's commands from the device inbox, through `control_loop.rs`.
The desktop's runner uses the same run-once loop for every thread.
A run leases its thread only while its agent can apply a command, and ends the lease when it exits.
`nexus codex` runs Codex's TUI as a client of a CLI-owned `codex app-server` on a private Unix socket.
The CLI's own client of that server starts, steers and interrupts turns, and answers approvals.
Codex sends each approval to both clients, and the first answer wins.
If that app-server cannot start, Codex runs as before, and the phone only reads the session.
`nexus claude` sessions stay read-only from the phone.

## Durable state

`storage.rs` keeps sync ownership, UUIDv7 identifiers, confirmed cursors, ciphertext outboxes, and deletion tombstones in SQLite.
New threads bind before their first stored event.
Linked threads and forks inherit only verified ownership.
Legacy logs remain local until an explicit ownership migration exists.

Each payload preserves raw event JSON and an encrypted snapshot of the title and repository metadata.
Streamed deltas never enter the source log or this engine.
An idle rename appears with the next stored event.
Metadata-only updates require the final viewer contract.

Local event numbers start at one; sync event numbers start at zero.
Stage ciphertext before uploading, then advance only through Gateway's confirmed contiguous prefix.
Resolve uncertain commits before preparing subsequent events under a rotated key.
Retry pending ciphertext unchanged, including after restart or revocation.
A changed content policy pauses an incompatible pending batch instead of replacing its bytes.

Deletion drains in-flight runner uploads and retains a tombstone until remote deletion succeeds.
Remote deletion must remove the locked keys as well as the thread.
Deletion does not require a successful content-policy lookup.
Deletion can wait for the current thread's bounded requests.
Integrating hosts must show progress while deletion awaits its lock.

## Limits and validation

The worker wakes every 250 milliseconds, stages at most 32 events, and caps a batch at 512 KiB.
Each plaintext payload is limited to 256 KiB.
The engine checks serialized payload size and usage metadata before publishing locked keys.
Oversized events remain pending and block only their thread.
Malformed usage stays pending rather than inventing billing observations.
Each request times out after five seconds.
Per-thread locks let unrelated deletions proceed during slow uploads.
Retries back off per thread, and up to four threads progress concurrently.
Each pass prunes retry state for threads outside the current identity's reachable targets.
Checkpoints and signed links stay outside phase one.

Run `cargo test --locked -p nexus-workbench` from the repository root.
Fault tests use an in-process transport and test-only AES-GCM.
`gateway_e2e.rs` is the two-device test against a real Gateway on Postgres.
Its header says how to run it; without `NEXUS_SYNC_E2E_GATEWAY` and `TEST_DATABASE_URL` it returns early.
