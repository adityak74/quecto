#!/usr/bin/env bash
# Convenience wrapper: run a single real (non-mocked) task through run_evals.sh.
#
# Usage:
#   ./evals/run_one_real.sh [env=LOCAL|ANTHROPIC|OPENAI] [model=<name>] [task=<dir-name>]
#
# Defaults to the local qwen3.6:35b model against the image eval task.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

args=()
has_task=false
for arg in "$@"; do
    args+=("$arg")
    [[ "$arg" == task=* ]] && has_task=true
done
[[ "$has_task" == false ]] && args+=("task=tb_11_image_color_identification")

exec ./evals/run_evals.sh "${args[@]}"
