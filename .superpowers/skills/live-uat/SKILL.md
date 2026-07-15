---
name: live-uat
description: Use when you need to perform a live User Acceptance Test (UAT) on the quecto-agent's features, specifically for observing its real-time telemetry, output, and behavior in cmux.
---

# Live UAT Skill for Quecto Agent

This skill describes the standard operating procedure for testing `quecto-agent` interactively using `cmux` to observe both its user-facing output and its backend telemetry traces.

## 1. Setup the Environment

Ensure you have a clean slate in `cmux`. 
If you have stray panes from previous tests, identify and close them, and verify port availability (e.g., port 4318 for the OTEL server).

## 2. Create cmux Panes

You will need two separate cmux panes running concurrently alongside your main orchestration pane:
1. **OTEL Mock Server Pane**: To host a mock OpenTelemetry collector.
2. **Quecto Agent Pane**: To run the interactive `quecto-agent` binary.

Example setup:
```bash
cmux new-split right  # For OTEL server (Pane A)
cmux new-split down   # For Agent (Pane B)
```

## 3. Run the OTEL Mock Server

In Pane A, navigate to your scratch directory and start a mock Python server that intercepts `http://localhost:4318/v1/traces`.

Example Python server (`otel_server.py`):
```python
import http.server
import socketserver
import time
import subprocess

class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        print(f"\n--- Received POST request on {self.path} ---", flush=True)
        content_length = int(self.headers.get('Content-Length', 0))
        if content_length > 0:
            body = self.rfile.read(content_length)
            filename = f"trace_{int(time.time()*1000)}.bin"
            with open(filename, 'wb') as f:
                f.write(body)
            # Run strings and grep to extract the thinking trace
            try:
                result = subprocess.run(
                    f"strings {filename} | grep -A 40 -i 'model_thinking'", 
                    shell=True, capture_output=True, text=True
                )
                if result.stdout.strip():
                    print("\n[OTEL Trace - Model Thinking Captured!]")
                    print(result.stdout.strip())
                    print("-" * 50, flush=True)
            except Exception as e:
                pass
        self.send_response(200)
        self.end_headers()

with socketserver.TCPServer(("", 4318), Handler) as httpd:
    print("Serving at port 4318", flush=True)
    httpd.serve_forever()
```

Launch it via cmux:
```bash
cmux send --surface <Pane_A_Surface> "python3 otel_server.py\n"
```

## 4. Run the Agent

In Pane B, set up the environment variables to point to a local Ollama instance and the OTEL mock server, then run the agent.

```bash
cmux send --surface <Pane_B_Surface> "export QUECTO_BASE_URL='http://localhost:11434/v1' && export QUECTO_MODEL='qwen3.6:35b-mlx' && export OTEL_EXPORTER_OTLP_ENDPOINT='http://localhost:4318' && cd /Users/adityakarnam/Projects/quecto && cargo run --release -p quecto-agent --features otel -- chat --yes\n"
```

## 5. Execute Test and Observe

1. Send an interactive prompt to the agent in Pane B.
   ```bash
   cmux send --surface <Pane_B_Surface> "What is 2+2?\n"
   ```
2. Wait for the agent to process the response.
3. Read the screen of the OTEL server in Pane A to observe the extracted telemetry traces in real time!
   ```bash
   cmux read-screen --surface <Pane_A_Surface> --lines 50
   ```

## 6. Cleanup

Always cleanly terminate the servers to free up ports, and close the cmux panes once testing is complete.
```bash
cmux close-surface --surface <Pane_A_Surface>
cmux close-surface --surface <Pane_B_Surface>
lsof -i :4318 | grep Python | awk '{print $2}' | xargs kill -9
```
