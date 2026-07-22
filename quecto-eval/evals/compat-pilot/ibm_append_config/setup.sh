#!/bin/sh
set -e
cat > settings.yaml <<'EOF'
retries: 5
mode: fast
EOF
