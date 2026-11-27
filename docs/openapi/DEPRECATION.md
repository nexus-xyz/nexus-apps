# Deprecation policy

This policy says how long an old API version or operation keeps working, and how clients learn about a change.
Client developers and Gateway developers read it.

## Versions

- The major version is in the path: `/v1`.
- Inside a major version, changes are additive only. We add operations, optional fields, optional parameters and new error codes.
- A breaking change ships in a new major version, such as `/v2`.
- Clients must ignore unknown response fields and handle unknown error codes by HTTP status.

## How long the old version works

- An old major version keeps working for at least 12 months after the new one ships.
- A deprecated operation or field inside a version keeps working for at least 6 months after we mark it.
- A security fix can shorten these periods. We announce it in the changelog.

## Beta operations

- Operations marked `x-internal-stability: beta` are public, but the periods above do not cover them.
- Today these are the `Billing` and `Cloud machines` operations: every path under `/v1/billing` and `/v1/cloud`.
- A beta operation can change, including a breaking change, or go away with a changelog entry and at least 30 days' notice.
- When a beta operation becomes stable, we remove the marker and announce it in the changelog. From then on, the periods above apply.

## How clients learn about it

- The contract marks the operation or field with `deprecated: true`, and names the replacement in its description.
- Each answer from a deprecated operation carries a `Deprecation` header (RFC 9745) with the date it was deprecated.
- The answer also carries a `Sunset` header (RFC 8594) with the date the operation stops working.
- A `Link` header with `rel="deprecation"` points to the changelog entry.
- The changelog lists each deprecation, its replacement and its sunset date.
- After the sunset date, the operation answers 410 `gone`.
