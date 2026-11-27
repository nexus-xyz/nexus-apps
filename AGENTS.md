<!-- BEGIN shared rules (generated; do not edit inside this block) -->

# Nexus engineering rules

These rules apply to every Nexus repository.
Agents and people follow them when they change code, docs or pull requests.
Rules keep stable ids, because code cites some of them by number.

## Content

- **R4. Flag gaps.** If the docs do not answer a question, say so. Do not extrapolate from old material.
- **R6. Small files.** Keep files small and focused. Prefer explicit code to clever code.
- **R24. Write for a human reader.** PR bodies, review comments and Markdown docs follow the ASD-STE100 writing rules:
  - one idea per sentence, and 20 words or fewer per sentence;
  - active voice, simple tenses, and one word for one meaning;
  - the imperative for instructions.

  Code names, paths and product names stay as they are.
  Put the conclusion first.
  Do not write a preamble, a recap of the question, repeated hedges or filler words ("robust", "seamless", "leverage").
  Do not use headings, tables or bold for text that fits in three sentences.
  A review comment says the problem, why it matters and the fix.
  A doc starts with what it is for and who reads it.
  Keep history in git, not in the text.
  When you edit a doc, write only the new text this way.
  Frozen archives and generated files are exempt.
  Put long output (logs, test runs) in a collapsed `<details>` block or a linked file.
  When the user has their own format or tool, use it. These limits still apply.

## Code

- **R8. Optional endpoints mount unconfigured.** An endpoint whose upstreams are not configured still mounts.
  - Missing config returns a machine-readable 503.
  - Total upstream failure returns a machine-readable 502.
  - A usable partial snapshot returns a 200 with per-entry errors.
- **R11. Cached endpoints bound their refresh.** A cached snapshot refresh runs under an explicit deadline (`tokio::time::timeout`, Go `context.WithTimeout` or the equivalent). The deadline is at least 5 s below the HTTP write timeout. Without one, a slow upstream wedges every in-flight request.
- **R12. Format Rust.** Run `cargo fmt` from the workspace root before you commit Rust changes.
- **R7. Lint Markdown.** Markdown passes `markdownlint-cli2` with the repository's `.markdownlint.jsonc`.

## Pull requests

- **R18. One concern per PR.** Split unrelated work into separate PRs. Single-purpose PRs review faster and revert cleanly.
- **R16. Close the loop on review feedback.** Answer each review comment, nits included. Say how you addressed it, link a follow-up issue, or say why you will not do it. Do not merge over unanswered comments.
- **R21. Check the author before you merge.** Run `gh pr view <number> --json author --jq '.author.login'`. Merging a PR that another person opened is unusual: get the user's confirmation first.

## CI

- **R27. Be frugal with CI.** Every job takes a runner slot that another PR waits for. Add the fewest checks that cover the risk, and prove the need first.
  - Extend an existing workflow before you add one.
  - Never check the same thing twice. If two checks cover the same files, merge them, or give each a distinct failure mode.
  - Use a `paths:` filter.
  - Never trigger the same work on both `push` and `pull_request`.
  - Set `timeout-minutes` on each job.
  - Add `concurrency` with `cancel-in-progress` for PR runs.
  - Say in the PR body which pipeline you extended, and how many runner minutes the change adds.
- **R28. Keep CI fast.** PR checks take 3 to 5 minutes at p50 and 10 at most at p90. No job waits more than 15 minutes for a runner. The runner queue is the limit, not the runtime.
  - Put short checks in one job. GitHub bills each job as at least one minute, and each job takes a queue slot.
  - Do not trigger on `labeled` or `edited` unless the check reads labels or the title.
  - Run independent steps with `parallel:`. Do not put `name:`, `id:` or `if:` on a `parallel:` group: GitHub rejects the whole file.
  - Test only what changed. A path filter covers every file the job reads.
  - Make a slow check faster; do not move it to a schedule.
  - When `main` is red, fix it or revert the cause before you merge anything else.
  - A check blocks a merge only when it is deterministic, fails only on a product defect, has an owner who fixes it the same day, and fits the time limit. Anything else runs as signal, or in quarantine.
  - A test that fails and then passes on the same commit is a defect. Fix it or quarantine it the same day, with an owner and an expiry date. Never loosen the assertion. Do not rerun it until it passes. Tests use fakes, not the network.
  - Say in the PR body how long a new or changed job takes.

## Contributions

This repository is public.
Nexus maintainers approve every change before it merges.

- Open a GitHub issue before a large change, so a maintainer can agree on the approach.
- A maintainer links each merged PR to Nexus's internal tracker. Contributors do not need access to it.
- Never commit secrets, customer data, or text copied from Nexus's private repositories.
- Do not link to Nexus's private repositories or internal tools. Readers of this repository cannot open them.

<!-- END shared rules -->

## This repository

Add this repository's own rules here, outside the generated block.
