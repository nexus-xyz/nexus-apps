# The Nexus API

Everything in Nexus UI and the CLI goes through one API, under `/v1` on the organization's Gateway.
Read this page when you write code that calls Nexus, or code that calls models through it.

## The basics

- The base address is the Gateway's, with `/v1`: for example `https://api.nexus.inc/v1`. `status` shows this session's Gateway.
- Send a Nexus token as `Authorization: Bearer <token>`. See `auth` for how the person gets an API key.
- Bodies are JSON. Every answer has an `x-request-id`.
- `GET /v1/openapi.json` is the full description of the API on that Gateway, with every registered product.
- Do not invent an endpoint. Read the description, or the docs under `/docs/api` on the Nexus site.

```sh
curl "$NEXUS_URL/v1/tenants/me" -H "Authorization: Bearer $NEXUS_API_KEY"
```

## Call models

| Endpoint | What it is |
| --- | --- |
| `POST /v1/chat/completions` | OpenAI's Chat Completions for every model: streaming, tools and images |
| `GET /v1/models` | The models that the caller may use, as an OpenAI list |
| `POST /v1/anthropic/v1/messages` | Anthropic's Messages API, passed through |
| `POST /v1/openai/v1/responses` | OpenAI's Responses API, passed through |

- A tool that speaks OpenAI's API takes Nexus as its provider: base address `<gateway>/v1`, a Nexus key as the bearer token.
- A model name is a catalog id, such as `anthropic/claude-sonnet-5.5`, or `nexus/auto`.
- Every call follows the caller's model policy and budget, and is recorded under their name.
- The answer names the model that ran: the `model` field, and the `x-nexus-model` header.
- The pass-through routes answer errors in the provider's own format, so their SDKs understand them.

## Cloud sessions

`POST /v1/sessions` starts a cloud coding session.

- `prompt` is required. `agent` is optional when `model` is set.
- Omitting `agent` (or sending `""` or `"auto"`) picks the cloud agent from the catalog model.
- Sending both `agent` and a catalog `model` that belongs to another agent fails with `400` (`agent_model_mismatch`), naming `agent`, `model` and `modelAgent` under `error.nexus`.
- Omitting both `agent` and `model` fails with `400` (`model_required`). Unknown or non-cloud catalog models fail with `unknown_model`, `model_coming_soon`, or `unsupported_cloud_model`.

## Headers that matter

| Header | What it does |
| --- | --- |
| `x-nexus-team` | Names the team that the call is attributed to. The caller must be in that team |
| `Idempotency-Key` | Makes a retry safe: the same key answers with the first result. Every write and model call accepts it |

## Products

A product's capabilities are at `/v1/{product}/{capability}/{verb}`, such as `POST /v1/robustness/review/run`.

## Errors

Errors share one shape: `{ "error": { "message", "type", "code" } }`.
Match on `code` in code, never on the message. See `errors`.
Requests are rate-limited for each key, and a limited request gets `429`.

## Not available yet

`@nexus/kit`, the typed client that the CLI and Nexus UI use, is not published to npm.
