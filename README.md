<div align="center">

# quecto

### The smallest AI harness of all time.

*One endpoint. Two files. Zero async. One ~1.2 MB binary.*

<br/>

[![The Moat](https://img.shields.io/badge/the%20moat-~1.2%20MB%20binary-2ea44f?style=for-the-badge)](#-the-moat-12-mb)

[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![Dependencies](https://img.shields.io/badge/dependencies-2-brightgreen?style=flat-square)](#dependencies)
[![Async](https://img.shields.io/badge/async-zero-black?style=flat-square)](#philosophy)
[![Rust](https://img.shields.io/badge/rust-edition%202021-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![Tests](https://img.shields.io/badge/tests-24%20passing-success?style=flat-square)](#testing)
[![Status](https://img.shields.io/badge/status-experimental-yellow?style=flat-square)](#status)

</div>

---

`quecto` — the [SI metric prefix](https://en.wikipedia.org/wiki/Metric_prefix) for **10⁻³⁰**, the smallest unit in the metric system. If *kilo* is 10³ and *quecto* is 10⁻³⁰, this project lives at the extreme: a universal harness built from the smallest possible composable units.

**One job:** take a prompt, run it through any OpenAI-compatible LLM — cloud (OpenAI) or local (Ollama, LM Studio, vLLM) — and return the output, buffered or streamed. Everything else you build on top.

---

## 📣 Announcements

- **`2026-07-10` — Core crate landed.** The full `quecto` core is on `main`: four-function library API, streaming with SSE + non-SSE fallback, and a one-shot / REPL / `--init` CLI. 24 tests, clippy-clean, two dependencies.
- **`2026-07-10` — Size-optimized build.** A tuned release profile ships the whole harness as a **single ~1.2 MB self-contained binary** (statically-linked TLS, no runtime).
- **Next up — `quecto-agent`.** The companion crate (agent loop, tools, sandbox, verification, session state) is fully specced and ready to build on the core.

---

## 🛡️ The Moat: 1.2 MB

The whole harness is a **single self-contained binary** — no runtime, no interpreter, statically-linked rustls TLS:

| Build | Size |
|---|---:|
| Default `--release` | 2.6 MB |
| Stripped | 2.2 MB |
| **Size-optimized profile** (shipped) | **~1.2 MB** |

Two direct dependencies (`ureq` + `serde_json`), ~30 transitive crates, **no `tokio`, no `reqwest`, no async runtime.** Small is the feature.

---

## Quick start

```bash
# Build the ~1.2 MB binary
git clone https://github.com/adityak74/quecto
cd quecto
cargo build --release      # → target/release/quecto

# One-shot
quecto "write me a haiku about small things"

# Interactive REPL (Ctrl-D to quit)
quecto

# Bootstrap your environment (prints eval-able exports)
eval "$(quecto --init)"
```

Point it anywhere OpenAI-compatible — **no API key needed for local models:**

```bash
# Local (Ollama / LM Studio / vLLM)
export QUECTO_BASE_URL="http://localhost:11434/v1"
export QUECTO_MODEL="qwen2.5-coder"
quecto "refactor this function"

# Cloud (OpenAI)
export QUECTO_BASE_URL="https://api.openai.com/v1"
export QUECTO_API_KEY="sk-..."
export QUECTO_MODEL="gpt-4o"
```

### Configuration

| Variable | Default | Purpose |
|---|---|---|
| `QUECTO_BASE_URL` | `https://api.openai.com/v1` | OpenAI-compatible endpoint |
| `QUECTO_API_KEY` | *(optional)* | Bearer token; omit for local servers |
| `QUECTO_MODEL` | `gpt-4o` | Model name |
| `QUECTO_SYSTEM` | *(optional)* | System prompt, prepended as a `{role:system}` message |
| `QUECTO_STREAM` | `1` | `0` uses the buffered path instead of streaming |

---

## Library API

Four functions: two opinion-free primitives and two conveniences layered on top.

```rust
// Primitives — you supply the exact URL, headers, and JSON body.
quecto_raw(url, headers, body)                 -> Result<Value, _>   // buffered
quecto_stream(url, headers, body, on_delta)    -> Result<String, _> // streamed (SSE)

// Conveniences — OpenAI-flavored sugar over the primitives.
quecto_to(prompt, base_url, api_key, model)    -> Result<String, _>
quecto(prompt)                                 -> Result<String, _> // reads env
```

```rust
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let reply = quecto::quecto("What is the smallest SI prefix?")?;
    println!("{reply}");
    Ok(())
}
```

Because the primitives neither shape the request nor discard the response, you can pass a `tools` array and read `tool_calls` straight off the returned `Value` — the only hook an agent layer needs.

---

## Use it for

- **Agents** — multi-step reasoning, tool use, autonomous workflows
- **Data** — extract, transform, classify, summarize at scale
- **Code** — scaffolding, refactoring, reviews, tests
- **Content** — writing, editing, SEO, translation, formatting
- **Research** — fact-checking, synthesis, comparison, deep-dive
- **Anything** — if an LLM can reason through it

---

## Philosophy

```
… → mega (10⁶) → kilo (10³) → base → milli (10⁻³) → micro (10⁻⁶) → … → quecto (10⁻³⁰)
```

1. **LLMs are the backend.** The harness is just the glue.
2. **Everything is composable.** Small pieces → big things.
3. **Describe it, run it.** If you can explain it to an LLM, quecto handles it.

`quecto` is the smallest possible unit. This project takes that literally: break any task down to its smallest composable piece, then compose them back up. The primitives decide nothing; every opinion is optional sugar you can bypass.

---

## Roadmap

| Component | Home | Status |
|---|---|---|
| Model adapter (talk to the model) | **`quecto` core** | ✅ shipped |
| Agent loop · tools · sandbox · verify · session | `quecto-agent` | 📐 specced |
| MCP integrations | `quecto-mcp` | 🔮 planned |

The core never gains an async runtime, tool execution, or state — companions build on top of `quecto_raw`.

---

## Dependencies

```toml
ureq = { version = "2", features = ["json"] }   # synchronous HTTP (rustls TLS)
serde_json = "1"                                 # build bodies, parse responses
```

## Testing

```bash
cargo test          # 24 tests: unit + HTTP + streaming + CLI (dependency-free mock server)
cargo clippy --all-targets
```

## Status

**Early / experimental.** A pet project, built in the open.

---

## Star history

<a href="https://star-history.com/#adityak74/quecto&Date">
  <img src="https://img.shields.io/github/stars/adityak74/quecto?style=social" alt="GitHub stars">
</a>

⭐ **Be the first star** — the full history chart renders [here](https://star-history.com/#adityak74/quecto&Date) once the repo has stargazers.

---

## License

Released under the **[MIT License](LICENSE)** — do whatever you want with it, just keep the copyright notice.

© 2026 Aditya
