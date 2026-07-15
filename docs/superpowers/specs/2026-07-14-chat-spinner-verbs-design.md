# Chat REPL Spinner Verbs Design

## Goal

Add a very low-effort Claude-like loading indicator to `quecto-agent chat` while the harness is waiting for the model, without changing the core `quecto` REPL or the agent's behavior.

## Scope

- Change only the coding-harness REPL path in `quecto-agent`.
- Leave the core `quecto` stateless REPL clean and unchanged.
- Do not add a dependency or an async runtime.
- Do not record spinner text in sessions or include it in model messages.

## User experience

When chat is running in an interactive terminal and the model call is in progress, show a single temporary line such as:

```text
⠋ Thinking…
```

The animation cycles lightweight spinner frames and rotates through verbs. It stops and clears its line before tool activity, verification output, or the assistant response is rendered. If stdout is not a TTY, the spinner is disabled so redirected and test output remains stable.

## Configuration

Use `QUECTO_SPINNER_VERBS` as an optional comma-separated replacement list:

```sh
export QUECTO_SPINNER_VERBS="Brewing,Refactoring,Ship-shaping"
```

Parsing trims whitespace and ignores empty entries. A configured value with no usable entries falls back to the built-in defaults.

The built-in list contains the full supplied playful verb set; it is stored as
static strings and adds no dependency or runtime cost beyond the binary data.

The authoritative list lives in `DEFAULT_SPINNER_VERBS` in
`quecto-agent/src/render.rs`; it includes all verbs supplied for this feature.

## Design

The existing `LineRenderer` remains responsible for terminal output. Add a small spinner state/helper that:

1. starts immediately before each `model.complete(...)` call;
2. periodically redraws one line using a carriage return and a clear-line escape;
3. stops immediately after the model returns, including error paths;
4. clears the temporary line before normal renderer output continues.

The agent loop calls this lifecycle through the renderer boundary, so spinner concerns do not leak into the model, tool, recorder, or session layers. The renderer's current TTY decision controls whether the spinner is active.

## Testing

- Unit-test environment parsing with unset, whitespace, custom, and empty-only values.
- Unit-test that the built-in defaults are available and custom values replace them.
- Unit-test spinner lifecycle using a writer/testable helper without requiring a real terminal; verify start output is temporary-status output and stop clears it.
- Run the existing workspace test suite to ensure chat, renderer, persistence, and core REPL behavior remain intact.

## Non-goals

- No spinner in one-shot mode or the core `quecto` REPL.
- No CLI flags or config-file support.
- No attempt to display model/tool-specific progress.
- No full copy of the external verb list in source; users who want it can provide it through the environment.
