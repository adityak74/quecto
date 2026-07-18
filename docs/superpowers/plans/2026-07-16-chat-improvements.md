# Chat Improvements Implementation Plan

## Global Constraints
- Target crate: `quecto-agent`
- We must not break existing non-interactive modes.

## Task 1: Chat Mode UX (`run_command` display and Crossterm Bracketed Paste)
- Modify `agent.rs` line ~376 where `self.renderer.tool(&call.name, &out.summary);` happens. If `call.name` is "run_command", format the name to `run_command(cmd)` where `cmd` is `call.arguments.get("command")`.
- Replace the simple `stdin.lock().lines()` loop in `quecto-agent/src/main.rs` (the `chat()` function) with a crossterm event loop.
- The crossterm loop should:
  - Enable raw mode.
  - Enable bracketed paste (`crossterm::terminal::EnableBracketedPaste`).
  - Read crossterm `Event`s.
  - Keep an internal buffer of text segments: `enum Segment { Text(String), Paste(String) }`.
  - When `Event::Key(Char(c))` happens, append to current text segment.
  - When `Event::Paste(s)` happens, push `Paste(s)` to the segments.
  - When rendering the prompt, render `Text` normally, and for `Paste(s)` render `[pasted +{} characters]` where `{}` is the length of `s`.
  - Handle `Event::Key(Enter)` to send the prompt by concatenating all segments.
  - Allow simple Backspace handling (at the end of the input).
  - Handle Ctrl-C/Ctrl-D to break/exit.

## Task 2: Background Process Management
- Create `quecto-agent/src/tools/background.rs`
- Add tools:
  - `start_background_server`: Takes a `command` string. Spawns it with `std::process::Command`, redirects stdout/stderr, stores the `Child` in a global/agent-specific map, returns a PID.
  - `kill_background_server`: Takes `pid`. Kills it.
  - `list_background_servers`: Lists currently running background processes.
- Add these to the agent's tool registry.
- Render background processes in the chat mode status? The user said "tracked, like 2 background process running", so `list_background_servers` or a chat mode status command `/status` could show it. Let's make sure `/status` in chat mode prints the count of background processes.

## Task 3: Subagents Support
- Add a new tool `invoke_subagent` in `tools/subagent.rs`.
- The tool takes `prompt` and `role`.
- It recursively spawns a new `Agent` with the same `model` and `system` prompt + `role` context, and calls `run(&prompt)`.
- It captures the result of `agent.run()` and returns it as a string to the parent agent.
- Make sure `Agent` exposes a way to clone itself or create a subagent easily.

## Task 4: Knowledge Base Tools
- Create `quecto-agent/src/tools/notes.rs`.
- Implement `take_note` tool: creates/appends to markdown files in `./.qkb/`.
- Implement `search_notes` tool: searches files in `./.qkb/` (can use `ripgrep` or a simple Rust string search if fast enough, or just read the files since it's local notes).
- Register these tools in `builtin_tools()`.
