# Local authorization for team session keys

This file explains the runner's local trust settings for administrators who deploy Nexus on managed computers.

Team sharing is off without `sessionTeamSync` in the machine's managed configuration.
Gateway's `multiplayerSessions` setting alone grants no access.
The local policy applies to desktop, CLI and server runners.

Use the native managed file:

- macOS: `/Library/Application Support/Nexus/managed.json`.
- Windows: `%ProgramData%\Nexus\managed.json`.
- Linux: `/etc/nexus/managed.json`.
- `NEXUS_MANAGED_CONFIG` selects another local file.

Add one policy for each Gateway and organization:

```json
{
  "sessionTeamSync": [{
    "gateway": "https://gateway.example",
    "tenantId": "org_example",
    "members": [{
      "personId": "usr_example",
      "deviceId": "dev_example",
      "encryptionKey": "<base64url X25519 public key>",
      "signingKey": "<base64url Ed25519 public key>"
    }]
  }]
}
```

Verify each member root's exact public keys on that member's device before admission.
Do not copy unverified keys from Gateway's directory.
An empty `members` list admits only the runner's own confirmed device chain.
Legacy roots pinned automatically from Gateway grant no access.
Observed teammate revocations carry forward into the organization-scoped store.

The runner reads this file again for each team operation.
Missing, malformed or duplicate matching policies disable sharing.
A thread that Gateway does not list is private.
The runner rotates a content key when it knows that key had team access and sharing becomes unavailable.

Removing an admission stops new copies and refuses that root's signatures.
A person who already holds a team epoch keeps it until that epoch rotates.
Previously disclosed content keys cannot become secret again.
