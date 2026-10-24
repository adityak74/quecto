# Harness ablation findings (exploratory)

Supersedes `pilot-v0/FINDINGS.md`, whose B/F verification result was invalid (see below).

## Data
- `pilot-v0-corrected/`: qwen3.6:35b-a3b-coding-mxfp8, 6 arms x 24 tasks x 3 repeats = 432 episodes. The B/F context_and_state cells from `pilot-v0/` are replaced by the rerun in `pilot-v0b-gate/` (30 episodes). Raw `pilot-v0/` is kept for the record only.
- `pilot-gemma4/`: second model, `gemma4:26b`, arms A/B/C/F on the 10 context_and_state + verification_and_honesty tasks x 3 repeats = 120 episodes.
- `terminal-bench-pilot.md`: external check, A 4/20 vs F 7/20.

## The leak and the fix
In pilot-v0, arms B/F were given the visible `verify.sh`, which for context_and_state tasks contained the secret's derivation, so B/F could pass without a retained transcript (B scored 15/15). The runner now copies `gate.sh` (non-revealing: existence/shape checks only) as the visible gate when a task has one; grading still uses the pristine `verify.sh`. After the fix B on those tasks is 5/15, the same as A, which confirms the leak.

## Results (pass rate, Wilson 95% CI)
qwen (n=72/arm): A 0.81, B 0.83, C 0.92, D 0.75, E 0.75, F 0.94 (0.87-0.98).
Paired vs A: B +0.03 (-0.03..+0.08), C +0.11 (-0.04..+0.28), D -0.06, E -0.06, F +0.14 (-0.03..+0.32).
gemma4:26b (n=30/arm, 10 tasks): A 0.37, B 0.33, C 0.70, F 0.73. Paired vs A: C +0.33 (0.00..+0.70), F +0.37 (+0.03..+0.70).

## What holds up
- Persistent state is the one clear effect, and it replicates across both models: context_and_state is 15/15 with C on both models vs 5/15 (qwen A) and 3/15 (gemma A). Failures without it are `context_or_state_loss`.
- Arm F (full) is at or near the top on both models, but the F-over-A gain comes from the state-retention tasks; F's paired CI on qwen includes 0.
- Verification gate (B) alone: no measurable pass-rate effect on either model (+0.03 qwen, -0.03 gemma). It reduced false completions on qwen (11 -> 4) and on gemma (17 -> 8), but gemma still false-completes often.
- Reasoning=high alone (D) did not help on qwen and cost ~1.5x tokens, ~2.5x latency. Arm E (seed context only) is indistinguishable from A.
- Tokens per pass: F and C use far fewer than A on both models, since failed A runs burn budget.

## Caveats
- repository_repair and tool_use_reliability are at ceiling for qwen and cannot separate arms. The second model was run only on the two categories that can.
- Tasks are author-written and the state/honesty tasks are designed around the features tested; arm E is not a real context-policy arm.
- Few repeats, two models, the gemma run is a subset; intervals are wide. Not a benchmark claim.
- Terminal-Bench result is n=1 on 20 hand-picked tasks, confounded by a 40-step cap.
