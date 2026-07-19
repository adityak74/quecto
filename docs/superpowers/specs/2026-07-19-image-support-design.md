# Image Support for quecto-agent

**Date:** 2026-07-19
**Status:** Approved

## Overview

Add multimodal image input support to quecto-agent so users can send images alongside text prompts to vision-capable models (e.g. Qwen 3.6, GPT-4o, Claude). Images can be provided via clipboard paste, file path reference, drag-and-drop, or the `--image` CLI flag. Both the interactive REPL (chat mode) and one-shot mode are supported.

## Design Decisions

- **No vision gating** — images are sent to whatever model is configured. If the model doesn't support vision, the API will error. This is the user's responsibility.
- **Lossy compression for storage** — images are JPEG-compressed at quality 80 before persisting to SQLite. In-memory during the active session, images retain their original format.
- **Clipboard is a feature flag** — `arboard` is behind `features = ["clipboard"]` so headless/CI builds don't pull in platform clipboard dependencies.

## 1. Core Data Model

Refactor `Message.content` from `String` to `Vec<ContentPart>`:

```rust
/// A single part of a message's content.
#[derive(Clone, Debug, PartialEq)]
pub enum ContentPart {
    Text(String),
    Image {
        /// Raw image bytes (PNG, JPEG, GIF, WebP).
        data: Vec<u8>,
        /// MIME type, e.g. "image/png".
        mime_type: String,
    },
}

pub struct Message {
    pub role: String,
    pub content: Vec<ContentPart>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub reasoning_content: Option<String>,
}
```

### Convenience API

Existing constructors remain ergonomic — `Message::user("text")` wraps the string in `vec![ContentPart::Text(s)]`:

```rust
impl Message {
    fn plain(role: &str, content: impl Into<String>) -> Self {
        Message {
            role: role.into(),
            content: vec![ContentPart::Text(content.into())],
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn user_multimodal(parts: Vec<ContentPart>) -> Self {
        Message {
            role: "user".into(),
            content: parts,
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// Concatenate all text parts into a single string.
    pub fn text(&self) -> String {
        self.content.iter().filter_map(|p| match p {
            ContentPart::Text(t) => Some(t.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join("")
    }

    /// Whether this message contains any image parts.
    pub fn has_images(&self) -> bool {
        self.content.iter().any(|p| matches!(p, ContentPart::Image { .. }))
    }
}
```

All existing code reading `m.content` is mechanically migrated to `m.text()`.

## 2. Wire Format Serialization

### OpenAI-compatible (Ollama, OpenAI, OpenRouter, etc.)

Text-only messages serialize `content` as a plain string (backward compatible). Messages with images serialize as an array of content blocks:

```json
{"role": "user", "content": [
  {"type": "text", "text": "what's in this image?"},
  {"type": "image_url", "image_url": {
    "url": "data:image/png;base64,iVBOR..."
  }}
]}
```

Implementation in `message_to_json`:

```rust
fn message_to_json(m: &Message) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("role".into(), json!(m.role));

    if m.has_images() {
        let blocks: Vec<Value> = m.content.iter().map(|part| match part {
            ContentPart::Text(t) => json!({"type": "text", "text": t}),
            ContentPart::Image { data, mime_type } => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(data);
                json!({"type": "image_url", "image_url": {
                    "url": format!("data:{};base64,{}", mime_type, b64)
                }})
            }
        }).collect();
        obj.insert("content".into(), Value::Array(blocks));
    } else {
        // Reasoning content prefix + text, same as today
        let content = if let Some(reasoning) = &m.reasoning_content {
            format!("<think>\n{}\n</think>\n{}", reasoning, m.text())
        } else {
            m.text()
        };
        obj.insert("content".into(), json!(content));
    }
    // ... tool_calls, tool_call_id unchanged
}
```

### Anthropic

Same branching — text-only uses plain string, multimodal uses content blocks with Anthropic's `image` block shape:

