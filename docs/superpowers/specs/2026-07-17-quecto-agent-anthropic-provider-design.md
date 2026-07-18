# quecto-agent: Anthropic Claude API (Messages API) compatibility

## Problem

`quecto-agent`'s only model client, `HttpModel`, hardcodes an OpenAI-compatible
wire format: `POST {base_url}/chat/completions`, `Authorization: Bearer`,
flat-string message content, OpenAI-shaped tool calls
(`{"type":"function","function":{name,arguments}}`), and OpenAI-shaped
responses (`choices[0].message`). Anthropic's Messages API
(`POST {base_url}/v1/messages`) differs on every one of those axes: auth
header, request shape (`system` is a top-level field, not a message role;
content is an array of typed blocks; tool calls are `tool_use` content
blocks; tool results are `tool_result` blocks inside a `user` message;
`max_tokens` is required), and response shape (`content: [...]` blocks,
`stop_reason` instead of `finish_reason`).

quecto-agent has no way to talk to Claude models directly today (only via an
OpenAI-compatible proxy in front of them). This adds native support.

## Goals

- Add a `Provider` selector (`openai` default, `anthropic`) to flavor/CLI
  config, so the model client dispatches to the right request-builder and
  response-parser.
- Full "core parity" for the Anthropic Messages API: system/user/assistant
  messages, tool calls (`tool_use`/`tool_result`), and reasoning-mode mapped
  to Anthropic's `thinking` parameter.
- Keep `HttpModel`'s public shape and the `Model` trait unchanged — this is
  an internal request/response translation change, not a new abstraction
  layer exposed to callers.

## Non-goals (v1)

- Streaming (SSE). Neither provider streams through `HttpModel` today (both
  use the buffered `quecto_raw` primitive); this stays true for both.
- Anthropic features beyond core chat: vision/PDF input, prompt caching,
  citations, batches, extended "effort" tuning beyond the thinking
  token-budget ladder below.
- Any provider beyond OpenAI-compatible and Anthropic. The `Provider` enum
  leaves room to add more later but nothing else is being wired up now.

## Design

### `Provider` selection

New enum in `quecto-agent/src/provider.rs`:

```rust
pub enum Provider {
    OpenAiCompatible,
    Anthropic,
}
```

Plain enum (not `dyn Trait`), matching the rest of the codebase's style
(`flavor.rs`, `reasoning::ReasoningMode`) — there are exactly two variants
and no external plugin surface.

- New flavor field `provider: Option<String>`, parsed via `FromStr` (same
  pattern as `ReasoningMode`), default `OpenAiCompatible` when absent or
  unset.
- New CLI flag `--provider <openai|anthropic>`, same override precedence as
  `--model`/`--base-url` (CLI > flavor > default).
- New flavor field `max_tokens: Option<u32>` (default `4096`) — Anthropic
  requires `max_tokens` on every request; OpenAI-compatible requests ignore
  it (unchanged behavior).
- Auth: both providers read the key from `QUECTO_API_KEY` (unchanged env var
  name). Anthropic sends it as `x-api-key` plus a fixed
  `anthropic-version: 2023-06-01` header instead of `Authorization: Bearer`.

### Request building

`HttpModel::complete_with_options` currently inlines
`messages_to_body(&self.model, messages)` (OpenAI shape) then POSTs to
`self.url`. This becomes provider-dispatched:

- `self.url` becomes provider-agnostic (`base_url` only); each adapter
  appends its own path (`/chat/completions` vs `/v1/messages`) when building
  the request, rather than baking the path into the stored URL. (Currently
  `join_url(&base_url, "chat/completions")` is computed once in `main.rs`
  and stored on `HttpModel`; this moves the suffix decision into the
  provider so `--provider anthropic` doesn't require also overriding the
  join logic at every call site.)
