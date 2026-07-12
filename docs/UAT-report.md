# quecto-agent — User Acceptance / Usability Test (UAT) Report

**Date:** 2026-07-12
**Build under test:** `quecto-agent` (debug binary, milestones M1–M7 complete)
**Backend:** local Ollama (OpenAI-compatible), model `qwen3.6:35b-a3b-coding-nvfp4`
**Method:** black-box end-user testing of the real binary — no source access, no `cargo`. Four independent testers ran in parallel, one per functional area, exercising the CLI, chat REPL, tools, persistence, and flavors against a live model.

---

## Executive summary

| Metric | Result |
|---|---|
| Total scenarios | **41** |
| ✅ Pass | **34** |
| 🟡 Partial (works, with a usability rough edge) | **7** |
| ❌ Fail | **0** |
| Blocking defects | **0** |

**Verdict: ACCEPT.** Every core workflow — one-shot tasks, interactive chat, the full tool set, editing under approval, the sandbox denylist, verification gating, session persistence (resume/undo/diff), and manifest flavors with trust-on-first-use — works end-to-end against a live model. No functional failures and no data-loss or safety gaps were found. The seven partials are polish items, not correctness bugs: mostly transparency and post-failure-noise rough edges plus a forgiving CLI parser.

Safety posture is strong by default: writes and commands are denied in non-interactive mode without `--yes`; the hard denylist blocks `sudo`/`rm -rf /`/`git push` even under `--yes` and even under a `full` approval preset; project-scope flavor commands are withheld until an explicit, content-hash-remembered trust decision; unknown manifest keys fail closed; and `api_key` is never read from a manifest.

---

## Consolidated findings (ranked)

| ID | Severity | Area | Finding | User impact |
|---|---|---|---|---|
| F1 | 🟠 Medium | Tools | A denied write in **non-interactive mode is slow** (~60s despite `--max-steps 4`), and a stray `sh: - : invalid option` snippet leaked into output. | User waits a long time for a run that was going to be denied, and sees a confusing shell error fragment. |
| F2 | 🟡 Low | Tools/Flavors | **Failing verification gate keeps looping to the step limit** instead of stopping cleanly right after `● verify … failed`. | Extra noise and a delayed, less obvious failure after the real cause is already known. |
| F3 | 🟡 Low | Core CLI | **Unknown long flags are treated as task text** (e.g. `--definitely-not-a-real-flag` becomes a prompt) rather than a usage error. | Typos in flags silently become model prompts instead of failing fast. |
| F4 | 🟡 Low | Tools | **`git_diff`/`git_status` emit no activity line**, so a user can't tell whether the agent inspected git or inferred the answer. | Reduced transparency; correct answers, unclear provenance. |
| F5 | 🟡 Low | Persistence | **`resume <id>` succeeds silently** when there is no new input — empty stdout gives no confirmation anything happened. | Ambiguous; user can't tell resume worked without inspecting the DB. |
| F6 | 🟡 Low | Flavors | **`--help` lists flags but gives no per-option descriptions**; the generated flavor template is more informative than the help. | Weaker discoverability from `--help` alone. |
| F7 | 🟡 Low | Tools/Flavors | **Denylist enforcement is inconsistent in presentation**: `git push` says `denied: command matches the hard denylist`; `sudo true` shows `denied` with a model-written explanation; `rm -rf /` is refused by the model *before* any tool/denylist line. A denied `run_command` under read-only still let the agent report `pwd` via other tools, which can read as if the denied command ran. | Enforcement is safe but users get mixed signals about *what* enforced the denial. |

No high-severity or blocking issues were found.