```json
{"role": "user", "content": [
  {"type": "text", "text": "what's in this image?"},
  {"type": "image", "source": {
    "type": "base64",
    "media_type": "image/png",
    "data": "iVBOR..."
  }}
]}
```

## 3. REPL Input Mechanisms

### 3a. Segment enum extension

```rust
enum Segment {
    Text(String),
    Paste(String),
    Image { data: Vec<u8>, mime_type: String, index: usize },
}
```

Display renders `[Image N]` inline. On Enter, segments are converted to `Vec<ContentPart>` (adjacent text parts merged).

### 3b. Clipboard paste (Ctrl+V / Cmd+V)

On Ctrl+V, check the system clipboard for image data using `arboard::Clipboard::get_image()`. If found, encode the RGBA data to PNG and push a `Segment::Image`. If no image in clipboard, fall through to normal character input.

`arboard::Clipboard::new()` is created once at REPL start. Gated behind `#[cfg(feature = "clipboard")]`.

### 3c. File path reference (`@image path`)

When the user types `@image ./screenshot.png` (or `@img`), the pattern is detected on Enter. The file is read, MIME type is inferred from extension, and it becomes a `ContentPart::Image`. Invalid paths produce an inline error.

Regex: `@(?:image|img)\s+(\S+)`

### 3d. Drag-and-drop

Terminals paste file paths as text via bracketed paste. In the `Event::Paste(s)` handler, check if the pasted string is a path to an image file (`is_image_extension`). If so, read the file and push `Segment::Image` instead of `Segment::Paste`.

`is_image_extension` checks: `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`.

### 3e. Segment → ContentPart conversion

On Enter:

```rust
let mut parts: Vec<ContentPart> = Vec::new();
for seg in &segments {
    match seg {
        Segment::Text(t) | Segment::Paste(t) => {
            if let Some(ContentPart::Text(last)) = parts.last_mut() {
                last.push_str(t);
            } else if !t.is_empty() {
                parts.push(ContentPart::Text(t.clone()));
            }
        }
        Segment::Image { data, mime_type, .. } => {
            parts.push(ContentPart::Image {
                data: data.clone(),
                mime_type: mime_type.clone(),
            });
        }
    }
}
```

`@image` references in text segments are also extracted and converted during this phase.

## 4. Agent Interface

```rust
impl Agent {
    /// Existing text-only entry point — kept for backward compatibility.
    pub fn run(&mut self, task: &str) -> Outcome {
        self.run_multimodal(vec![ContentPart::Text(task.to_string())])
    }

    /// New multimodal entry point.
    pub fn run_multimodal(&mut self, parts: Vec<ContentPart>) -> Outcome {
        self.push_message(
            Message::user_multimodal(parts),
            MessageMetadata::default(),
        );
        self.run_loop()
    }
}
```

`run(&str)` becomes a thin wrapper — zero breakage for callers that don't use images.

## 5. One-Shot CLI

Add a repeatable `--image` flag:

```
quecto-agent --image ./screenshot.png --image ./error.png "what's wrong here?"
```

```rust
/// Attach image file(s) to the prompt. Can be specified multiple times.
#[arg(long = "image", global = true, value_name = "PATH")]
images: Vec<PathBuf>,
```

Images are loaded, combined with the task text into `Vec<ContentPart>`, and passed to `agent.run_multimodal()`.

MIME type is inferred from extension:

```rust
fn mime_from_extension(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }.to_string()
}
```

For piped (non-TTY) mode, images can only come from `--image` flags — no clipboard access.

## 6. Session Persistence

### Schema

New table (additive migration, no ALTER on existing tables):

```sql
CREATE TABLE IF NOT EXISTS message_images (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    message_seq INTEGER NOT NULL,
    part_index INTEGER NOT NULL,
    mime_type TEXT NOT NULL,
    data BLOB NOT NULL
);
```

