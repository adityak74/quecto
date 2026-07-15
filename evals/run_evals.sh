#!/bin/bash

# Evaluation harness for quecto-agent, inspired by TerminalBench tasks.
# Uses the base `quecto` binary as the judge over the OpenRouter API.

AGENT_MODEL=${AGENT_MODEL:-"qwen3.6:35b-mlx"}
AGENT_URL=${AGENT_URL:-"http://localhost:11434/v1"}

JUDGE_MODEL=${JUDGE_MODEL:-"google/gemini-2.0-flash-lite-preview-02-05:free"}
JUDGE_URL=${JUDGE_URL:-"https://openrouter.ai/api/v1"}

if [ -z "$OPENROUTER_API_KEY" ] && [[ "$JUDGE_URL" == *"openrouter"* ]]; then
    echo "Warning: OPENROUTER_API_KEY is not set. The judge might fail if using OpenRouter."
fi

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
    
    local workdir="evals/workspace_$task_id"
    rm -rf "$workdir"
    mkdir -p "$workdir"
    
    cd "$workdir"
    eval "$setup_cmd"
    
    echo "--> Executing Agent..."
    QUECTO_BASE_URL="$AGENT_URL" QUECTO_MODEL="$AGENT_MODEL" "$AGENT_BIN" --yes "$prompt" > agent_output.log 2>&1
    
    local state=$(find . -type f -not -name 'agent_output.log' -exec echo "--- {} ---" \; -exec cat {} \;)
    cd ../..
    
    echo "--> Judging result..."
    local judge_prompt="You are an expert evaluator for an autonomous coding agent.
The user asked the agent to: $prompt
The final files in the workspace (excluding the agent's internal log) contain:
$state

Evaluate if the agent successfully completed the task according to the prompt. 
You must output ONLY the word 'PASS' or 'FAIL'."

    local judge_result=$(QUECTO_BASE_URL="$JUDGE_URL" QUECTO_API_KEY="$OPENROUTER_API_KEY" QUECTO_MODEL="$JUDGE_MODEL" "$QUECTO_BIN" "$judge_prompt")
    
    if [[ "$judge_result" == *"PASS"* ]]; then
        echo "Result: ✅ PASS"
    else
        echo "Result: ❌ FAIL"
        echo "Judge said: $judge_result"
    fi
}

echo "Starting Evals (TerminalBench Subset)..."

run_task "tb_01_log_processing" \
    "Extract all unique IP addresses from access.log that encountered a 404 error and write them to 404_ips.txt, one per line." \
    "echo -e '192.168.1.1 GET / 200\n10.0.0.5 GET /secret 404\n192.168.1.1 GET /favicon 404\n10.0.0.5 POST /login 404' > access.log"

run_task "tb_02_refactoring" \
    "Refactor app.py to use the standard 'logging' module instead of print() statements. Configure it to log at INFO level. Replace all print() calls with logging.info()." \
    "echo -e 'def main():\n    print(\"Starting app\")\n    print(\"Finished app\")\n\nif __name__==\"__main__\":\n    main()' > app.py"

run_task "tb_03_bash_scripting" \
    "Write a bash script called backup.sh that creates a tar archive named backup.tar.gz containing all .config files in the current directory." \
    "touch db.config web.config main.py"

run_task "tb_04_system_info" \
    "Find the 2 largest files in the current directory and its subdirectories, and write their relative paths to largest.txt." \
    "mkdir sub && head -c 100 /dev/zero > small.txt && head -c 5000 /dev/zero > big.txt && head -c 10000 /dev/zero > sub/huge.txt"

run_task "tb_05_bug_fixing" \
    "Fix the syntax and logical errors in main.c so that it compiles with 'gcc main.c -o main' and prints exactly 'Success'." \
    "echo -e '#include <stdio.h>\nint main() {\n    printf(\"Succes\");\n    return 0\n}' > main.c"

echo "Evals finished."
