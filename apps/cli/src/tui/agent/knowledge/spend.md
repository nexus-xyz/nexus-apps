# Budgets, spend and billing

Nexus Gateway meters every model call and stops a call that the budget cannot cover.
Read this page for what was spent, what is left, who sets the caps, and who pays.

## How spend is controlled

- Every model call reserves its maximum cost before it runs. Then it settles on what it used.
- A call that does not fit is refused with `402`. So a budget is never overspent.
- Four caps can apply to one call: the organization's budget, a cap on each team on the call's path, the person's monthly spend limit, and the budget of the product that made the call.
- A product budget covers every call a product makes, such as Nexus Review's reviews, for anyone in the organization. It counts with a team or without.
- A budget is in US dollars for a UTC day, an ISO week or a calendar month. A product budget may be any of the three. A person's limit is for a UTC calendar month.
- The tightest cap stops the call.
- A reservation is generous. A call that may write much output reserves much. The budget must cover the largest call.

## Read it

```sh
nexus spend                            # the budget, what is spent, reserved and left, by product
nexus spend --by team                  # or person, model, provider, key, product, day
nexus spend calls --status refused     # each model call: settled, failed, refused or released
nexus analytics --by model --days 7    # usage over time
nexus budgets show me                  # the person's own month
```

- A member sees only what is left for them. Owners and admins see the organization.
- In the terminal session, `/spend` shows what is left and `/cost` shows this session's cost.
- Your own steps are model calls too. They count against the same budget.

## Set it

Owners and admins set the organization's budget, the team caps and the product budgets. Owners set the default limit for each person.
Owners, and the admins an owner allows, set one person's limit.
A product budget is set in Nexus UI, in Settings, then Budgets, then Products: Change opens the budget sheet with an amount and Day, Week or Month.
No `nexus` command sets a product budget yet. The API is `PUT /v1/budgets/products/{product}/{capability}` with `limitUsd` and `every` (`day`, `week` or `month`).

```sh
nexus budget set org 2000/month
nexus budget set team/web 500/month --dry-run    # show the change, change nothing
nexus budgets default 50                         # each person's monthly limit
nexus budgets set ada@acme.example 200           # one person's limit
```

## Who pays

- On the organization's own provider key, the provider bills the organization.
- On Nexus's provider accounts, a call costs the provider's price plus Nexus's rate. `nexus billing show` shows the rate.
- The free start pays for eligible models on Free. Paid plans include an AI allowance.
- Owners run `nexus billing show` to see AI dollars and cloud hours used, remaining, and when both reset.
- Teams plans share allowances across the organization. A large machine uses two cloud hours per hour.
- When the AI allowance is used up, credits pay first. Then the card pays if the owner enabled it.
- A new personal organization and a new company organization each get one free start. A company's is shared by its members. It expires after 30 days.
- `nexus billing` is for the owner: the card, credits, statements and receipts.
- `nexus keys add <provider> <key>` adds a provider key. It is for owners and admins. It takes a secret, so do not run it: see `auth`.
- On a paid plan, AI paid by card goes on the monthly Stripe invoice.
- On Free, the card is charged when unpaid usage reaches the billing threshold.

## When a call is refused

| Status and code | What to do |
| --- | --- |
| `402` `budget_exceeded` | Read `nexus spend`. An owner or admin raises the budget or the limit |
| `402` `subsidy_exhausted` | The free start is used up, or pays only for cheaper models. Pick a cheaper model, add a provider key, or set up billing |
| `402` `credit_exhausted` | An owner buys credits with `nexus billing credits buy <usd>`, or adds a provider key |
| `402` `charge_processing` | A charge for usage is still going through, such as a bank debit. The card pays for no more calls until it is paid. Credits and own provider keys still pay |
| `402` `voice_minutes_used_up` | Voice mode answers with the device's voice. An owner adds credit, or an admin raises the budget |
| `402` `usage_limit_reached` | An owner raises the monthly usage limit, or buys credits |
| `402` `payment_past_due`, `payment_failed` | An owner pays the invoice, or replaces the card with `nexus billing card add`. Calls on the organization's own provider keys keep running |
| `402` `payment_failed` for the bank's check (3-D Secure) | An owner approves that same charge on the bank's page with `nexus billing confirm` |
| `403` `provider_key_required` | The organization has no key for that provider. An owner or admin adds one |

A `402` message names the level that refused the call and when the limit resets.

## Native desktop limits

In GPUI, Budgets search and `/settings/budgets` open Usage's Limits tab.
Ordinary Usage opens Overview. Limits shows organization and person limits, the card usage limit, and read-only team caps.
The Gateway's permissions and `canManage`/`canGrant` decide who changes each value. A displayed owner role grants nothing.
The card limit requires `org.billing`. It changes the existing enabled payment mode; it does not turn payments on.
Payment settings opens native Billing. Stripe setup still uses a browser.
Writes serialize across this Limits view. Changed sign-ins discard old replies and block old controls.
Product budget sheets remain in Nexus web; Limits links there. Team cap edits remain in the CLI.
Native manual acceptance is pending for keyboard focus, scrolling, narrow layouts, account switches, and real Gateway errors.
