#!/bin/bash

# Simple evaluation harness for quecto-agent, using the base `quecto` binary as the judge!

AGENT_MODEL=${AGENT_MODEL:-"qwen3.6:35b-mlx"}
AGENT_URL=${AGENT_URL:-"http://localhost:11434/v1"}

JUDGE_MODEL=${JUDGE_MODEL:-"google/gemini-2.0-flash-lite-preview-02-05:free"}
JUDGE_URL=${JUDGE_URL:-"https://openrouter.ai/api/v1"}

if [ -z "$OPENROUTER_API_KEY" ] && [[ "$JUDGE_URL" == *"openrouter"* ]]; then
    echo "Warning: OPENROUTER_API_KEY is not set. The judge might fail if using OpenRouter."
fi

# Ensure binaries are built
cargo build --release -p quecto
cargo build --release -p quecto-agent
QUECTO_BIN="$(pwd)/target/release/quecto"
AGENT_BIN="$(pwd)/target/release/quecto-agent"

run_task() {
    local task_id=$1
    local prompt=$2
    local setup_cmd=$3
    
    echo "========================================"
    echo "Running Task: $task_id"
    
    # Create fresh workspace
    local workdir="evals/workspace_$task_id"
    rm -rf "$workdir"
    mkdir -p "$workdir"
    
    cd "$workdir"
    # Run setup
    eval "$setup_cmd"
    
    # Run Agent
    echo "--> Executing Agent..."
    QUECTO_BASE_URL="$AGENT_URL" QUECTO_MODEL="$AGENT_MODEL" "$AGENT_BIN" --yes "$prompt" > agent_output.log 2>&1
    
    # Gather state
    local state=$(cat * 2>/dev/null)
    local agent_log=$(cat agent_output.log)
    cd ../..
    
    # Run Judge using the base `quecto` binary
    echo "--> Judging result..."
    local judge_prompt="You are an expert code evaluator.
The user asked the agent to: $prompt
The final files in the workspace contain:
$state

Evaluate if the agent successfully completed the task. 
You must output ONLY the word 'PASS' or 'FAIL'."

    local judge_result=$(QUECTO_BASE_URL="$JUDGE_URL" QUECTO_API_KEY="$OPENROUTER_API_KEY" QUECTO_MODEL="$JUDGE_MODEL" "$QUECTO_BIN" "$judge_prompt")
    
    if [[ "$judge_result" == *"PASS"* ]]; then
        echo "Result: ✅ PASS"
    else
        echo "Result: ❌ FAIL"
        echo "Judge said: $judge_result"
    fi
}

echo "Starting Evals..."
run_task "01_hello_world" "Write a python script called hello.py that prints 'Hello Evaluation'" "touch .keep"
run_task "02_refactoring" "Refactor math.py to use python type hints for the add function." "echo 'def add(a, b): return a + b' > math.py"

