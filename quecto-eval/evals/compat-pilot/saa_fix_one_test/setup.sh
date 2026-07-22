#!/bin/sh
set -e
cat > maths.py <<'EOF'
def add(a, b):
    return a - b

def multiply(a, b):
    return a + b
EOF
