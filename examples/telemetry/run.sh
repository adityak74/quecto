#!/usr/bin/env bash
# Builds quecto-agent with the `otel` feature and runs a chat session that
# exports spans to the Jaeger instance started by docker-compose.yml.
set -euo pipefail

cd "$(dirname "$0")"

export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://localhost:4318}"
export OTEL_SERVICE_NAME="${OTEL_SERVICE_NAME:-quecto-agent}"

echo "==> Starting Jaeger (OTLP HTTP on :4318, UI on http://localhost:16686)"
docker compose up -d

echo "==> Building quecto-agent with the otel feature"
cargo build --manifest-path ../../Cargo.toml -p quecto-agent --features otel

echo "==> Launching quecto-agent chat (spans export to Jaeger as you talk)"
echo "    Open http://localhost:16686 and select service 'quecto-agent' to watch traces."
../../target/debug/quecto-agent chat
