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

run_task "tb_01_git_conflict_resolution" \
    "This repository is currently in a merge conflict state on file.txt. Resolve the conflict by keeping both changes (the upstream changes on top, then the incoming changes below it). Commit the resolved file with message 'resolved'." \
    "git init && git config user.name 'eval' && git config user.email 'eval@eval.com' && echo 'line1' > file.txt && git add file.txt && git commit -m 'init' && git checkout -b feature && echo 'line2 feature' >> file.txt && git commit -am 'feature' && git checkout main && echo 'line2 main' >> file.txt && git commit -am 'main' && git merge feature || true" \
    "Check if file.txt contains both 'line2 main' and 'line2 feature' without git conflict markers (<<<<<<<, =======, >>>>>>>). Check if 'git log' shows a commit with message 'resolved'."

run_task "tb_02_package_refactoring" \
    "Refactor this project into a Python package named 'app'. Move main.py into app/cli.py. Move utils.py and config.py into app/core/. Fix all the relative imports. Create an __init__.py file so that 'python3 -m app.cli' runs without ImportErrors." \
    "echo -e 'import utils\nprint(\"main\")' > main.py && echo -e 'import config\nprint(\"utils\")' > utils.py && echo -e 'print(\"config\")' > config.py" \
    "Check if 'app/cli.py', 'app/core/utils.py', and 'app/core/config.py' exist. Check if 'app/__init__.py' exists. Check if imports were correctly updated (e.g. from .core import utils)."

run_task "tb_03_advanced_sed_awk" \
    "Using ONLY standard CLI tools (awk, sed, grep, etc.) and no python/node scripts, clean data.csv. You must: 1) Remove all completely empty lines. 2) Strip trailing commas from the end of lines. 3) Convert all uppercase email domains (e.g., @GMAIL.COM) to lowercase. Save the output to clean.csv." \
    "echo -e 'name,email,\nAlice,alice@GMAIL.COM,\n\nBob,bob@yahoo.com,\nCharlie,charlie@HOTMAIL.COM,' > data.csv" \
    "Check if clean.csv exists. It must NOT contain empty lines. It must NOT have trailing commas. It MUST have lowercase email domains (e.g. alice@gmail.com). There must be no Python or Node scripts in the workspace."

run_task "tb_04_openssl_decryption" \
    "The file secret.enc is encrypted with openssl aes-256-cbc using the password 'hunter2'. Decrypt it to secret.txt." \
    "echo 'My super secret data' > raw.txt && openssl enc -aes-256-cbc -salt -pass pass:hunter2 -in raw.txt -out secret.enc -pbkdf2 && rm raw.txt" \
    "Check if 'secret.txt' exists and contains exactly the string 'My super secret data'."

run_task "tb_05_dynamic_dependency_script" \
    "Write a Python script scraper.py that parses index.html and extracts the text of all <li> elements strictly inside the <ul id=\"target\">. Write the text, comma-separated, to output.txt. The script must use BeautifulSoup. If BeautifulSoup is missing, the script must gracefully catch the ImportError and use subprocess to run 'pip install beautifulsoup4' dynamically before importing it again and executing the logic." \
    "echo '<html><body><ul id=\"target\"><li>Item1</li><li>Item2</li></ul><ul><li>Ignore</li></ul></body></html>' > index.html" \
    "Check if 'scraper.py' exists. Check if it imports BeautifulSoup. Check if it has a try/except block catching ImportError and running 'pip install'. Check if 'output.txt' exists and contains 'Item1,Item2' or similar."

echo "Evals finished."
