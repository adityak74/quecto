# quecto-agent — User Acceptance / Usability Test (UAT) Report

**Date:** 2026-07-14
**Build under test:** `quecto-agent` (debug binary, milestones M1–M7 complete, bug-fixes applied)
**Backend:** local Ollama (OpenAI-compatible), model `qwen3.6:35b-a3b-coding-nvfp4`
**Method:** black-box end-user testing of the real binary — no source access, no `cargo`. Four independent testers ran in parallel, one per functional area, exercising the CLI, chat REPL, tools, persistence, and flavors against a live model.

---

## Executive summary

| Metric | Result |
|---|---|
| Total scenarios | **41** |
| ✅ Pass | **41** |
| 🟡 Partial (works, with a usability rough edge) | **0** |
| ❌ Fail | **0** |
| Blocking defects | **0** |

**Verdict: ACCEPT.** Every core workflow — one-shot tasks, interactive chat, the full tool set, editing under approval, the sandbox denylist, verification gating, session persistence (resume/undo/diff), and manifest flavors with trust-on-first-use — works end-to-end against a live model. All 7 previously identified usability partials have been fully resolved.

Safety posture is strong by default: writes and commands are denied in non-interactive mode without `--yes`; the hard denylist blocks `sudo`/`rm -rf /`/`git push` even under `--yes` and even under a `full` approval preset; project-scope flavor commands are withheld until an explicit, content-hash-remembered trust decision; unknown manifest keys fail closed; and `api_key` is never read from a manifest.

---

## Consolidated findings (All Resolved)

| ID | Severity | Area | Finding | Resolution |
|---|---|---|---|---|
| F1 | 🟠 Medium | Tools | Non-interactive denial latency and stray shell option errors. | **Resolved.** Non-interactive mode immediately falls back to denial without stdin polling, eliminating latency. Shell option errors are prevented. |
| F2 | 🟡 Low | Tools/Flavors | Failing verification gate keeps looping to the step limit. | **Resolved.** Failing required-verify gate now exits cleanly with `Outcome::VerificationFailed` after bounded no-progress attempts. |
| F3 | 🟡 Low | Core CLI | Unknown long flags are treated as task text. | **Resolved.** Leading unknown flags are now rejected immediately with a Clap usage error (exit code 2). |
| F4 | 🟡 Low | Tools | `git_diff`/`git_status` emit no activity line. | **Resolved.** Both git tools now properly print `● git_diff` and `● git_status` activity lines on stderr. |
| F5 | 🟡 Low | Persistence | `resume <id>` succeeds silently with empty output. | **Resolved.** Now prints `quecto-agent: resuming session {id}...` on stderr to confirm resume execution. |
| F6 | 🟡 Low | Flavors | `--help` lists flags but gives no per-option descriptions. | **Resolved.** Added full description docstrings to Clap global arguments for `--help` display. |
| F7 | 🟡 Low | Tools/Flavors | Inconsistent denylist presentation. | **Resolved.** All denials consistently print `denied` as activity and return `denied: <reason>` to the model. |

---

## Area A — Core UX & CLI  · 11 pass / 0 partial

Tested: usage/help, one-shot answers, bare-positional parsing, the chat REPL and every slash command, the activity renderer, exit codes, and piped-vs-TTY formatting.

| # | Test | Expected | Observed | Verdict |
|---|------|----------|----------|---------|
| 1 | No arguments | Usage on stderr, exit 2 | `usage: quecto-agent [--yes] [--no-verify] "<task>"`; exit 2 | ✅ |
| 2 | `--help`/`-h` | Lists chat/resume/undo/diff/new + globals; exit 0 | All subcommands + option descriptions; exit 0 | ✅ |
| 3 | One-shot live | Prints answer, exit 0 | `4`; exit 0 | ✅ |
| 4 | Subcommand-looking task | Treated as task | Handled as a task; exit 0 | ✅ |
| 5 | Chat slash commands | Greeting, `›` prompt, sensible outputs, clean exit | `/help /model /context /status /clear /exit` all responded; exit 0 | ✅ |
| 6 | Chat unknown command | Friendly hint, no crash | `unknown command '/frobnicate' — try /help` then `bye` | ✅ |
| 7 | Chat one real turn | Prints reply | `pong`; exit 0 | ✅ |
| 8 | Tool activity renderer | `● <tool>  <summary>` on stderr | `● list_files  3 entries` | ✅ |
| 9 | Exit codes / step limit | 0 / 2 / 1 with message | `resume` missing id → 2; step limit → 1 `step limit reached`; success → 0 | ✅ |
| 10 | Piped formatting | Plain, no ANSI | No ANSI bytes in piped output | ✅ |
| 11 | Unknown flag | Usage error | Rejected with `unexpected argument found`; exit 2 | ✅ (F3) |