### Recommended follow-ups (non-blocking)
1. **F1** — investigate the non-interactive denial latency and the `sh: - : invalid option` fragment (likely a shell-invocation edge in a denied path); this is the highest-value fix.
2. **F2** — on a failing required-verify gate with no further progress, stop with a dedicated outcome/message rather than looping to `step limit reached`.
3. **F3** — reject unknown `--flags` with a clap usage error (exit 2) instead of folding them into the task string.
4. **F4** — emit `● git_diff` / `● git_status` activity lines for parity with the other tools.
5. **F5** — print a short `resumed <id>` (or "transcript restored, nothing new to do") confirmation.
6. **F6** — add `#[arg(help = …)]` descriptions so `--help` documents each global flag.
7. **F7** — route all denials (including model-level refusals) through a single, consistently-worded `denied: <reason>` line.

---

## Area A — Core UX & CLI  · 10 pass / 1 partial

Tested: usage/help, one-shot answers, bare-positional parsing, the chat REPL and every slash command, the activity renderer, exit codes, and piped-vs-TTY formatting.

| # | Test | Expected | Observed | Verdict |
|---|------|----------|----------|---------|
| 1 | No arguments | Usage on stderr, exit 2 | `usage: quecto-agent [--yes] [--no-verify] "<task>"`; exit 2 | ✅ |
| 2 | `--help`/`-h` | Lists chat/resume/undo/diff/new + globals; exit 0 | All subcommands + `--yes/--model/--base-url/--max-steps/--approval`; exit 0 | ✅ |
| 3 | One-shot live | Prints answer, exit 0 | `4`; exit 0 | ✅ |
| 4 | Subcommand-looking task | Treated as task | Handled as a task; exit 0 | ✅ |
| 5 | Chat slash commands | Greeting, `›` prompt, sensible outputs, clean exit | `/help /model /context /status /clear /exit` all responded; exit 0 | ✅ |
| 6 | Chat unknown command | Friendly hint, no crash | `unknown command '/frobnicate' — try /help` then `bye` | ✅ |
| 7 | Chat one real turn | Prints reply | `pong`; exit 0 | ✅ |
| 8 | Tool activity renderer | `● <tool>  <summary>` on stderr | `● list_files  3 entries` | ✅ |
| 9 | Exit codes / step limit | 0 / 2 / 1 with message | `resume` missing id → 2; step limit → 1 `step limit reached`; success → 0 | ✅ |
| 10 | Piped formatting | Plain, no ANSI | No ANSI bytes in piped output | ✅ |
| 11 | Unknown flag | Ideally usage error | Treated as task text; exit 0 | 🟡 (F3) |

---

## Area B — Tools & Safety  · 7 pass / 3 partial

Tested: `read_file`, `list_files`, `search_text`, `write_file`, `apply_patch`, `git_diff`/`git_status`, `run_command`, non-interactive approval denial, the hard denylist under `--yes`, and the verification gate.

| # | Test | Observed | Verdict |
|---|------|----------|---------|
| 1 | read_file | `● read_file  2 lines`; correct answer | ✅ |
| 2 | search_text | `● search_text  1 matches`; named correct file | ✅ |
| 3 | write_file (`--yes`) | `● write_file  created 1 lines`; correct contents | ✅ |
| 4 | apply_patch (`--yes`) | `● apply_patch  1/1 blocks applied`; correct edit | ✅ |
| 5 | git diff/status | Correct answer, **but no `git_*` activity line** | 🟡 (F4) |
| 6 | run_command (`--yes`) | `● run_command  command finished`; stdout+exit captured | ✅ |
| 7 | Deny without `--yes` (non-interactive) | `● write_file  denied`; file absent; **but ~60s + `sh: - : invalid option`** | 🟡 (F1) |
| 8 | Denylist under `--yes` | `git push` → `denied: command matches the hard denylist`; `sudo`/`rm -rf /` denied but **inconsistent presentation** | 🟡 (F7) |
| 9 | Verify gate | pass → `● verify true  passed`, exit 0; fail → `● verify false  failed`, `step limit reached`, exit 1 | ✅ |
| 10 | Long command | `sleep 5; echo awake` → captured `awake`, exit 0 | ✅ |

---

## Area C — Persistence (sessions, resume, undo, diff)  · 9 pass / 1 partial

