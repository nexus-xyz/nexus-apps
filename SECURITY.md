# Security policy

## Report a vulnerability

Do not open a public issue for a security problem. Report it in private, in one of two ways:

- Use GitHub private vulnerability reporting. Open the repository's Security tab and choose "Report a vulnerability".
- Email <security@nexus.inc>.

Include what you found, the version or commit, your platform, and steps to reproduce it.
A proof of concept helps. Do not include other people's data.

## What to expect

- We acknowledge your report within 72 hours.
- We assess it and tell you whether we accept it. We keep you updated as we work on a fix.
- We fix confirmed issues, publish a release and a security advisory, and credit you if you want to be credited.
- Please give us a reasonable time to fix the issue before you disclose it. We ask for 90 days from your report.

## Safe harbour

We support good-faith security research. If you follow this policy, we will not start legal action against you,
and we will not ask others to. We consider your research authorized under the Computer Fraud and Abuse Act and
similar laws. We will not bring a claim against you under the DMCA for working around a technical measure to do it.
We will work with you to understand and fix the problem.

Good faith means that you:

- Test the code on your own computers. Use only your own accounts.
- Do not access, change or keep data that belongs to other people.
- Do not disrupt the service or other users, and do not run denial of service tests.
- Stop and report to us if you reach private data by accident.
- Do not publish details before we have had time to fix the problem.

## Scope

In scope: the code in this repository, built or installed and run on your own computers.

Out of scope: the Nexus Gateway and every other server or system that we run. Do not test them. Normal use of
the app with your own account is fine. If you find a problem in a server while you use the app, report it through
the same channels, and we will route it.

## Supported versions

We fix security issues in the latest release and on `main`. Older releases do not get fixes. Update to the latest release.

| Version | Supported |
| --- | --- |
| Latest release | Yes |
| `main` | Yes |
| Older releases | No |
