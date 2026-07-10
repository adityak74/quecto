# Full-Blown Coding-Agent Harness — Reference

> Reference capture of everything a complete CLI coding-agent harness needs. This is the
> **maximal** picture. quecto stays a tiny core; this document is the map we compress
> *against* to decide what (if anything) belongs in quecto vs. companion crates.

## The shape of a coding-agent harness

A CLI coding-agent harness is a controlled loop around an LLM:

```
User task
   ↓
CLI session
   ↓
Agent loop ↔ Model
   ↓
Tools: read/search/edit/run/test/git
   ↓
Repository
   ↓
Verification and final response
```

A coding agent becomes useful when it can inspect a repository, modify files, run commands,
observe results, and repeat — the same fundamental capability set exposed by mature
coding-agent systems.

---

## Minimum viable harness

### 1. CLI interface

The user-facing application. Supports:

```bash
agent "fix the failing authentication tests"

agent \
  --model qwen3.6:35b-mlx \
  --max-steps 30 \
  --approval risky \
  "implement pagination"
```

Basic CLI capabilities:

- Interactive and one-shot modes
- Model selection
- Working-directory selection
- Approval mode
- Maximum steps
- Session resume
- Streaming model output
- Ctrl+C cancellation

Suggested commands: `agent run <task>`, `agent chat`, `agent resume <session>`,
`agent diff`, `agent undo`, `agent config`, `agent models`.

### 2. Model adapter

Isolates the rest of the harness from individual model providers.

```python
class ModelProvider:
    async def generate(
        self,
        messages: list[Message],
        tools: list[ToolDefinition],
        options: GenerationOptions,
    ) -> ModelResponse:
        ...
```

Support first: Ollama, OpenAI-compatible endpoints; optional Anthropic and Gemini adapters.

Normalized response:

```python
class ModelResponse:
    text: str | None
    tool_calls: list[ToolCall]
    usage: TokenUsage
    stop_reason: str
```

Do not build provider-specific logic into the agent loop. Keep model differences inside
adapters.

### 3. Instruction loader

Repository-specific rules. Load from: `AGENTS.md`, `CLAUDE.md`, `README.md`,
`CONTRIBUTING.md`, `.agent/instructions.md`.

Includes: build commands, test commands, architecture, code style, files that must not be
changed, repository conventions.

Precedence:

```
Global instructions
    ↓
User configuration
    ↓
Repository AGENTS.md
    ↓
Nested directory AGENTS.md
    ↓
Current task
```

### 4. Repository context engine

The model should not receive the entire repository on every call. The context engine:

- Discovers repository files
- Respects `.gitignore`
- Detects languages and frameworks
- Reads selected files
- Searches symbols and text
- Tracks recently accessed files
- Includes current Git changes
- Enforces a token budget

Start with deterministic retrieval: file tree, text search, filename search, Git history,
imports/references, recently edited files. No vector database needed for the MVP — ripgrep,
file paths, imports, and model-directed search are usually enough.

Later: tree-sitter symbol indexing, language-server integration, embedding-based retrieval,
dependency-graph construction.

### 5. Tool registry

Tools are structured functions with names, descriptions, and input schemas (the same pattern
MCP uses).

```json
{
  "name": "read_file",
  "description": "Read a UTF-8 text file from the current repository.",
  "input_schema": {
    "type": "object",
    "properties": {
      "path": { "type": "string" },
      "start_line": { "type": "integer" },
      "end_line": { "type": "integer" }
    },
    "required": ["path"]
  }
}
```

Clear tool descriptions matter — the model uses them to decide which tool to invoke and how.

### 6. Essential coding tools

| Category | Tools |
|---|---|
| Repository inspection | `list_files`, `read_file`, `search_files`, `search_text` |
| Modification | `apply_patch`, `create_file`, `delete_file` |
| Execution | `run_command` |
| Git awareness | `git_status`, `git_diff`, `git_log` |
| Human interaction | `ask_user`, `request_approval` |

The most important seven: `list_files`, `read_file`, `search_text`, `apply_patch`,
`run_command`, `git_diff`, `ask_user`.

Avoid a generic unrestricted filesystem API. Tools must enforce that paths remain inside the
active repository.

