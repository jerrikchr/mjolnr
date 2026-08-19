# Provider contract

Phase 0 documentation discovery. Captured 2026-07-15 and refreshed against the live OpenAI boundary on 2026-07-16.

**Status legend used throughout:**

- **[confirmed]** — read directly from the official documentation cited in that section.
- **[inferred]** — consistent with official docs but not stated verbatim. **Must be verified against a captured fixture in its implementation phase before being relied upon.** An inferred fact is a hypothesis, not a contract.

Provider APIs drift. Re-run this discovery before any phase that touches an adapter, and treat a mismatch between this document and a live response as a bug in this document.

---

## 0. Cross-provider traps

The differences that will cause real defects, collected before the per-provider detail.

| Trap | Detail | Affects |
|---|---|---|
| **Tool arguments stream as text fragments** | Anthropic and OpenAI stream partial JSON strings. Parsing before the provider's completion boundary will parse invalid JSON. | OpenAI, Anthropic |
| **…but not everywhere** | Ollama returns `tool_calls` as complete objects, not fragments. A shared "accumulate fragments" assumption is wrong for it. | Ollama |
| **Fragments are keyed, not contiguous** | Interleave by `index` (Anthropic) / item id (OpenAI). Never assume the next delta belongs to the block you last saw. | OpenAI, Anthropic |
| **HTTP 200 does not mean success** | OpenRouter reports mid-stream errors with `finish_reason: "error"` while the status stays 200. OpenAI does the same via `response.failed` — **including `rate_limit_exceeded`**, so a rate limit arrives by two different paths (§1). | OpenRouter, OpenAI |
| **Usage details are breakdowns, not additions** | OpenAI's `cached_tokens` is a subset of `input_tokens`; `reasoning_tokens` a subset of `output_tokens`. Gemini's `promptTokenCount` already includes cached. Adding them double-counts. | OpenAI, Gemini |
| **SSE comment frames are legal** | OpenRouter emits `: OPENROUTER PROCESSING` keep-alives. Lines beginning `:` must be skipped before JSON parsing. | OpenRouter |
| **Usage is cumulative, not incremental** | Anthropic's `message_delta.usage` counts are cumulative. Summing them inflates the total. | Anthropic |
| **Two different id fields** | OpenAI `function_call` carries both `id` (`fc_…`) and `call_id` (`call_…`). The result must reference **`call_id`**. | OpenAI |
| **Secrets in URLs** | Gemini documents the API key as a query parameter. URLs are logged by tracing/proxies. See §3.2. | Gemini |
| **Streaming defaults on** | Ollama's `stream` defaults to `true`, unlike every other provider here. | Ollama |
| **Unknown events are expected** | Anthropic explicitly states new event types may be added and clients must handle them gracefully. | All |

That last row is the official justification for `UnknownUpstreamEvent`: retaining unknown events diagnostically is what the provider asks of us, not defensive paranoia.

---

## 1. OpenAI — Responses API

