# Harness ablation report

120 episodes. Exploratory: small sample, intervals are wide; infrastructure errors are counted, not dropped.

## Per-arm results

| Arm | n | Pass | Rate | 95% CI (Wilson) | False completions | Infra errors | Mean tokens | Tokens / pass | Mean latency (s) | Mean tool calls |
|---|---|---|---|---|---|---|---|---|---|---|
| A | 30 | 11 | 0.37 | 0.22–0.54 | 17 | 0 | 37192 | 101432 | 22.7 | 10.9 |
| B | 30 | 10 | 0.33 | 0.19–0.51 | 8 | 0 | 49629 | 148886 | 35.9 | 15.9 |
| C | 30 | 21 | 0.70 | 0.52–0.83 | 9 | 0 | 12524 | 17892 | 8.7 | 5.9 |
| F | 30 | 22 | 0.73 | 0.56–0.86 | 4 | 0 | 21706 | 29599 | 31.2 | 5.6 |

## Paired comparison vs arm A (per-task pass-rate difference)

| Arm | Tasks paired | Mean Δ pass rate | 95% bootstrap CI |
|---|---|---|---|
| B | 10 | -0.033 | -0.133 to +0.067 |
| C | 10 | +0.333 | +0.000 to +0.700 |
| F | 10 | +0.367 | +0.033 to +0.700 |

## Failure attribution

| Failure class | A | B | C | F |
|---|---|---|---|---|
| budget_exhaustion | 0 | 6 | 0 | 0 |
| context_or_state_loss | 10 | 5 | 0 | 0 |
| incorrect_completion | 7 | 3 | 9 | 4 |
| policy_block | 2 | 4 | 0 | 0 |
| tool_execution_failure | 0 | 2 | 0 | 0 |
| verification_failure | 0 | 0 | 0 | 4 |
