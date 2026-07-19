# Quecto SOTA Evals Design

## 1. Overview
This design outlines the architecture for upgrading the Quecto evaluation harness to a State-of-the-Art (SOTA) system, bringing it in line with Anthropic's best practices for agent evaluations. The new harness will be written natively in Rust, supporting Grader Composability, Transcript Analysis, Telemetry Tracking, and Suite Management.

## 2. Architecture & Configuration

### The Crate
A new binary crate `quecto-eval` will be created within the Cargo workspace. It will be the single entry point for running evaluation suites.

### Task Definition
Each task is self-contained in a directory and defined by an `eval.yaml` file, moving away from implicit shell script conventions.

Example `eval.yaml`:
```yaml
id: tb_01_git_conflict_resolution
suite: regression
prompt_file: prompt.md
setup_script: setup.sh

graders:
  - type: script
    command: verify.sh
  - type: llm_rubric
    rubric: "Did the agent explain the git conflict resolution clearly? Output PASS or FAIL."

telemetry_thresholds:
  max_turns: 10
  max_tokens: 15000
```

### Execution Flow
1. **Discovery:** The harness parses all `eval.yaml` files in the `evals/suites/` directory, filtering by the `--suite` flag.
2. **Setup:** A temporary workspace is instantiated, and `setup_script` is executed.
3. **Execution:** `quecto-agent` is spawned as a child process. The harness injects `QUECTO_TRACE_FILE` into the environment, instructing the agent to dump structured API interaction logs.
4. **Grading:** All configured graders are executed. The eval passes only if all graders pass (AND logic).
5. **Telemetry Export:** Results, latencies, and token usages are logged to a persistent store.

## 3. Graders & Transcript Analysis Components

### The Grader Trait
A core `Grader` trait will allow polymorphic execution of evaluators.
```rust
#[async_trait]
pub trait Grader {
    async fn evaluate(&self, ctx: &EvalContext) -> anyhow::Result<GraderResult>;
}

pub struct GraderResult {
    pub passed: bool,
    pub reason: String,
}
```

### Built-in Graders
1. **ScriptGrader**: Executes deterministic shell scripts (e.g., `verify.sh`).
2. **LlmRubricGrader**: Constructs a prompt using the current workspace state and the task's rubric, then queries the configured LLM Judge. Automatically retries up to 3 times on API errors.
3. **TelemetryGrader**: Evaluates metadata extracted from the agent's trace file against thresholds (e.g., `max_turns`).

### Transcript Analysis (Trace File)
Parsing unstructured terminal output is brittle. `quecto-agent` will be modified to support a telemetry dump:
- If `QUECTO_TRACE_FILE` is set, the agent appends JSON objects for every API request/response.
- `quecto-eval` deserializes this file into a `Transcript` struct for use by the `TelemetryGrader` and the storage backend.

## 4. Data Storage & Suite Management

### Suite Management
Tasks are strictly categorized by suite (e.g., `capability` for hard tasks to hill-climb on, `regression` for tasks that should maintain a near 100% pass rate). The CLI will enforce suite targeting: `cargo run -p quecto-eval -- --suite regression`.

### Telemetry Storage
A local SQLite database (`evals/results/telemetry.db`) will be used to store historical run data to track regressions over time.
Schema fields will include:
- `run_id` (UUID)
- `timestamp`
- `task_id`
- `suite`
- `agent_model`
- `judge_model`
- `passed` (boolean)
- `total_tokens`
- `total_latency_ms`
- `total_turns`
- `error_message` (if the infra/LLM failed)

## 5. Error Handling
- **Infrastructure Failures:** If `setup.sh` fails, the test state is `ERROR` (not `FAIL`).
- **LLM Flakiness:** API timeouts in the `LlmRubricGrader` result in retries; persistent failures result in an `ERROR` state. This prevents infra issues from contaminating capability metrics.
