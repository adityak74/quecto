#!/usr/bin/env bash
# =============================================================================
# QuECTO Evaluation Harness
#
# Runs the smoke test suite in evals/smoke/*. Each task directory must contain:
#   prompt.md   — task instruction
#   setup.sh    — workspace initialisation
#   verify.sh   — deterministic pass/fail check (exit 0 = PASS, non-zero = FAIL)
#   judge.md    — (optional) LLM judge fallback if verify.sh is absent
#
# Usage:
#   ./evals/run_evals.sh [env=LOCAL|ANTHROPIC|OPENAI] [model=<name>] [task=<dir-name>] [--llm-judge]
#
#   env=LOCAL       (default) talks to a local Ollama server, OpenAI-compatible wire format.
#   env=ANTHROPIC   talks to the Anthropic Messages API. Requires ANTHROPIC_API_KEY and model=.
#   env=OPENAI      talks to the OpenAI Chat Completions API. Requires OPENAI_API_KEY and model=.
#
#   task=<dir-name> restricts the run to a single task under evals/smoke/ (e.g. task=tb_11_image_color_identification).
#
# Examples:
#   ./evals/run_evals.sh
#   ./evals/run_evals.sh env=LOCAL model=qwen3.6:35b-mlx
#   ./evals/run_evals.sh env=ANTHROPIC model=claude-sonnet-4-5-20250929
#   ./evals/run_evals.sh env=OPENAI model=gpt-5 task=tb_11_image_color_identification
#
# Environment variables (all optional; env=/model=/task= args above take precedence):
#   AGENT_ENV             Same as env= (default: LOCAL)
#   AGENT_MODEL           Same as model=
#   AGENT_URL             OpenAI/Anthropic-compatible base URL, overrides the env= default
#   AGENT_API_KEY         API key for ANTHROPIC/OPENAI envs, overrides ANTHROPIC_API_KEY/OPENAI_API_KEY
#   JUDGE_MODEL           Model for LLM judge     (default: google/gemini-2.0-flash-lite-preview-02-05:free)
#   JUDGE_URL             Judge API base URL      (default: https://openrouter.ai/api/v1)
#   OPENROUTER_API_KEY    Required when JUDGE_URL points to OpenRouter
#
# For Harbor / Terminal-Bench 2.x:
#   harbor run \
#     -d terminal-bench/terminal-bench-2 \
#     -m qwen3.6:35b \
#     --agent evals.harbor.quecto_agent:QuectoAgent
# =============================================================================
set -euo pipefail

USE_LLM_JUDGE=false
ENV_ARG="${AGENT_ENV:-LOCAL}"
MODEL_ARG="${AGENT_MODEL:-}"
TASK_ARG=""

for arg in "$@"; do
    case "$arg" in
        env=*) ENV_ARG="${arg#env=}" ;;
        model=*) MODEL_ARG="${arg#model=}" ;;
        task=*) TASK_ARG="${arg#task=}" ;;
        --llm-judge) USE_LLM_JUDGE=true ;;
        *)
            echo "quecto-eval: unrecognised argument '$arg'" >&2
            exit 2
            ;;
    esac
done

ENV_ARG="$(echo "$ENV_ARG" | tr '[:lower:]' '[:upper:]')"

case "$ENV_ARG" in
    LOCAL)
        AGENT_URL="${AGENT_URL:-http://localhost:11434/v1}"
        AGENT_PROVIDER="openai"
        AGENT_MODEL="${MODEL_ARG:-qwen3.6:35b}"
        AGENT_API_KEY="${AGENT_API_KEY:-}"
        ;;
    ANTHROPIC)
        AGENT_URL="${AGENT_URL:-https://api.anthropic.com/v1}"
        AGENT_PROVIDER="anthropic"
        AGENT_MODEL="$MODEL_ARG"
        AGENT_API_KEY="${AGENT_API_KEY:-${ANTHROPIC_API_KEY:-}}"
        ;;
    OPENAI)
        AGENT_URL="${AGENT_URL:-https://api.openai.com/v1}"
        AGENT_PROVIDER="openai"
        AGENT_MODEL="$MODEL_ARG"
        AGENT_API_KEY="${AGENT_API_KEY:-${OPENAI_API_KEY:-}}"
        ;;
    *)
        echo "quecto-eval: unknown env '$ENV_ARG' (expected LOCAL, ANTHROPIC, or OPENAI)" >&2
        exit 2
        ;;
esac

if [[ "$ENV_ARG" != "LOCAL" ]]; then
    if [[ -z "$AGENT_MODEL" ]]; then
        echo "quecto-eval: env=$ENV_ARG requires model=<name>" >&2
        exit 2
    fi
    if [[ -z "$AGENT_API_KEY" ]]; then
        echo "quecto-eval: env=$ENV_ARG requires an API key (set AGENT_API_KEY, or ANTHROPIC_API_KEY/OPENAI_API_KEY)" >&2
        exit 2
    fi
fi