### 7. Agent loop

The core of the harness.

```python
async def run_agent(task: str) -> AgentResult:
    state = initialize_state(task)

    while state.steps < state.max_steps:
        context = build_context(state)

        response = await model.generate(
            messages=context.messages,
            tools=tool_registry.schemas(),
            options=state.generation_options,
        )

        if response.tool_calls:
            for tool_call in response.tool_calls:
                result = await execute_tool_safely(tool_call)
                state.add_observation(tool_call, result)

        elif response.stop_reason == "complete":
            return finalize(response, state)

        else:
            state.add_message(response.text)

    return AgentResult(
        status="step_limit_reached",
        summary=create_summary(state),
    )
```

The loop: reason → choose tool → execute tool → observe result → update context → continue
or finish.

Must enforce: maximum steps, token budget, command timeout, repeated-action detection,
cancellation, tool-output truncation, completion criteria.

### 8. File-editing engine

Patch-based edits as the primary mechanism.

```
*** Begin Patch
*** Update File: src/auth.ts
@@
- const timeout = 1000;
+ const timeout = 5000;
*** End Patch
```

The editing layer should: validate the target path, check expected source lines exist,
reject ambiguous patches, preserve line endings, produce a Git-style diff, record previous
file contents, support rollback. Avoid making the model rewrite entire large files.

### 9. Command execution sandbox

The shell tool is the most powerful and dangerous component.

```python
run_command(
    command="pytest tests/auth",
    cwd="/repo",
    timeout_seconds=120,
)
```

Enforce: repository-scoped working directory, command timeout, output-size limit,
environment-variable filtering, secret redaction, blocked command patterns, approval before
destructive operations, process-tree termination.

Approval levels: `read-only`, `edit`, `command`, `risky`, `always-ask`.

Example policy:

| Operation | Default |
|---|---|
| Read files | Allow |
| Search repository | Allow |
| Edit repository file | Allow and show diff |
| Run tests | Allow |
| Install dependencies | Ask |
| Network access | Ask |
| Delete files | Ask |
| `sudo`, disk operations | Deny |
| Push Git commits | Ask |
| Access files outside repo | Deny |

### 10. Verification loop

The agent should not stop immediately after modifying code:

```
Inspect diff → Run formatter → Run targeted tests → Run type checker/linter
→ Fix failures → Summarize result
```

Verification tools: `format_code`, `run_linter`, `run_typecheck`, `run_tests`, `run_build`.

Infer commands from repository files:

| File | Likely ecosystem |
|---|---|
| `package.json` | Node.js |
| `pyproject.toml` | Python |
| `Cargo.toml` | Rust |
| `go.mod` | Go |
| `pom.xml` | Maven |
| `build.gradle` | Gradle |

Explicit completion gate:

```python
completion = (
    changes_exist
    and diff_reviewed
    and required_tests_passed
    and no_unresolved_tool_errors
)
```

### 11. Session state

Persist enough to resume interrupted work.

```json
{
  "session_id": "abc123",
  "task": "Add pagination",
  "repository": "/projects/api",
  "model": "qwen3.6:35b-mlx",
  "messages": [],
  "tool_calls": [],
  "files_read": [],
  "files_modified": [],
  "commands_run": [],
  "test_results": [],
  "token_usage": {},
  "checkpoints": []
}
```

Use SQLite initially: `sessions`, `messages`, `tool_calls`, `file_changes`, `checkpoints`,
`usage`. No distributed memory system needed for a local CLI.

### 12. Terminal renderer

Display model activity clearly without exposing hidden reasoning.

```
● Searching for authentication middleware
  Found 7 matches
● Reading src/auth/middleware.ts
● Editing src/auth/middleware.ts
  +12 -4
● Running pytest tests/auth
  18 passed, 1 failed
● Fixing expired-token test
● Running pytest tests/auth
  19 passed
```

Interactive commands: `/help`, `/model`, `/context`, `/diff`, `/status`, `/compact`,
`/undo`, `/approve`, `/deny`, `/clear`, `/exit`.

---

## Minimum architecture

