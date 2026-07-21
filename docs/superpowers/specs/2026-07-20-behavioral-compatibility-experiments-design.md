# Behavioral Compatibility Experiments — Design

## 1. Overview

The paper *"API-Compatible, Behavior-Incompatible"* (`~/Projects/api-compatible-behavior-incompatible-paper`) defines a directional behavioral-compatibility test for LLM-agent runtime substitution: given a reference runtime and a candidate runtime, does the candidate introduce statistically supported negative flips over a versioned suite of observable execution contracts, while preserving verified task success and resource budgets?

The manuscript is a pre-results draft — every empirical claim is a `RESULT PLACEHOLDER`. This design covers the first phase of turning that protocol into real, run experiments using `quecto-agent` as the runtime under test. It intentionally does **not** attempt the full study (24-task pilot / 60-task confirmatory / 20-task external validation across 6 runtime configs and 6 contracts) — see §9 for what's deferred.

`quecto` itself is just the minimal core execution library; `quecto-agent` is the actual runnable coding agent and is the runtime this study measures.

## 2. Scope

**In scope for this iteration:**
- Structured JSONL trace events emitted directly by `quecto-agent`, sufficient to evaluate two contracts: `verify_after_final_change` and `no_success_before_evidence`.
- A contract evaluator engine in `quecto-eval` that reads these traces and returns pass/fail/not-applicable.
- A manifest-driven paired runner in `quecto-eval` that runs a task through a reference and a candidate runtime configuration for N repetitions, with workspace snapshot/restore between runs.
- One substitution axis: reasoning mode (`QUECTO_REASONING_MODE=low` vs `high`), same model/provider — cheapest path to a real (non-placeholder) result, no multi-provider adapter work required.
- A small curated task pool (existing 10 smoke tasks + a handful of new tasks that actually exercise verification-ordering and premature-success-claim behavior).
- Python analysis script(s) in the paper repo computing reference eligibility, paired transition counts, CNFR, confidence bounds, and a compatibility verdict.

**Out of scope for this iteration** (see §9): the other 4 contracts, cross-provider/cross-model substitution, the full 24/60/20 sealed task splits, freeze → confirmatory → external-validation → enforcement-ablation stages, and full checkpoint/fork/replay machinery.

## 3. Existing infrastructure this builds on

- `quecto-agent/src/agent.rs` already has a `TraceEvent` struct and a `QUECTO_TRACE_FILE` env var that appends JSON lines to a file, but today only emits a single `"turn"` event type (`{event_type, tokens_used, duration_ms}`) — see `agent.rs:16` and `agent.rs:402`.
- `quecto-agent/src/recorder.rs` has a `RunRecorder` trait with a `SqliteRecorder` implementation, called on every message/change during a run.
- `quecto-agent/src/verify.rs` has `VerifyReport`/`VerifyResult`, produced by the completion-gate verifier — the natural source for `verifier.start`/`verifier.result` events.
- `quecto-eval` (`config.rs`, `grader.rs`, `runner.rs`) is the evaluation-harness crate outlined in `docs/superpowers/specs/2026-07-19-quecto-evals-sota-design.md`. It already has a `Transcript` struct (currently just `{turns, tokens, latency_ms}`) and a `telemetry.db` SQLite schema (`runs` table). `runner.rs::run_suite` is currently a `todo!()` stub — this is where the paired runner logic belongs.

This design extends both seams rather than introducing a new crate.

## 4. Architecture

### 4.1 Trace event emission (`quecto-agent`)

Extend `TraceEvent` from a single flat struct into an enum (or add new variants) covering the event types needed by the two in-scope contracts:

