# Errors and their fixes

Read this page when a `nexus` command, an API call or your own model call fails.
The CLI prints an error as one line and a hint. It exits `1` for a failure and `2` for a usage error.
With `--json`, the error is `{"error": {"message", "hint"}}` on stderr. `NEXUS_DEBUG=1` adds the stack trace.

## By status

| Status | Meaning |
| --- | --- |
| `400` | A field is missing or not valid. The message names it |
| `401` | No token, or one that Gateway does not accept. The person signs in again: `nexus auth login` |
| `402` | Money: the budget, a limit, the credits, or a payment. See the code |
| `403` | The role or the key may not do this |
| `404` | Nothing matches in this organization |
| `409` | The request conflicts with the current state. See the code |
| `426` | The CLI is older than this Gateway supports. Run `nexus update` |
| `429` | Too many requests from this key in a minute. Wait and retry |
| `500` | Something failed on Gateway. The cause is in its logs, not in the answer |
| `502`, `503`, `504` | A provider, a product or Gateway itself did not answer in time. Retry later |

## By code

| Code | What to do |
| --- | --- |
| `role_required` | Ask an owner or admin. The message says which roles may |
| `not_in_team` | Name a team that the person belongs to: `nexus team list` |
| `unknown_team` | Check the slug: `nexus team list` |
| `model_not_allowed` | Use a model that the policies allow: `nexus models`. `nexus policy test` names the rule |
| `model_not_found` | Check the name: `nexus models --all` |
| `model_not_supported` | The model is coming soon, or not served on that API |
| `operator_paused` | A control pauses the caller's model calls: `nexus policy controls` |
| `budget_exceeded` | Read `nexus spend`. An owner or admin raises the budget or the limit |
| `charge_processing` | A charge for usage is still going through. The card pays again once it is paid. Meanwhile an owner buys credits |
| `subsidy_exhausted`, `credit_exhausted`, `usage_limit_reached` | See `spend`: who pays, and what the owner does |
| `payment_past_due`, `payment_failed` | An owner pays the invoice or replaces the card. Calls on own provider keys keep running |
| `provider_key_required` | An owner or admin adds the provider's key. See `auth` |
| `subscriptions_disabled` | The organization turned off agents on personal plans. Run the agent through Gateway: `nexus claude --api` |
| `cli_too_old` | Run `nexus update` |
| `wiki_off` | Nexus Wiki and Ask your code were removed. Do not offer them |
| `page_content_blocked` | The page holds something that Gateway refuses. The message says which. See `pages` |
| `page_version_conflict` | Someone wrote a newer version. Read it, then update again |
| `page_too_large` | A version holds at most 16 MiB |
| `nexus_ui_only` | The person does this in Nexus UI |
| `idempotency_in_progress` | The first request with that key still runs. Retry in a moment |
| `database_busy` | Nothing was written. Retry |

## Your own turn

| What the person sees | Why | What helps |
| --- | --- | --- |
| The agent stopped after 40 steps | A turn has at most 40 steps | They send a prompt, and you go on |
| The model reached its output limit | A step's output was cut, and none of its tools ran | Split the work into smaller edits |
| The model returned invalid arguments | A tool call was not valid JSON, and none of its tools ran | Try again with a smaller edit |
| The model ended without an answer | The model sent nothing | They send the prompt again |
| A `402` or `403` on your call | The budget or the model policy refused your step | See the codes above |

## When nothing here fits

Give the person the command, its exit code, and the exact message. Do not guess a cause.
{{triage}}