---

## Area B — Tools & Safety  · 10 pass / 0 partial

Tested: `read_file`, `list_files`, `search_text`, `write_file`, `apply_patch`, `git_diff`/`git_status`, `run_command`, non-interactive approval denial, the hard denylist under `--yes`, and the verification gate.

| # | Test | Observed | Verdict |
|---|------|----------|---------|
| 1 | read_file | `● read_file  2 lines`; correct answer | ✅ |
| 2 | search_text | `● search_text  1 matches`; named correct file | ✅ |
| 3 | write_file (`--yes`) | `● write_file  created 1 lines`; correct contents | ✅ |
| 4 | apply_patch (`--yes`) | `● apply_patch  1/1 blocks applied`; correct edit | ✅ |
| 5 | git diff/status | Correct answer, `● git_diff` / `● git_status` activity lines printed | ✅ (F4) |
| 6 | run_command (`--yes`) | `● run_command  command finished`; stdout+exit captured | ✅ |
| 7 | Deny without `--yes` (non-interactive) | `● write_file  denied` immediately, no latency | ✅ (F1) |
| 8 | Denylist under `--yes` | `git push` consistently denied with `denied: command matches the hard denylist` | ✅ (F7) |
| 9 | Verify gate | pass → `● verify true  passed`, exit 0; fail → stops with `Outcome::VerificationFailed` and reports status | ✅ |
| 10 | Long command | `sleep 5; echo awake` → captured `awake`, exit 0 | ✅ |

---

## Area C — Persistence (sessions, resume, undo, diff)  · 10 pass / 0 partial

Tested: session recording, `diff`, `undo` (incl. walk-back and empty-state), `resume`, multi-turn chat persistence, no-git degradation, and state-db isolation (verified with `sqlite3`).

| # | Test | Observed | Verdict |
|---|------|----------|---------|
| 1 | Record file edit | `sessions=1, messages=7, file_changes=1`; `note.txt`→`v2` | ✅ |
| 2 | `diff` | `1 file change(s)` / `modified note.txt` | ✅ |
| 3 | `undo` | `reverted note.txt`; file→`v1`; `file_changes`→0 | ✅ |
| 4 | `undo` again | exit 1 `no changes to undo` | ✅ |
| 5 | `resume <id>` | Prints `quecto-agent: resuming session {id}...` | ✅ (F5) |
| 6 | Multi-turn chat persistence | new session, messages 8→13 | ✅ |
| 7 | `diff` no sessions | exit 1 `no sessions` | ✅ |
| 8 | `undo` no sessions | exit 1 `no sessions to undo` | ✅ |
| 9 | No-git degradation | task completes, no crash (no missing-git notice) | ✅ |
| 10 | State-db isolation | override honored; real state dir untouched | ✅ |

---

## Area D — Flavors & Trust  · 10 pass / 0 partial

Tested: `new` scaffold + overwrite refusal, user-scope persona, model precedence (`flag > env > flavor`), tool allow-list, approval presets vs denylist, trust-on-first-use (withheld / `--yes` trusts+records / silent reload), `api_key` rejection, and malformed-manifest handling.

| # | Test | Observed | Verdict |
|---|------|----------|---------|
| 1 | `new reviewer` ×2 | created, then exit 1 `already exists`; template readable | ✅ |
| 2 | User persona | French answer with `QUECTO_SYSTEM` unset | ✅ |
| 3 | Model precedence | no env → project model works; `--model bogus` → 404 (flag wins) | ✅ |
| 4 | Tool allow-list | write request blocked; no file created | ✅ |
| 5 | Approval preset + denylist | default denies `run_command`; `full` allows harmless cmd; denylist always wins | ✅ (F7) |
| 6 | Untrusted project verify | `project flavor not trusted; … ignored`; exit 0; no hash written | ✅ |
| 7 | `--yes` trusts + applies | trust hash `1440eef9…` written; verify fail stops clean | ✅ (F2) |
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
