# Skill: run-evals

## Overview
This skill automates the execution of the **quecto-agent evaluation harness** located at `evals/run_evals.sh`. It builds the required binaries (if needed) and runs the full suite of TerminalBench‑style tasks, reporting PASS/FAIL for each.

## Usage
1. Open a terminal in the root of the `quecto` repository.
2. (Optional) Set environment variables to control models/URLs:
   - `AGENT_MODEL` – e.g. `qwen3.6:35b-mlx`
   - `AGENT_URL` – e.g. `http://localhost:11434/v1`
   - `JUDGE_MODEL` – e.g. `google/gemini-2.0-flash-lite-preview-02-05:free`
   - `JUDGE_URL` – e.g. `https://openrouter.ai/api/v1`
   - `OPENROUTER_API_KEY` – required when using OpenRouter as the judge.
3. Run the skill from any Antigravity session with:
   ```
   agy skill run-evals
   ```
   The skill will:
   - Build `quecto` and `quecto-agent` in release mode if they are missing.
   - Execute `./evals/run_evals.sh`.
   - Stream the output, showing each task and its PASS/FAIL result.

## Implementation Details
The skill simply invokes the following command internally:
```bash
cd "$(git rev-parse --show-toplevel)" && ./evals/run_evals.sh
```
It captures the exit code and returns it to the user.

## Notes
- The harness already includes task‑specific judging criteria, so the skill does not need to supply extra prompts.
- To run a subset of tasks, edit or remove the corresponding directories under `evals/tasks/` before invoking the skill.

---
**Author**: Antigravity
**Version**: 1.0
