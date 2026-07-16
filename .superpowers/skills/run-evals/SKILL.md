# Skill: run-evals

## Overview
This skill runs the **QuECTO evaluation suite**, which covers two modes:

1. **Smoke tests** (`evals/smoke/`) — 10 hard TerminalBench-style tasks, each with a deterministic `verify.sh` verifier. No LLM judge needed.
2. **Harbor / Terminal-Bench 2.x** (`evals/harbor/`) — runs the full 89-task Terminal-Bench benchmark via the Harbor framework.

---

## Running the Smoke Suite

From the repo root, run:

```bash
./evals/run_evals.sh
```

By default this uses **deterministic `verify.sh` scripts** (no API key needed). To override with an LLM judge:

```bash
./evals/run_evals.sh --llm-judge
```

### Environment Variables (all optional)

| Variable | Default | Purpose |
|---|---|---|
| `AGENT_MODEL` | `qwen3.6:35b-mlx` | Local model for quecto-agent |
| `AGENT_URL` | `http://localhost:11434/v1` | Ollama (or any OpenAI-compat) endpoint |
| `JUDGE_MODEL` | `google/gemini-2.0-flash-lite-preview-02-05:free` | Judge model (LLM mode only) |
| `JUDGE_URL` | `https://openrouter.ai/api/v1` | Judge API (LLM mode only) |
| `OPENROUTER_API_KEY` | _(none)_ | Required only when JUDGE_URL is OpenRouter |

Example with a custom model:

```bash
AGENT_MODEL=llama3.1:8b ./evals/run_evals.sh
```

---

## Running Harbor / Terminal-Bench 2.x

This uses the adapter at `evals/harbor/quecto_agent.py`.

### One-time setup

```bash
pip install harbor
# Build the quecto-agent binary
cargo build --release -p quecto-agent
```

### Run the benchmark

```bash
harbor run \
  -d terminal-bench/terminal-bench-2 \
  -m qwen3.6:35b-mlx \
  --agent evals.harbor.quecto_agent:QuectoAgent
```

Set `QUECTO_AGENT_BIN` if the binary is not at `target/release/quecto-agent`.

---

## Smoke-Testing the Harbor Adapter

Run the adapter smoke tests (no Harbor install needed — uses stdlib mocks):

```bash
python3 evals/harbor/test_smoke.py
```

Expected output:

```
Ran 6 tests in ~8s

OK
```

The test suite covers:
- `install()` raises `FileNotFoundError` when binary is absent
- `install()` passes silently when binary exists
- `run()` returns agent stdout on success
- `run()` surfaces stderr on non-zero exit
- `run()` correctly forwards `QUECTO_MODEL` and `QUECTO_BASE_URL` env vars
- Integration: real `quecto-agent` binary creates a file on disk

---

## Adding New Tasks

Drop a new directory under `evals/smoke/` with these three files:

```
evals/smoke/my_task/
├── prompt.md    # Task instruction for the agent
├── setup.sh     # Bash script to initialise the workspace
└── verify.sh    # Deterministic checker — exit 0 = PASS
```

Optionally add `judge.md` for the `--llm-judge` fallback. The harness will auto-discover the new task on the next run.

---

**Author**: Antigravity  
**Version**: 2.0
