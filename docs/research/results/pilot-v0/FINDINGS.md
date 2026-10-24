# Pilot v0 findings (exploratory)

Model `qwen3.6:35b-a3b-coding-mxfp8` (local Ollama), 6 arms x 24 tasks x 3 repeats = 432 episodes, seed 20261024,
600 s / 30 step budget, commit ebb8420. Full tables: `report.md`, raw episodes: `episodes.jsonl`.

## Pass counts by category (cell size in parentheses)

| Category | A | B | C | D | E | F |
|---|---|---|---|---|---|---|
| context_and_state (15) | 5 | 15 | 15 | 3 | 4 | 13 |
| repository_repair (24) | 24 | 24 | 23 | 24 | 22 | 24 |
| tool_use_reliability (18) | 18 | 18 | 15 | 17 | 17 | 17 |
| verification_and_honesty (15) | 11 | 13 | 13 | 10 | 11 | 15 |

## What the data supports
- Persistent state (C) fixes the resume tasks: context_and_state 15/15 vs 5/15 for A (A/D/E failures are all `context_or_state_loss`, as designed). This comparison is clean: neither arm sees the verifier.
- Verification (B) removed false completions on the honesty tasks (11 in A, 0 in B) and F had 2.
- F used fewer tokens per pass than A (about 18k vs 35k), mostly because failed A runs burn budget.

## Verifier leakage (invalidates the B/F context_and_state cells)
Arms B and F get the visible `verify.sh` in the workspace. On context_and_state tasks that file contains the derivation of the one-time secret (e.g. `cs_02` computes the sha256 prefix), so B/F can pass without any retained transcript. B 15/15 and F 13/15 on that category are therefore **not** evidence for verification, and the headline B/F deltas vs A (+0.17, +0.15) are inflated. Excluding context_and_state (57 episodes per arm): A 53, B 55, C 51, F 56 passes, i.e. differences of 0-5 episodes, well inside noise. Fix for a rerun: use a gate verifier that checks the outcome without revealing the secret (or have the gate be a different script than the grader).

## What it does not support
- Reasoning=high alone (D) did not help; it was slightly worse (-0.06, CI includes 0) and cost ~1.5x the tokens and 2.5x the latency of A.
- Arm E (seed context on) is not distinguishable from A.
- repository_repair and tool_use_reliability are at ceiling for this model (141/144 and 102/108 passes across arms); they cannot separate arms. The signal comes from context_and_state and verification_and_honesty, which were designed to need the harness features.

## Caveats
- Arm E only toggles seed-context; no real context-policy arm exists yet.
- Task set is author-written and the resume/honesty tasks favour the features under test.
- 3 repeats, single model, 2 infra errors (counted, not dropped). Exploratory, not a benchmark claim.
