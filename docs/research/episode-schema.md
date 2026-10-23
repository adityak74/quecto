# Episode schema `quecto.episode/v0`

One JSON object per run, one line per object in `episodes.jsonl`. Written by `quecto-eval ablate`; consumed by `quecto-eval report`.

```jsonc
{
  "schema": "quecto.episode/v0",
  "episode_id": "<experiment>-<arm>-<task>-<repetition>",
  "harness_version": "quecto-agent <version>@<git sha>",
  "model": {"id": "...", "base_url": "...", "reasoning_fixed": "high"},
  "task": {"id": "...", "category": "...", "environment_hash": "<sha256 of workspace after setup>", "resume_task": false},
  "configuration": {"arm": "A", "args": ["--no-verify"], "env": {"QUECTO_REASONING_MODE": "none"}},
  "budget": {"max_steps": 30, "timeout_secs": 900},
  "repetition": 0,
  "outcome": {"passed": true, "termination": "complete", "timed_out": false, "false_completion": false},
  "events": {"turns": 0, "tool_calls": 0, "tool_failures": 0, "verifier_runs": 0, "verifier_failures": 0, "claims": 0, "infra_errors": 0},
  "token_usage": {"prompt": 0, "completion": 0, "total": 0},
  "latency_ms": 0,
  "failure_class": "none",
  "artifacts": {"traces": ["<episode>-s1.jsonl"]}
}
```

## Notes

- **Versioned.** The `schema` field changes on any breaking change.
- **Not yet replayable.** Episodes record configuration, hashes, and aggregate event counts; raw prompts and tool outputs stay in the per-run trace files (`traces/`) and logs, which are opt-in to share.
- **Traces.** Each stage writes a trace file (`<episode>-sN.jsonl`) of agent events: `turn` (with `prompt_tokens`, `completion_tokens`, `duration_ms`), `tool.call`, `tool.result`, `verifier.start/result`, `assistant.claim`, `termination`, `infrastructure.error`.
- **Safe to share.** Arm env contains no secrets (API keys are never written). Workspaces are deleted after grading.
- **Failure class** is assigned by deterministic rules; see [failure-taxonomy.md](failure-taxonomy.md).

Not yet implemented: JSON-schema validation of episodes, artifact hashing beyond the environment hash, per-event model-call records inside the episode.
