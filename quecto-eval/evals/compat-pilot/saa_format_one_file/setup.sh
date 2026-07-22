#!/bin/sh
set -e
cat > app.js <<'EOF'
function main() {
    console.log('hi');
}
EOF
cat > server.js <<'EOF'
function init() {
    console.log('init');
}
EOF
