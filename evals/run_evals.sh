#!/bin/bash

# Evaluation harness for quecto-agent, featuring complicated TerminalBench-style tasks.
# Uses the base `quecto` binary as the judge over the OpenRouter API.

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
    local judge_criteria=$4
    
    echo "========================================"
    echo "Running Task: $task_id"
    
    local workdir="evals/workspace_$task_id"
    rm -rf "$workdir"
    mkdir -p "$workdir"
    
    cd "$workdir"
    # Execute the setup payload in the workspace
    eval "$setup_cmd"
    
    echo "--> Executing Agent..."
    # The agent gets free rein in the workspace
    QUECTO_BASE_URL="$AGENT_URL" QUECTO_MODEL="$AGENT_MODEL" "$AGENT_BIN" --yes "$prompt" > agent_output.log 2>&1
    
    # Read the final state of all files in the directory to present to the judge
    local state=$(find . -type f -not -name 'agent_output.log' -not -path '*/.git/*' -exec echo -e "\n--- {} ---" \; -exec cat {} \;)
    
    if [ -d ".git" ]; then
        state="$state\n\n--- GIT STATUS ---\n$(git status)\n\n--- GIT LOG ---\n$(git log -n 3)"
    fi
    cd ../..
    
    echo "--> Judging result..."
    local judge_prompt="You are an expert evaluator for an autonomous coding agent.
The user asked the agent to: $prompt
The final files in the workspace (excluding the agent's internal log and .git internals) contain:
$state

Here is the strict judging criteria you must use to evaluate success:
$judge_criteria

Evaluate if the agent successfully completed the task according to the prompt and the criteria. 
You must output ONLY the word 'PASS' or 'FAIL'."

    local judge_result=$(QUECTO_BASE_URL="$JUDGE_URL" QUECTO_API_KEY="$OPENROUTER_API_KEY" QUECTO_MODEL="$JUDGE_MODEL" "$QUECTO_BIN" "$judge_prompt")
    
    if [[ "$judge_result" == *"PASS"* ]]; then
        echo "Result: ✅ PASS"
    else
        echo "Result: ❌ FAIL"
        echo "Judge said: $judge_result"
    fi
}

echo "Starting Evals (Complicated TerminalBench Subset)..."

# Iterate over all task directories
for task_dir in evals/tasks/*; do
    if [ -d "$task_dir" ]; then
        task_id=$(basename "$task_dir")
        prompt=$(cat "$task_dir/prompt.md")
        setup_cmd=$(cat "$task_dir/setup.sh")
        judge_criteria=$(cat "$task_dir/judge.md")
        run_task "$task_id" "$prompt" "$setup_cmd" "$judge_criteria"
    fi
done

echo "Evals finished."
