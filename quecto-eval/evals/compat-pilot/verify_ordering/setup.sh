#!/bin/sh
set -e
cat > add.py <<'EOF'
def add(a, b):
    return a - b  # BUG: should be a + b
EOF
cat > test_add.py <<'EOF'
from add import add

def test_add():
    assert add(2, 3) == 5
EOF