- `messages_to_anthropic_body(model, messages, max_tokens)`:
  - Splits `system`-role messages out of `messages` into the top-level
    `system: string` field (concatenated if more than one; matches how a
    single system message is the norm today).
  - Converts remaining messages to Anthropic's block-content shape:
    - `assistant` messages with `tool_calls` → `content` is an array with a
      `text` block (if `content` non-empty) followed by one `tool_use` block
      per call: `{"type":"tool_use","id":c.id,"name":c.name,"input":c.arguments}`.
    - `tool` messages (tool results) → re-roled to `user`,
      `content: [{"type":"tool_result","tool_use_id":m.tool_call_id,"content":m.content}]`.
    - Plain `user`/`assistant` text messages → `content` stays a plain
      string (Anthropic accepts both string and block-array content, so no
      block-wrapping needed when there's nothing but text).
  - `tools` (if non-empty): converts each OpenAI-shaped tool def
    (`{"type":"function","function":{name,description,parameters}}`) to
    Anthropic's flat shape (`{"name","description","input_schema":parameters}`).
  - `max_tokens: max_tokens` always included (required by the API).
- Reasoning mode: `reasoning::apply_reasoning_mode` currently detects the
  provider by sniffing whether the endpoint URL ends with
  `/chat/completions`. This becomes an explicit `Provider` parameter instead
  of URL-sniffing. For `Provider::Anthropic`, inject
  `thinking: {"type":"enabled","budget_tokens":N}` where `N` comes from a
  fixed ladder:

  | ReasoningMode | budget_tokens |
  |---|---|
  | None | *(thinking omitted entirely)* |
  | Minimal | 1024 |
  | Low | 4000 |
  | Medium | 10000 |
  | High | 24000 |
  | XHigh | 32000 |

  (Anthropic's minimum `budget_tokens` is 1024; `Minimal` maps there rather
  than to 0.) `provider_reasoning_parameters` telemetry captures the same
  `{"thinking": {...}}` value that's sent, so the existing OTel span
  recording and `MessageMetadata` persistence needs no further changes.

### Response parsing

`parse_assistant_completion` currently assumes
`resp.choices[0].message.{content,tool_calls,finish_reason}`. A new
`parse_anthropic_completion(resp: &Value) -> Result<ModelCompletion, BoxErr>`
handles Anthropic's shape:

- `content: [...]` blocks: `type: "text"` blocks concatenated into
  `AssistantMessage.content`; `type: "tool_use"` blocks become `ToolCall { id, name, arguments: input }`.
- `stop_reason` (`"end_turn"`, `"tool_use"`, `"max_tokens"`, `"stop_sequence"`)
  maps to `finish_reason` — pass the raw string through unchanged (matches
  how OpenAI's `finish_reason` string is passed through today; downstream
  code that branches on `finish_reason` is provider-agnostic already since
  it only cares about `"tool_use"`-equivalent vs not... **needs
  verification during implementation**: check `agent.rs`'s finish-reason
  branches to confirm `"tool_use"` is the only string compared, since that
  string happens to already match Anthropic's).
- `usage.output_tokens` feeds `actual_reasoning_tokens` telemetry when
  `thinking` was requested (Anthropic doesn't separately report reasoning
  vs. output tokens the way some OpenAI-compatible reasoning models do via
  `usage.reasoning_tokens`; document this as a best-effort approximation,
  not a true reasoning-token count).
- No `<think>` tag extraction needed — Anthropic returns thinking as a
  distinct block type when present in the response (`type: "thinking"`),
  not embedded in the text content. Extract `thinking` blocks into
  `AssistantMessage.reasoning_content` directly, parallel to how OpenAI's
  `reasoning_content`/`thinking`/`reasoning` fields are checked today.

### Testing

- Unit tests in `provider.rs` / `model.rs` mirroring existing coverage:
  request-shape tests (`messages_to_anthropic_body_shape`,
  `anthropic_system_message_extracted_to_top_level`,
  `anthropic_tool_result_reroled_to_user`), reasoning-injection test
  (`injects_thinking_payload_for_anthropic`), response-parsing tests
  (text-only, tool-use, thinking-block extraction).
- One local-server integration test analogous to the existing
  `TcpListener`-based fake-server test in `model.rs` (~line 766), pointed at
  a fake `/v1/messages` endpoint returning a canned Anthropic response, to
  exercise the full `ConfiguredHttpModel::complete_with_options` path
  end-to-end for the Anthropic provider.
- Extend `quecto-agent/tests/model.rs` (existing integration test file) with
  an Anthropic-provider case if it currently drives `HttpModel` against a
  local mock server.

## Open questions for implementation (not blocking design approval)

- Confirm `agent.rs`'s `finish_reason` branching doesn't assume any
  OpenAI-specific values beyond `"tool_use"` (which Anthropic also uses) —
  if it does, those branches need a provider-agnostic finish-reason
  normalization instead of passing the raw string through.
- Confirm whether `max_tokens: Option<u32>` should also be surfaceable via a
  CLI flag (`--max-tokens`) or flavor-file-only for v1; leaning flavor-only
  unless a concrete need for CLI override comes up during implementation.
