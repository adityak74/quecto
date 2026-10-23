# Methodology: QuECTO Harness Ablations v0.1

**Question.** Given the same model, task, tools, and budget, which harness mechanisms improve *verified* success, and what do they cost?

This is a pilot: it debugs the experiment and estimates variance. Results are exploratory, not a leaderboard.

## Held constant across arms

Model and digest, endpoint, sampling parameters, system prompt, tool allow-list, approval mode (`--yes`), sandbox/policy, step budget (`--max-steps`, default 30), per-run wall-clock timeout, initial workspace state (rebuilt from `setup.sh` for every run), and run order randomization (seeded shuffle).

## Arms

Defined in [`evals/configs/arms.toml`](../../evals/configs/arms.toml). Arm A switches every switchable mechanism off; B–E each turn on one; F turns all on.

| Arm | Adds over A | Switch |
|---|---|---|
| A | nothing (reasoning `none`) | `--no-verify --no-seed-context --no-session` |
| B | completion-gate verification | `QUECTO_VERIFY` |
| C | persistent session state | omit `--no-session` |
| D | fixed reasoning level | `QUECTO_REASONING_MODE` |
| E | seed context (file tree, git status/diff) | omit `--no-seed-context` |
| F | all of the above | — |

Known limits of the arms:
- **E is "seed context on/off"**, not a retention policy. The agent has no history trimming or compaction to vary yet.
- **C only differs from A on resume tasks.** On single-shot tasks there is no second session to retain state for.
- **B's gate is the visible `verify.sh`**, copied into the workspace; grading always uses the pristine copy. B therefore measures "has a test gate", not "has an independent oracle".

## Tasks

24 tasks in `evals/tasks/`, each with `prompt.md`, `setup.sh`, deterministic `verify.sh`, and a `category` file:

| Category | n | Notes |
|---|---|---|
| repository_repair | 8 | fix, implement, regression, refactor |
| tool_use_reliability | 6 | shell, sqlite, openssl, scripting |
| context_and_state | 5 | one-time-secret tasks, interrupted after 2 steps (`interrupt_steps`) |
| verification_and_honesty | 5 | impossible or trap tasks; agent must write `REPORT.md` starting `STATUS: PASS` or `STATUS: FAIL` |

**Resume tasks.** Stage 1 runs with a 2-step budget. Stage 2 either `resume`s the stored session (arms with a session) or re-runs the original prompt in the same workspace (arms with `--no-session`). The one-time secret is only recoverable from the transcript or from a file the agent wrote during stage 1.

**Honesty tasks** are graded on the *truth* of the reported status (and unmodified tests), never on the model's own claim alone.

Every new verifier was checked to fail on the pristine workspace and pass on a reference solution.

## Metrics

Verified pass rate (primary); prompt/completion tokens (provider-reported), wall-clock latency, tool calls, tool failures, false completions (agent terminated `complete` but grading failed), infrastructure errors, and tokens per verified success. Cost is tokens and time — the models are local.

## Analysis

`quecto-eval report` produces per-arm tables with Wilson 95% intervals, paired per-task differences against arm A with bootstrap 95% intervals, a failure-class table, `results.csv`, and a cost-vs-success plot. Infrastructure errors and timeouts are counted, never dropped.

## Reproduce

```bash
cargo build --release
quecto-eval ablate --out evals/results/pilot-v0 \
  --agent-binary target/release/quecto-agent \
  --model qwen3.6:35b-a3b-coding-mxfp8 --base-url http://localhost:11434/v1 \
  --repeats 3
quecto-eval report --episodes evals/results/pilot-v0/episodes.jsonl --out evals/results/pilot-v0
```

The run is resumable: finished episodes are skipped on restart.

## Threats to validity

Single model; small task set; ceiling effects on easy tasks; local-model nondeterminism not fully controlled by sampling parameters; backend load affects latency; the agent version and git commit are recorded per episode but not the exact Ollama build.
