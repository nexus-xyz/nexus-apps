# Leak gate

Every pull request runs `.github/workflows/leak-gate.yml`, which runs `.github/scripts/leak_gate.py tree` on every
tracked file:

| Tool | Config | On a finding |
| --- | --- | --- |
| gitleaks 8.30.1 | `gitleaks.toml` (gitleaks' default rules plus the allowlists here) | fails |
| semgrep 1.186.0 | `semgrep.yml`, tested against `semgrep.rs` and `semgrep.ts` | fails |
| trufflehog 3.100.0 | none, verified findings only | fails |
| gitleaks | `blocklist.toml` (internal ticket ids, links, staff addresses) | reports only |

A finding names the tool, the rule, the file and the line. It never prints the matched value.

## Run it locally

Install the three tools, then:

```sh
python3 .github/scripts/leak_gate.py tree     # the whole tree, as CI does
python3 .github/scripts/leak_gate.py staged   # only what you are about to commit (the pre-commit hook)
```

`pre-commit install` wires the staged scan into `git commit` (`.pre-commit-config.yaml`).

## Adding an exception

Never allowlist a value. Name the file and the rule, and write in the entry's `description` what the matched text
is and why it is not a secret. A test fake is better named as one (`fake_`, `test_`, `dummy_`) than allowlisted.
A secret trufflehog confirmed live is rotated, never allowlisted.