### Lossy Compression

Before writing to SQLite, images are compressed to JPEG quality 80:

```rust
fn compress_for_storage(data: &[u8], mime_type: &str) -> (Vec<u8>, String) {
    const JPEG_QUALITY: u8 = 80;
    const PASSTHROUGH_THRESHOLD: usize = 100_000; // 100KB

    // Already JPEG and small enough — skip re-encoding
    if mime_type == "image/jpeg" && data.len() < PASSTHROUGH_THRESHOLD {
        return (data.to_vec(), "image/jpeg".into());
    }

    // Decode → re-encode as JPEG
    match image::load_from_memory(data) {
        Ok(img) => {
            let mut buf = Vec::new();
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut buf, JPEG_QUALITY
            );
            if img.write_with_encoder(encoder).is_ok() && !buf.is_empty() {
                return (buf, "image/jpeg".into());
            }
        }
        Err(_) => {}
    }
    (data.to_vec(), mime_type.into())
}
```

Typical size reduction:

| Input | Raw | After JPEG 80 |
|---|---|---|
| PNG screenshot (1080p) | ~2MB | ~180KB |
| PNG screenshot (4K) | ~8MB | ~600KB |
| Photo JPEG from clipboard | ~500KB | passthrough |
| Small diagram PNG | ~50KB | ~30KB |

### Write Path

In `record_message_with_metadata`:
1. Store `m.text()` in `messages.content` (unchanged column)
2. For each `ContentPart::Image`, compress and insert into `message_images` with the `part_index`

### Read Path

In `load_message_records`:
1. Load text from `messages.content` → `vec![ContentPart::Text(content)]`
2. Query `message_images WHERE session_id AND message_seq` → insert `ContentPart::Image` at their `part_index` positions
3. Reconstruct the full `Vec<ContentPart>` in order

### Backward Compatibility

- Old sessions load identically — `message_images` is empty
- Old binaries ignore the new table entirely

## 7. New Dependencies

```toml
base64 = "0.22"
image = { version = "0.25", default-features = false, features = ["jpeg", "png"] }
arboard = { version = "3", optional = true }

[features]
clipboard = ["dep:arboard"]
```

## 8. Files Changed

| File | Change |
|---|---|
| `model.rs` | `ContentPart` enum, `Message.content` → `Vec<ContentPart>`, `text()` + `has_images()`, `message_to_json` multimodal |
| `provider.rs` | `messages_to_anthropic_body` multimodal content blocks |
| `main.rs` | `Segment::Image`, clipboard handler, `@image` parsing, drag-and-drop, segment→ContentPart, `--image` CLI flag |
| `agent.rs` | `run_multimodal(Vec<ContentPart>)`, `run()` wraps it |
| `session.rs` | `message_images` table, compressed write, image-aware load |
| `Cargo.toml` | `base64`, `image`, `arboard` (optional) |
| All `m.content` consumers | Mechanical migration to `m.text()` |

## 9. Testing Strategy

### Unit Tests
1. `ContentPart` / `Message` model — constructors, `text()`, `has_images()`
2. Serialization — `message_to_json` OpenAI format with/without images (1×1 PNG fixture)
3. Serialization — `messages_to_anthropic_body` Anthropic format with images
4. Persistence round-trip — write message with image → load → verify
5. Compression — PNG in → JPEG out, verify smaller; JPEG passthrough for small inputs
6. `@image` parsing — `extract_image_refs` returns correct paths and cleaned text
7. CLI `--image` — clap integration test
8. REPL segment → ContentPart conversion — mixed Text + Image segments

### Integration Tests
9. Session resume with images — create, record, load, verify images survive

### Manual Tests
10. Clipboard paste → `[Image 1]` → send → verify API payload
11. `quecto-agent --image ./test.png "describe this"` → one-shot works
12. Drag PNG into terminal → detected as image not text paste
