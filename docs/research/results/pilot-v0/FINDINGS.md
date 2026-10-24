# Pilot v0 findings (exploratory)

Model `qwen3.6:35b-a3b-coding-mxfp8` (local Ollama), 6 arms x 24 tasks x 3 repeats = 432 episodes, seed 20261024,
600 s / 30 step budget, commit ebb8420. Full tables: `report.md`, raw episodes: `episodes.jsonl`.

## Pass counts by category (of 18 per cell, 15 for 5-task categories x 3)

| Category | A | B | C | D | E | F |
|---|---|---|---|---|---|---|
| context_and_state (15) | 5 | 15 | 15 | 3 | 4 | 13 |
| repository_repair (24) | 24 | 24 | 23 | 24 | 22 | 24 |
| tool_use_reliability (18) | 18 | 18 | 15 | 17 | 17 | 17 |
| verification_and_honesty (15) | 11 | 13 | 13 | 10 | 11 | 15 |

## What the data supports
- Verification (B) and the full harness (F) beat the minimal arm A: +0.17 (CI +0.06..+0.31) and +0.15 (CI 0.00..+0.32) per-task pass rate. B eliminated false completions (11 in A, 0 in B).
- Persistent state (C) fixes the resume tasks: context_and_state 15/15 vs 5/15 for A. Failures in A/D/E on these are all `context_or_state_loss`, as designed.
- F used fewer tokens per pass than A (about 18k vs 35k), mostly because failed A runs burn budget.

## What it does not support
- Reasoning=high alone (D) did not help; it was slightly worse (-0.06, CI includes 0) and cost ~1.5x the tokens and 2.5x the latency of A.
- Arm E (seed context on) is not distinguishable from A.
- repository_repair and tool_use_reliability are at ceiling for this model (166/168 combined excluding nothing); they cannot separate arms. The signal comes from context_and_state and verification_and_honesty, which were designed to need the harness features.

## Caveats
- Arm B passes 15/15 on resume tasks without a session, while A (also no session) gets 5/15. Not explained; needs a trace read before claiming the verify gate causes it (possibly the gate keeps the agent working past the 1-step interrupt).
- Arm E only toggles seed-context; no real context-policy arm exists yet.
- Task set is author-written and the resume/honesty tasks favour the features under test.
- 3 repeats, single model, 2 infra errors (counted, not dropped). Exploratory, not a benchmark claim.
