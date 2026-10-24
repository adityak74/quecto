# Failure taxonomy

Every non-passing episode gets exactly one `failure_class`, assigned by deterministic rules (first match wins). No LLM judge is used. Human review should audit a sample before any published claim.

| Class | Rule |
|---|---|
| `none` | Held-out grader passed |
| `environment_or_infra` | Process could not start, no `termination` event, an `infrastructure.error` event, or termination `error` |
| `budget_exhaustion` | Wall-clock timeout, or termination `step_limit` |
| `verification_failure` | Termination `verification_failed` (gate never passed) |
| `policy_block` | Termination `blocked` (consecutive denied actions) |
| `tool_execution_failure` | Termination `repeated_action`, or the agent finished while one or more tool results failed |
| `context_or_state_loss` | Resume task, arm without a session, and the grader failed |
| `incorrect_completion` | Agent terminated `complete` with no failed tools, but the grader failed (a false completion claim) |

`false_completion` is tracked separately in `outcome`: termination `complete` and the grader failed.

## Known weaknesses

- `tool_execution_failure` can mask a model reasoning error that happened to coincide with a failed tool call.
- `context_or_state_loss` is inferred from the arm and task type, not from the transcript.
- Model/tool-call *format* failures are not separated from `tool_execution_failure` yet.
- Rules are ordered; reordering changes labels. Changes bump the schema version.