Sources: [streaming guide](https://developers.openai.com/api/docs/guides/streaming-responses), [function calling guide](https://developers.openai.com/api/docs/guides/function-calling), and — **authoritatively** — the [official OpenAPI 3.1 specification](https://github.com/openai/openai-openapi) (`openapi.yaml`, MIT).

> **The machine-readable spec is the best source available for this provider** and resolved every gap the prose guides left open. It is a 2.7MB OpenAPI 3.1 document. Read it directly (`curl` + grep) rather than fetching the rendered reference page, which exceeds fetch size limits.
>
> **It is a reference, not a dependency.** We do not vendor it, generate from it, or add it to the build. AGENTS.md §8 permits reading official published specifications; that is exactly what this is.

- **Endpoint [confirmed]:** `POST https://api.openai.com/v1/responses` (`servers[0].url` = `https://api.openai.com/v1`, path `/responses`, `operationId: createResponse`)
- **Auth [confirmed]:** `securitySchemes.ApiKeyAuth` = `type: http, scheme: bearer` → `Authorization: Bearer <key>`
- **Transport:** SSE. Enabled with `stream: true`. **[confirmed]**
- **Termination [confirmed]:** `event: done` / `data: [DONE]` — same sentinel shape as OpenRouter, so the transport layer can recognise it once.
- **Framing:** semantic events, each typed with a predefined schema. **[confirmed]**

### Related endpoints [confirmed]

`/responses/{response_id}` (retrieve), `/responses/{response_id}/cancel`, `/responses/{response_id}/input_items`, `/responses/input_tokens`.

> **`/cancel` does not apply to smed's MVP.** The spec states: *"Only responses created with the `background` parameter set to `true` can be cancelled."* smed streams synchronously, so cancellation remains a client-side stream drop (§6.7). Recorded because the endpoint's existence invites the wrong conclusion.

### Tool-call lifecycle **[confirmed]**

Define:

```json
{ "type": "function", "name": "…", "description": "…",
  "parameters": { "type": "object", "properties": {}, "required": [], "additionalProperties": false } }
```

smed sends function tools with `strict: true`. OpenAI strict mode requires
`additionalProperties: false` on every object and requires every key in
`properties` to appear in `required`. A semantically optional argument is
therefore required on the wire with `null` included in its allowed type; the
tool resolves that explicit null to its local default. JSON Schema dialect
markers such as the internal `$schema` declaration are removed at the adapter
boundary rather than sent as function parameters.

Model emits into `output`:

```json
{ "type": "function_call", "id": "fc_…", "call_id": "call_…", "name": "…", "arguments": "{\"k\":\"v\"}" }
```

Caller returns:

```json
{ "type": "function_call_output", "call_id": "call_…", "output": "…" }
```

> **`arguments` is a JSON-encoded *string*, not an object.** Parse it; never forward it as a value.
>
> **Correlate on `call_id`, never `id`.** Both exist on the same object. Using `id` will appear to work until it doesn't.

Argument delta and done events identify the output item through `item_id`
(`fc_…`), not the callable handle. The adapter must retain the
`item_id → call_id` relationship from `response.output_item.added`, key every
canonical fragment and completed call by `call_id`, and take the function name
from `response.function_call_arguments.done`.

### Streaming events — all [confirmed] against the OpenAPI spec

Verified as literal wire strings in `openapi.yaml`:

| Event | Meaning |
|---|---|
| `response.created` | stream opened |
| `response.in_progress` | generation underway |
| `response.output_item.added` | a new output item (e.g. a function call) begins |
| `response.output_text.delta` | assistant text fragment |
| `response.output_text.done` | text item complete |
| `response.function_call_arguments.delta` | argument fragment |
| `response.function_call_arguments.done` | **the parse boundary for tool arguments** |
| `response.output_item.done` | output item complete |
| `response.completed` | terminal, success |
| `response.failed` | terminal, failure |
| `response.incomplete` | terminal, truncated |

> **Phase 0 recorded these as [inferred] from the dotted pattern; the spec confirms every one.** The guess was right — which is exactly why the labelling mattered. Had one been wrong, the label is what would have caught it rather than a confusing runtime failure.

**`response.incomplete` was not predicted.** A third terminal state beyond success/failure — a response that stopped early (e.g. token limit). smed must treat it as its own outcome: it is neither an error nor a complete answer, and reporting it as either would violate AGENTS.md §1.3 (never lie about state).

The streaming guide lists SDK-level class names (`ResponseCreatedEvent`, `ResponseOutputTextDelta`, `ResponseRefusalDelta`, …). **Those are SDK type names, not wire strings** — do not map against them.

Refusals stream as a distinct channel (`ResponseRefusalDelta`/`ResponseRefusalDone` as SDK names). smed must not render a refusal as assistant text; map it to a typed outcome.

### Terminal events carry a `Response` object — read the status, not the event name

`response.completed`, `response.failed`, and `response.incomplete` each carry a full `Response`. The interesting fields **[confirmed]**:

| Field | Shape | Notes |
|---|---|---|
| `status` | `completed \| failed \| in_progress \| cancelled \| queued \| incomplete` | The authority on outcome |
| `error` | `{ code: ResponseErrorCode, message } \| null` | Present on failure |
| `incomplete_details` | `{ reason: max_output_tokens \| content_filter } \| null` | **Why** it stopped early |
| `usage` | `ResponseUsage \| null` | Null on failure |

Every streaming event also carries a `sequence_number` (integer). Useful for detecting a dropped or reordered frame; the transport does not otherwise guarantee it.

### ⚠️ A rate limit can arrive mid-stream with HTTP 200

`ResponseErrorCode` includes **`rate_limit_exceeded`**, alongside `server_error`, `invalid_prompt`, and a long tail of image-specific codes. **[confirmed]**

So a rate limit reaches smed by *two different paths*:

1. **Pre-stream:** HTTP 429 with an `ErrorResponse` body.
2. **Mid-stream:** HTTP **200**, then `response.failed` with `error.code = "rate_limit_exceeded"`.

This is the same trap OpenRouter sets (§4), now confirmed for OpenAI too. It generalises to a rule rather than a per-provider quirk:

> **Never infer success from HTTP status on a streaming request. The body is the authority.**

Both paths must map to `PROVIDER_RATE_LIMIT`. Mapping only the 429 would let a mid-stream rate limit surface as a generic protocol error, and the user would retry into the same wall.

### Error envelopes — documented and observed shapes

- **Non-200 responses:** `ErrorResponse` = `{ "error": { type, message, param, code } }`. All four fields required; `param` and `code` are nullable.
- **Documented mid-stream SSE frame:** `event: error` with top-level `{ "type": "error", "code", "message", "param", "sequence_number" }`. The actionable classification is `code`; `type` is always the generic string `error`.
- **Observed during live request setup on 2026-07-16:** the SSE data used `{ "type": "error", "error": { type, message, code } }`. smed accepts both shapes and retains only the stable code, never the provider's raw prose.

Do not report the SSE `type` as the failure reason. Doing so collapses every
error to the useless word `error` and hides codes such as
`invalid_function_parameters` or `insufficient_quota`.

### `ResponseUsage` **[confirmed]**

```text
input_tokens            (required)
input_tokens_details    (required) → { cached_tokens }        (required)
output_tokens           (required)
output_tokens_details   (required) → { reasoning_tokens }     (required)
total_tokens            (required)
```

> **The `_details` are breakdowns, not additions.** `cached_tokens` is a subset of `input_tokens`; `reasoning_tokens` is a subset of `output_tokens`. Adding them to their parent double-counts — the same shape of error Gemini invites with `promptTokenCount` (§3).

smed's canonical [`Usage`] carries `input_tokens` and `output_tokens` only. The breakdowns are real information the MVP has no surface for; dropping them is deliberate, not an oversight.

### Still open

- Whether `max_output_tokens` is required in a request (the spec marks it nullable; a live smoke test settles it).
- Reasoning-model specifics — out of MVP scope.

---

## 1.5 OpenAI Codex — ChatGPT subscription backend

Sources: the official Codex [device-code implementation](https://github.com/openai/codex/blob/main/codex-rs/login/src/device_code_auth.rs), [login server implementation](https://github.com/openai/codex/blob/main/codex-rs/login/src/server.rs), [Responses endpoint implementation](https://github.com/openai/codex/blob/main/codex-rs/codex-api/src/endpoint/responses.rs), and [app-server authentication contract](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md). These are official OpenAI source, read as contract evidence; no code was copied or translated.

This provider is intentionally named `openai-codex`, not an `openai` mode. It has a different credential lifecycle, endpoint, model catalogue, and quota vocabulary. Only the Responses SSE decoder is shared.

### Support boundary

This route spends ChatGPT Plus/Pro subscription quota rather than API credits. It is **not a published OpenAI API contract**. It is implemented by the official Codex CLI and currently tolerated for this use, but can change or disappear without notice. Anthropic closed its analogous third-party subscription route. smed therefore keeps API keys as the canonical supported OpenAI path and identifies itself honestly as `smed` on requests.

smed never reads or live-shares `~/.codex/auth.json`. Refresh tokens rotate and are single-use; two owners racing the same generation can strand both. A login creates a smed-owned token chain in smed's own owner-only credential file.

### Device-code login

The official sequence is **[confirmed from official source and offline contract-tested]**:

1. `POST https://auth.openai.com/api/accounts/deviceauth/usercode` with the public Codex client id `app_EMoamEEZ73f0CkXaXp7hrann`.
2. Show `https://auth.openai.com/codex/device` and the returned user code.
3. Poll `POST /api/accounts/deviceauth/token`; HTTP 403/404 means authorization is still pending, with a 15-minute ceiling.
4. Exchange the returned authorization code and PKCE verifier at `POST https://auth.openai.com/oauth/token`, using redirect URI `https://auth.openai.com/deviceauth/callback`.
5. Persist access token, rotating refresh token, expiry, and the ChatGPT account id derived from the access-token JWT claims in smed's owner-only credential file.

Refresh uses `grant_type=refresh_token` and the same public client id. Refresh is single-flight inside one smed process. After an exchange succeeds, the replacement chain is persisted before the new access token can be used for `/responses`. If persistence fails, smed sends no model request and requires a fresh login rather than risking use of an unowned token generation.

### Provider request

- Endpoint: `POST https://chatgpt.com/backend-api/codex/responses`. **[confirmed from official source and a live smed request on 2026-07-16]**
- Auth: `Authorization: Bearer <OAuth access token>`. **[confirmed from official source and live]**
- Account routing: `chatgpt-account-id` from the access-token JWT claim. **[confirmed from official source and live]**
- Client identity: `originator: smed` and `User-Agent: smed/<version>`. This is smed's honesty policy, not an upstream requirement. **[confirmed accepted live]**
- Body: non-empty `instructions`, canonical `input`, strict function `tools`, `tool_choice: "auto"`, `parallel_tool_calls: true`, `store: false`, and `stream: true`. **[confirmed accepted live with `gpt-5.4` on 2026-07-16]**
- Stream: the Responses SSE dialect in §1. **[confirmed from official source and live]**

smed fetches the authenticated account catalogue from
`GET /backend-api/codex/models`, sends the same bearer token and
`chatgpt-account-id` boundary as generation, and exposes only entries with
`supported_in_api: true` and `visibility: "list"`. Display name, context
window, input modalities, and reasoning availability come from that response.
Static entries remain adapter defaults only; they are not the `/model`
projection. A successful re-auth triggers a fresh runtime discovery generation,
and stale results from an older generation are discarded.

The subscription endpoint uses non-strict function definitions. This is
deliberate: smed's `spawn_subagent.result_schema` is itself a user-declared
JSON Schema, so its possible properties cannot be closed when the outer tool
definition is published. Sending that tool with `strict: true` is rejected by
the live Codex backend as `invalid_function_parameters`. smed removes
`$schema` dialect markers but preserves optionality and the dynamic nested
schema. This does not change the API-key `openai` adapter, whose built-in tool
contract remains strict.

Live verification on 2026-07-27 discovered and completed a text turn with
`gpt-5.6-sol`. The account catalogue also exposed `gpt-5.6-terra`,
`gpt-5.6-luna`, `gpt-5.5`, `gpt-5.4`, and `gpt-5.4-mini`; hidden
`codex-auto-review` was correctly excluded. Account grants remain the source of
truth and can change independently of smed.

### Subscription quota

ChatGPT usage is governed by rolling plan windows, distinct from API request rate limits. `usage_limit_reached`, `plan_quota_exceeded`, `rate_limit_reached`, and the official workspace credit/usage-limit variants map to stable reason code `PROVIDER_PLAN_QUOTA`, carrying an absolute reset Unix time when the response supplies one. A normal API-style `rate_limit_exceeded` remains `PROVIDER_RATE_LIMIT`. The distinction is persisted and rendered so future automation can classify the failure without parsing prose.

---

## 2. Anthropic — Messages API

Sources: [create a message](https://platform.claude.com/docs/en/api/messages/create), [authentication](https://platform.claude.com/docs/en/manage-claude/authentication), [API versioning](https://platform.claude.com/docs/en/api/versioning), [streaming messages](https://platform.claude.com/docs/en/build-with-claude/streaming), [handle tool calls](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls), and [errors](https://platform.claude.com/docs/en/api/errors).

- **Endpoint:** `POST https://api.anthropic.com/v1/messages`. **[confirmed]**
- **Authentication:** `x-api-key: <credential>`. **[confirmed]**
- **Version:** `anthropic-version: 2023-06-01` is mandatory. **[confirmed]**
- **Request:** `model`, `messages`, and `max_tokens` are required for smed's streamed call; system instructions use the top-level `system` field. **[confirmed]**

- **Transport:** SSE. Enabled with `"stream": true`. **[confirmed]**
- **Framing:** each event has both an SSE event name (`event: message_stop`) *and* a matching `type` in its JSON data. **[confirmed]** Prefer the `data.type` field as the source of truth; the SSE event name duplicates it.

### Event flow **[confirmed]**

```text
message_start                       Message object with empty content
  ├─ content_block_start            per block, carries index
  ├─ content_block_delta *          zero or more, carries index
  └─ content_block_stop             per block
message_delta *                     top-level changes; carries usage
message_stop                        terminal
```

Plus, at any point: `ping` events (any number), and `error` events.

```sse
event: error
data: {"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}
```

`overloaded_error` corresponds to what would be HTTP 529 in a non-streaming context. **[confirmed]**

> **Official instruction:** *"new event types may be added, and your code should handle unknown event types gracefully."* Unknown events are retained, never fatal.

That forward-compatibility rule applies to an unknown top-level `type`, not to corrupt data for a type smed already understands. The adapter first reads the event envelope: an unknown type becomes `UnknownUpstream`, while a known type whose required payload cannot decode fails immediately with `PROVIDER_PROTOCOL`. Treating both as unknown would hide the corrupt frame and produce a misleading error later.

### Delta types **[confirmed]**

- `text_delta` — text content
- `input_json_delta` — tool arguments
- `thinking_delta`, `signature_delta` — extended thinking

### Tool-call assembly **[confirmed]**

`tool_use` block deltas carry `partial_json` — **partial JSON strings**, while the final `tool_use.input` is always an object:

```sse
event: content_block_delta
data: {"type":"content_block_delta","index":1,
       "delta":{"type":"input_json_delta","partial_json":"{\"location\": \"San Fra"}}}
```

**Accumulate `partial_json` per `index`; parse only on `content_block_stop`.** Current models emit one complete key/value at a time, so there may be *delays between events* mid-tool-call — a quiet stream is not a hung stream. The chunking is deliberately finer-grained than needed so future models can stream more granularly; do not build in an assumption that a delta is a whole key.

The assistant emits `{ "type": "tool_use", "id", "name", "input" }`. The next user-role message returns `{ "type": "tool_result", "tool_use_id": <same id>, "content", "is_error" }`; result blocks precede any user text. **[confirmed]** Anthropic has no tool role on the wire.

### Usage **[confirmed]**

Carried on `message_delta`. **Token counts are cumulative.** Take the latest value; do not sum.

### Fallback blocks **[confirmed]**

During server-side fallback, a `fallback` content block arrives as a `content_block_start`/`content_block_stop` pair **with no deltas between**. A parser assuming every block has deltas will mishandle it.

### Phase 6 implementation decisions

- smed offers pinned current IDs `claude-sonnet-5`, `claude-opus-4-8`, and `claude-haiku-4-5-20251001`, with the published 1M/1M/200k context limits.
- Thinking and signature blocks are intentionally not canonicalized. They therefore cannot be replayed as if private reasoning state migrated to another provider.
- HTTP 401/403 map to authentication, 429 and 529/`overloaded_error` map to rate limiting, and no error path automatically retries.
- The adapter declares image input unsupported even though the models support it, because smed does not yet translate canonical image references onto this wire. Capability metadata describes the adapter path that exists, not provider marketing.

---

## 3. Gemini — generateContent

Source: [generateContent reference](https://ai.google.dev/api/generate-content)

- **Endpoints [confirmed]:**
  - `POST https://generativelanguage.googleapis.com/v1beta/{model=models/*}:generateContent`
  - `POST https://generativelanguage.googleapis.com/v1beta/{model=models/*}:streamGenerateContent`
- **Streaming [confirmed]:** add `?alt=sse` for SSE framing. Without it, the response is a stream of `GenerateContentResponse` instances in a different framing — **always send `alt=sse`** so one SSE decoder serves all SSE providers.
- **Model is part of the path**, not the body — unlike every other provider here.

### 3.2 Authentication — a smed decision, not a doc quote

The reference documents the key as a **query parameter**: `?key=$GEMINI_API_KEY`. **[confirmed]**

> **smed must not put a credential in a URL.** Query strings land in `tracing` spans, `reqwest` debug output, proxy logs, and crash reports. That directly violates AGENTS.md §3 ("secrets never leave their boundary") and defeats the redaction rules, because the secret would be inside a field we otherwise treat as safe to log.
>
> **Decision:** use the `x-goog-api-key` request header instead. **[inferred — not stated in the page consulted]**
>
> **Phase 7 must verify the header is accepted before the adapter ships.** If it is not, escalate rather than silently falling back to the query parameter: a URL-embedded secret needs an explicit owner decision and a redacting URL formatter, not a shrug.

### Response shape **[confirmed]**

```text
GenerateContentResponse
├─ candidates[]
│   ├─ content.parts[]        text | functionCall{name, args}
│   ├─ finishReason
│   └─ safetyRatings[]
├─ promptFeedback
├─ usageMetadata
└─ modelVersion, responseId, modelStatus
```

- `functionCall.args` is a **structured object**, not an encoded string — unlike OpenAI. **[confirmed]**
- Streaming yields successive `GenerateContentResponse` instances; deltas arrive as partial `parts`. Reassembly rules are **[inferred]** — verify in Phase 7.

### Usage **[confirmed]**

`usageMetadata`: `promptTokenCount` (includes cached), `cachedContentTokenCount`, `candidatesTokenCount`, `totalTokenCount`, `toolUsePromptTokenCount`, `thoughtsTokenCount`, plus per-modality breakdowns.

Note `promptTokenCount` *includes* cached content — smed's usage display must not double-count it against `cachedContentTokenCount`.

`promptFeedback` and `safetyRatings` can terminate a response for policy reasons. That is not an error and must not map to `PROVIDER_PROTOCOL`; it needs an honest typed outcome.

---

## 4. OpenRouter — chat completions

Source: [streaming](https://openrouter.ai/docs/api/reference/streaming)

- **Endpoint [confirmed]:** `POST https://openrouter.ai/api/v1/chat/completions`
- **Headers [confirmed]:** `Authorization: Bearer <OPENROUTER_API_KEY>`, `Content-Type: application/json`
- **Streaming [confirmed]:** `"stream": true`
- **Termination [confirmed]:** `data: [DONE]`
- **Usage [confirmed]:** final chunk carries `usage`

### Comment frames **[confirmed]**

OpenRouter sends comment lines such as `: OPENROUTER PROCESSING` to prevent connection timeouts. Per the SSE spec these are safely ignorable, but **lines beginning with `:` must be skipped before JSON parsing or the parser will crash.**

Using a spec-compliant SSE decoder (`eventsource-stream`) handles this; a hand-rolled `split("data: ")` does not. This is a concrete argument for the dependency.

### Errors **[confirmed]**

- **Pre-stream:** standard JSON error body with a real HTTP status.
- **Mid-stream:** delivered as an SSE event with a top-level `error` field and `finish_reason: "error"` in `choices`, **while the HTTP status remains 200 OK**.

> smed must not infer success from HTTP status on any streaming provider. The stream body is the authority.

Plan anti-pattern (§Phase 7): *"do not label OpenRouter as merely an OpenAI alias."* Its routing, comment frames, error placement, and per-model capability variance are its own contract.

---

## 5. Ollama — local chat

Source: [chat endpoint](https://docs.ollama.com/api/chat)

- **Endpoint [confirmed]:** `POST /api/chat`, default host `http://localhost:11434`
- **Authentication [confirmed]:** none required (empty security array). No credential path at all — the `SecretStore` is not involved.
- **Framing [confirmed]:** `application/x-ndjson` — one JSON object per line. **Not SSE.** Plan anti-pattern: *"do not parse Ollama as SSE."*
- **`stream` defaults to `true` [confirmed]** — the only provider here that streams unless told otherwise.

### Request **[confirmed]**

Required `model`, `messages[]` (role + content). Optional: `stream`, `tools[]`, `format`, `think`, `options`, `keep_alive`, `logprobs`, `top_logprobs`.

### Chunk shape **[confirmed]**

```json
{ "model": "…", "created_at": "…", "done": false,
  "message": { "role": "…", "content": "…", "thinking": "…", "tool_calls": [], "images": [] } }
```

### Tool calls **[confirmed]**

`message.tool_calls[]` carries function name and an **arguments object**. **Complete objects, not fragments** — no accumulation, no parse boundary. The canonical mapping must not route Ollama through the fragment-assembly path.

### Final chunk **[confirmed]**

`done: true`, `done_reason`, `total_duration`, `load_duration`, `prompt_eval_count` (input tokens), `prompt_eval_duration`, `eval_count` (output tokens), `eval_duration`, `logprobs`.

Durations are **nanoseconds**. Token counts map to smed's usage from `prompt_eval_count`/`eval_count`.

### Not documented

Error response format is unspecified in the page consulted. Phase 7 must capture real failures (endpoint down, model not pulled, bad model name) and map them honestly — an unavailable local endpoint is the expected case for a user who never installed Ollama and must diagnose clearly, not crash.

---

## 5.1 LM Studio — local OpenAI compatibility plus native catalogue

Sources: LM Studio's official [REST API overview](https://lmstudio.ai/docs/developer/rest),
[authentication guide](https://lmstudio.ai/docs/developer/core/authentication), and
[model listing reference](https://lmstudio.ai/docs/developer/rest/models).

- **Server [confirmed]:** local default `http://localhost:1234`; OpenAI-compatible
  generation uses `POST /v1/chat/completions`. smed accepts a host, IP, or full
  URL during `smed auth login lm-studio`, normalizes it to the `/v1` root, and
  stores it in `.smed/providers/lm-studio.url`. The
  `SMED_LM_STUDIO_BASE_URL` environment variable takes precedence.
- **Authentication [confirmed]:** disabled by default and therefore optional in
  smed. When enabled, LM Studio accepts bearer API tokens. smed resolves
  `LM_API_TOKEN` or its owner-only `lm-studio` credential file and sends the
  header only when a non-blank token exists.
- **Catalogue [confirmed]:** smed uses native `GET /api/v1/models`, not the
  sparse OpenAI-compatible list. It accepts only `type: "llm"` entries whose
  `capabilities.trained_for_tool_use` is true, then maps `key`, `display_name`,
  `max_context_length`, `capabilities.vision`, and `capabilities.reasoning`.
- **Loaded context caveat [observed]:** `max_context_length` is model metadata,
  not the context chosen for a loaded instance. A model loaded at 8,192 tokens
  can therefore appear capable but refuse smed's larger governed prompt. The
  server's refusal is a typed protocol failure; smed does not guess or truncate
  safety instructions to make it fit.
- **Stream error envelopes [observed]:** LM Studio can emit an SSE data object
  containing root `error` and `message` fields with no `choices`. The decoder
  permits an absent choices array so it can parse and surface that error instead
  of misclassifying it as malformed JSON.

---

## 5.5 Image input — verified 2026-07-25 

Re-verified against current official documentation immediately before the
encoders were written, per the Phase 0 contract-lock rule. Image payload shapes
have moved more than once, so an inferred shape here is a request that fails
after the tokens are spent.

### OpenAI — Responses API **[confirmed]**

A content **part**, which means `InputItem::Message.content` can no longer be a
bare `String` when an image is present:

```json
{ "type": "input_image", "image_url": "data:image/png;base64,…", "detail": "auto" }
```

`image_url` accepts a data URI directly. JPEG, PNG, WebP, and non-animated GIF.
Documented ceilings are far above smed's own: 512 MB per request, 1500 images.

### Anthropic — Messages API **[confirmed]**

```json
{ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "…" } }
```

`image/jpeg`, `image/png`, `image/gif`, `image/webp`; animations are ignored past
the first frame. **10 MB per image** base64-encoded on the Claude API direct
path, 5 MB via Bedrock/Vertex. Max 8000×8000 px. 100 images per request on
200k-context models, 600 otherwise, under a 32 MB request ceiling.

Two facts worth encoding as behaviour rather than as comments: images placed
*before* text perform better, and above 20 images per request a stricter
per-image dimension limit applies. smed's own per-request cap sits well below
that threshold, so the stricter regime is unreachable by construction.

### Gemini — `generateContent` **[confirmed]**

```json
{ "inlineData": { "mimeType": "image/png", "data": "…" } }
```

camelCase, consistent with the adapter's existing `functionCall` and
`thoughtSignature` spellings — smed targets the camelCase JSON surface, not the
snake_case proto spelling, and mixing them in one `parts` array would be a
request that half-decodes. `image/png`, `image/jpeg`, `image/webp`, `image/heic`,
`image/heif`. **Inline data caps the whole request at 20 MB**, text included;
past that the Files API is required, which §Phase 29 puts out of scope in favour
of refusing.

### openai_compat — `chat/completions` **[inferred, deliberately]**

The data-URI part shape (`{"type":"image_url","image_url":{"url":"data:…"}}`) is
the de-facto convention across compatible endpoints, but "compatible" is a claim
each endpoint makes for itself. This is the one adapter where a declared
capability and reality most easily diverge, so it is marked inferred and the
capability is opt-in per model rather than assumed.

### openai_codex — **none**

Declares `images_in: false`. It stays refused, and the refusal is the feature.

Each of these is a plan requirement now backed by a citation rather than an assumption.

1. **Provider-specific parsers are mandatory**. Five providers, five framings, three tool-call models (fragmented string, fragmented per-index string, complete object), two auth placements, one non-SSE transport. A single "OpenAI-compatible" abstraction would be a lie.
2. **The SSE transport decoder and the provider event decoder are separate layers**. Comment frames and `[DONE]` are transport concerns; `response.function_call_arguments.delta` is a provider concern.
3. **`UnknownUpstreamEvent` is required by Anthropic's stated versioning policy** — not defensive over-engineering.
4. **Never infer success from HTTP status.** OpenRouter proves the body is the authority.
5. **Tool argument parsing happens only at the provider-defined completion boundary** — `content_block_stop` (Anthropic), `response.function_call_arguments.done` (OpenAI), immediately (Ollama, Gemini).
6. **Usage normalisation is per-provider arithmetic**: cumulative (Anthropic), final-chunk (OpenRouter, Ollama), cache-inclusive (Gemini).
7. **Cancellation is a client-side drop** for every provider *as smed uses them*. OpenAI publishes `/responses/{id}/cancel`, but it applies **only to `background: true` responses**, which the MVP does not create (§1). Reconnecting a generation POST is forbidden, so cancellation means dropping the response stream and recording it — never retrying.
8. **Prefer machine-readable specs where they exist.** OpenAI's `openapi.yaml` (MIT) and OpenRouter's `openapi.yaml` (already in ) are authoritative in a way prose guides are not, and they answered questions the guides left open. Neither becomes a dependency: read, cite, implement independently.