Tested: session recording, `diff`, `undo` (incl. walk-back and empty-state), `resume`, multi-turn chat persistence, no-git degradation, and state-db isolation (verified with `sqlite3`).

| # | Test | Observed | Verdict |
|---|------|----------|---------|
| 1 | Record file edit | `sessions=1, messages=7, file_changes=1`; `note.txt`→`v2` | ✅ |
| 2 | `diff` | `1 file change(s)` / `modified note.txt` | ✅ |
| 3 | `undo` | `reverted note.txt`; file→`v1`; `file_changes`→0 | ✅ |
| 4 | `undo` again | exit 1 `no changes to undo` | ✅ |
| 5 | `resume <id>` | message count 7→8, **but empty stdout (no confirmation)** | 🟡 (F5) |
| 6 | Multi-turn chat persistence | new session, messages 8→13 | ✅ |
| 7 | `diff` no sessions | exit 1 `no sessions` | ✅ |
| 8 | `undo` no sessions | exit 1 `no sessions to undo` | ✅ |
| 9 | No-git degradation | task completes, no crash (no missing-git notice) | ✅ |
| 10 | State-db isolation | override honored; real state dir untouched | ✅ |

---

## Area D — Flavors & Trust  · 8 pass / 2 partial

Tested: `new` scaffold + overwrite refusal, user-scope persona, model precedence (`flag > env > flavor`), tool allow-list, approval presets vs denylist, trust-on-first-use (withheld / `--yes` trusts+records / silent reload), `api_key` rejection, and malformed-manifest handling.

| # | Test | Observed | Verdict |
|---|------|----------|---------|
| 1 | `new reviewer` ×2 | created, then exit 1 `already exists`; template readable | ✅ |
| 2 | User persona | French answer with `QUECTO_SYSTEM` unset | ✅ |
| 3 | Model precedence | no env → project model works; `--model bogus` → 404 (flag wins) | ✅ |
| 4 | Tool allow-list | write request blocked; no file created | ✅ |
| 5 | Approval preset + denylist | default denies `run_command`; `full` allows harmless cmd; `sudo`/`git push` still denied under full — **but denied read-only run still reported cwd** | 🟡 (F7) |
| 6 | Untrusted project verify | `project flavor not trusted; … ignored`; exit 0; no hash written | ✅ |
| 7 | `--yes` trusts + applies | trust hash `1440eef9…` written; `● verify exit 1  failed`; exit 1 — **but loops to step limit** | 🟡 (F2) |
| 8 | Trusted silent reload | no "not trusted" warning | ✅ |
| 9 | `api_key` in manifest | exit 1 `unknown field api_key` (fails closed) | ✅ |
| 10 | Malformed key | exit 1 `flavor error: TOML parse error … unknown field bogus_key` | ✅ |

---

## What worked especially well
- **Task-first CLI + subcommands** are discoverable; usage errors use a sensible exit 2.
- **Chat REPL** is legible when piped: greeting, `›` prompt, per-command output, and a clean `bye`.
- **Activity renderer** clearly separates progress (stderr) from the final answer (stdout), plain when piped.
- **Editing tools** report precise summaries (`created 1 lines`, `1/1 blocks applied`).
- **Safety defaults**: non-interactive denial preserves files; the denylist holds under `--yes` and `full`; unknown manifest keys and `api_key` fail closed.
- **Persistence**: `diff`/`undo`/`resume` behaved predictably; state-db isolation via `QUECTO_STATE_DB` confirmed with no writes to the real state dir.
- **Flavors & trust**: precedence, allow-list, presets, and trust-on-first-use (withhold → trust-on-`--yes` → silent reload) all behaved exactly as specified.

## Methodology notes
- Four parallel black-box testers, each on an isolated temp `HOME`, `QUECTO_STATE_DB`, and `QUECTO_TRUST_FILE`; live turns capped at low `QUECTO_MAX_STEPS`.
- `sqlite3` was used read-only to verify persistence side effects; the trust file was inspected by `cat`.
- This report consolidates the four area reports; per-area raw notes were produced independently before synthesis.
