---
name: live-test-quecto-cmux
description: Use when verifying a change to the quecto binary by running it for real in a terminal — driving the one-shot and/or REPL against a live model, capturing the actual output, or producing terminal demo images for docs/README. For end-to-end/live testing and verification during development, not unit tests.
---

# Live-testing quecto in cmux

## Overview

Drive the real `quecto` binary in a dedicated cmux pane, against a live OpenAI-compatible server (local Ollama by default), capture the genuine terminal output with `read-screen`, and optionally render it as a committable terminal SVG. Proves the binary works end-to-end — the thing unit tests can't show.

**Requires:** running inside cmux (`CMUX_SOCKET_PATH` set) and a reachable model server.

## Procedure

1. **Build / locate the binary.**
   ```bash
   cargo build --release            # → target/release/quecto
   # or use the installed one: ~/.cargo/bin/quecto
   ```

2. **Pick a fast model and WARM IT FIRST.** List what's installed, then pick one — on Apple Silicon prefer an MLX model (much faster than CPU GGUF):
   ```bash
   curl -s http://localhost:11434/api/tags | grep -oE '"name":"[^"]+"'   # or: ollama list
   ```
   A cold model load takes longer than quecto's 60s read timeout and fails with `timed out reading response`, so warm it with a blocking call before the demo:
   ```bash
   curl -s -m 120 http://localhost:11434/v1/chat/completions -H 'Content-Type: application/json' \
     -d '{"model":"MODEL","messages":[{"role":"user","content":"hi"}]}' >/dev/null && echo warm
   ```

3. **Get clean output.** Qwen3.x reasoning models dump a huge `<think>…</think>` block. Prefix the prompt with `/no_think`, or use a non-thinking model (e.g. gemma). Keep prompts short.

4. **Create a demo pane and capture its ref** (always target it with `--surface`):
   ```bash
   S=$(cmux new-split right | awk '{print $2}')   # "surface:N"
   [ -n "$S" ] || { echo "no surface ref — abort"; }   # guard: empty ref targets the wrong pane
   cmux send --surface $S "clear\n"
   cmux send --surface $S "export QUECTO_BASE_URL=http://localhost:11434/v1 QUECTO_MODEL=MODEL\n"
   ```

5. **One-shot** — run, then wait until it finishes. Prefer polling for the returned shell prompt over a blind sleep (generation time varies by model):
   ```bash
   cmux send --surface $S "quecto '/no_think write a haiku about small things'\n"
   # poll: done when a fresh shell prompt (%/$) is the last visible line
   for i in $(seq 1 24); do sleep 5
     tail=$(cmux read-screen --surface $S --lines 3)
     case "$tail" in *"% "|*"$ ") break;; esac
   done
   cmux read-screen --surface $S --lines 15
   ```
   (A fixed `sleep 40` also works for a known model — MLX 35B ≈ 40s — but the poll adapts.)

6. **REPL** — start it, send one line, wait, read, then `Ctrl-D`:
   ```bash
   cmux send --surface $S "quecto\n"; sleep 2
   cmux send --surface $S "/no_think what is the smallest SI prefix?\n"
   sleep 45
   cmux read-screen --surface $S --lines 15
   cmux send-key --surface $S ctrl+d               # exit REPL
   ```

7. **Close the pane** when done: `cmux close-surface --surface $S`.

8. **Docs only (skip for plain verification) — terminal SVG.** When you want a demo image for the README (not just to verify), hand the captured lines into a terminal-styled SVG under `docs/assets/`, validate, eyeball:
   ```bash
   python3 -c "import xml.dom.minidom; xml.dom.minidom.parse('docs/assets/demo.svg')"   # well-formed?
   qlmanage -t -s 780 -o /tmp docs/assets/demo.svg                                       # → /tmp/demo.svg.png
   ```
   GitHub renders committed static SVGs referenced by relative path in the README.

## Verifying the result

Read the captured screen and confirm the behavior you changed:

- **One-shot works:** the model's text appears after the command, followed by a trailing newline and a fresh shell prompt (exit 0). For streaming, tokens should land incrementally, not all at once at the end.
- **REPL works:** the answer prints, then a **fresh `quecto›` prompt** returns for the next turn (proves the loop continued and is stateless).
- **Failure is loud, not silent:** on a bad endpoint/model, quecto prints `quecto: <error>` to **stderr** and one-shot exits 1; the REPL prints the error and keeps looping. Pointing at an offline server is itself a valid negative test.

If you're validating a specific change (e.g. streaming), compare the observed shape against what the change intended — don't just confirm "some text appeared."

## Gotchas

| Symptom / trap | Fix |
|---|---|
| Want a screenshot image | cmux `screenshot` is **browser-only** — it can't image a terminal pane. Use `read-screen` (text) + render an SVG. A raw `screencapture` grabs the whole desktop (messy, non-deterministic). |
| `timed out reading response` | Cold model load > 60s timeout. Warm the model first (step 2). |
| Output is a wall of reasoning | Reasoning model — add `/no_think` or switch models (step 3). |
| Poll "finished" fires early | Don't grep for words that also appear in the *command* line (e.g. "things"). Wait for the shell prompt line, or just `sleep` a model-appropriate duration. |
| Multi-line prompt splits into `quote>` | `cmux send` treats `\n` as Enter. Write the prompt to `scratchpad/prompt.md` and send `quecto "$(cat scratchpad/prompt.md)"\n`. |
| Split lands in the wrong pane | Always pass `--surface $S`; without it cmux splits the focused pane. |

## Quick reference

```bash
S=$(cmux new-split right | awk '{print $2}')        # make + capture pane
cmux send --surface $S "CMD\n"                       # run a command
cmux read-screen --surface $S --lines 15             # capture output
cmux send-key --surface $S ctrl+d                    # exit REPL
cmux close-surface --surface $S                      # tear down
```