- `run.start`, `run.end`
- `tool.call`, `tool.result` (from `Registry`/`Tool` execution in `agent.rs`)
- `mutation` (from `FileChange`, already tracked by `RunRecorder::change`)
- `verifier.start`, `verifier.result` (from `Verifier`/`VerifyReport` in `verify.rs`)
- `assistant.claim` (the model's completion message when it declares the task done)
- `termination` (final `Outcome` variant: `Complete`/`StepLimit`/`Error`)
- `infrastructure.error` (provider/tool errors distinct from behavioral failures)

Every event carries: `experiment_id`, `task_id`, `runtime_id`, `run_id`, `repetition`, `quecto_commit` (short git SHA baked in via build script or `env!`), `snapshot_hash`. These identifiers are supplied to the agent via new env vars (`QUECTO_EXPERIMENT_ID`, `QUECTO_TASK_ID`, `QUECTO_RUNTIME_ID`, `QUECTO_RUN_ID`, `QUECTO_REPETITION`) set by the `quecto-eval` runner, read once at `Agent::new` alongside the existing `QUECTO_TRACE_FILE` lookup.

Existing `"turn"` events and the `QUECTO_TRACE_FILE` mechanism are kept as-is; new event types are additive.

### 4.2 Contract evaluators (`quecto-eval`)

A new module (e.g. `quecto-eval/src/contracts.rs`) that:
1. Deserializes a contract YAML (schema: `experiments/contracts/*.yaml` in the paper repo — `verify_after_final_change.yaml`, `no_success_before_evidence.yaml`).
2. Reads the JSONL trace produced by one run.
3. Checks each `required`/`forbidden` predicate against the event sequence (e.g. `verifier_invoked`, `verifier_after_final_mutation`, `stale_verification`).
4. Returns `pass` / `fail` / `not-applicable` plus the list of satisfied/violated predicates.

Contract YAMLs are read directly from the paper repo path (no copy) to keep them as the single source of truth; `quecto-eval` takes the path as a CLI/config argument.

### 4.3 Manifest-driven paired runner (`quecto-eval`)

Implement `runner.rs::run_suite` (currently `todo!()`) to:
1. Parse an experiment manifest (schema per `experiments/manifests/primary.example.yaml`, trimmed to a reasoning-mode-only reference/candidate pair).
2. For each task in the pool, for each of N repetitions: snapshot the task workspace, run the **reference** config (`quecto-agent` subprocess with `QUECTO_REASONING_MODE=high` and the identifying env vars), restore the snapshot, run the **candidate** config (`QUECTO_REASONING_MODE=low`), restore again.
3. Snapshot = tar the task workspace + SHA-256 hash before each run; restore = untar. (Minimal implementation — not the full checkpoint/fork/replay from quecto's roadmap.)
4. After each run, invoke the verifier (already-existing `verify.sh`-per-task convention) and the two contract evaluators (§4.2) against that run's JSONL trace.
5. Persist per-run results to `telemetry.db`, extending the existing `runs` table with columns: `runtime_id`, `run_id`, `repetition`, `experiment_id`, and one pass/fail column per evaluated contract.

### 4.4 Analysis (Python, paper repo)

New scripts under `experiments/analysis/` in the paper repo, reading `telemetry.db`:
- Reference eligibility: one-sided confidence bound on reference pass rate per contract/task ≥ threshold (0.90 for critical contracts, per the manifest).
- Paired transition counts (N11/N10/N01/N00) per contract/task.
- CNFR + one-sided confidence bound.
- Compatibility verdict (`compatible` / `breaking` / `inconclusive`) per contract.
- Output: a markdown/CSV report intended to fill the manuscript's Section 10 placeholders.

## 5. Task pool

- The existing 10 tasks under `quecto/evals/smoke/`.
- 4-6 new tasks under the same directory convention (`prompt.md`, `setup.sh`, `verify.sh`), specifically designed so that verification-ordering and premature-success-claims are observable — e.g. a task with a verifier that's slow/costly enough that a shortcut-prone runtime might claim success before running it, and a task with an easy-to-produce false-positive completion state that only a real verifier run would catch.

## 6. Pilot design

- Substitution axis: reasoning mode only. Reference = `QUECTO_REASONING_MODE=high`, candidate = `QUECTO_REASONING_MODE=low`. Same model, same provider, same adapter — isolates the reasoning-mode variable per the paper's one-factor-substitution preference (§3.1 of the manuscript).
- Repetitions: 3 per task per runtime (matches the manuscript's pilot repetition count, scaled down from 24 tasks/4 runtimes to ~14-16 tasks/2 runtimes for this iteration).
- Contracts evaluated: `verify_after_final_change`, `no_success_before_evidence` only.

## 7. Error handling

- `infrastructure.error` events (provider timeouts, rate limits, tool-execution infra failures) are recorded as a distinct event type and excluded from contract pass/fail scoring — mirrors the manuscript's requirement to record provider outages separately from agent behavior.
- If workspace snapshot/restore fails or produces a mismatched hash, the run is marked `ERROR` (not `FAIL`) and excluded from the paired comparison, per the manuscript's stop conditions ("stop a condition if the task environment is corrupted").
- Reference-eligibility gating: a contract/task pair is only used for compatibility comparison if the reference's confidence-bound pass rate clears the threshold; otherwise it's reported as ineligible rather than scored.

## 8. Testing strategy

- Unit tests for each contract evaluator against hand-built synthetic JSONL traces — both a passing trace and a trace that violates each forbidden predicate (e.g. `stale_verification`).
- Unit tests for the new `TraceEvent` variants' serialization (extending the existing `test_trace_event_serialization` pattern in `agent.rs`).
- One integration test that runs a real smoke task end-to-end through the new pipeline and manually verifies: every required event type is present in the trace, the snapshot-restore cycle reproduces an identical workspace hash, and the contract evaluator returns the expected verdict. This is the "instrumentation validation" gate from `experiments/README.md` step 1 and must pass before the pilot (§6) is run for real.

## 9. Deferred / future work

Not part of this design; to be scoped separately once the pilot proves the instrumentation:
- The remaining 4 contracts (`diagnose_before_retry`, `inspect_before_modify`, `limit_modification_scope`, `stop_after_acceptance`).
- Cross-provider (OpenAI-compatible vs. Anthropic-native) and cross-model substitution axes.
- Scaling the task pool to the full 24-task pilot / 60-task confirmatory / 20-task external-validation sealed splits described in the manuscript.
- Freeze, confirmatory study, external validation, and enforcement-layer ablation stages.
- Full checkpoint/fork/replay (currently a `quecto` roadmap item) in place of the tar+hash snapshot approach used here.
