# Terminal-Bench 2.0 pilot: arm A vs arm F (external check)

Model `qwen3.6:35b-a3b-coding-mxfp8` (local Ollama) through Harbor 0.22 and the `evals/harbor/quecto_agent.py` adapter. 20 hand-picked tasks, 1 attempt each, `QUECTO_MAX_STEPS=40`, default Harbor task timeouts, x86_64 musl binary, commit ebb8420.
Arm A: `--no-verify --no-seed-context --no-session`, reasoning none. Arm F: defaults, reasoning high (no `QUECTO_VERIFY` command is available in TB, so F's verification is the agent's own).

**Result: A 4/20, F 7/20.** Exploratory, n=1 per task: the difference is 3 tasks and is not statistically meaningful (Wilson 95% CI for A 0.08-0.42, F 0.18-0.59).

| Task | A | F |
|---|---|---|
| build-cython-ext | fail NonZeroAgentExitCodeError | fail NonZeroAgentExitCodeError |
| cancel-async-tasks | fail  | pass  |
| configure-git-webserver | fail NonZeroAgentExitCodeError | fail NonZeroAgentExitCodeError |
| count-dataset-tokens | fail  | fail NonZeroAgentExitCodeError |
| crack-7z-hash | fail NonZeroAgentExitCodeError | fail NonZeroAgentExitCodeError |
| extract-elf | fail AgentTimeoutError | pass NonZeroAgentExitCodeError |
| fix-code-vulnerability | fail NonZeroAgentExitCodeError | pass  |
| fix-git | pass  | fail  |
| git-multibranch | fail NonZeroAgentExitCodeError | fail NonZeroAgentExitCodeError |
| kv-store-grpc | fail  | pass  |
| log-summary-date-ranges | fail  | fail NonZeroAgentExitCodeError |
| multi-source-data-merger | pass  | pass  |
| nginx-request-logging | fail ApiRateLimitError | fail NonZeroAgentExitCodeError |
| openssl-selfsigned-cert | fail  | pass  |
| overfull-hbox | fail  | fail  |
| pypi-server | fail NonZeroAgentExitCodeError | fail  |
| regex-log | fail NonZeroAgentExitCodeError | fail NonZeroAgentExitCodeError |
| sanitize-git-repo | pass  | pass  |
| sqlite-with-gcov | fail NonZeroAgentExitCodeError | fail  |
| vulnerable-secret | pass  | fail AgentTimeoutError |

## Caveats
- Task selection was by name from the 89-task set, not random; 1 attempt each.
- Many failures are harness-limit exits, not wrong answers: `NonZeroAgentExitCodeError` was step limit at 40 steps and policy denials despite `--yes` (counts below). Both arms share the limit, but it caps what either can show.
- `extract-elf` in arm F passed the verifier despite a nonzero agent exit.
- One ApiRateLimitError appeared in arm A (`nginx-request-logging`).
- Reasoning mode and tool/verify settings differ together between A and F; this cannot attribute the gap to one feature.
- Direction agrees with the corrected internal results (full harness >= minimal), but both are small. The earlier internal verifier leak has been fixed and rerun (see `FINDINGS.md`); it does not affect this external check.
- Last line of each agent log, per arm (A / F): "step limit reached" 4 / 3, "several actions were denied" 5 / 4, timed out 1 / 3, none of these 10 / 10. So about half of each arm's runs ended on a harness limit or a policy denial rather than a model decision to stop. Not resolved: whether the denials (commands such as `git push` refused even with `--yes`) are intended policy or a bug, and how many of those tasks would pass without them.
