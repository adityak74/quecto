# quecto-agent — `/capsule-create` UAT Report

**Date:** 2026-07-25
**Build under test:** `quecto-agent` (release binary, commit `2a35855`)
**Backend:** local Ollama (OpenAI-compatible), model `qwen3.6:35b`
**Method:** black-box testing of the real REPL binary, driven live in a dedicated terminal pane against a running model — no unit-test mocks, no source access during the test itself. A scratch project directory was used as `cwd` so `.quecto/capsules` writes were isolated from the real repo.

---

## Executive summary

| Metric | Result |
|---|---|
| Total scenarios | **5** |
| ✅ Pass | **5** |
| ❌ Fail | **0** |

**Verdict: ACCEPT.** `/capsule-create` works end-to-end against a live model: drafting, name reconciliation, file write, immediate registration/activation, invocation, and both rejection paths (duplicate name, path-traversal) all behaved exactly per the design spec (`docs/superpowers/specs/2026-07-25-capsule-create-design.md`).

---

## Scenarios

| # | Test | Expected | Observed | Verdict |
|---|------|----------|----------|---------|
| 1 | `/capsule-create haiku-writer <description>` | Model drafts a `CAPSULE.md`, name is reconciled to the requested `<name>` if the model's `name:` differs, file is written to `<cwd>/.quecto/capsules/haiku-writer/CAPSULE.md`, capsule is registered and activated in the same call | Model drafted frontmatter with `name: haiku-witter` (a typo); REPL silently corrected it to `haiku-writer` per spec; notice `"created and loaded capsule haiku-writer at <path>"`; file on disk confirmed well-formed with the corrected name | ✅ |
| 2 | `/capsules` after creation | Lists the new capsule, marked active | `● haiku-writer — Responds exclusively in 5-7-5 haikus about whatever topic the user provides.` | ✅ |
| 3 | `/haiku-writer paperclips` | Invokes the freshly-created capsule without a separate `/load`; the model follows the drafted instructions | Model returned a genuine 5-7-5 haiku about paperclips (not free-form text), proving the capsule's instructions were actually folded into the system prompt and honored | ✅ |
| 4 | `/capsule-create haiku-writer <description>` (duplicate name) | Rejected before any model call, per the "already exists" row of the error table | `capsule haiku-writer already exists (see /capsules)` | ✅ |
| 5 | `/capsule-create ../evil <description>` (path traversal) | Rejected before any model call; nothing written outside the capsules directory | `../evil is not a valid capsule name (must be a single path component, no '/' or '..')`; confirmed no `evil` directory was created anywhere under the scratch dir | ✅ |

## Notes

- Scenario 1 is a genuine end-to-end proof of the spec's "Reconcile name" step (§5 of the handler flow): the model drafted a slightly different name than requested, and the REPL's authoritative-CLI-argument correction was observed live, not just in a unit test with a scripted fake model.
- Scenario 5 is a live confirmation of the final-review security fix (commit `ee87a6e`) that closed a path-traversal / arbitrary-write gap.
- No permission prompts were needed; the REPL was run with `--yes` (auto-approve) since capsule creation itself performs no shell/edit tool calls — only the meta-prompt turn and a direct `fs::write` inside the handler.
