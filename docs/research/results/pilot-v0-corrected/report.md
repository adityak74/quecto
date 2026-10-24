# Harness ablation report

432 episodes. Exploratory: small sample, intervals are wide; infrastructure errors are counted, not dropped.

## Per-arm results

| Arm | n | Pass | Rate | 95% CI (Wilson) | False completions | Infra errors | Mean tokens | Tokens / pass | Mean latency (s) | Mean tool calls |
|---|---|---|---|---|---|---|---|---|---|---|
| A | 72 | 58 | 0.81 | 0.70–0.88 | 11 | 0 | 28220 | 35032 | 22.5 | 8.4 |
| B | 72 | 60 | 0.83 | 0.73–0.90 | 4 | 0 | 34982 | 41978 | 20.8 | 10.0 |
| C | 72 | 66 | 0.92 | 0.83–0.96 | 5 | 1 | 13604 | 14840 | 24.6 | 5.7 |
| D | 72 | 54 | 0.75 | 0.64–0.84 | 11 | 0 | 43024 | 57365 | 55.7 | 10.0 |
| E | 72 | 54 | 0.75 | 0.64–0.84 | 12 | 1 | 33523 | 44697 | 33.5 | 9.0 |
| F | 72 | 68 | 0.94 | 0.87–0.98 | 3 | 0 | 17549 | 18581 | 24.4 | 6.0 |

## Paired comparison vs arm A (per-task pass-rate difference)

| Arm | Tasks paired | Mean Δ pass rate | 95% bootstrap CI |
|---|---|---|---|
| B | 24 | +0.028 | -0.028 to +0.083 |
| C | 24 | +0.111 | -0.042 to +0.278 |
| D | 24 | -0.056 | -0.139 to +0.014 |
| E | 24 | -0.056 | -0.153 to +0.028 |
| F | 24 | +0.139 | -0.028 to +0.319 |

## Failure attribution

| Failure class | A | B | C | D | E | F |
|---|---|---|---|---|---|---|
| budget_exhaustion | 2 | 5 | 0 | 3 | 5 | 1 |
| context_or_state_loss | 7 | 4 | 0 | 5 | 8 | 0 |
| environment_or_infra | 0 | 0 | 1 | 0 | 1 | 0 |
| incorrect_completion | 3 | 0 | 4 | 5 | 3 | 2 |
| policy_block | 1 | 3 | 0 | 4 | 0 | 0 |
| tool_execution_failure | 1 | 0 | 1 | 1 | 1 | 1 |
