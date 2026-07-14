# Chat Spinner Final Fix Report

## Scope

Applied the consolidated final-review fixes for the `quecto-agent chat` spinner only. The core `quecto` REPL was not modified and no dependencies were added.

## Fixes

- Replaced the public generic `LineRenderer::with_spinner` constructor with the chat-specific `chat_spinner_renderer` path. The spinner worker is private and receives the same synchronized stdout sink used for clears and normal renderer output.
- Replaced the 120 ms uninterruptible sleep with `std::sync::mpsc::Receiver::recv_timeout`. Stopping sends the worker a message before joining, so a normal stop is prompt.
- Made worker startup deterministic: `working()` waits for the first frame write through a standard-library synchronization channel. This is only used by the chat spinner and avoids test sleeps.
- Added focused in-memory-writer tests for enabled frame emission, clear-before-normal output, repeated starts, drop cleanup, and write-error cleanup. Added an agent test proving model errors still bracket `working`/`working_done`.
- Documented `QUECTO_SPINNER_VERBS` trimming, ignored empty entries, empty-only fallback to the six defaults, and non-TTY disabling.

## TDD evidence

- RED: `rtk cargo test -p quecto-agent render::tests::enabled_spinner` failed before the implementation change. The captured sink contained only `"\r\\x1b[2Kready\\n"` instead of the expected spinner frame followed by clear/output, proving the worker was writing global stdout instead of the renderer sink.
- GREEN: `rtk cargo test -p quecto-agent render::tests` passed after the shared-sink worker and channel wakeup change.

## Verification

- `rtk cargo test -p quecto-agent render::tests` — 14 passed, 157 filtered out.
- `rtk cargo test -p quecto-agent` — 171 passed across 5 suites.
- `rtk cargo test --workspace` — 195 passed across 11 suites.
- `rtk cargo clippy --workspace --all-targets -- -D warnings` — no issues found.
- `rtk git diff --check` — clean.

## Formatting note

`rtk cargo fmt --all -- --check` still reports pre-existing drift outside this fix in `quecto-agent/src/agent.rs`, `session.rs`, `tools/patch.rs`, and existing regions of `main.rs`. The changed renderer is formatted; those unrelated regions were not reformatted.
