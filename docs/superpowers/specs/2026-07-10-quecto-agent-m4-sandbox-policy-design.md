# quecto-agent M4 Sandbox and Policy Design

**Status:** Approved design for Milestone 4

## Goal

Add safe Unix command execution and a single approval boundary to `quecto-agent`, plus
cancellation and repeated-action protection, while preserving the established M3 editing
interfaces.

## Scope

M4 adds:

- A fixed built-in approval policy.
- Approval enforcement before every registered tool dispatch.
- A Unix-only command sandbox and the `run_command` tool.
- Interactive confirmation, non-interactive denial, and `--yes` auto-approval.
- Cooperative cancellation through a shared atomic token and `ctrlc`.
- Repeated-action detection in the agent loop.

M4 does not add configurable flavor policies, verification commands, session persistence,
rich terminal rendering, `ask_user`, or Windows process management. Those remain later
milestones.

## Architecture

Policy enforcement belongs in the agent loop, immediately before registry dispatch. This
keeps the security boundary independent of individual tool implementations: every current
and future tool call is classified before it can execute. The registry remains responsible
only for schemas, lookup, dispatch, and converting tool failures into model observations.

The implementation adds four focused units:

- `policy.rs` classifies tool calls as `Allow`, `Ask`, or `Deny` and applies the command
  denylist before any interactive override.
- `approval.rs` resolves `Ask` using an injected confirmer, non-interactive safety, or the
  explicit `--yes` flag.
- `sandbox.rs` owns Unix child-process lifecycle, timeout/cancel polling, process-group
  termination, output capture/capping, and redaction.
- `tools/shell.rs` validates `run_command` arguments and delegates execution to the sandbox.

`Agent` owns the policy, approval mode, and shared cancellation token. The CLI installs one
`ctrlc` handler that sets that token and passes `--yes` into the agent configuration.

## Fixed Built-in Policy

The policy is intentionally not configurable in M4:

| Operation | Decision |
|---|---|
| `read_file`, `list_files`, `search_text`, `git_diff`, `git_status` | `Allow` |
| `write_file`, `apply_patch`, `run_command` | `Ask` |
| Unknown tools | `Deny` |
| Denylisted commands | `Deny` |

The command denylist is evaluated before `--yes` and before prompting. It rejects at least:

- `sudo` execution.
- Root-targeted recursive deletion such as `rm -rf /` and equivalent option ordering.
- Filesystem or disk-destruction utilities such as `mkfs`, `fdisk`, and `diskutil eraseDisk`.
- Absolute-path redirection or writes outside the repository.
- Git pushes.

The denylist is defense in depth, not a general shell parser or an OS security boundary. M4
does not claim containment against an actively malicious local process; it prevents common
destructive model-generated commands and limits execution lifecycle and context exposure.

## Approval and Interactivity

`Ask` resolves as follows:

| Mode | Resolution |
|---|---|
| Interactive TTY | Show tool name and summarized arguments on stderr; accept an explicit `y`/`yes` |
| Non-interactive stdin | Deny |
| `--yes` | Allow |

Any other response is denial. `--yes` never overrides `Deny`. Tests inject a deterministic
confirmer and do not read the real terminal.

Denied calls become structured tool observations so the model can choose another action;
they do not abort the whole agent run.

## Unix Command Sandbox

`run_command` accepts one required UTF-8 `command` string. It always starts at the canonical
repository root and exposes no model-controlled working-directory argument.

The sandbox uses `/bin/sh -c` and, on Unix, creates a separate child process group before
execution. It enforces:

- A fixed 120-second wall-clock timeout.
- Polling of the shared cancellation token while the child runs.
- Whole-process-group termination on timeout or cancellation, followed by child reaping.
- Captured stdout and stderr with independent bounded collection.
- Existing head/tail-style output truncation before returning content to the model.
- Best-effort replacement of non-empty environment values whose names contain `KEY`,
  `TOKEN`, `SECRET`, or `PASSWORD` in captured output.

The child inherits the environment unchanged so repository tests that require credentials do
not break. Redaction is explicitly best effort: repository files and child behavior remain
inside the model/operator trust boundary.

The tool observation reports exit status, stdout, and stderr. Timeout and cancellation are
distinct results so the agent loop can map cancellation to its terminal outcome.

## Cancellation

A shared `Arc<AtomicBool>` is created by the CLI and installed in the agent and sandbox
context. The `ctrlc` handler only sets the flag; it performs no I/O or process manipulation.

The agent checks cancellation before each model request and before each tool call. The
sandbox checks it while polling a child. Cancellation produces `Outcome::Cancelled`; the CLI
prints a concise message and exits unsuccessfully. A running command's entire process group
is terminated before returning.

## Repeated-Action Guard

After each tool result, the agent fingerprints the tool name, canonical JSON arguments, and
returned observation. It also records the current `Context::changes().len()`.

Three consecutive identical fingerprints terminate with `Outcome::RepeatedAction` only when
the change count has not advanced between those observations. Any different call, different
result, or recorded file mutation resets the streak. This stops genuine loops without
blocking the normal command-edit-command cycle.

## Error Handling

- Invalid or missing `run_command` arguments return a structured tool error.
- Spawn, wait, pipe-read, and kill failures are returned as tool observations unless the
  agent itself cannot continue safely.
- Denied calls are reported as `denied: <reason>` observations.
- Timeout is a completed tool observation and allows the model to recover.
- User cancellation terminates the agent run.
- Poisoned test or internal synchronization state is handled without panicking in production
  paths where practical.

## Testing

Tests are deterministic and do not require a network model:

- Policy tests cover every built-in tool class and denylist precedence over `--yes`.
- Approval tests cover affirmative, negative, non-interactive, and auto-approved decisions.
- Sandbox tests cover repository-root cwd, stdout/stderr and exit status, output caps,
  redaction, timeout, cancellation, and descendant-process termination.
- Tool tests cover schema, argument validation, and sandbox delegation.
- Agent tests prove allowed reads dispatch, edits are gated, denied calls become
  observations, cancellation terminates, three identical no-change actions stop, and a file
  change resets the repeat streak.
- CLI tests cover `--yes`, cancellation/error messages where practical, and preservation of
  the existing one-shot task interface.

## Dependency and Platform Constraints

M4 targets macOS and Linux only. Unix-specific process-group code is guarded with `cfg(unix)`;
the crate emits a clear compile-time error on unsupported targets until Windows support is
designed.

The expected new runtime dependencies are `ctrlc` for signal registration and `libc` for
Unix process-group operations. No async runtime, shell parser, sandbox framework, or CLI
framework is introduced. Argument parsing remains minimal: remove `--yes` from the existing
argument vector and join the remaining arguments as the task.

## Milestone Boundary

M4 is complete when model-requested edits and commands cannot execute without passing the
central fixed policy, commands have bounded Unix lifecycle and output, Ctrl-C cancels a
running child tree, and a no-progress tool loop terminates predictably. Configuration-driven
policy, trusted verification bypass, persistence, and richer interaction remain explicitly
deferred.
