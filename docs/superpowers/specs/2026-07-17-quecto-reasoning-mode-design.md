# QuECTO Reasoning Mode Design

This document specifies a narrowly scoped research-facing addition to QuECTO: a single normalized `reasoning_mode` control that can be set by configuration and overridden by the harness between completions. It intentionally does not include scheduling, checkpointing, replay policies, or export-format changes.

## Goal

Allow QuECTO to:

1. Accept a normalized reasoning-level setting such as `none`, `minimal`, `low`, `medium`, `high`, or `xhigh`.
2. Translate that setting into provider-specific request parameters without hiding or mutating the rest of the raw request body.
3. Let an external harness change the reasoning mode between completions or runs.
4. Record what was requested and what the provider actually exposed in the response.

## Non-Goals

This milestone does not include:

* per-turn schedule files or rule engines;
* checkpoint/fork or trace-retention interventions;
* benchmark orchestration logic;
* cross-provider semantic calibration beyond storing the provider-specific parameters used.

## Existing Architecture Fit

QuECTO already has the necessary seams:

* `quecto-agent/src/model.rs` owns OpenAI-compatible request-body construction in `HttpModel::complete`.
* `quecto-agent/src/agent.rs` owns the control loop and should remain provider-agnostic.
* `quecto-agent/src/session.rs` already persists `reasoning_content`.
* OTEL tracing already emits model-level events and span attributes when the `otel` feature is enabled.

Because provider-specific reasoning controls belong to request construction, the feature should live primarily in the model layer rather than the agent loop.

## User-Facing Control Surface

QuECTO should support one normalized knob:

* Environment variable: `QUECTO_REASONING_MODE`
* Optional flavor/config field: `reasoning_mode`
* Programmatic override: a per-call or mutable model option exposed to the harness

Accepted normalized values:

* `none`
* `minimal`
* `low`
* `medium`
* `high`
* `xhigh`

Behavioral rules:

1. If no reasoning mode is configured, QuECTO sends no provider-specific reasoning parameters.
2. If both env/config and harness override are present, the harness override wins for that completion.
3. Invalid reasoning-mode values fail fast before the request is sent.
4. QuECTO stores the normalized requested value even when the provider ignores or partially supports it.

## Internal Model

Add a small normalized type, for example:

```rust
pub enum ReasoningMode {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}
```

And an override carrier, for example:

```rust
pub struct CompletionOptions {
    pub reasoning_mode: Option<ReasoningMode>,
}
```

`HttpModel` should hold a default reasoning mode derived from env/config, while the harness can supply a per-completion override through the completion path. The agent loop should not interpret the reasoning mode; it only passes options through.

## Request Construction

When a reasoning mode is present, `HttpModel` should inject provider-specific fields into the raw request body immediately before dispatch. The translation layer should be explicit and inspectable.

Example OpenAI-compatible mapping:

```json
{
  "reasoning": {
    "effort": "low"
  }
}
```

Design constraints:

1. Translation must be centralized in one helper so providers can diverge later without touching the agent loop.
2. The final request body must still preserve arbitrary non-reasoning fields that QuECTO already supports.
3. The translated provider payload should be capturable for telemetry as structured JSON.

For unsupported providers or models, QuECTO should choose one of two explicit outcomes:

* omit unsupported reasoning parameters and record `reasoning_mode_applied=false`; or
* fail fast with a configuration error if the provider target is known to reject the parameter.

The initial implementation should prefer omission plus telemetry unless the target is known to hard-fail.

## Harness Mutability

The research harness needs to change the reasoning mode without rebuilding the rest of the run state. QuECTO should therefore expose one of these equivalent patterns:

* `HttpModel::with_completion_options(options)`
* `HttpModel::set_reasoning_mode(mode)`
* `Model::complete_with_options(messages, tools, options)`

The important requirement is semantic, not API style: the harness must be able to request `low` for one completion and `high` for the next while preserving the same agent/session context.

The preferred implementation is a new `complete_with_options` path with the existing `complete` method delegating to it using default options. That preserves backward compatibility for the current agent code while giving the harness a precise override point.

## Telemetry and Persistence

Each completion should capture, at minimum:

* `requested_reasoning_mode`
* `provider_reasoning_parameters`
* `reasoning_mode_applied`
* `reasoning_content_available`
* `actual_reasoning_tokens` when present in the provider response

Where to store it:

* OTEL `model_complete` span attributes and events when the `otel` feature is enabled
* session persistence for any response-derived fields worth resuming or later analysis

This milestone does not require a full research-events table or JSONL exporter. It is sufficient to record the requested mode and observed provider outputs in the existing session/tracing surfaces, with additive schema changes if needed.

## Response Parsing

QuECTO already extracts:

* `reasoning_content`
* `<think>...</think>` content

It should also parse provider usage metadata when available, especially reasoning-token counts, without depending on one provider-specific shape. This likely belongs beside `parse_assistant` or in a companion response-metadata parser that returns additive completion telemetry.

If reasoning tokens are absent, QuECTO should record `actual_reasoning_tokens = null`, not `0`.

## Error Handling

Validation and runtime behavior should be explicit:

1. Unknown reasoning mode: configuration error before request dispatch.
2. Malformed provider response: preserve existing parse failure behavior.
3. Unsupported provider parameter: omit or error according to known compatibility, but always surface the outcome in telemetry where possible.
4. Missing reasoning content despite requested mode: not an error; record absence.

## Testing Strategy

The implementation should add focused tests in three layers:

### 1. Configuration parsing

Verify:

* env parsing accepts valid normalized values;
* env parsing rejects invalid values;
* config/flavor parsing merges correctly with env defaults;
* harness override wins over defaults.

### 2. Request-body serialization

Verify:

* no reasoning fields are emitted by default;
* each normalized reasoning mode maps to the expected provider payload;
* existing request fields remain intact when reasoning fields are added.

### 3. Response metadata parsing

Verify:

* reasoning-token counts are extracted when present;
* absent counts remain `None`;
* existing reasoning-content extraction continues to work unchanged.

### 4. Harness override integration

Using a scripted or fake model path, verify that two consecutive completions can request different reasoning modes while keeping the same transcript/session state.

## Milestone Outcome

This milestone is complete when:

1. QuECTO accepts a normalized reasoning mode from env/config.
2. The harness can override the reasoning mode for an individual completion.
3. `HttpModel` injects provider reasoning parameters into the raw request body.
4. QuECTO records the requested mode and any observed reasoning-token metadata.
5. Existing agent behavior remains backward compatible when no reasoning mode is configured.

## Open Questions Resolved For This Milestone

To keep the implementation compact and research-useful, this spec fixes the following choices:

* No schedule DSL is included.
* No attempt is made to equate semantic strength across providers.
* Unsupported providers default to best-effort omission plus telemetry unless known to reject the field.
* The harness, not QuECTO, decides when to change the mode.