JUDGE_MODEL="${JUDGE_MODEL:-google/gemini-2.0-flash-lite-preview-02-05:free}"
JUDGE_URL="${JUDGE_URL:-https://openrouter.ai/api/v1}"

if [[ -z "${OPENROUTER_API_KEY:-}" ]] && [[ "$JUDGE_URL" == *"openrouter"* ]]; then
    echo "⚠️  Warning: OPENROUTER_API_KEY is not set — LLM judge may fail."
fi

# Build binaries
cargo build --release -p quecto 2>&1 | tail -3
cargo build --release -p quecto-agent 2>&1 | tail -3
QUECTO_BIN="$(pwd)/target/release/quecto"
AGENT_BIN="$(pwd)/target/release/quecto-agent"

PASS=0
FAIL=0

run_task() {
    local task_dir="$1"
    local task_id
    task_id="$(basename "$task_dir")"
    local ROOT="$(pwd)"

    echo ""
    echo "════════════════════════════════════════"
    echo " Task: $task_id"
    echo "════════════════════════════════════════"

    local workdir="evals/results/workspace_${task_id}"
    rm -rf "$workdir"
    mkdir -p "$workdir"

    # ── Setup ──────────────────────────────────
    (cd "$workdir" && bash "$ROOT/$task_dir/setup.sh") 2>&1 | sed 's/^/  [setup] /'

    # ── Execute agent ──────────────────────────
    echo "--> Running quecto-agent..."
    local prompt
    prompt="$(cat "$task_dir/prompt.md")"

    (
        cd "$workdir"
        QUECTO_BASE_URL="$AGENT_URL" QUECTO_MODEL="$AGENT_MODEL" \
        QUECTO_PROVIDER="$AGENT_PROVIDER" QUECTO_API_KEY="$AGENT_API_KEY" \
            "$AGENT_BIN" --yes --approval full "$prompt" > agent_output.log 2>&1
    ) || true   # agent exit code doesn't fail the harness

    # ── Verify ─────────────────────────────────
    local result="FAIL"

    if [[ -f "$task_dir/verify.sh" ]] && [[ "$USE_LLM_JUDGE" == "false" ]]; then
        echo "--> Verifying (deterministic)..."
        if (cd "$workdir" && bash "$ROOT/$task_dir/verify.sh" > verify.log 2>&1); then
            result="PASS"
        fi
    else
        echo "--> Judging (LLM)..."
        local state
        state="$(find "$workdir" -type f \
            -not -name 'agent_output.log' \
            -not -name 'verify.log' \
            -not -path '*/.git/*' \
            -exec printf '\n--- %s ---\n' {} \; \
            -exec cat {} \;)"

        if [[ -d "$workdir/.git" ]]; then
            state="$state

--- GIT STATUS ---
$(cd "$workdir" && git status)

--- GIT LOG ---
$(cd "$workdir" && git log -n 3)"
        fi

        local criteria
        criteria="$(cat "$task_dir/judge.md" 2>/dev/null || echo 'Did the agent complete the task described in the prompt?')"

        local judge_prompt="You are an expert evaluator for an autonomous coding agent.
The user asked the agent to: $prompt

Workspace state:
$state

Strict criteria:
$criteria

Output ONLY the single word PASS or FAIL."

        local judge_result
        judge_result="$(QUECTO_BASE_URL="$JUDGE_URL" QUECTO_API_KEY="${OPENROUTER_API_KEY:-}" \
            QUECTO_MODEL="$JUDGE_MODEL" "$QUECTO_BIN" "$judge_prompt" 2>/dev/null)"

        if [[ "$judge_result" == *"PASS"* ]]; then
            result="PASS"
        fi
    fi

    if [[ "$result" == "PASS" ]]; then
        echo "Result: ✅  PASS"
        PASS=$((PASS + 1))
    else
        echo "Result: ❌  FAIL"
        FAIL=$((FAIL + 1))
    fi
}

echo "QuECTO Smoke Eval Suite"
echo "Env   : $ENV_ARG"
echo "Agent : $AGENT_MODEL @ $AGENT_URL (provider: $AGENT_PROVIDER)"
if [[ "$USE_LLM_JUDGE" == "true" ]]; then
    echo "Judge : $JUDGE_MODEL @ $JUDGE_URL (LLM)"
else
    echo "Judge : deterministic verify.sh"
fi

if [[ -n "$TASK_ARG" ]]; then
    task_dir="evals/smoke/${TASK_ARG}/"
    if [[ ! -d "$task_dir" ]]; then
        echo "quecto-eval: unknown task '$TASK_ARG' (no such directory evals/smoke/$TASK_ARG)" >&2
        exit 2
    fi
    run_task "$task_dir"
else
    for task_dir in evals/smoke/*/; do
        [[ -d "$task_dir" ]] || continue
        run_task "$task_dir"
    done
fi

TOTAL=$((PASS + FAIL))
echo ""
echo "════════════════════════════════════════"
echo " Results: $PASS/$TOTAL passed"
echo "════════════════════════════════════════"

[[ $FAIL -eq 0 ]] && exit 0 || exit 1