```
┌──────────────────────────────────────┐
│              CLI / TUI               │
│ input, streaming, approval, commands │
└──────────────────┬───────────────────┘
                   │
┌──────────────────▼───────────────────┐
│              Agent Loop              │
│ state, limits, model calls, tool use │
└─────────┬─────────────────┬──────────┘
          │                 │
┌─────────▼────────┐  ┌─────▼────────────┐
│  Model Adapter   │  │   Context Engine │
│ Ollama/OpenAI API│  │ files/instructions│
└──────────────────┘  └─────┬────────────┘
                             │
                   ┌─────────▼──────────┐
                   │   Tool Executor    │
                   │ read/edit/run/git  │
                   └─────────┬──────────┘
                             │
                   ┌─────────▼──────────┐
                   │ Sandbox + Policy   │
                   │ approvals/limits   │
                   └─────────┬──────────┘
                             │
                   ┌─────────▼──────────┐
                   │ Repository + Git   │
                   └────────────────────┘
```

---

## Serious-harness additions

Add these after the minimum version works.

**Planning and task tracking** — keep plans short, update only when task state changes:

```
Task
 ├── inspect architecture
 ├── locate implementation
 ├── implement changes
 ├── add tests
 └── verify
```

**Checkpoints and rollback** — before significant edits (`git diff > checkpoint.patch`, or
internal snapshots). Commands: `agent checkpoint`, `agent undo`, `agent restore <checkpoint>`.

**Context compaction** — compact old history into: current objective, work completed,
important discoveries, files modified, tests executed, outstanding problems, repository
constraints. Preserve recent tool outputs; discard redundant logs.

**MCP support** — MCP provides external tools, resources, and prompt templates through a
standard interface. Useful integrations: GitHub, Jira, documentation, databases, browser
automation, cloud platforms, internal developer tools. Keep native filesystem, shell, and
Git tools built in; use MCP primarily for optional integrations.

**Subagents** — add only after the single-agent loop is reliable. Potential specialists:
`explorer`, `implementer`, `test-runner`, `reviewer`, `security-reviewer`. For local models,
prefer two or three focused workers over an unbounded swarm.

**Observability** — record: time per task, number of model turns, tool calls, command
failures, patch failures, test pass rate, tokens consumed, files changed, rollback
frequency. Enables comparing models (`qwen3.6:27b`, `qwen3.6:35b`, `qwen3-coder:30b`,
`devstral-small`) on real coding tasks.

---

## Recommended MVP stack (reference implementation)

> The originating reference suggests Python. quecto is Rust — this is captured as-is for
> context, not as a prescription.

| Concern | Suggestion |
|---|---|
| Language | Python |
| CLI | Typer |
| Terminal UI | Rich or Textual |
| HTTP client | httpx |
| Schemas | Pydantic |
| Storage | SQLite |
| Search | ripgrep |
| Git | subprocess initially |
| Patching | custom unified-diff engine |
| Model backend | Ollama + OpenAI-compatible adapter |
| Config | TOML |

Suggested project structure:

```
coding-agent/
├── cli.py
├── agent/
│   ├── loop.py
│   ├── state.py
│   ├── context.py
│   ├── prompts.py
│   └── compaction.py
├── models/
│   ├── base.py
│   ├── ollama.py
│   └── openai_compatible.py
├── tools/
│   ├── registry.py
│   ├── filesystem.py
│   ├── search.py
│   ├── patch.py
│   ├── shell.py
│   └── git.py
├── safety/
│   ├── policy.py
│   ├── approvals.py
│   └── sandbox.py
├── storage/
│   ├── sessions.py
│   └── checkpoints.py
└── ui/
    ├── renderer.py
    └── prompts.py
```

---

## The actual minimum

To get a functioning prototype, build only:

1. CLI input and streaming output
2. Ollama model adapter
3. Agent/tool-call loop
4. Repository instruction loader
5. `list_files`
6. `read_file`
7. `search_text`
8. `apply_patch`
9. `run_command`
10. `git_diff`
11. Approval policy
12. Test-and-fix loop
13. SQLite session persistence

That is enough for a genuine local coding-agent CLI. Planning systems, embeddings,
multi-agent orchestration, browser access, MCP, and long-term memory come later.
