#!/usr/bin/env bash
set -euo pipefail

cargo build --release -p quecto 2>&1 | tail -3
cargo build --release -p quecto-agent 2>&1 | tail -3
AGENT_BIN="$(pwd)/target/release/quecto-agent"
ROOT="$(pwd)"

task_dir="evals/smoke/tb_11_image_color_identification"
task_id="$(basename "$task_dir")"
workdir="evals/results/workspace_${task_id}"
rm -rf "$workdir"
mkdir -p "$workdir"

echo "--> Setup"
(cd "$workdir" && bash "$ROOT/$task_dir/setup.sh")

echo "--> Executing agent"
prompt="$(cat "$task_dir/prompt.md")"
(
    cd "$workdir"
    QUECTO_BASE_URL="http://localhost:11434/v1" QUECTO_MODEL="qwen3.6:35b" \
        "$AGENT_BIN" --yes "$prompt" > agent_output.log 2>&1
) || true

echo "--> Verify"
if (cd "$workdir" && bash "$ROOT/$task_dir/verify.sh" > verify.log 2>&1); then
    echo "✅ PASS"
else
    echo "❌ FAIL"
    cat "$workdir/verify.log"
fi
